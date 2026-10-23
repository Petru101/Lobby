//! Heroic: installed Epic (legendary), GOG (gogdl) and Amazon (nile) games, games added by
//! hand ("sideloaded"), and the prefix each game runs in. legendary's own config is read too,
//! for Epic games installed with legendary alone.

use super::{json, str_of, Home};
use crate::library::{Entry, Key, Source};
use std::path::{Path, PathBuf};

/// The prefix Heroic runs a game in, from its per-game settings.
fn prefix(heroic: &Path, app: &str) -> Option<PathBuf> {
    let config = json(&heroic.join("GamesConfig").join(format!("{app}.json")))?;
    str_of(config.get(app)?, "winePrefix").map(PathBuf::from)
}

/// Titles by app name from one of Heroic's library caches (`{"games": [...]}` or `{"library": [...]}`).
fn titles(file: &Path) -> Vec<(String, String)> {
    let Some(v) = json(file) else { return Vec::new() };
    let list = v.get("games").or_else(|| v.get("library")).and_then(|l| l.as_array()).cloned().unwrap_or_default();
    list.iter().filter_map(|g| Some((str_of(g, "app_name")?.to_string(), str_of(g, "title")?.to_string()))).collect()
}

pub fn games(home: &Home) -> Vec<Entry> {
    let mut entries: Vec<Entry> = Vec::new();
    let mut legendary: Vec<PathBuf> = Vec::new();
    for heroic in home.app_dirs("com.heroicgameslauncher.hgl", "config/heroic", ".config/heroic") {
        legendary.push(heroic.join("legendaryConfig/legendary/installed.json"));
        let with_prefix = |mut e: Entry, runner: &str, app: &str| {
            e.prefix = prefix(&heroic, app);
            e.keys.push(Key::Heroic(runner.into(), app.into()));
            e
        };

        let gog_titles = titles(&heroic.join("store_cache/gog_library.json"));
        if let Some(list) = json(&heroic.join("gog_store/installed.json")).and_then(|v| v.get("installed")?.as_array().cloned()) {
            for g in list {
                let (Some(app), Some(path)) = (str_of(&g, "appName"), str_of(&g, "install_path")) else { continue };
                if g.get("is_dlc").and_then(|d| d.as_bool()) == Some(true) || app == "gog-redist" {
                    continue;
                }
                let title = gog_titles.iter().find(|(a, _)| a == app).map(|(_, t)| t.clone()).unwrap_or_else(|| folder_name(path));
                let mut e = Entry::new(Source::Gog, title);
                e.folder = Some(PathBuf::from(path));
                // gogdl leaves the program to GOG's .info in the game's folder.
                e.program = str_of(&g, "executable").map(|x| Path::new(path).join(x)).or_else(|| gamefix::gog::read(Path::new(path)).map(|i| i.program));
                entries.push(with_prefix(e, "gog", app));
            }
        }

        let nile_titles = titles(&heroic.join("store_cache/nile_library.json"));
        if let Some(list) = json(&heroic.join("nile_config/nile/installed.json")).and_then(|v| v.as_array().cloned()) {
            for g in list {
                let (Some(app), Some(path)) = (str_of(&g, "id"), str_of(&g, "path")) else { continue };
                let title = nile_titles.iter().find(|(a, _)| a == app).map(|(_, t)| t.clone()).unwrap_or_else(|| folder_name(path));
                let mut e = Entry::new(Source::Amazon, title);
                e.folder = Some(PathBuf::from(path));
                entries.push(with_prefix(e, "nile", app));
            }
        }

        if let Some(list) = json(&heroic.join("sideload_apps/library.json")).and_then(|v| v.get("games")?.as_array().cloned()) {
            for g in list {
                let (Some(app), Some(title)) = (str_of(&g, "app_name"), str_of(&g, "title")) else { continue };
                let mut e = Entry::new(Source::Heroic, title);
                e.program = g.get("install").and_then(|i| str_of(i, "executable")).map(PathBuf::from);
                e.folder = str_of(&g, "folder_name").map(PathBuf::from);
                entries.push(with_prefix(e, "sideload", app));
            }
        }
    }

    legendary.push(home.join(".config/legendary/installed.json"));
    for file in legendary {
        let Some(serde_json::Value::Object(map)) = json(&file) else { continue };
        for (app, g) in map {
            let (Some(title), Some(path)) = (str_of(&g, "title"), str_of(&g, "install_path")) else { continue };
            if g.get("is_dlc").and_then(|d| d.as_bool()) == Some(true) || entries.iter().any(|e| e.folder.as_deref() == Some(Path::new(path))) {
                continue;
            }
            let mut e = Entry::new(Source::Epic, title);
            e.folder = Some(PathBuf::from(path));
            e.program = str_of(&g, "executable").map(|x| Path::new(path).join(x));
            // The Heroic config this came from, if any, knows its prefix.
            let heroic = file.ancestors().nth(3).map(Path::to_path_buf);
            e.prefix = heroic.as_ref().and_then(|h| prefix(h, &app));
            // Started through Heroic when Heroic installed it, else it's legendary's alone.
            if heroic.as_ref().is_some_and(|h| h.join("GamesConfig").is_dir()) {
                e.keys.push(Key::Heroic("legendary".into(), app.clone()));
            }
            entries.push(e);
        }
    }
    entries
}

fn folder_name(path: &str) -> String {
    Path::new(path).file_name().map_or_else(|| path.to_string(), |n| n.to_string_lossy().into_owned())
}
