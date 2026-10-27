//! Moving an installed game to another folder or drive ("Move…" on its page, `lobby move`):
//! its own folder, or, when the game lives inside its Windows setup (a Lutris or Faugus game
//! in drive_c), the whole Wine prefix. On one drive it's renamed; across drives it's copied
//! into `.lobby-moving-<name>` beside where it goes, checked (every file, every byte), renamed
//! into place, and only then is the old copy deleted, after everything that names the old
//! place names the new one: gamefix's records and the app-menu and desktop entries (the
//! program, Path, X-Gamefix-Game, WINEPREFIX), a gamefix install note in its prefix, Steam's
//! shortcuts (with Steam closed), Lobby's library, taken-over games, GOG installs, Wine
//! setups and the folders it looks in, and legendary's record for an Epic game
//! (`legendary move --skip-move`). Stopped halfway, the game still starts from where it was.

use crate::library::{Game, Key};
use std::fs;
use std::path::{Path, PathBuf};

/// What moves, and how its records follow.
pub struct Plan {
    /// The folder that moves: the game's, or its Wine prefix holding it.
    pub from: PathBuf,
    pub prefix: bool,
    /// An Epic game Lobby installed: legendary is told where it went.
    pub epic: Option<String>,
}

/// What would move, or why the game can't be moved by Lobby.
pub fn plan(game: &Game) -> Result<Plan, String> {
    use crate::uninstall::How;
    if game.kind != crate::library::Kind::Game || game.gone {
        return Err("Only an installed game can be moved.".into());
    }
    match crate::uninstall::how(game) {
        How::Steam(_) => return Err("Steam moves its own games: in Steam, its Properties → Installed Files → Move install folder.".into()),
        How::SteamCopy(..) => return Err("It's in a Steam library Steam doesn't use: Move into Steam puts it in one of Steam's.".into()),
        _ => {}
    }
    if crate::battlenet::game_folder(game).is_some() {
        return Err("Battle.net keeps its own record of where the game is: move it from Battle.net.".into());
    }
    // A launcher still starts it (one Lobby hasn't taken over): it wouldn't find it after.
    if let crate::launch::Plan::Run { command, .. } = crate::launch::plan(game) {
        if let Some(who) = crate::launch::hands_over(&command) {
            return Err(format!("{who} starts it, and wouldn't find it in another place."));
        }
    }
    let epic = match crate::stores::installed_in(game) {
        Some((crate::stores::Store::Epic, app)) => Some(app),
        _ => None,
    };
    let folder = game.folder().map(Path::to_path_buf).or_else(|| game.program().and_then(Path::parent).map(Path::to_path_buf)).ok_or("Lobby doesn't know the game's folder.")?;
    // A game inside its Wine prefix goes with the whole prefix.
    let prefix = game.entries.iter().filter_map(|e| e.prefix.clone()).chain(crate::windows_saves::prefix(game)).map(|p| real(&p)).find(|p| p.join("drive_c").is_dir() && real(&folder).starts_with(p));
    let from = prefix.clone().unwrap_or_else(|| real(&folder));
    crate::uninstall::found_folder(game, &from).map_err(|what| {
        // found_folder says "which holds …" or "the top of a drive".
        let what = what.strip_prefix("which ").map_or(format!("is {what}"), str::to_string);
        format!("Its folder, {}, {what}, so Lobby can't tell what's only this game's.", from.display())
    })?;
    Ok(Plan { from, prefix: prefix.is_some(), epic })
}

