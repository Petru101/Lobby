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

/// Title and portrait art (`art_square`, what Heroic's cards show) by app name, from one of
/// Heroic's library caches (`{"games": [...]}` or `{"library": [...]}`).
struct Titles(Vec<(String, String, Option<String>)>);

impl Titles {
    fn read(file: &Path) -> Titles {
        let Some(v) = json(file) else { return Titles(Vec::new()) };
        let list = v.get("games").or_else(|| v.get("library")).and_then(|l| l.as_array()).cloned().unwrap_or_default();
        Titles(list.iter().filter_map(|g| Some((str_of(g, "app_name")?.to_string(), str_of(g, "title")?.to_string(), str_of(g, "art_square").map(String::from)))).collect())
    }

    fn title(&self, app: &str) -> Option<String> {
        self.0.iter().find(|(a, ..)| a == app).map(|(_, t, _)| t.clone())
    }

    fn art(&self, app: &str) -> Option<&str> {
        self.0.iter().find(|(a, ..)| a == app).and_then(|(.., art)| art.as_deref())
    }
}

/// A cover: Heroic's downloaded copy (images-cache/<sha256 of its address>) when it has one,
/// else the address.
fn cover(heroic: &Path, url: &str) -> String {
    let cached = heroic.join("images-cache").join(crate::sha256_hex(url.as_bytes()));
    if cached.is_file() { cached.display().to_string() } else { url.to_string() }
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

        let gog_titles = Titles::read(&heroic.join("store_cache/gog_library.json"));
        if let Some(list) = json(&heroic.join("gog_store/installed.json")).and_then(|v| v.get("installed")?.as_array().cloned()) {
            for g in list {
                let (Some(app), Some(path)) = (str_of(&g, "appName"), str_of(&g, "install_path")) else { continue };
                if g.get("is_dlc").and_then(|d| d.as_bool()) == Some(true) || app == "gog-redist" {
                    continue;
                }
                let title = gog_titles.title(app).unwrap_or_else(|| folder_name(path));
                let mut e = Entry::new(Source::Gog, title);
                e.cover = gog_titles.art(app).map(|u| cover(&heroic, u));
                e.folder = Some(PathBuf::from(path));
                // gogdl leaves the program to GOG's .info in the game's folder.
                e.program = str_of(&g, "executable").map(|x| Path::new(path).join(x)).or_else(|| gamefix::gog::read(Path::new(path)).map(|i| i.program));
                entries.push(with_prefix(e, "gog", app));
            }
        }

        let nile_titles = Titles::read(&heroic.join("store_cache/nile_library.json"));
        if let Some(list) = json(&heroic.join("nile_config/nile/installed.json")).and_then(|v| v.as_array().cloned()) {
            for g in list {
                let (Some(app), Some(path)) = (str_of(&g, "id"), str_of(&g, "path")) else { continue };
                let title = nile_titles.title(app).unwrap_or_else(|| folder_name(path));
                let mut e = Entry::new(Source::Amazon, title);
                e.cover = nile_titles.art(app).map(|u| cover(&heroic, u));
                e.folder = Some(PathBuf::from(path));
                entries.push(with_prefix(e, "nile", app));
            }
        }

        if let Some(list) = json(&heroic.join("sideload_apps/library.json")).and_then(|v| v.get("games")?.as_array().cloned()) {
            for g in list {
                let (Some(app), Some(title)) = (str_of(&g, "app_name"), str_of(&g, "title")) else { continue };
                let mut e = Entry::new(Source::Heroic, title);
                e.cover = str_of(&g, "art_square").map(|u| if u.starts_with("http") { cover(&heroic, u) } else { u.to_string() });
                e.program = g.get("install").and_then(|i| str_of(i, "executable")).map(PathBuf::from);
                e.folder = str_of(&g, "folder_name").map(PathBuf::from);
                entries.push(with_prefix(e, "sideload", app));
            }
        }
    }

    legendary.push(home.join(".config/legendary/installed.json"));
    for file in legendary {
        let Some(serde_json::Value::Object(map)) = json(&file) else { continue };
        // The Heroic config this came from, if any: it knows the prefix and the art.
        let heroic = file.ancestors().nth(3).map(Path::to_path_buf).filter(|h| h.join("GamesConfig").is_dir());
        let titles = heroic.as_ref().map(|h| Titles::read(&h.join("store_cache/legendary_library.json")));
        for (app, g) in map {
            let (Some(title), Some(path)) = (str_of(&g, "title"), str_of(&g, "install_path")) else { continue };
            if g.get("is_dlc").and_then(|d| d.as_bool()) == Some(true) || entries.iter().any(|e| e.folder.as_deref() == Some(Path::new(path))) {
                continue;
            }
            let mut e = Entry::new(Source::Epic, title);
            e.folder = Some(PathBuf::from(path));
            if let (Some(h), Some(titles)) = (&heroic, &titles) {
                e.cover = titles.art(&app).map(|u| cover(h, u));
            }
            e.program = str_of(&g, "executable").map(|x| Path::new(path).join(x));
            e.prefix = heroic.as_ref().and_then(|h| prefix(h, &app));
            // Started through Heroic when Heroic installed it, else it's legendary's alone.
            if heroic.is_some() {
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
