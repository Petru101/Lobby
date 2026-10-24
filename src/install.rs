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

fn archive_kind(file: &Path) -> Option<&'static str> {
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
fn unpack(file: &Path, into: &Path) -> Result<(), String> {
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

/// Unpacks the archive into a new folder in the games folder, named after the game.
fn unpack_into_games(game: &Game, file: &Path) -> Result<PathBuf, String> {
    let games = gamefix::setup::games_folder();
    fs::create_dir_all(&games).map_err(|e| format!("The games folder {} isn't there: {e}", games.display()))?;
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
}

/// A game's files in place, not set up yet: the folder (or the installer, which gamefix runs),
/// and the name to set it up under (None: gamefix names it after what it installs).
pub struct Placed {
    pub target: PathBuf,
    pub name: Option<String>,
    lines: Vec<String>,
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
        return Ok(Placed { target: folder, name: Some(game.name.clone()), lines });
    }
    let file = download(game).ok_or_else(|| format!("{}'s download isn't there any more.", game.name))?;
    // An unpacked game is named as lobby names it; an installer or a disc image is run by
    // gamefix, which names it after what it installs.
    if archive_kind(&file).is_some() {
        let folder = unpack_into_games(game, &file)?;
        let lines = vec![format!("✓ Unpacked it into {}.", folder.display())];
        return Ok(Placed { target: folder, name: Some(game.name.clone()), lines });
    }
    Ok(Placed { target: file, name: None, lines: Vec::new() })
}

/// gamefix sets the placed game up (an app-menu entry with its fixes, which starts it).
pub fn set_up(placed: Placed) -> Installed {
    let mut lines = placed.lines;
    match crate::launch::set_up(&placed.target, placed.name.as_deref()) {
        Ok((done, entry)) => {
            lines.extend(done);
            Installed { lines, entry }
        }
        Err(e) => {
            lines.push(format!("✗ {e}"));
            Installed { lines, entry: None }
        }
    }
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
