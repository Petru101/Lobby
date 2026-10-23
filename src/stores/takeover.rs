//! Taking over the Epic and GOG games Heroic installed, so Heroic can be removed: lobby copies
//! Heroic's records of them into its own (legendary's installed.json and manifests, gogdl's
//! manifests, lobby's GOG record), and keeps each game's Wine prefix and the Proton it runs
//! with (its settings and saves are there). Nothing of Heroic's is changed or used afterwards:
//! Heroic keeps working until it's removed.

use super::{epic, gog, read_json, save_wine_setup, Installed, Store, WineSetup};
use crate::sources::Home;
use serde_json::{Map, Value};
use std::fs;
use std::path::{Path, PathBuf};

/// Heroic's config folders (flatpak, else installed normally).
fn heroic_dirs(home: &Home) -> Vec<PathBuf> {
    home.app_dirs("com.heroicgameslauncher.hgl", "config/heroic", ".config/heroic")
}

fn legendary_installed(heroic: &Path) -> Map<String, Value> {
    read_json(&heroic.join("legendaryConfig/legendary/installed.json"))
}

fn gog_installed(heroic: &Path) -> Vec<Value> {
    let v: Value = read_json(&heroic.join("gog_store/installed.json"));
    v.get("installed").and_then(|i| i.as_array()).cloned().unwrap_or_default()
}

/// The names of the games Heroic installed that lobby hasn't taken over.
pub fn heroic_games(home: &Home) -> Vec<String> {
    let epic_ours: Vec<String> = epic::installed().into_iter().map(|g| g.id).collect();
    let gog_ours: Vec<String> = gog::installed().into_iter().map(|g| g.id).collect();
    let mut games = Vec::new();
    for heroic in heroic_dirs(home) {
        for (id, g) in legendary_installed(&heroic) {
            if g.get("is_dlc").and_then(|d| d.as_bool()) != Some(true) && !epic_ours.contains(&id) {
                games.push(g.get("title").and_then(|t| t.as_str()).unwrap_or(&id).to_string());
            }
        }
        for g in gog_installed(&heroic) {
            let Some(id) = g.get("appName").and_then(|a| a.as_str()) else { continue };
            if g.get("is_dlc").and_then(|d| d.as_bool()) != Some(true) && !gog_ours.iter().any(|o| o == id) {
                games.push(gog_title(&heroic, id));
            }
        }
    }
    games
}

/// A GOG game's name: lobby's owned list, else Heroic's library cache, else its id.
fn gog_title(heroic: &Path, id: &str) -> String {
    if let Some(o) = super::owned(Store::Gog).into_iter().find(|o| o.id == id) {
        return o.title;
    }
    let cache: Value = read_json(&heroic.join("store_cache/gog_library.json"));
    cache.get("games").and_then(|g| g.as_array()).into_iter().flatten().find(|g| g.get("app_name").and_then(|a| a.as_str()) == Some(id)).and_then(|g| g.get("title")?.as_str().map(String::from)).unwrap_or_else(|| id.to_string())
}

/// The prefix and Proton Heroic runs a game with: its own settings, else Heroic's defaults.
/// The Proton is the one the prefix names in config_info when it has one (something may have
/// upgraded it since), else Heroic's choice when that's a Proton.
fn wine_setup(heroic: &Path, id: &str) -> Option<WineSetup> {
    let game: Value = read_json(&heroic.join("GamesConfig").join(format!("{id}.json")));
    let defaults: Value = read_json(&heroic.join("config.json"));
    let settings = game.get(id).cloned().unwrap_or(Value::Null);
    let default = defaults.get("defaultSettings").cloned().unwrap_or(Value::Null);
    let prefix = [&settings, &default].iter().find_map(|s| s.get("winePrefix")?.as_str().filter(|p| !p.is_empty()).map(PathBuf::from))?;
    let chosen = [&settings, &default].iter().find_map(|s| s.get("wineVersion").filter(|w| w.get("type").and_then(|t| t.as_str()) == Some("proton"))?.get("bin")?.as_str().map(|b| Path::new(b).parent().map(Path::to_path_buf))).flatten();
    let proton = crate::launch::proton_of(&prefix).or(chosen);
    Some(WineSetup { prefix, proton })
}

