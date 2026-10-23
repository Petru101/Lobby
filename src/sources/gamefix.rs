//! gamefix's records (`~/.local/share/gamefix/games/<name>.log`): games it installed, and the
//! Steam shortcuts and app-menu entries it made, which tie its fixes to those entries.

use super::Home;
use crate::library::{Entry, Key, Source};
use std::fs;
use std::path::PathBuf;

pub fn records(home: &Home) -> Vec<Entry> {
    let mut entries = Vec::new();
    for file in fs::read_dir(home.join(".local/share/gamefix/games")).into_iter().flatten().flatten() {
        let path = file.path();
        if path.extension().is_none_or(|e| e != "log") {
            continue;
        }
        let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let mut e = Entry::new(Source::Gamefix, stem);
        e.annotation = true;
        for line in fs::read_to_string(&path).unwrap_or_default().lines() {
            let f: Vec<&str> = line.split('\t').collect();
            match f.as_slice() {
                ["installed-game", prefix, program, name, ..] => {
                    e.prefix = Some(PathBuf::from(prefix));
                    e.program = Some(PathBuf::from(program));
                    e.name = name.to_string();
                    e.annotation = false;
                }
                ["steam-shortcut", id, ..] | ["steam-compat", id, ..] => {
                    if let Ok(id) = id.parse::<u32>() {
                        // Non-Steam games' ids have the top bit set.
                        e.keys.push(if id & 0x8000_0000 != 0 { Key::SteamShortcut(id) } else { Key::SteamApp(id) });
                    }
                }
                ["created", file, ..] if file.ends_with(".desktop") => e.keys.push(Key::Desktop(PathBuf::from(file))),
                _ => {}
            }
        }
        entries.push(e);
    }
    entries
}
