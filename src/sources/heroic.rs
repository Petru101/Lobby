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

/// What Heroic records of playing each game, by its app name (store/timestamp.json): when
/// last, and how long (totalPlayed is in minutes, given here in seconds).
fn played(heroic: &Path) -> std::collections::HashMap<String, (Option<u64>, Option<u64>)> {
    let Some(serde_json::Value::Object(map)) = json(&heroic.join("store/timestamp.json")) else { return Default::default() };
    map.into_iter()
        .map(|(app, t)| (app, (str_of(&t, "lastPlayed").and_then(crate::library::unix_time), t.get("totalPlayed").and_then(|m| m.as_u64()).filter(|&m| m > 0).map(|m| m * 60))))
        .collect()
}

pub fn games(home: &Home) -> Vec<Entry> {
    let mut entries: Vec<Entry> = Vec::new();
    let mut legendary: Vec<PathBuf> = Vec::new();
    for heroic in home.app_dirs("com.heroicgameslauncher.hgl", "config/heroic", ".config/heroic") {
        legendary.push(heroic.join("legendaryConfig/legendary/installed.json"));
        let times = played(&heroic);
        let with_prefix = |mut e: Entry, runner: &str, app: &str| {
            e.prefix = prefix(&heroic, app);
            (e.last_played, e.playtime) = times.get(app).copied().unwrap_or_default();
            e.keys.push(Key::Heroic(runner.into(), app.into()));
            if let Some(store) = store_of(runner) {
                e.keys.push(Key::Store(store.into(), app.into()));
            }
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
                let mut e = Entry::new(Source::Heroic, title);
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
                let mut e = Entry::new(Source::Heroic, title);
                e.cover = nile_titles.art(app).map(|u| cover(&heroic, u));
                e.folder = Some(PathBuf::from(path));
                e.program = fuel(Path::new(path)).map(|f| f.0);
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
            let mut e = Entry::new(if heroic.is_some() { Source::Heroic } else { Source::Epic }, title);
            e.folder = Some(PathBuf::from(path));
            e.keys.push(Key::Store("epic".into(), app.clone()));
            if let (Some(h), Some(titles)) = (&heroic, &titles) {
                e.cover = titles.art(&app).map(|u| cover(h, u));
            }
            e.program = str_of(&g, "executable").map(|x| Path::new(path).join(x));
            e.prefix = heroic.as_ref().and_then(|h| prefix(h, &app));
            e.size = g.get("install_size").and_then(|s| s.as_u64()).filter(|&s| s > 0);
            (e.last_played, e.playtime) = heroic.as_ref().and_then(|h| played(h).get(&app).copied()).unwrap_or_default();
            // Started through Heroic when Heroic installed it, else it's legendary's alone.
            if heroic.is_some() {
                e.keys.push(Key::Heroic("legendary".into(), app.clone()));
            }
            entries.push(e);
        }
    }
    entries
}

/// An Amazon game's own record of how it starts (fuel.json in its folder, which nile reads):
/// the program, its arguments and the folder it runs in.
fn fuel(folder: &Path) -> Option<(PathBuf, Vec<String>, PathBuf)> {
    let v = json(&folder.join("fuel.json"))?;
    let main = v.get("Main")?;
    let program = folder.join(str_of(main, "Command")?.replace('\\', "/"));
    let args = main.get("Args").and_then(|a| a.as_array()).into_iter().flatten().filter_map(|a| a.as_str().map(String::from)).collect();
    let cwd = str_of(main, "WorkingSubdirOverride").filter(|d| !d.is_empty()).map_or_else(|| program.parent().unwrap_or(folder).to_path_buf(), |d| folder.join(d.replace('\\', "/")));
    Some((program, args, cwd))
}

/// How Heroic starts a game it added by hand (sideloaded) or installed from Amazon: the
/// program and its arguments, the folder it runs in, and for a Windows game the prefix and the
/// Proton Heroic picked for it (its own settings, else Heroic's defaults), with the variables
/// set for it.
pub struct Launch {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub prefix: Option<PathBuf>,
    pub proton: Option<PathBuf>,
    pub env: Vec<(String, String)>,
}

pub fn launch(runner: &str, app: &str) -> Option<Launch> {
    let home = Home::find()?;
    home.app_dirs("com.heroicgameslauncher.hgl", "config/heroic", ".config/heroic").into_iter().find_map(|heroic| {
        let (program, mut args, cwd) = match runner {
            "sideload" => {
                let list = json(&heroic.join("sideload_apps/library.json"))?.get("games")?.as_array()?.clone();
                let g = list.into_iter().find(|g| str_of(g, "app_name") == Some(app))?;
                let program = PathBuf::from(str_of(g.get("install")?, "executable")?);
                let cwd = program.parent().map(Path::to_path_buf);
                (program, Vec::new(), cwd)
            }
            "nile" => {
                let list = json(&heroic.join("nile_config/nile/installed.json"))?.as_array()?.clone();
                let path = list.iter().find(|g| str_of(g, "id") == Some(app)).and_then(|g| str_of(g, "path"))?.to_string();
                let (program, args, cwd) = fuel(Path::new(&path))?;
                (program, args, Some(cwd))
            }
            _ => return None,
        };
        if !program.is_file() {
            return None;
        }
        let game = json(&heroic.join("GamesConfig").join(format!("{app}.json"))).and_then(|c| c.get(app).cloned()).unwrap_or_default();
        let defaults = json(&heroic.join("config.json")).and_then(|c| c.get("defaultSettings").cloned()).unwrap_or_default();
        let setting = |key: &str| [&game, &defaults].into_iter().find_map(|s| s.get(key).filter(|v| !v.is_null()).cloned());
        let prefix = setting("winePrefix").and_then(|p| p.as_str().filter(|p| !p.is_empty()).map(PathBuf::from));
        let proton = setting("wineVersion").filter(|w| str_of(w, "type") == Some("proton")).and_then(|w| Path::new(str_of(&w, "bin")?).parent().map(Path::to_path_buf));
        let env = game.get("enviromentOptions").and_then(|e| e.as_array()).into_iter().flatten().filter_map(|o| Some((str_of(o, "key")?.to_string(), str_of(o, "value").unwrap_or_default().to_string()))).filter(|(k, _)| !k.is_empty()).collect();
        args.extend(crate::launch::shell_words(str_of(&game, "launcherArgs").unwrap_or_default()));
        Some(Launch { program, args, cwd, prefix, proton, env })
    })
}

/// The store a Heroic runner installs from, as `Key::Store` names it.
fn store_of(runner: &str) -> Option<&'static str> {
    match runner {
        "legendary" => Some("epic"),
        "gog" => Some("gog"),
        "nile" => Some("amazon"),
        _ => None,
    }
}

fn folder_name(path: &str) -> String {
    Path::new(path).file_name().map_or_else(|| path.to_string(), |n| n.to_string_lossy().into_owned())
}
