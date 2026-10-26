//! Where a game keeps its saves on this computer, for "Saves" on its page and `lobby how`:
//! Ludusavi's manifest paths (`windows_saves::index`) resolved where the game runs here (its
//! Wine prefix for a Windows game, the home folder for a Linux one, or its own home when it's
//! Isolated, and its install folder), plus the folders the stores' clouds sync: Steam Cloud's
//! copy in userdata/<user>/<app>/remote, and GOG's and Epic's for games Lobby installed. Only
//! what's there: a game not played yet has none.

use crate::library::{Game, Key, Source};
use crate::stores::Store;
use crate::windows_saves::{expand, index, paths_for, prefix_user};
use std::path::{Path, PathBuf};

pub struct SaveFolder {
    pub path: PathBuf,
    /// Who says the saves are there: "Saves" (Ludusavi's manifest), or the store whose cloud
    /// syncs that folder.
    pub what: &'static str,
}

pub fn find(game: &Game) -> Vec<SaveFolder> {
    let mut out: Vec<SaveFolder> = Vec::new();
    let mut add = |path: PathBuf, what: &'static str| {
        // "My Games//FalloutNV" (a GOG location) as one path.
        let path: PathBuf = path.components().collect();
        if path.exists() && !out.iter().any(|f| f.path == path) {
            out.push(SaveFolder { path, what });
        }
    };
    let folder = game.folder().map(Path::to_path_buf);
    let prefix = crate::windows_saves::prefix(game).filter(|p| p.join("drive_c").is_dir());
    let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    let data = std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).unwrap_or_else(|| home.join(".local/share"));
    // A Linux game kept Isolated has a home of its own.
    let homes: Vec<PathBuf> = [Some(home.clone()), Some(data.join("lobby/sandboxes").join(&game.id)).filter(|h| h.is_dir())].into_iter().flatten().collect();

    if let Some(paths) = paths_for(game, &index()) {
        for (path, os) in &paths.all {
            let windows = os.as_deref().is_none_or(|o| o == "windows");
            let linux = os.as_deref() == Some("linux") || os.is_none() && prefix.is_none();
            let mut found = Vec::new();
            if windows {
                if let Some(pfx) = &prefix {
                    found.extend(in_windows(path, pfx, folder.as_deref()));
                }
            }
            if linux {
                for h in &homes {
                    found.extend(in_linux(path, h, folder.as_deref()));
                }
            }
            for f in found {
                add(if f.is_dir() { f } else { f.parent().map(Path::to_path_buf).unwrap_or(f) }, "Saves");
            }
        }
    }

    // Where the game's engine keeps saves, for games the manifest doesn't know (most itch.io
    // games): Unity's persistentDataPath (its *_Data/app.info names the company and product),
    // an Unreal project's Saved/SaveGames, RPG Maker MV/MZ's save folder.
    if let Some(program) = game.program() {
        for (path, what) in engine_folders(program, prefix.as_deref(), &homes) {
            add(path, what);
        }
    }

    // Steam Cloud's copy (some games keep their saves only there).
    if let Some(app) = game.entries.iter().filter(|e| e.source == Source::Steam).flat_map(|e| e.keys.iter()).find_map(|k| if let Key::SteamApp(id) = k { Some(*id) } else { None }) {
        for root in crate::sources::Home::find().map(|h| crate::sources::steam::roots(&h)).unwrap_or_default() {
            for user in std::fs::read_dir(root.join("userdata")).into_iter().flatten().flatten() {
                add(user.path().join(app.to_string()).join("remote"), "Steam Cloud");
            }
        }
    }
    // The folders Lobby syncs with GOG's and Epic's clouds.
    match (crate::stores::installed_in(game), &prefix, &folder) {
        (Some((Store::Gog, id)), Some(pfx), Some(dir)) => {
            for f in crate::stores::saves::gog_folders(&id, pfx, dir) {
                add(f, "GOG cloud");
            }
        }
        (Some((Store::Epic, app)), ..) => {
            let installed = crate::stores::config_home().join("lobby/legendary/installed.json");
            let save = std::fs::read_to_string(installed).ok().and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok()).and_then(|v| v.get(&app)?.get("save_path")?.as_str().map(PathBuf::from));
            if let Some(save) = save {
                add(save, "Epic cloud");
            }
        }
        _ => {}
    }

    // Not a folder holding many things: the game's folder itself is fine (some keep saves
    // beside the program), a user folder isn't.
    let user = prefix.as_deref().map(prefix_user);
    out.retain(|f| f.path != home && Some(&f.path) != user.as_ref() && !homes.contains(&f.path));
    // A folder inside another one listed goes.
    let all: Vec<PathBuf> = out.iter().map(|f| f.path.clone()).collect();
    out.retain(|f| !all.iter().any(|a| a != &f.path && f.path.starts_with(a)));
    out
}

