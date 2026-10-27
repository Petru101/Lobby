//! Making imported games Lobby's own. Another launcher's files are read once, when a game is
//! imported; what the game needs is Lobby's from then on, and the launcher keeps working:
//! - each Wine prefix an imported game runs in (Heroic's, Lutris's, Faugus's) is moved into
//!   Lobby's folder on the same drive (~/.local/share/lobby/prefixes, else
//!   <drive>/Lobby/prefixes) with a link left where it was, so the launcher finds it there and
//!   there's one set of saves;
//! - a Wine or Proton build of the launcher's own (Lutris's runners/wine/...) is copied into
//!   ~/.local/share/lobby/tools;
//! - Lobby's records are pointed at the new places.
//!
//! Runs after each scan's take-over; a prefix something runs in is left for the next time.
//! `own_prefix` gives a game in a prefix other games share a copy of its own.

use crate::library::{self, Game, Kind};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// The prefixes Lobby keeps for store games taken over from Heroic that aren't made yet (not
/// played since): made in Lobby's folder instead, linked from where Heroic would make them.
fn unmade() -> Vec<(PathBuf, PathBuf)> {
    let setups: std::collections::BTreeMap<String, crate::stores::WineSetup> = crate::stores::read_json(&crate::store::dir().join("wine-setups.json"));
    let lobby = lobby_dirs();
    setups
        .into_values()
        .map(|s| unit(&s.prefix))
        .filter(|p| fs::symlink_metadata(p).is_err() && !lobby.iter().any(|d| p.starts_with(d)))
        .filter_map(|p| Some((p.clone(), home_for(&p)?)))
        .collect()
}

/// The Wine prefixes imported games run in and where Lobby would keep them: (from, to).
fn prefix_moves(games: &[Game]) -> Vec<(PathBuf, PathBuf, Vec<String>)> {
    let mut recorded: Vec<PathBuf> = Vec::new();
    let setups: std::collections::BTreeMap<String, crate::stores::WineSetup> = crate::stores::read_json(&crate::store::dir().join("wine-setups.json"));
    recorded.extend(setups.into_values().map(|s| s.prefix));
    for t in crate::taken::load().into_iter().filter(|t| t.from != "Steam") {
        recorded.extend(t.prefix);
        recorded.extend(t.command.iter().find_map(|w| w.strip_prefix("WINEPREFIX=")).map(PathBuf::from));
    }
    // Only the ones a game runs in now (Andromeda's old Lutris prefix isn't: the EA app's is).
    let used: Vec<(PathBuf, String)> = games
        .iter()
        .filter(|g| g.kind == Kind::Game && !g.gone)
        .filter_map(|g| {
            let plan = crate::launch::plan(g);
            Some((library::norm(&unit(&crate::launch::proton_and_prefix(g, &plan).1?)), g.name.clone()))
        })
        .collect();
    let mut out: Vec<(PathBuf, PathBuf, Vec<String>)> = Vec::new();
    for from in recorded.iter().map(|p| unit(p)) {
        let real = library::norm(&from);
        if out.iter().any(|(f, ..)| library::norm(f) == real) || !movable(&from) {
            continue;
        }
        let names: Vec<String> = used.iter().filter(|(p, _)| *p == real).map(|(_, n)| n.clone()).collect();
        if names.is_empty() {
            continue;
        }
        let Some(to) = home_for(&from) else { continue };
        out.push((from, to, names));
    }
    out
}

/// The folder that is the prefix as a whole: Proton's `<folder>/pfx` is its folder.
fn unit(prefix: &Path) -> PathBuf {
    match prefix.file_name() {
        Some(n) if n == "pfx" && prefix.parent().is_some_and(|p| p.join("pfx").is_dir()) => prefix.parent().unwrap().to_path_buf(),
        _ => prefix.to_path_buf(),
    }
}

