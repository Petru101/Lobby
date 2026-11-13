//! Installing a download: an archive is unpacked into the games folder (gamefix's, the one it
//! installs into too) and the folder set up by gamefix; an installer or a disc image is run by
//! gamefix, which sets up what it installs. Either way the game then starts from its gamefix
//! app-menu entry, with lobby or without. The download itself is kept.

use crate::library::{Game, Kind};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// In an unpacked folder: the download it came from, so a scan ties the two together whatever
/// they're called.
pub const SOURCE_NOTE: &str = ".lobby-download";

/// The download's file.
pub fn download(game: &Game) -> Option<PathBuf> {
    game.entries.iter().filter(|e| e.kind == Kind::Download).find_map(|e| e.program.clone()).filter(|p| p.is_file())
}

/// The download's files, for gamefix to tell whether it has finished. None when Install wouldn't
/// use them (Steam's install).
fn download_parts(game: &Game) -> Option<Vec<PathBuf>> {
    if game.kind != Kind::Download || steam_owned(game).is_some() {
        return None;
    }
    let file = download(game)?;
    let folder = game.entries.iter().find(|e| e.kind == Kind::Download && e.program.as_deref() == Some(file.as_path())).and_then(|e| e.folder.as_deref());
    Some(gamefix::download::parts(&file, folder))
}

/// Whether the download looks like it's still coming, from names and sizes: for the card, which
/// asks often. Install looks closer (`place`).
pub fn still_downloading(game: &Game) -> bool {
    download_parts(game).is_some_and(|parts| parts.iter().any(|f| gamefix::download::looks_unfinished(f)))
}

/// Of these downloads, the ones still coming, for the window, which asks every few seconds. The
/// quick look alone counts a file written in the last two minutes as coming, so a finished
/// download kept its badge that long (the user waited about a minute after every download, and
/// looking again changed nothing). The files it flags get gamefix's closer look, Install's: a
/// file written lately but unchanged for 3 s is finished. One wait for all of them.
pub fn still_coming<'a>(games: impl IntoIterator<Item = &'a Game>) -> std::collections::HashSet<String> {
    let flagged: Vec<(&str, Vec<PathBuf>)> = games
        .into_iter()
        .filter_map(|g| download_parts(g).filter(|parts| parts.iter().any(|f| gamefix::download::looks_unfinished(f))).map(|parts| (g.id.as_str(), parts)))
        .collect();
    let all: Vec<PathBuf> = flagged.iter().flat_map(|(_, parts)| parts.iter().cloned()).collect();
    let left = gamefix::download::unfinished(&all);
    flagged.into_iter().filter(|(_, parts)| parts.iter().any(|p| left.contains(p))).map(|(id, _)| id.to_string()).collect()
}

pub(crate) fn archive_kind(file: &Path) -> Option<&'static str> {
    let name = file.file_name()?.to_string_lossy().to_lowercase();
    if [".tar.gz", ".tar.xz", ".tar.bz2", ".tgz"].iter().any(|e| name.ends_with(e)) {
        Some("tar")
    } else if name.ends_with(".rar") {
        Some("rar")
    } else if name.ends_with(".zip") || name.ends_with(".7z") {
        Some("7z")
    } else {
        None
    }
}

fn run(program: &str, args: &[&str]) -> Result<(), String> {
    let out = Command::new(program).args(args).stdin(Stdio::null()).output().map_err(|e| format!("{program}: {e}"))?;
    if out.status.success() {
        return Ok(());
    }
    let err = String::from_utf8_lossy(&out.stderr);
    let last = err.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("it stopped with an error");
    Err(format!("{program} couldn't unpack it: {last}"))
}

/// Unpacks the archive into the (new, empty) folder. No password is ever asked for.
pub(crate) fn unpack(file: &Path, into: &Path) -> Result<(), String> {
    let (f, d) = (file.to_string_lossy(), into.to_string_lossy());
    match archive_kind(file) {
        Some("tar") => run("tar", &["-xf", &f, "-C", &d]),
        Some("rar") => run("unrar", &["x", "-p-", "-y", "-idq", &f, &format!("{d}/")]).or_else(|_| run("unar", &["-q", "-f", "-D", "-o", &d, &f])),
        Some(_) => run("7z", &["x", "-y", "-pX", &format!("-o{d}"), &f]),
        None => Err("not an archive".into()),
    }
}