fn engine_folders(program: &Path, prefix: Option<&Path>, homes: &[PathBuf]) -> Vec<(PathBuf, &'static str)> {
    let mut out = Vec::new();
    let Some(dir) = program.parent() else { return out };
    let user = prefix.map(prefix_user);
    // Unity: <Game>_Data/app.info is the company's name, then the product's.
    let info = std::fs::read_dir(dir).into_iter().flatten().flatten().map(|e| e.path().join("app.info")).find(|p| p.parent().is_some_and(|d| d.to_string_lossy().ends_with("_Data")) && p.is_file());
    if let Some(info) = info.and_then(|p| std::fs::read_to_string(p).ok()) {
        let mut lines = info.lines().map(str::trim);
        if let (Some(company), Some(product)) = (lines.next().filter(|l| !l.is_empty()), lines.next().filter(|l| !l.is_empty())) {
            if let Some(u) = &user {
                out.push((u.join("AppData/LocalLow").join(company).join(product), "Saves"));
            }
            for h in homes {
                out.push((h.join(".config/unity3d").join(company).join(product), "Saves"));
            }
        }
    }
    // Unreal: <Project>/Binaries/<Platform>/<program>.
    let parts: Vec<String> = program.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
    if let Some(i) = parts.iter().rposition(|p| p == "Binaries").filter(|&i| i > 0) {
        let project = &parts[i - 1];
        if let Some(u) = &user {
            out.push((u.join("AppData/Local").join(project).join("Saved/SaveGames"), "Saves"));
        }
        for h in homes {
            out.push((h.join(".config/Epic").join(project).join("Saved/SaveGames"), "Saves"));
        }
    }
    // RPG Maker MV (www/save) and MZ (save) beside the NW.js program.
    if dir.join("package.json").is_file() {
        out.push((dir.join("www/save"), "Saves"));
        out.push((dir.join("save"), "Saves"));
    }
    out
}

/// A manifest path in the game's Wine prefix (or its install folder).
fn in_windows(path: &str, prefix: &Path, folder: Option<&Path>) -> Vec<PathBuf> {
    let user = prefix_user(prefix);
    let c = prefix.join("drive_c");
    let (start, rest) = path.split_once('/').unwrap_or((path, ""));
    let base = match start {
        "<home>" => user.clone(),
        "<winDocuments>" => user.join("Documents"),
        "<winAppData>" => user.join("AppData/Roaming"),
        "<winLocalAppData>" => user.join("AppData/Local"),
        "<winLocalAppDataLow>" => user.join("AppData/LocalLow"),
        "<winPublic>" => c.join("users/Public"),
        "<winProgramData>" => c.join("ProgramData"),
        "<winDir>" => c.join("windows"),
        "<base>" => match folder {
            Some(f) => f.to_path_buf(),
            None => return Vec::new(),
        },
        _ => return Vec::new(),
    };
    let name = user.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    resolve(&base, rest, &name)
}

/// A manifest path in a Linux home (or the install folder).
fn in_linux(path: &str, home: &Path, folder: Option<&Path>) -> Vec<PathBuf> {
    let (start, rest) = path.split_once('/').unwrap_or((path, ""));
    let base = match start {
        "<home>" => home.to_path_buf(),
        "<xdgData>" => home.join(".local/share"),
        "<xdgConfig>" => home.join(".config"),
        "<base>" => match folder {
            Some(f) => f.to_path_buf(),
            None => return Vec::new(),
        },
        _ => return Vec::new(),
    };
    resolve(&base, rest, &std::env::var("USER").unwrap_or_default())
}

fn resolve(base: &Path, rest: &str, user: &str) -> Vec<PathBuf> {
    let rest = rest.replace("<storeUserId>", "*").replace("<storeGameId>", "*").replace("<osUserName>", user);
    if rest.contains('<') {
        return Vec::new();
    }
    let parts: Vec<&str> = rest.split('/').filter(|p| !p.is_empty()).collect();
    let mut found = Vec::new();
    expand(base, &parts, &mut found);
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_in_a_prefix() {
        let dir = std::env::temp_dir().join(format!("lobby-saves-{}", std::process::id()));
        let user = dir.join("pfx/drive_c/users/steamuser");
        std::fs::create_dir_all(user.join("Documents/My Games/Game/Saves")).unwrap();
        std::fs::write(user.join("Documents/My Games/Game/Saves/slot1.sav"), "").unwrap();
        std::fs::create_dir_all(dir.join("game/save")).unwrap();
        assert_eq!(in_windows("<winDocuments>/my games/game/saves/*.sav", &dir.join("pfx"), None), vec![user.join("Documents/My Games/Game/Saves/slot1.sav")]);
        assert_eq!(in_windows("<base>/save", &dir.join("pfx"), Some(&dir.join("game"))), vec![dir.join("game/save")]);
        assert!(in_windows("<regHkcu>/Software/X", &dir.join("pfx"), None).is_empty());
        assert_eq!(in_linux("<xdgData>/Game", &dir, None), Vec::<PathBuf>::new());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
