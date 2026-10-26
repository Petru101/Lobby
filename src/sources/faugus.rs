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

/// How Faugus starts a game: umu-run in its prefix, with its launch arguments (variables, and
/// maybe a program they go before) in front and its game arguments after; `env` its global
/// variables (envar.json, "NAME=value" lines).
pub struct Launch {
    pub program: PathBuf,
    pub prefix: PathBuf,
    pub before: String,
    pub after: String,
    pub env: Vec<(String, String)>,
    /// The runner Faugus uses: "Default" read as its default runner; a Proton's name
    /// (compatibilitytools.d) or folder, "Linux-Native" or "Steam".
    pub runner: String,
    pub protonfix: Option<String>,
    pub mangohud: bool,
}

pub fn launch(id: &str) -> Option<Launch> {
    let home = Home::find()?;
    home.app_dirs("io.github.Faugus.faugus-launcher", "data/faugus-launcher", ".config/faugus-launcher").iter().find_map(|dir| {
        let list = json(&dir.join("games.json"))?.as_array()?.clone();
        let g = list.into_iter().find(|g| str_of(g, "gameid") == Some(id))?;
        Some(Launch {
            program: PathBuf::from(str_of(&g, "path")?),
            prefix: PathBuf::from(str_of(&g, "prefix")?),
            before: str_of(&g, "launch_arguments").unwrap_or_default().to_string(),
            after: str_of(&g, "game_arguments").unwrap_or_default().to_string(),
            protonfix: str_of(&g, "protonfix").map(String::from),
            runner: match str_of(&g, "runner").unwrap_or("Default") {
                "Default" => json(&home.join(".var/app/io.github.Faugus.faugus-launcher/config/faugus-launcher/config.json"))
                    .or_else(|| json(&home.join(".config/faugus-launcher/config.json")))
                    .and_then(|c| c.get("default-runner")?.as_str().map(|s| s.trim_matches('"').to_string()))
                    .unwrap_or_default(),
                other => other.to_string(),
            },
            env: home
                .app_dirs("io.github.Faugus.faugus-launcher", "config/faugus-launcher", ".config/faugus-launcher")
                .iter()
                .filter_map(|d| json(&d.join("envar.json"))?.as_array().cloned())
                .flatten()
                .filter_map(|l| l.as_str().and_then(|l| l.split_once('=')).map(|(k, v)| (k.trim().to_string(), v.trim().to_string())))
                .collect(),
            mangohud: g.get("mangohud").is_some_and(|m| m.as_bool() == Some(true) || m.as_str() == Some("True")),
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
            // Seconds, and a local time without a zone ("2026-10-26T07:49:39.505004").
            e.playtime = g.get("playtime").and_then(|p| p.as_u64()).filter(|&p| p > 0);
            e.last_played = str_of(&g, "last_played").and_then(crate::library::unix_time);
            entries.push(e);
        }
    }
    entries
}