fn real(p: &Path) -> PathBuf {
    fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

fn device(p: &Path) -> Option<u64> {
    fs::metadata(p).ok().map(|m| std::os::unix::fs::MetadataExt::dev(&m))
}

/// Free bytes for files there.
pub fn free_space(dir: &Path) -> Option<u64> {
    let name = std::ffi::CString::new(std::os::unix::ffi::OsStrExt::as_bytes(dir.as_os_str())).ok()?;
    let mut s: libc::statvfs = unsafe { std::mem::zeroed() };
    (unsafe { libc::statvfs(name.as_ptr(), &mut s) } == 0).then(|| s.f_bavail as u64 * s.f_frsize as u64)
}

/// Files and bytes under a folder (links not followed).
pub fn measure(dir: &Path) -> (u64, u64) {
    let (mut files, mut bytes) = (0, 0);
    let mut dirs = vec![dir.to_path_buf()];
    while let Some(d) = dirs.pop() {
        for e in fs::read_dir(&d).into_iter().flatten().flatten() {
            let Ok(m) = e.path().symlink_metadata() else { continue };
            if m.is_dir() {
                dirs.push(e.path());
            } else {
                files += 1;
                bytes += if m.is_file() { m.len() } else { 0 };
            }
        }
    }
    (files, bytes)
}

/// Where the game goes when moved into `into`: a folder of its own name there.
pub fn target(plan: &Plan, into: &Path) -> Option<PathBuf> {
    Some(into.join(plan.from.file_name()?))
}

/// Moves the game into `into` (its folder keeps its name). `progress` gets 0.0–1.0 while
/// copying. Returns what was done, for the user.
pub fn move_game(game: &Game, into: &Path, progress: &dyn Fn(f64)) -> Result<Vec<String>, String> {
    let plan = plan(game)?;
    let games = crate::store::Library::load().map(|l| l.games).unwrap_or_default();
    if crate::running::find(&games).contains_key(&game.id) {
        return Err(format!("{} is running: stop it first.", game.name));
    }
    let into = real(into);
    if !into.is_dir() {
        return Err(format!("{} isn't a folder.", into.display()));
    }
    let to = target(&plan, &into).ok_or("no folder name")?;
    if real(&to) == real(&plan.from) {
        return Err(format!("It's already in {}.", into.display()));
    }
    if to.exists() {
        return Err(format!("There's already a {} there.", to.display()));
    }
    if into.starts_with(&plan.from) {
        return Err("It can't go inside its own folder.".into());
    }
    let mounts = crate::drives::mounts();
    if plan.prefix && crate::drives::mount_of(&into, &mounts).is_some_and(|m| m.windows_format()) {
        return Err("It lives in its Windows setup (Wine prefix), and Proton needs that on a Linux-format drive, not a Windows-format one.".into());
    }
    // Steam reads its shortcuts only at start and writes them on quitting.
    let steam = gamefix::steam::Steam::find();
    let shortcut_files: Vec<PathBuf> = shortcut_files().into_iter().filter(|f| fs::read(f).is_ok_and(|d| contains(&d, plan.from.as_os_str().as_encoded_bytes()))).collect();
    if !shortcut_files.is_empty() && steam.as_ref().is_some_and(|s| s.running()) {
        return Err("A Steam shortcut starts it from there: close Steam first, so Lobby can point the shortcut at the new place.".into());
    }
    let same_drive = device(&plan.from) == device(&into);
    let mut done = Vec::new();
    if same_drive {
        fs::rename(&plan.from, &to).map_err(|e| format!("Couldn't move {}: {e}", plan.from.display()))?;
    } else {
        let (files, bytes) = measure(&plan.from);
        if free_space(&into).is_some_and(|free| free < bytes + bytes / 50 + (100 << 20)) {
            return Err(format!("There's not enough room there: it needs {}, and {} is free.", crate::windows_saves::size_text(bytes), crate::windows_saves::size_text(free_space(&into).unwrap_or(0))));
        }
        let temp = into.join(format!(".lobby-moving-{}", plan.from.file_name().unwrap().to_string_lossy()));
        if temp.exists() {
            fs::remove_dir_all(&temp).map_err(|e| format!("{}: {e}", temp.display()))?;
        }
        let copied = std::cell::Cell::new(0u64);
        let result = copy_tree(&plan.from, &temp, &(plan.from.clone(), to.clone()), &|n| {
            copied.set(copied.get() + n);
            progress(copied.get() as f64 / bytes.max(1) as f64);
        });
        let checked = result.and_then(|()| {
            let got = measure(&temp);
            if got == (files, bytes) {
                Ok(())
            } else {
                Err(format!("the copy has {} files and {} bytes, not {files} and {bytes}", got.0, got.1))
            }
        });
        if let Err(e) = checked.and_then(|()| fs::rename(&temp, &to).map_err(|e| e.to_string())) {
            let _ = fs::remove_dir_all(&temp);
            return Err(format!("Couldn't copy it: {e}. Nothing changed: it's still in {}.", plan.from.display()));
        }
        done.push(format!("Copied {} files ({}) to {}.", files, crate::windows_saves::size_text(bytes), to.display()));
    }
    // Everything that names the old place, written as the scan writes paths (/home, not
    // /var/home: else the next scan wouldn't know its own game).
    let written = crate::library::norm(&to);
    let n = point_records(&plan.from, &written, game, &shortcut_files)?;
    done.push(format!("Pointed {n} records at the new place."));
    if let Some(app) = &plan.epic {
        let mut c = crate::stores::epic::legendary_command()?;
        c.args(["-y", "move", app, &into.display().to_string(), "--skip-move"]);
        crate::stores::output(c).map_err(|e| format!("legendary didn't take the new place: {e}"))?;
        done.push("Told legendary where it is now.".into());
    }
    // Looked at from now on, wherever that is.
    let mut places = crate::places::Places::load();
    let roots: Vec<PathBuf> = crate::sources::Home::find().map(|h| crate::sources::folders::roots(&h)).unwrap_or_default().iter().map(|r| real(r)).collect();
    if !roots.iter().any(|r| to.starts_with(r)) && !places.games.iter().any(|g| real(g) == to) {
        places.games.push(written.clone());
        places.save()?;
    }
    if !same_drive {
        fs::remove_dir_all(&plan.from).map_err(|e| format!("It's in the new place and starts from there, but the old copy couldn't be deleted ({e}): delete {} yourself.", plan.from.display()))?;
        done.push(format!("Deleted the old copy in {}.", plan.from.display()));
    }
    crate::log::line(&format!("moved {} from {} to {}", game.name, plan.from.display(), to.display()));
    Ok(done)
}

/// Copies a folder: files (their permissions and times), links as links (one pointing inside
/// the folder moving, `moving.0`, by its full path points to the same file in its new place,
/// `moving.1`), folders.
fn copy_tree(from: &Path, to: &Path, moving: &(PathBuf, PathBuf), progress: &dyn Fn(u64)) -> Result<(), String> {
    use std::io::{Read, Write};
    fs::create_dir_all(to).map_err(|e| format!("{}: {e}", to.display()))?;
    for e in fs::read_dir(from).map_err(|e| format!("{}: {e}", from.display()))? {
        let e = e.map_err(|e| e.to_string())?;
        let (src, dst) = (e.path(), to.join(e.file_name()));
        let meta = src.symlink_metadata().map_err(|e| format!("{}: {e}", src.display()))?;
        if meta.file_type().is_symlink() {
            let mut link = fs::read_link(&src).map_err(|e| format!("{}: {e}", src.display()))?;
            if let Ok(inside) = link.strip_prefix(&moving.0) {
                link = moving.1.join(inside);
            }
            std::os::unix::fs::symlink(&link, &dst).map_err(|e| format!("{}: {e}", dst.display()))?;
        } else if meta.is_dir() {
            copy_tree(&src, &dst, moving, progress)?;
        } else if meta.is_file() {
            let mut input = fs::File::open(&src).map_err(|e| format!("{}: {e}", src.display()))?;
            let mut output = fs::File::create(&dst).map_err(|e| format!("{}: {e}", dst.display()))?;
            let mut buf = vec![0u8; 4 << 20];
            loop {
                let n = input.read(&mut buf).map_err(|e| format!("{}: {e}", src.display()))?;
                if n == 0 {
                    break;
                }
                output.write_all(&buf[..n]).map_err(|e| format!("{}: {e}", dst.display()))?;
                progress(n as u64);
            }
            let _ = output.set_permissions(meta.permissions());
            if let Ok(modified) = meta.modified() {
                let _ = output.set_times(fs::FileTimes::new().set_modified(modified));
            }
            output.sync_all().map_err(|e| format!("{}: {e}", dst.display()))?;
        }
    }
    let _ = fs::set_permissions(to, fs::metadata(from).map_err(|e| e.to_string())?.permissions());
    Ok(())
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty() && hay.windows(needle.len()).any(|w| w == needle)
}

/// Both ways a home path is written here (/var/home is /home on Bazzite).
fn spellings(p: &Path) -> Vec<String> {
    let s = p.display().to_string();
    let mut out = vec![s.clone()];
    if let Some(rest) = s.strip_prefix("/var/home/") {
        out.push(format!("/home/{rest}"));
    } else if let Some(rest) = s.strip_prefix("/home/") {
        out.push(format!("/var/home/{rest}"));
    }
    // /var/home/… first: /home/… is inside it.
    out.sort_by_key(|s| std::cmp::Reverse(s.len()));
    out
}

/// `text` with the old place replaced by the new one, only where it's the whole path or the
/// start of one ("Woodo" never matches "Woodo 2"). In an app-menu entry (`words`) a space ends
/// a path too, as in an unquoted `WINEPREFIX=/a/pfx umu-run`, unless a folder beside it is
/// named on with that space; anywhere else a path ends at a quote, a tab or a line's end.
pub fn repoint(text: &str, from: &Path, to: &Path, words: bool) -> String {
    repoint_beside(text, from, to, words.then(|| siblings(from)).as_deref())
}

/// Names in the folder holding `from` that start with its name and a space.
fn siblings(from: &Path) -> Vec<String> {
    let Some(name) = from.file_name().map(|n| format!("{} ", n.to_string_lossy())) else { return Vec::new() };
    fs::read_dir(from.parent().unwrap_or(Path::new("/"))).into_iter().flatten().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| n.starts_with(&name)).collect()
}