/// Takes over every game Heroic installed that lobby hasn't. Returns what it did, a line each.
pub fn take_over(home: &Home) -> Result<Vec<String>, String> {
    let mut done = Vec::new();
    for heroic in heroic_dirs(home) {
        // Epic: legendary's own records, copied into lobby's legendary config.
        let theirs = legendary_installed(&heroic);
        let ours_file = epic::config().join("installed.json");
        let mut ours: Map<String, Value> = read_json(&ours_file);
        let manifests = heroic.join("legendaryConfig/legendary/manifests");
        fs::create_dir_all(epic::config().join("manifests")).map_err(|e| e.to_string())?;
        for (id, mut g) in theirs {
            if ours.contains_key(&id) {
                continue;
            }
            // Where legendary keeps the saves is worked out again from lobby's prefix.
            if let Some(o) = g.as_object_mut() {
                o.insert("save_path".into(), Value::Null);
            }
            for m in fs::read_dir(&manifests).into_iter().flatten().flatten().filter(|m| m.file_name().to_string_lossy().starts_with(&format!("{id}_"))) {
                fs::copy(m.path(), epic::config().join("manifests").join(m.file_name())).map_err(|e| e.to_string())?;
            }
            let dlc = g.get("is_dlc").and_then(|d| d.as_bool()) == Some(true);
            let title = g.get("title").and_then(|t| t.as_str()).unwrap_or(&id).to_string();
            ours.insert(id.clone(), g);
            if dlc {
                continue;
            }
            let setup = wine_setup(&heroic, &id);
            done.push(line(&title, Store::Epic, setup.as_ref()));
            if let Some(setup) = setup {
                save_wine_setup(Store::Epic, &id, setup)?;
            }
        }
        fs::write(&ours_file, serde_json::to_string_pretty(&ours).unwrap()).map_err(|e| e.to_string())?;

        // GOG: lobby's record, and gogdl's manifest (what its updates patch from).
        let ours: Vec<String> = gog::installed().into_iter().map(|g| g.id).collect();
        fs::create_dir_all(gog::manifests_dir()).map_err(|e| e.to_string())?;
        let their_manifests = heroic.parent().map(|c| c.join("heroic_gogdl/manifests")).unwrap_or_default();
        for g in gog_installed(&heroic) {
            let (Some(id), Some(path)) = (g.get("appName").and_then(|a| a.as_str()), g.get("install_path").and_then(|p| p.as_str())) else { continue };
            if g.get("is_dlc").and_then(|d| d.as_bool()) == Some(true) || ours.iter().any(|o| o == id) {
                continue;
            }
            let folder = PathBuf::from(path);
            let platform = g.get("platform").and_then(|p| p.as_str()).filter(|p| *p != "windows").map(String::from);
            let program = match platform.as_deref() {
                // GOG's Linux games start with the start.sh it puts beside them.
                Some("linux") => Some(folder.join("start.sh")).filter(|p| p.is_file()),
                _ => gamefix::gog::read(&folder).map(|i| i.program),
            };
            let build = g.get("buildId").and_then(|b| b.as_str()).filter(|b| !b.is_empty()).map(String::from);
            let title = gog_title(&heroic, id);
            let manifest = their_manifests.join(id);
            if manifest.is_file() && !gog::manifests_dir().join(id).exists() {
                fs::copy(&manifest, gog::manifests_dir().join(id)).map_err(|e| e.to_string())?;
            }
            let setup = if platform.is_none() { wine_setup(&heroic, id) } else { None };
            done.push(if platform.is_some() { format!("{title} (GOG, the Linux version): starts as it is.") } else { line(&title, Store::Gog, setup.as_ref()) });
            if let Some(setup) = setup {
                save_wine_setup(Store::Gog, id, setup)?;
            }
            gog::record(Installed { id: id.to_string(), title, folder, program, build, platform })?;
        }
    }
    Ok(done)
}

fn line(title: &str, store: Store, setup: Option<&WineSetup>) -> String {
    match setup {
        Some(WineSetup { prefix, proton: Some(proton) }) => format!("{title} ({}): keeps its Wine prefix {} and {}.", store.name(), prefix.display(), proton.file_name().map_or_else(|| proton.display().to_string(), |n| n.to_string_lossy().into_owned())),
        Some(WineSetup { prefix, proton: None }) => format!("{title} ({}): keeps its Wine prefix {}.", store.name(), prefix.display()),
        None => format!("{title} ({}): gamefix sets it up the first time it's played.", store.name()),
    }
}
