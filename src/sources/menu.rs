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
    if !game || get("Type").is_some_and(|t| t != "Application") || get("NoDisplay") == Some("true") {
        return None;
    }
    let mut e = Entry::new(Source::AppMenu, name);
    e.keys.push(Key::Desktop(file.to_path_buf()));
    if let Some(id) = exec.split("steam://rungameid/").nth(1).and_then(|s| s.split(|c: char| !c.is_ascii_digit()).next()).and_then(|s| s.parse::<u64>().ok()) {
        // A non-Steam game's run id is its shortcut id shifted up (with 0x02000000 below).
        e.keys.push(if id > u32::MAX as u64 { Key::SteamShortcut((id >> 32) as u32) } else { Key::SteamApp(id as u32) });
    }
    // The last file the command names: the game's program, or what a runner opens.
    e.program = words(exec).into_iter().rev().map(|w| PathBuf::from(w.rsplit_once('=').map_or(w.as_str(), |(_, v)| v))).find(|p| p.is_absolute() && p.is_file());
    // Its working folder is the game's when the program is in it, or when no program could be
    // found (a runner's command naming a folder that was since deleted).
    e.folder = get("Path").filter(|p| !p.is_empty()).map(PathBuf::from).filter(|p| e.program.as_ref().is_none_or(|prog| prog.starts_with(p)));
    // Wine's entries: `env WINEPREFIX="/path" wine C:\\...\\game.exe`.
    if let Some(rest) = exec.split("WINEPREFIX=").nth(1) {
        let pfx = match rest.strip_prefix('"') {
            Some(quoted) => quoted.split('"').next(),
            None => rest.split_whitespace().next(),
        };
        e.prefix = pfx.filter(|p| p.starts_with('/')).map(PathBuf::from);
    }
    Some(e)
}

/// A command line's words, with "quoted parts" kept whole.
fn words(exec: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    let mut chars = exec.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => quoted = !quoted,
            '\\' if quoted => cur.extend(chars.next()),
            c if c.is_whitespace() && !quoted => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}