/// Lobby's folders for prefixes: in its data folder, or on another drive its own folder there.
fn lobby_dirs() -> Vec<PathBuf> {
    let mut dirs = vec![crate::store::dir().join("prefixes")];
    dirs.extend(crate::drives::mounts().iter().map(|m| m.point.join("Lobby/prefixes")));
    dirs
}

/// A launcher's prefix Lobby can make its own: a real folder (not the link a move left) holding
/// a prefix, not Lobby's, gamefix's or Steam's, nor a folder of many things.
fn movable(dir: &Path) -> bool {
    let Ok(meta) = fs::symlink_metadata(dir) else { return false };
    if !meta.is_dir() || library::prefix_in(dir).is_none() {
        return false;
    }
    let real = library::norm(dir);
    let text = real.display().to_string();
    if lobby_dirs().iter().any(|d| real.starts_with(library::norm(d))) || text.contains("/steamapps/compatdata/") || real.starts_with(library::norm(&crate::store::dir().parent().unwrap_or(Path::new("/")).join("gamefix"))) {
        return false;
    }
    let home = crate::sources::Home::find();
    let wide = home.as_ref().map(|h| crate::sources::folders::roots(h)).unwrap_or_default();
    real.components().count() > 3 && !wide.iter().any(|r| library::norm(r) == real) && home.is_none_or(|h| library::norm(&h.home) != real)
}

fn device(p: &Path) -> Option<u64> {
    fs::metadata(p).ok().map(|m| std::os::unix::fs::MetadataExt::dev(&m))
}

/// Where Lobby keeps a prefix moved from `from`: on the same drive, a free name.
fn home_for(from: &Path) -> Option<PathBuf> {
    let data = crate::store::dir();
    let existing = from.ancestors().find(|a| a.exists())?;
    let base = if device(existing) == device(data.parent()?) {
        data.join("prefixes")
    } else {
        let mounts = crate::drives::mounts();
        crate::drives::mount_of(existing, &mounts)?.point.join("Lobby/prefixes")
    };
    let name = from.file_name()?.to_string_lossy().into_owned();
    (1..).map(|n| if n == 1 { base.join(&name) } else { base.join(format!("{name} {n}")) }).find(|p| fs::symlink_metadata(p).is_err())
}

/// Something runs in the prefix now (a game, its wineserver, a launcher): its environment
/// names it, or its working folder is in it.
fn in_use(prefix: &Path) -> bool {
    let real = library::norm(prefix);
    fs::read_dir("/proc").into_iter().flatten().flatten().filter(|e| e.file_name().to_string_lossy().chars().all(|c| c.is_ascii_digit())).any(|p| {
        let env = fs::read(p.path().join("environ")).unwrap_or_default();
        let named = env.split(|b| *b == 0).filter_map(|v| std::str::from_utf8(v).ok()).filter_map(|v| v.strip_prefix("WINEPREFIX=").or_else(|| v.strip_prefix("STEAM_COMPAT_DATA_PATH="))).any(|v| library::norm(Path::new(v)).starts_with(&real));
        named || fs::read_link(p.path().join("cwd")).is_ok_and(|c| library::norm(&c).starts_with(&real))
    })
}

/// Lobby's own files that name places: pointed at a prefix's or tool's new place.
fn point_records(from: &Path, to: &Path) -> Result<(), String> {
    let lobby = crate::store::dir();
    let mut files: Vec<PathBuf> = ["library.json", "taken-over.json", "wine-setups.json", "gog-installed.json"].iter().map(|f| lobby.join(f)).collect();
    files.push(crate::stores::config_home().join("lobby/places.json"));
    // legendary's record of where each Epic game's cloud saves are.
    files.push(crate::stores::config_home().join("lobby/legendary/installed.json"));
    for game in fs::read_dir(lobby.join("save-backups")).into_iter().flatten().flatten() {
        files.extend(fs::read_dir(game.path()).into_iter().flatten().flatten().map(|b| b.path().join("backup.json")).filter(|f| f.is_file()));
    }
    for f in files {
        crate::moving::repoint_file(&f, from, to)?;
    }
    Ok(())
}

