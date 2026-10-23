//! Wine prefixes (the folders with a fake C: drive): the games installers put in them, found
//! from the Start Menu and desktop shortcuts the installers made. This is how a game installed
//! with a setup.exe is found even when nothing else recorded it.

use super::Home;
use crate::library::{self, Entry, Kind, Source};
use gamefix::disc;
use std::fs;
use std::path::PathBuf;

/// Where prefixes are kept without any one game naming them.
pub fn usual(home: &Home) -> Vec<PathBuf> {
    let mut dirs = vec![home.join(".wine")];
    let mut parents = vec![home.join("Faugus"), home.join("Games/Heroic/Prefixes"), home.join("Games/umu")];
    parents.extend(home.app_dirs("com.usebottles.bottles", "data/bottles/bottles", ".local/share/bottles/bottles"));
    // Proton's prefixes for non-Steam games (their ids have the top bit set): an installer
    // added to Steam installs its game there.
    for steam in [".local/share/Steam", ".var/app/com.valvesoftware.Steam/.local/share/Steam"] {
        let compat = home.join(steam).join("steamapps/compatdata");
        for entry in fs::read_dir(&compat).into_iter().flatten().flatten() {
            if entry.file_name().to_string_lossy().parse::<u32>().is_ok_and(|id| id & 0x8000_0000 != 0) {
                dirs.push(entry.path());
            }
        }
    }
    for parent in parents {
        dirs.extend(fs::read_dir(&parent).into_iter().flatten().flatten().map(|e| e.path()));
    }
    dirs
}

pub fn scan(dirs: Vec<PathBuf>) -> Vec<Entry> {
    let mut seen: Vec<PathBuf> = Vec::new();
    let mut entries = Vec::new();
    for dir in dirs {
        let Some(pfx) = library::prefix_in(&dir) else { continue };
        let pfx = library::norm(&pfx);
        if seen.contains(&pfx) {
            continue;
        }
        seen.push(pfx.clone());
        let mut shortcuts = disc::shortcuts(&pfx);
        // Several shortcuts to one program (Start Menu and desktop).
        shortcuts.sort_by(|a, b| a.0.cmp(&b.0));
        shortcuts.dedup_by(|a, b| a.0 == b.0);
        for (program, label) in shortcuts {
            let mut e = Entry::new(Source::Prefix, label);
            e.kind = if library::store_app(&program) { Kind::StoreApp } else { Kind::Game };
            e.program = Some(program);
            e.prefix = Some(pfx.clone());
            entries.push(e);
        }
    }
    entries
}
