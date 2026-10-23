//! Faugus Launcher: its games.json.

use super::{json, str_of, Home};
use crate::library::{Entry, Source};
use std::path::PathBuf;

pub fn games(home: &Home) -> Vec<Entry> {
    let mut entries = Vec::new();
    for dir in home.app_dirs("io.github.Faugus.faugus-launcher", "data/faugus-launcher", ".config/faugus-launcher") {
        let Some(list) = json(&dir.join("games.json")).and_then(|v| v.as_array().cloned()) else { continue };
        for g in list {
            let Some(title) = str_of(&g, "title") else { continue };
            let mut e = Entry::new(Source::Faugus, title);
            e.program = str_of(&g, "path").map(PathBuf::from);
            e.prefix = str_of(&g, "prefix").map(PathBuf::from);
            entries.push(e);
        }
    }
    entries
}
