//! The search of all drives (asked for, never by itself): every folder on the drives and in
//! the home folder, down to a depth, for games outside the folders lobby looks in. A folder is a
//! game when it shows an engine's files (Unity, Unreal, Godot, GameMaker, RPG Maker, Ren'Py,
//! LÖVE, NW.js), GOG's record or Steam's library beside a program; the search doesn't go into
//! it then. Steam libraries (steamapps) are noted too. System folders, Wine prefixes, backups'
//! Windows folders and developers' folders are passed over. What it finds is shown to the user
//! to pick from.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

pub struct Found {
    pub folder: PathBuf,
    /// What showed it's a game ("Unity", "GOG", ...).
    pub why: &'static str,
}

pub struct Results {
    pub games: Vec<Found>,
    pub libraries: Vec<PathBuf>,
}

/// Folders never gone into (lowercase names).
const PASSED: &[&str] = &[
    "$recycle.bin", "system volume information", "windows", "programdata", "appdata", "application data", "local settings", "windowsapps", "wpsystem",
    "perflogs", "recovery", "lost+found", "proc", "sys", "dev", "node_modules", "__pycache__", "site-packages", "dist-packages", "drive_c", "dosdevices",
    "prefixes", "pfx", "compatdata", "shadercache", ".git", "projects", "flatpak", "snap", "containers", "steamapps", "content", "paks",
];

const MAX_DEPTH: usize = 7;

/// Where to look: every drive, and the home folder, each once (/media is a link to /run/media
/// on Fedora's atomic systems).
pub fn starts(home: &crate::sources::Home) -> Vec<PathBuf> {
    let mut all: Vec<PathBuf> = [PathBuf::from("/run/media").join(&home.user), PathBuf::from("/media").join(&home.user), PathBuf::from("/mnt")]
        .iter()
        .flat_map(|m| fs::read_dir(m).into_iter().flatten().flatten().map(|d| d.path()))
        .collect();
    all.push(home.home.clone());
    let mut out: Vec<PathBuf> = Vec::new();
    for p in all {
        let real = fs::canonicalize(&p).unwrap_or(p);
        if !out.contains(&real) {
            out.push(real);
        }
    }
    out
}

/// A folder's identity (device and inode): the same folder reached by another path (a link, a
/// bind mount) is still the same.
fn id(path: &Path) -> Option<(u64, u64)> {
    let m = fs::metadata(path).ok()?;
    Some((std::os::unix::fs::MetadataExt::dev(&m), std::os::unix::fs::MetadataExt::ino(&m)))
}

/// What the folder's own files show, if it's a game.
fn game_signs(dir: &Path) -> Option<&'static str> {
    let names: Vec<(String, bool)> = fs::read_dir(dir).ok()?.flatten().map(|e| (e.file_name().to_string_lossy().to_lowercase(), e.file_type().is_ok_and(|t| t.is_dir()))).collect();
    let file = |n: &str| names.iter().any(|(m, d)| !d && m == n);
    let folder = |n: &str| names.iter().any(|(m, d)| *d && m == n);
    let ends = |e: &str| names.iter().any(|(m, d)| !d && m.ends_with(e));
    let program = ends(".exe") || ends(".x86_64") || ends(".x86");
    if file("unityplayer.dll") || file("unityplayer.so") || names.iter().any(|(m, d)| *d && m.ends_with("_data") && (file(&format!("{}.exe", &m[..m.len() - 5])) || file(&format!("{}.x86_64", &m[..m.len() - 5])))) {
        return Some("Unity");
    }
    if folder("engine") && program && !file("unrealeditor.exe") {
        return Some("Unreal Engine");
    }
    if ends(".pck") && program {
        return Some("Godot");
    }
    if file("data.win") && program {
        return Some("GameMaker");
    }
    if ends(".rgssad") || ends(".rgss2a") || ends(".rgss3a") || (file("game.ini") && file("game.exe")) {
        return Some("RPG Maker");
    }
    if folder("renpy") && folder("game") {
        return Some("Ren'Py");
    }
    if ends(".love") {
        return Some("LÖVE");
    }
    if file("nw.pak") && program {
        return Some("NW.js");
    }
    if names.iter().any(|(m, d)| !d && m.starts_with("goggame-") && m.ends_with(".info")) {
        return Some("GOG");
    }
    if (file("steam_api.dll") || file("steam_api64.dll") || file("libsteam_api.so")) && program {
        return Some("a Steam game");
    }
    None
}

/// Searches; `skip` are folders already looked in (lobby's folders, the games it knows: a
/// drive that is itself one of lobby's folders isn't searched at all),
/// `seen` counts the folders gone through, `stop` ends it early.
pub fn search(starts: &[PathBuf], skip: &[PathBuf], seen: &AtomicUsize, stop: &AtomicBool) -> Results {
    let mut results = Results { games: Vec::new(), libraries: Vec::new() };
    let skip: std::collections::HashSet<(u64, u64)> = skip.iter().filter_map(|s| id(s)).collect();
    let mut gone_into: std::collections::HashSet<(u64, u64)> = std::collections::HashSet::new();
    let mut stack: Vec<(PathBuf, usize)> = starts.iter().map(|s| (s.clone(), 0)).collect();
    while let Some((dir, depth)) = stack.pop() {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        seen.fetch_add(1, Ordering::Relaxed);
        // Once, by identity, whatever the path; and never where lobby already looks.
        let Some(this) = id(&dir) else { continue };
        if gone_into.contains(&this) || skip.contains(&this) {
            continue;
        }
        gone_into.insert(this);
        if dir.join("steamapps").is_dir() && depth > 0 {
            results.libraries.push(dir.clone());
        }
        if depth > 0 {
            if let Some(why) = game_signs(&dir) {
                results.games.push(Found { folder: dir, why });
                continue;
            }
        }
        if depth >= MAX_DEPTH {
            continue;
        }
        for entry in fs::read_dir(&dir).into_iter().flatten().flatten() {
            if !entry.file_type().is_ok_and(|t| t.is_dir()) {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_lowercase();
            let path = entry.path();
            if name.starts_with('.') || PASSED.contains(&name.as_str()) {
                continue;
            }
            stack.push((path, depth + 1));
        }
    }
    results
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signs() {
        let dir = std::env::temp_dir().join(format!("lobby-search-{}", std::process::id()));
        let game = dir.join("Spilled/game");
        let tool = dir.join("rufus");
        fs::create_dir_all(game.join("Spilled!_Data")).unwrap();
        fs::create_dir_all(&tool).unwrap();
        fs::write(game.join("Spilled!.exe"), b"").unwrap();
        fs::write(tool.join("rufus.exe"), b"").unwrap();
        assert_eq!(game_signs(&game), Some("Unity"));
        assert_eq!(game_signs(&tool), None);
        let seen = AtomicUsize::new(0);
        let found = search(&[dir.clone()], &[], &seen, &AtomicBool::new(false));
        assert_eq!(found.games.iter().map(|f| f.folder.clone()).collect::<Vec<_>>(), [game]);
        fs::remove_dir_all(&dir).unwrap();
    }
}