/// A folder name from the game's name: what file systems allow, and not one already used.
pub fn free_folder(parent: &Path, name: &str) -> PathBuf {
    let clean: String = name.chars().map(|c| if c == '/' || c == '\\' || c.is_control() || c == ':' { ' ' } else { c }).collect();
    let base = clean.split_whitespace().collect::<Vec<_>>().join(" ");
    let base = if base.is_empty() || base.starts_with('.') { "Game".to_string() } else { base };
    (1..).map(|n| parent.join(if n == 1 { base.clone() } else { format!("{base} {n}") })).find(|p| !p.exists()).unwrap()
}

/// The folder's only entry, when it holds just one archive (a download packed twice).
fn only_archive(dir: &Path) -> Option<PathBuf> {
    let entries: Vec<PathBuf> = fs::read_dir(dir).ok()?.flatten().map(|e| e.path()).collect();
    match entries.as_slice() {
        [one] if one.is_file() && archive_kind(one).is_some() => Some(one.clone()),
        _ => None,
    }
}

/// A folder an earlier Install unpacked this download into, still there: used again, not
/// unpacked a second time (an install whose installer was cancelled left 1.4 GB of discs).
fn unpacked_before(games: &Path, file: &Path) -> Option<PathBuf> {
    fs::read_dir(games).ok()?.flatten().map(|e| e.path()).find(|dir| fs::read_to_string(dir.join(SOURCE_NOTE)).is_ok_and(|t| Path::new(t.trim_end_matches('\n')) == file))
}

/// Unpacks the archive into a new folder in the games folder, named after the game.
fn unpack_into_games(game: &Game, file: &Path) -> Result<PathBuf, String> {
    let games = gamefix::setup::games_folder();
    fs::create_dir_all(&games).map_err(|e| format!("The games folder {} isn't there: {e}", games.display()))?;
    if let Some(dir) = unpacked_before(&games, file) {
        return Ok(dir);
    }
    // Unpacked beside its final place, then renamed: a failed unpack leaves no half game.
    let work = games.join(format!(".lobby-unpacking-{}", std::process::id()));
    let _ = fs::remove_dir_all(&work);
    fs::create_dir_all(&work).map_err(|e| e.to_string())?;
    let unpacked = unpack(file, &work).and_then(|_| {
        while let Some(inner) = only_archive(&work) {
            unpack(&inner, &work)?;
            fs::remove_file(&inner).map_err(|e| e.to_string())?;
        }
        Ok(())
    });
    if let Err(e) = unpacked {
        let _ = fs::remove_dir_all(&work);
        return Err(e);
    }
    let folder = free_folder(&games, &game.name);
    fs::rename(&work, &folder).map_err(|e| format!("Couldn't name the folder {}: {e}", folder.display()))?;
    fs::write(folder.join(SOURCE_NOTE), format!("{}\n", file.display())).map_err(|e| e.to_string())?;
    Ok(folder)
}

pub struct Installed {
    /// What happened, a line each (gamefix's ✓ and ✗ lines among them).
    pub lines: Vec<String>,
    /// The app-menu entry that starts it, when it could be set up.
    pub entry: Option<PathBuf>,
    /// The extras put in after it ("the patch", "the fix").
    pub extras: Vec<&'static str>,
}

/// A game's files in place, not set up yet: the folder (or the installer, which gamefix runs),
/// and the name to set it up under (None: gamefix names it after what it installs).
pub struct Placed {
    pub target: PathBuf,
    pub name: Option<String>,
    lines: Vec<String>,
    /// Patches and fixes downloaded beside it, put in once it's installed.
    extras: Vec<crate::extras::Extra>,
}

