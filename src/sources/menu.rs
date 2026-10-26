//! App-menu and desktop entries (.desktop files) for games: the ones people and programs
//! (gamefix, Steam, Wine) make.

use super::Home;
use crate::library::{Entry, Key, Source};
use std::fs;
use std::path::{Path, PathBuf};

fn find(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if entry.file_type().is_ok_and(|t| t.is_dir()) {
            if depth < 4 {
                find(&path, depth + 1, out);
            }
        } else if path.extension().is_some_and(|e| e == "desktop") {
            out.push(path);
        }
    }
}

pub fn entries(home: &Home) -> Vec<Entry> {
    let mut files = Vec::new();
    find(&home.join(".local/share/applications"), 0, &mut files);
    find(&home.join("Desktop"), 0, &mut files);
    let mut entries = Vec::new();
    for file in files {
        let Some(e) = read(&file) else { continue };
        entries.push(e);
    }
    entries
}

fn read(file: &Path) -> Option<Entry> {
    let text = fs::read_to_string(file).ok()?;
    let mut fields = Vec::new();
    let mut main = false;
    for line in text.lines() {
        if line.starts_with('[') {
            main = line.trim() == "[Desktop Entry]";
        } else if main {
            if let Some((k, v)) = line.split_once('=') {
                fields.push((k.trim().to_string(), v.trim().to_string()));
            }
        }
    }
    let get = |k: &str| fields.iter().find(|(key, _)| key == k).map(|(_, v)| v.as_str());
    let name = get("Name")?;
    let exec = get("Exec").unwrap_or_default();
    let wine = file.components().any(|c| c.as_os_str() == "wine");
    let game = get("Categories").is_some_and(|c| c.split(';').any(|c| c == "Game"))
        || file.file_name().is_some_and(|n| n.to_string_lossy().starts_with("gamefix-"))
        || exec.contains("steam://rungameid/")
        || wine;
    // lobby's own entry is in Games too.
    let lobby = file.file_stem().is_some_and(|s| s == "io.github.Petru101.Lobby");
    if !game || lobby || get("Type").is_some_and(|t| t != "Application") || get("NoDisplay") == Some("true") {
        return None;
    }
    let mut e = Entry::new(Source::AppMenu, name);
    e.keys.push(Key::Desktop(file.to_path_buf()));
    e.icon = get("Icon").map(PathBuf::from).filter(|p| p.is_absolute());
    if let Some(id) = exec.split("steam://rungameid/").nth(1).and_then(|s| s.split(|c: char| !c.is_ascii_digit()).next()).and_then(|s| s.parse::<u64>().ok()) {
        // A non-Steam game's run id is its shortcut id shifted up (with 0x02000000 below).
        e.keys.push(if id > u32::MAX as u64 { Key::SteamShortcut((id >> 32) as u32) } else { Key::SteamApp(id as u32) });
    }
    // The last file the command names: the game's program, or what a runner opens.
    e.program = crate::launch::desktop_exec(&text).map(|(w, _)| w).unwrap_or_default().into_iter().rev().map(|w| PathBuf::from(w.rsplit_once('=').map_or(w.as_str(), |(_, v)| v))).find(|p| p.is_absolute() && p.is_file());
    // Its working folder is the game's when the program is in it, or when no program could be
    // found (a runner's command naming a folder that was since deleted).
    e.folder = get("Path").filter(|p| !p.is_empty()).map(PathBuf::from).filter(|p| e.program.as_ref().is_none_or(|prog| prog.starts_with(p)));
    // Wine's entries: `env WINEPREFIX="/path" wine C:\\...\\game.exe`, or with the whole
    // `"WINEPREFIX=/a path"` quoted (gamefix's): the command's own words, as the desktop-entry
    // spec splits them.
    let words = crate::launch::desktop_exec(&text).map(|(w, _)| w).unwrap_or_default();
    e.prefix = words.iter().find_map(|w| w.strip_prefix("WINEPREFIX=")).filter(|p| p.starts_with('/')).map(PathBuf::from);
    Some(e)
}
