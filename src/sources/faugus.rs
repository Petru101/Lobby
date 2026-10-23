//! Faugus Launcher: its games.json.

use super::{json, str_of, Home};
use crate::library::{Entry, Key, Source};
use std::path::PathBuf;

/// Whether Faugus starts the game with nothing of its own (no launch or game arguments, no
/// protonfix): then starting it another way loses nothing.
pub fn plain_settings(id: &str) -> bool {
    let Some(home) = Home::find() else { return false };
    home.app_dirs("io.github.Faugus.faugus-launcher", "data/faugus-launcher", ".config/faugus-launcher").iter().any(|dir| {
        json(&dir.join("games.json")).and_then(|v| v.as_array().cloned()).into_iter().flatten().any(|g| {
            str_of(&g, "gameid") == Some(id) && ["launch_arguments", "game_arguments", "protonfix"].iter().all(|k| str_of(&g, k).is_none())
        })
    })
}

pub fn games(home: &Home) -> Vec<Entry> {
    let mut entries = Vec::new();
    for dir in home.app_dirs("io.github.Faugus.faugus-launcher", "data/faugus-launcher", ".config/faugus-launcher") {
        let Some(list) = json(&dir.join("games.json")).and_then(|v| v.as_array().cloned()) else { continue };
        for g in list {
            let Some(title) = str_of(&g, "title") else { continue };
            let mut e = Entry::new(Source::Faugus, title);
            e.program = str_of(&g, "path").map(PathBuf::from);
            e.prefix = str_of(&g, "prefix").map(PathBuf::from);
            e.keys.extend(str_of(&g, "gameid").map(|id| Key::Faugus(id.into())));
            e.cover = str_of(&g, "cover").map(String::from);
            e.icon = str_of(&g, "icon").map(PathBuf::from);
            entries.push(e);
        }
    }
    entries
}