fn repoint_beside(text: &str, from: &Path, to: &Path, siblings: Option<&[String]>) -> String {
    let mut out = text.to_string();
    let to = to.display().to_string();
    let name_len = from.file_name().map_or(0, |n| n.to_string_lossy().len());
    for old in spellings(from) {
        let mut result = String::with_capacity(out.len());
        let mut rest = out.as_str();
        while let Some(i) = rest.find(&old) {
            let tail = &rest[i + old.len()..];
            let after = tail.chars().next();
            let space = after == Some(' ') && siblings.is_some_and(|sib| !sib.iter().any(|s| tail.starts_with(&s[name_len..])));
            let ends = space || after.is_none_or(|c| matches!(c, '/' | '"' | '\'' | '\n' | '\t' | '\\' | ';' | '\0'));
            result.push_str(&rest[..i]);
            result.push_str(if ends { &to } else { &old });
            rest = &rest[i + old.len()..];
        }
        result.push_str(rest);
        out = result;
    }
    out
}

/// Rewrites a text file naming the old place; whether it changed.
pub(crate) fn repoint_file(file: &Path, from: &Path, to: &Path) -> Result<bool, String> {
    let Ok(text) = fs::read_to_string(file) else { return Ok(false) };
    let new = repoint(&text, from, to, file.extension().is_some_and(|x| x == "desktop"));
    if new == text {
        return Ok(false);
    }
    let tmp = file.with_extension("lobby-moving");
    fs::write(&tmp, &new).and_then(|()| fs::rename(&tmp, file)).map_err(|e| format!("{}: {e}", file.display()))?;
    Ok(true)
}