/// The Wine and Proton builds of another launcher's own that imported games run with (a folder
/// under Lutris's runners or Heroic's tools), and where Lobby keeps its copy.
fn tool_copies() -> Vec<(PathBuf, PathBuf)> {
    let mut words: Vec<String> = crate::taken::load().into_iter().flat_map(|t| t.command).collect();
    let setups: std::collections::BTreeMap<String, crate::stores::WineSetup> = crate::stores::read_json(&crate::store::dir().join("wine-setups.json"));
    words.extend(setups.into_values().filter_map(|s| s.proton).map(|p| p.display().to_string()));
    let tools = crate::store::dir().join("tools");
    let mut out: Vec<(PathBuf, PathBuf)> = Vec::new();
    for w in words {
        let path = w.split_once('=').map_or(w.as_str(), |(_, v)| v);
        let parts: Vec<&str> = path.split('/').collect();
        // .../lutris/runners/<kind>/<build>, .../heroic/tools/<kind>/<build>
        let Some(i) = parts.windows(2).position(|p| p == ["lutris", "runners"] || p == ["heroic", "tools"]) else { continue };
        if parts.len() < i + 4 {
            continue;
        }
        let dir = PathBuf::from(parts[..i + 4].join("/"));
        if dir.is_dir() && !out.iter().any(|(d, _)| *d == dir) {
            out.push((dir.clone(), tools.join(parts[i + 3])));
        }
    }
    out
}

/// Makes what imported games need Lobby's own. Returns what it did (or would do, `dry`), a
/// line each.
pub fn run(games: &[Game], dry: bool) -> Vec<String> {
    let mut done = Vec::new();
    for (from, to, names) in prefix_moves(games) {
        let who = match names.as_slice() {
            [one] => one.clone(),
            [a, rest @ ..] => format!("{a} and {} more", rest.len()),
            [] => String::new(),
        };
        if dry {
            done.push(format!("{who}: its Wine setup {} moves to {}, with a link left behind.", from.display(), to.display()));
            continue;
        }
        if in_use(&from) {
            done.push(format!("{who}: its Wine setup {} is in use; it moves next time.", from.display()));
            continue;
        }
        let moved = fs::create_dir_all(to.parent().unwrap()).and_then(|()| fs::rename(&from, &to)).and_then(|()| std::os::unix::fs::symlink(&to, &from));
        match moved.map_err(|e| e.to_string()).and_then(|()| point_records(&from, &to)) {
            Ok(()) => {
                crate::log::line(&format!("import: {} → {} ({who})", from.display(), to.display()));
                done.push(format!("✓ {who}: Wine setup moved to {} (a link stays at {}).", to.display(), from.display()));
            }
            Err(e) => {
                crate::log::line(&format!("import: {} not moved: {e}", from.display()));
                done.push(format!("{who}: its Wine setup {} couldn't be moved: {e}", from.display()));
            }
        }
    }
    for (from, to) in unmade() {
        if dry {
            done.push(format!("{} isn't made yet: it's made in {} instead, with a link.", from.display(), to.display()));
            continue;
        }
        let made = fs::create_dir_all(&to).and_then(|()| fs::create_dir_all(from.parent().unwrap_or(Path::new("/")))).and_then(|()| std::os::unix::fs::symlink(&to, &from));
        match made.map_err(|e| e.to_string()).and_then(|()| point_records(&from, &to)) {
            Ok(()) => done.push(format!("✓ {} will be made in {} (a link stays at {}).", unit(&from).display(), to.display(), from.display())),
            Err(e) => done.push(format!("{} couldn't be set up in Lobby's folder: {e}", from.display())),
        }
    }
    for (from, to) in tool_copies() {
        if dry {
            done.push(format!("{} is copied to {}.", from.display(), to.display()));
            continue;
        }
        let copied = to.is_dir() || Command::new("cp").args(["-a", "--reflink=auto"]).arg(&from).arg(&to).stdin(Stdio::null()).status().is_ok_and(|s| s.success());
        match copied.then(|| point_records(&from, &to)) {
            Some(Ok(())) => {
                crate::log::line(&format!("import: {} copied to {}", from.display(), to.display()));
                done.push(format!("✓ {} copied to {}.", from.display(), to.display()));
            }
            Some(Err(e)) => done.push(format!("{} copied, but records weren't changed: {e}", to.display())),
            None => {
                let _ = fs::remove_dir_all(&to);
                done.push(format!("{} couldn't be copied.", from.display()));
            }
        }
    }
    done
}

