//! Steam: installed games from each library's app manifests, and non-Steam games from every
//! user's shortcuts.vdf.

use super::Home;
use crate::library::{Entry, Key, Source};
use gamefix::{steam, vdf};
use std::fs;
use std::path::{Path, PathBuf};

fn roots(home: &Home) -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = [".local/share/Steam", ".steam/steam", ".var/app/com.valvesoftware.Steam/.local/share/Steam"]
        .iter()
        .map(|p| home.join(p))
        .filter(|p| p.join("steamapps").is_dir())
        .collect();
    let mut seen = Vec::new();
    roots.retain(|r| {
        let c = fs::canonicalize(r).unwrap_or_else(|_| r.clone());
        !seen.contains(&c) && {
            seen.push(c);
            true
        }
    });
    roots
}

fn libraries(root: &Path) -> Vec<PathBuf> {
    let mut out = vec![root.to_path_buf()];
    let text = fs::read_to_string(root.join("steamapps/libraryfolders.vdf")).unwrap_or_default();
    let Ok(doc) = vdf::parse(&text) else { return out };
    if let Some(vdf::Node::Map(folders)) = vdf::get(&doc, &["libraryfolders"]) {
        for (_, folder) in folders {
            if let vdf::Node::Map(fields) = folder {
                if let Some((_, vdf::Node::Str(path))) = fields.iter().find(|(k, _)| k == "path") {
                    out.push(PathBuf::from(vdf::unescape(path)));
                }
            }
        }
    }
    out
}

/// Steam's own tools, installed like games.
fn tool(appid: u32, name: &str, dir: &Path) -> bool {
    appid == 228980 || dir.join("toolmanifest.vdf").is_file() || ["Proton", "Steam Linux Runtime", "Steamworks"].iter().any(|t| name.starts_with(t))
}

pub fn apps(home: &Home) -> Vec<Entry> {
    let mut entries: Vec<Entry> = Vec::new();
    for root in roots(home) {
        for lib in libraries(&root) {
            let apps = lib.join("steamapps");
            for file in fs::read_dir(&apps).into_iter().flatten().flatten() {
                let fname = file.file_name().to_string_lossy().into_owned();
                let Some(appid) = fname.strip_prefix("appmanifest_").and_then(|s| s.strip_suffix(".acf")).and_then(|s| s.parse::<u32>().ok()) else { continue };
                if entries.iter().any(|e| e.keys.contains(&Key::SteamApp(appid))) {
                    continue;
                }
                let Ok(doc) = vdf::parse(&fs::read_to_string(file.path()).unwrap_or_default()) else { continue };
                let get = |k: &str| match vdf::get(&doc, &["AppState", k]) {
                    Some(vdf::Node::Str(s)) => Some(vdf::unescape(s)),
                    _ => None,
                };
                let (Some(name), Some(installdir)) = (get("name"), get("installdir")) else { continue };
                let folder = apps.join("common").join(&installdir);
                if tool(appid, &name, &folder) {
                    continue;
                }
                let mut e = Entry::new(Source::Steam, name);
                e.folder = Some(folder);
                e.keys.push(Key::SteamApp(appid));
                e.cover = Some(library_art(&root, appid));
                entries.push(e);
            }
        }
    }
    entries
}

/// Steam's portrait art for an app it has shown: `appcache/librarycache/<appid>/` or a folder
/// inside it (newer clients call it library_capsule.jpg), else the same art on Steam's CDN.
fn library_art(root: &Path, appid: u32) -> String {
    let dir = root.join("appcache/librarycache").join(appid.to_string());
    let names = ["library_600x900.jpg", "library_capsule.jpg"];
    let mut dirs = vec![dir.clone()];
    dirs.extend(fs::read_dir(&dir).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.is_dir()));
    dirs.iter()
        .flat_map(|d| names.iter().map(move |n| d.join(n)))
        .find(|p| p.is_file())
        .map_or_else(|| format!("https://shared.steamstatic.com/store_item_assets/steam/apps/{appid}/library_600x900.jpg"), |p| p.display().to_string())
}

/// Art a user picked for a non-Steam game in Steam: `config/grid/<id>p.png` (portrait).
fn grid_art(user: &Path, id: u32) -> Option<PathBuf> {
    ["png", "jpg", "jpeg", "webp"].iter().map(|ext| user.join("config/grid").join(format!("{id}p.{ext}"))).find(|p| p.is_file())
}

fn text(node: &steam::Node) -> Option<String> {
    match node {
        steam::Node::Str(s) => Some(String::from_utf8_lossy(s).trim_matches('"').to_string()).filter(|s| !s.is_empty()),
        _ => None,
    }
}

pub fn shortcuts(home: &Home) -> Vec<Entry> {
    let mut entries = Vec::new();
    for root in roots(home) {
        for user in fs::read_dir(root.join("userdata")).into_iter().flatten().flatten() {
            let Ok(data) = fs::read(user.path().join("config/shortcuts.vdf")) else { continue };
            let Ok(doc) = steam::parse(&data) else { continue };
            let Some((_, steam::Node::Map(list))) = doc.iter().find(|(k, _)| k.eq_ignore_ascii_case(b"shortcuts")) else { continue };
            for (_, item) in list {
                let steam::Node::Map(fields) = item else { continue };
                let field = |name: &str| fields.iter().find(|(k, _)| k.eq_ignore_ascii_case(name.as_bytes())).map(|(_, v)| v);
                let Some(name) = field("AppName").and_then(text) else { continue };
                let mut e = Entry::new(Source::SteamShortcut, name);
                if let Some(steam::Node::Int(id)) = field("appid") {
                    e.keys.push(Key::SteamShortcut(*id));
                    e.cover = grid_art(&user.path(), *id).map(|p| p.display().to_string());
                }
                e.icon = field("icon").and_then(text).map(PathBuf::from).filter(|p| p.is_absolute());
                let exe = field("Exe").and_then(text).unwrap_or_default();
                let options = field("LaunchOptions").and_then(text).unwrap_or_default();
                // A Lutris game added to Steam by Lutris: `lutris lutris:rungameid/2`.
                if let Some(id) = options.split_whitespace().find_map(|w| w.strip_prefix("lutris:rungameid/")).and_then(|s| s.parse().ok()) {
                    e.keys.push(Key::Lutris(id));
                } else if exe.starts_with('/') {
                    e.program = Some(PathBuf::from(&exe));
                }
                // A runner started in the game's folder (dosbox, flatpak): the folder is all
                // there is to go by.
                if e.program.as_ref().is_none_or(|p| !p.starts_with(&home.home) && !p.starts_with("/run/media") && !p.starts_with("/mnt")) {
                    e.folder = field("StartDir").and_then(text).map(PathBuf::from);
                }
                entries.push(e);
            }
        }
    }
    entries
}