/// Puts a game's files in place: a download unpacked into the games folder (an installer or a
/// disc image stays as it is), or an owned game downloaded from its store. Takes a while.
/// What the game needs to run (`launch::setup_need`) is asked about after this, before
/// `set_up`.
pub fn place(game: &Game, progress: &dyn Fn(f64)) -> Result<Placed, String> {
    if game.kind == Kind::Owned {
        let (store, id) = owned_in(game).ok_or_else(|| format!("{} isn't owned in a store lobby is signed in to.", game.name))?;
        let folder = store.install(&id, progress)?;
        let lines = vec![format!("✓ Downloaded it from {} into {}.", store.name(), folder.display())];
        return Ok(Placed { target: folder, name: Some(game.name.clone()), lines, extras: Vec::new() });
    }
    let file = download(game).ok_or_else(|| format!("{}'s download isn't there any more.", game.name))?;
    if let Some(part) = download_parts(game).and_then(|parts| gamefix::download::unfinished(&parts).into_iter().next()) {
        return Err(format!("{} is still downloading ({} isn't all there yet). Install it once that's finished.", game.name, part.display()));
    }
    // An unpacked game is named as lobby names it; an installer or a disc image is run by
    // gamefix, which names it after what it installs.
    let extras = crate::extras::for_download(&file);
    if archive_kind(&file).is_some() {
        let folder = unpack_into_games(game, &file)?;
        let lines = vec![format!("✓ Unpacked it into {}.", folder.display())];
        return Ok(Placed { target: folder, name: Some(game.name.clone()), lines, extras });
    }
    Ok(Placed { target: file, name: None, lines: Vec::new(), extras })
}

/// The patches and fixes Install puts in after the game, for `lobby how`.
pub fn extras(game: &Game) -> Vec<crate::extras::Extra> {
    download(game).map(|f| crate::extras::for_download(&f)).unwrap_or_default()
}

/// gamefix sets the placed game up (an app-menu entry with its fixes, which starts it).
pub fn set_up(placed: Placed) -> Installed {
    let mut lines = placed.lines;
    match crate::launch::set_up(&placed.target, placed.name.as_deref()) {
        Ok((done, entry)) => {
            let target = entry.as_deref().and_then(crate::extras::Target::from_entry);
            let (Some(target), false) = (target, placed.extras.is_empty()) else {
                lines.extend(done);
                return Installed { lines, entry, extras: Vec::new() };
            };
            // The game's patches and fixes, then gamefix again, which sees the game as they
            // left it (a No-CD program instead of a protected one, a fix's DLLs to load); its
            // earlier warnings were about the game before them.
            lines.extend(done.into_iter().filter(|l| !l.starts_with('✗')));
            let (put_lines, extras) = crate::log::timed("install: patches and fixes", || crate::extras::put_in(&placed.extras, &target));
            lines.extend(put_lines);
            if extras.is_empty() {
                return Installed { lines, entry, extras };
            }
            match crate::launch::set_up(&placed.target, placed.name.as_deref()) {
                Ok((again, entry)) => {
                    lines.extend(again.into_iter().filter(|l| l.starts_with('✗')));
                    Installed { lines, entry, extras }
                }
                Err(e) => {
                    lines.push(format!("✗ {e}"));
                    Installed { lines, entry, extras }
                }
            }
        }
        Err(e) => {
            lines.push(format!("✗ {e}"));
            Installed { lines, entry: None, extras: Vec::new() }
        }
    }
}

/// The app id of a game the Steam account owns and hasn't installed: Steam installs it
/// (`install_with_steam`), not Lobby.
pub fn steam_owned(game: &Game) -> Option<u32> {
    if game.kind != Kind::Owned {
        return None;
    }
    game.entries.iter().filter(|e| e.source == crate::library::Source::Steam).flat_map(|e| e.keys.iter()).find_map(|k| if let crate::library::Key::SteamApp(id) = k { Some(*id) } else { None })
}

/// Opens Steam's install window for a game the account owns (Steam picks the library and
/// downloads it; the next scan finds it).
pub fn install_with_steam(game: &Game, appid: u32) -> Result<(), String> {
    crate::launch::start("Steam", &["steam".to_string(), format!("steam://install/{appid}")], None, &crate::store::log_file(&game.id))
}