/// The other games in the library that run in the game's prefix (none: it has one of its own).
pub fn sharing(game: &Game, all: &[Game]) -> (Option<PathBuf>, Vec<String>) {
    let plan = crate::launch::plan(game);
    let Some(prefix) = crate::launch::proton_and_prefix(game, &plan).1 else { return (None, Vec::new()) };
    let real = fs::canonicalize(&prefix).unwrap_or(prefix.clone());
    (Some(prefix), crate::uninstall::prefix_shared_with(&real, game, all))
}

/// Gives a game that shares its Wine prefix with other games a copy of its own (a reflink copy
/// on btrfs: no room taken until they differ), on the same drive, in Lobby's folder. Only for
/// a store game Lobby keeps the Wine setup of (Heroic's shared prefix). Returns the copy.
pub fn own_prefix(game: &Game, all: &[Game]) -> Result<PathBuf, String> {
    let (store, id) = crate::stores::installed_in(game).ok_or("Only an Epic or GOG game Lobby took over from Heroic can be given its own Wine setup this way.")?;
    let setup = crate::stores::wine_setup(store, &id).ok_or("It has no Wine setup of Heroic's to copy.")?;
    let (_, others) = sharing(game, all);
    if others.is_empty() {
        return Err("Its Wine setup is its own already.".into());
    }
    let from = fs::canonicalize(unit(&setup.prefix)).map_err(|e| format!("{}: {e}", setup.prefix.display()))?;
    if in_use(&from) {
        return Err("Something runs in its Wine setup now: close it first.".into());
    }
    let name = game.folder().and_then(|f| f.file_name()).map_or_else(|| game.id.clone(), |n| n.to_string_lossy().into_owned());
    let base = home_for(&from).and_then(|p| p.parent().map(Path::to_path_buf)).ok_or("no folder for it")?;
    let to = (1..).map(|n| if n == 1 { base.join(&name) } else { base.join(format!("{name} {n}")) }).find(|p| fs::symlink_metadata(p).is_err()).unwrap();
    fs::create_dir_all(&base).map_err(|e| e.to_string())?;
    let ok = Command::new("cp").args(["-a", "--reflink=auto"]).arg(&from).arg(&to).stdin(Stdio::null()).status().is_ok_and(|s| s.success());
    if !ok {
        let _ = fs::remove_dir_all(&to);
        return Err(format!("Couldn't copy {}.", from.display()));
    }
    crate::stores::save_wine_setup(store, &id, crate::stores::WineSetup { prefix: to.clone(), proton: setup.proton })?;
    crate::log::line(&format!("import: {} got its own Wine setup {} (from {})", game.name, to.display(), from.display()));
    Ok(to)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_prefix_is_its_folder() {
        let dir = std::env::temp_dir().join(format!("lobby-import-{}", std::process::id()));
        fs::create_dir_all(dir.join("g/pfx/drive_c")).unwrap();
        assert_eq!(unit(&dir.join("g/pfx")), dir.join("g"));
        assert_eq!(unit(&dir.join("g")), dir.join("g"));
        fs::remove_dir_all(&dir).unwrap();
    }
}