fn shortcut_files() -> Vec<PathBuf> {
    let Some(home) = crate::sources::Home::find() else { return Vec::new() };
    crate::sources::steam::roots(&home).iter().flat_map(|r| fs::read_dir(r.join("userdata")).into_iter().flatten().flatten().map(|u| u.path().join("config/shortcuts.vdf"))).filter(|f| f.is_file()).collect()
}

/// Points every record naming the old place at the new one; how many files changed.
fn point_records(from: &Path, to: &Path, game: &Game, shortcut_files: &[PathBuf]) -> Result<usize, String> {
    let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    let data = std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).unwrap_or_else(|| home.join(".local/share"));
    let mut files: Vec<PathBuf> = Vec::new();
    // gamefix's records, and the entries it and others made.
    let listed = |dir: &Path, ext: &str| -> Vec<PathBuf> { fs::read_dir(dir).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == ext)).collect() };
    files.extend(listed(&data.join("gamefix/games"), "log"));
    files.extend(listed(&data.join("applications"), "desktop"));
    files.extend(listed(&home.join("Desktop"), "desktop"));
    files.extend(game.entries.iter().flat_map(|e| e.keys.iter()).filter_map(|k| if let Key::Desktop(p) = k { Some(p.clone()) } else { None }));
    // gamefix's install note in the game's prefix (it names the program when that's elsewhere).
    files.extend(game.entries.iter().filter_map(|e| e.prefix.clone()).chain(crate::windows_saves::prefix(game)).map(|p| repoint(&p.display().to_string(), from, to, false)).map(|p| PathBuf::from(p).join("gamefix-install.txt")));
    // Lobby's own.
    let lobby = crate::store::dir();
    files.extend(["library.json", "taken-over.json", "gog-installed.json", "wine-setups.json"].iter().map(|f| lobby.join(f)));
    files.push(crate::stores::config_home().join("lobby/places.json"));
    files.sort();
    files.dedup();
    let mut changed = 0;
    for f in &files {
        if repoint_file(f, from, to)? {
            changed += 1;
        }
    }
    // Steam's shortcuts (binary): their Exe, StartDir and launch options.
    for f in shortcut_files {
        let data = fs::read(f).map_err(|e| e.to_string())?;
        let mut doc = gamefix::steam::parse(&data)?;
        fn walk(items: &mut [(Vec<u8>, gamefix::steam::Node)], from: &Path, to: &Path) {
            for (_, node) in items {
                match node {
                    gamefix::steam::Node::Map(m) => walk(m, from, to),
                    gamefix::steam::Node::Str(s) => {
                        if let Ok(text) = std::str::from_utf8(s) {
                            *s = repoint(text, from, to, false).into_bytes();
                        }
                    }
                    gamefix::steam::Node::Int(_) => {}
                }
            }
        }
        walk(&mut doc, from, to);
        let new = gamefix::steam::write(&doc);
        if new != data {
            let _ = fs::copy(f, f.with_extension("vdf.lobby-backup"));
            fs::write(f, new).map_err(|e| format!("{}: {e}", f.display()))?;
            changed += 1;
        }
    }
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_whole_paths_are_repointed() {
        let (from, to) = (Path::new("/run/media/p/Games/Woodo"), Path::new("/home/p/Games/Woodo"));
        let entry = "Exec=env WINEPREFIX=/var/home/p/pfx umu-run /run/media/p/Games/Woodo/Woodo.exe\nPath=/run/media/p/Games/Woodo\nX-Other=/run/media/p/Games/Woodo 2/x.exe\n";
        assert_eq!(repoint_beside(entry, from, to, Some(&["Woodo 2".into()])), "Exec=env WINEPREFIX=/var/home/p/pfx umu-run /home/p/Games/Woodo/Woodo.exe\nPath=/home/p/Games/Woodo\nX-Other=/run/media/p/Games/Woodo 2/x.exe\n");
        // A home path written either way.
        assert_eq!(repoint("\"WINEPREFIX=/var/home/p/Faugus/g\"", Path::new("/home/p/Faugus/g"), Path::new("/run/media/p/Games/g"), true), "\"WINEPREFIX=/run/media/p/Games/g\"");
        // An unquoted prefix followed by the next word, in an entry.
        assert_eq!(repoint("env WINEPREFIX=/home/p/Faugus/g umu-run x.exe", Path::new("/home/p/Faugus/g"), Path::new("/m/g"), true), "env WINEPREFIX=/m/g umu-run x.exe");
        // Elsewhere a space doesn't end a path: another game's "Mover 2" stays, on disk or not.
        assert_eq!(repoint("{\"folder\":\"/s/Mover 2\",\"f\":\"/s/Mover\"}", Path::new("/s/Mover"), Path::new("/d/Mover"), false), "{\"folder\":\"/s/Mover 2\",\"f\":\"/d/Mover\"}");
        assert_eq!(repoint("{\"folder\":\"/home/p/Faugus/gx\"}", Path::new("/home/p/Faugus/g"), Path::new("/x"), false), "{\"folder\":\"/home/p/Faugus/gx\"}");
    }

    #[test]
    fn copies_files_links_and_times() {
        let dir = std::env::temp_dir().join(format!("lobby-move-{}", std::process::id()));
        let src = dir.join("src");
        fs::create_dir_all(src.join("drive_c/Games")).unwrap();
        fs::write(src.join("drive_c/Games/game.exe"), vec![7u8; 10_000]).unwrap();
        std::os::unix::fs::symlink("../drive_c", src.join("c:")).unwrap();
        let total = std::cell::Cell::new(0);
        std::os::unix::fs::symlink(src.join("drive_c/Games"), src.join("games")).unwrap();
        copy_tree(&src, &dir.join("dst"), &(src.clone(), dir.join("final")), &|n| total.set(total.get() + n)).unwrap();
        assert_eq!(fs::read_link(dir.join("dst/games")).unwrap(), dir.join("final/drive_c/Games"));
        assert_eq!(measure(&src), measure(&dir.join("dst")));
        assert_eq!(total.get(), 10_000);
        assert_eq!(fs::read_link(dir.join("dst/c:")).unwrap(), PathBuf::from("../drive_c"));
        assert_eq!(fs::metadata(src.join("drive_c/Games/game.exe")).unwrap().modified().unwrap(), fs::metadata(dir.join("dst/drive_c/Games/game.exe")).unwrap().modified().unwrap());
        fs::remove_dir_all(&dir).unwrap();
    }
}