/// The store and the game's id there, for a game owned in a store lobby is signed in to.
pub fn owned_in(game: &Game) -> Option<(crate::stores::Store, String)> {
    game.entries.iter().filter(|e| e.kind == Kind::Owned).flat_map(|e| e.keys.iter()).find_map(|k| match k {
        crate::library::Key::Store(s, id) => Some((crate::stores::Store::from_id(s)?, id.clone())),
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asks_gamefix_about_a_downloads_own_files() {
        use crate::library::{Entry, Source};
        use std::os::unix::fs::FileExt;
        let dir = std::env::temp_dir().join(format!("lobby-unfinished-{}", std::process::id()));
        fs::create_dir_all(dir.join("repack")).unwrap();
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
        let setup = dir.join("repack/setup.exe");
        fs::write(&setup, vec![b'M'; 2 << 20]).unwrap();
        fs::File::options().write(true).open(&setup).unwrap().set_modified(old).unwrap();
        let mut e = Entry::new(Source::Folder, "G");
        (e.kind, e.program, e.folder) = (Kind::Download, Some(setup.clone()), Some(dir.join("repack")));
        let game: Game = serde_json::from_value(serde_json::json!({"name": "G", "kind": "Download", "entries": [], "missing": false})).unwrap();
        let mut game = Game { entries: vec![e], ..game };
        assert!(!still_downloading(&game));
        // RoadCraft's repack at 30%: its first part's start there, most of it not on disk.
        let part = fs::File::create(dir.join("repack/fg-01.bin")).unwrap();
        part.set_len(4 << 20).unwrap();
        part.write_all_at(b"ArC\x01", 0).unwrap();
        part.set_modified(old).unwrap();
        assert!(still_downloading(&game));
        let refused = place(&game, &|_| {}).err().unwrap();
        assert!(refused.contains("still downloading") && refused.contains("fg-01.bin"), "{refused}");
        // Loose in a folder of downloads, others' files say nothing about it.
        game.entries[0].folder = None;
        assert!(!still_downloading(&game));
        // An installed game has no download to wait for.
        game.entries[0].folder = Some(dir.join("repack"));
        game.kind = Kind::Game;
        assert!(!still_downloading(&game));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_download_just_finished_is_not_coming() {
        use crate::library::{Entry, Source};
        let dir = std::env::temp_dir().join(format!("lobby-coming-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("game.zip");
        fs::write(&file, vec![b'P'; 1 << 20]).unwrap();
        let mut e = Entry::new(Source::Folder, "G");
        (e.kind, e.program) = (Kind::Download, Some(file.clone()));
        let game: Game = serde_json::from_value(serde_json::json!({"id": "g", "name": "G", "kind": "Download", "entries": [], "missing": false})).unwrap();
        let game = Game { entries: vec![e], ..game };
        // Written a moment ago: the quick look can't tell, the closer one can.
        assert!(still_downloading(&game));
        assert!(still_coming([&game]).is_empty());
        // Still being written.
        let writer = std::thread::spawn({
            let file = file.clone();
            move || {
                for _ in 0..8 {
                    std::thread::sleep(std::time::Duration::from_millis(500));
                    use std::io::Write;
                    fs::OpenOptions::new().append(true).open(&file).unwrap().write_all(&[b'P'; 4096]).unwrap();
                }
            }
        });
        assert_eq!(still_coming([&game]), ["g".to_string()].into());
        writer.join().unwrap();
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn folder_names() {
        let dir = std::env::temp_dir().join(format!("lobby-install-{}", std::process::id()));
        fs::create_dir_all(dir.join("Battle Tank")).unwrap();
        assert_eq!(free_folder(&dir, "Battle Tank"), dir.join("Battle Tank 2"));
        assert_eq!(free_folder(&dir, "Gnome Issues: Holiday / Chronicles"), dir.join("Gnome Issues Holiday Chronicles"));
        assert_eq!(free_folder(&dir, ".hidden"), dir.join("Game"));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn archive_kinds() {
        assert_eq!(archive_kind(Path::new("/d/KH4 - Linux.tar.gz")), Some("tar"));
        assert_eq!(archive_kind(Path::new("/d/Dungeon Dash.rar")), Some("rar"));
        assert_eq!(archive_kind(Path::new("/d/Glober.zip")), Some("7z"));
        assert_eq!(archive_kind(Path::new("/d/StarshipAssassinSetup_beta5.exe")), None);
    }
}
