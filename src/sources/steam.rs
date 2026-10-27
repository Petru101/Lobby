//! Steam: installed games from each library's app manifests, and non-Steam games from every
//! user's shortcuts.vdf.

use super::Home;
use crate::library::{Entry, Key, Source};
use gamefix::{steam, vdf};
use std::fs;
use std::path::{Path, PathBuf};

pub(crate) fn roots(home: &Home) -> Vec<PathBuf> {
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

pub(crate) fn libraries(root: &Path) -> Vec<PathBuf> {
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

/// A library's installed apps: id, name, folder, size on disk (Steam's own tools left out).
fn library_apps(lib: &Path) -> Vec<(u32, String, PathBuf, Option<u64>)> {
    let apps = lib.join("steamapps");
    let mut out = Vec::new();
    for file in fs::read_dir(&apps).into_iter().flatten().flatten() {
        let fname = file.file_name().to_string_lossy().into_owned();
        let Some(appid) = fname.strip_prefix("appmanifest_").and_then(|s| s.strip_suffix(".acf")).and_then(|s| s.parse::<u32>().ok()) else { continue };
        let Ok(doc) = vdf::parse(&fs::read_to_string(file.path()).unwrap_or_default()) else { continue };
        let get = |k: &str| match vdf::get(&doc, &["AppState", k]) {
            Some(vdf::Node::Str(s)) => Some(vdf::unescape(s)),
            _ => None,
        };
        let (Some(name), Some(installdir)) = (get("name"), get("installdir")) else { continue };
        let folder = apps.join("common").join(&installdir);
        if !tool(appid, &name, &folder) {
            out.push((appid, name, folder, get("SizeOnDisk").and_then(|s| s.parse().ok()).filter(|&s| s > 0)));
        }
    }
    out
}

/// The libraries Steam lists (its own folder among them), their real paths.
pub fn known_libraries(home: &Home) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    for root in roots(home) {
        for lib in libraries(&root) {
            let lib = fs::canonicalize(&lib).unwrap_or(lib);
            if !out.contains(&lib) {
                out.push(lib);
            }
        }
    }
    out
}

pub fn apps(home: &Home) -> Vec<Entry> {
    let mut entries: Vec<Entry> = Vec::new();
    for root in roots(home) {
        let played = played(&root);
        for lib in libraries(&root) {
            for (appid, name, folder, size) in library_apps(&lib) {
                if entries.iter().any(|e| e.keys.contains(&Key::SteamApp(appid))) {
                    continue;
                }
                let mut e = Entry::new(Source::Steam, name);
                e.folder = Some(folder);
                e.size = size;
                (e.last_played, e.playtime) = played.get(&appid).copied().unwrap_or_default();
                e.keys.push(Key::SteamApp(appid));
                e.cover = Some(library_art(&root, appid));
                entries.push(e);
            }
        }
    }
    entries
}

/// Steam libraries Steam doesn't list: an old Windows install's, a copied or plugged-in
/// drive's. Looked for where they usually are on each drive (SteamLibrary, Steam, Program
/// Files' Steam, one folder down), and where the search of all drives found them.
pub fn stray_libraries(home: &Home) -> Vec<PathBuf> {
    let known = known_libraries(home);
    let mut spots: Vec<PathBuf> = Vec::new();
    for media in [PathBuf::from("/run/media").join(&home.user), PathBuf::from("/media").join(&home.user), PathBuf::from("/mnt")] {
        for drive in fs::read_dir(&media).into_iter().flatten().flatten().map(|d| d.path()) {
            let mut tops = vec![drive.clone()];
            tops.extend(fs::read_dir(&drive).into_iter().flatten().flatten().filter(|d| d.file_type().is_ok_and(|t| t.is_dir())).map(|d| d.path()));
            for top in tops {
                for name in ["SteamLibrary", "Steam", "steam", "Program Files (x86)/Steam", "Program Files/Steam"] {
                    spots.push(top.join(name));
                }
            }
        }
    }
    spots.extend(crate::places::Places::load().libraries);
    let mut out: Vec<PathBuf> = Vec::new();
    for lib in spots.into_iter().filter(|l| l.join("steamapps").is_dir()) {
        let real = fs::canonicalize(&lib).unwrap_or(lib.clone());
        if !known.contains(&real) && !out.iter().any(|o| fs::canonicalize(o).ok().as_ref() == Some(&real)) {
            out.push(lib);
        }
    }
    out
}

/// The games in Steam libraries Steam doesn't list: playable once moved into Steam's own.
pub fn stray_apps(home: &Home) -> Vec<Entry> {
    let mut entries = Vec::new();
    for lib in stray_libraries(home) {
        for (appid, name, folder, _) in library_apps(&lib) {
            if !folder.is_dir() {
                continue;
            }
            let mut e = Entry::new(Source::SteamElsewhere, name);
            e.folder = Some(folder);
            e.keys.push(Key::SteamApp(appid));
            e.cover = roots(home).first().map(|r| library_art(r, appid));
            entries.push(e);
        }
    }
    entries
}

/// Steam's portrait art for an app it has shown: `appcache/librarycache/<appid>/` or a folder
/// inside it (newer clients call it library_capsule.jpg), else the same art on Steam's CDN.
pub(super) fn library_art(root: &Path, appid: u32) -> String {
    let dir = root.join("appcache/librarycache").join(appid.to_string());
    let names = ["library_600x900.jpg", "library_capsule.jpg"];
    let mut dirs = vec![dir.clone()];
    dirs.extend(fs::read_dir(&dir).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.is_dir()));
    dirs.iter()
        .flat_map(|d| names.iter().map(move |n| d.join(n)))
        .find(|p| p.is_file())
        .map_or_else(|| format!("https://shared.steamstatic.com/store_item_assets/steam/apps/{appid}/library_600x900.jpg"), |p| p.display().to_string())
}

/// What Steam records of playing each app and shortcut, by id, from every account's
/// localconfig.vdf (UserLocalConfigStore/Software/Valve/Steam/apps): when last (Unix) and how
/// long (its Playtime is in minutes, given here in seconds).
pub(super) fn played(root: &Path) -> std::collections::HashMap<u32, (Option<u64>, Option<u64>)> {
    let mut out = std::collections::HashMap::new();
    for user in fs::read_dir(root.join("userdata")).into_iter().flatten().flatten() {
        let Ok(doc) = vdf::parse(&fs::read_to_string(user.path().join("config/localconfig.vdf")).unwrap_or_default()) else { continue };
        let apps = ["UserLocalConfigStore", "Software", "Valve", "Steam", "apps"].iter().try_fold(&doc, |d, k| match d.iter().find(|(key, _)| key.eq_ignore_ascii_case(k)) {
            Some((_, vdf::Node::Map(m))) => Some(m),
            _ => None,
        });
        for (id, app) in apps.into_iter().flatten() {
            let (Ok(id), vdf::Node::Map(fields)) = (id.parse::<u32>(), app) else { continue };
            let number = |k: &str| fields.iter().find(|(key, _)| key.eq_ignore_ascii_case(k)).and_then(|(_, v)| if let vdf::Node::Str(s) = v { s.parse::<u64>().ok() } else { None }).filter(|&n| n > 0);
            let (last, minutes) = (number("LastPlayed"), number("Playtime"));
            if last.is_none() && minutes.is_none() {
                continue;
            }
            let entry: &mut (Option<u64>, Option<u64>) = out.entry(id).or_default();
            entry.0 = entry.0.max(last);
            entry.1 = entry.1.max(minutes.map(|m| m * 60));
        }
    }
    out
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

/// A non-Steam game's shortcut as Steam starts it: its program, the folder it starts in, its
/// launch options.
pub struct Shortcut {
    pub exe: String,
    pub start_dir: Option<PathBuf>,
    pub options: String,
}

pub fn shortcut(appid: u32) -> Option<Shortcut> {
    let home = Home::find()?;
    roots(&home).into_iter().flat_map(|root| fs::read_dir(root.join("userdata")).into_iter().flatten().flatten().collect::<Vec<_>>()).find_map(|user| {
        let doc = steam::parse(&fs::read(user.path().join("config/shortcuts.vdf")).ok()?).ok()?;
        let (_, steam::Node::Map(list)) = doc.iter().find(|(k, _)| k.eq_ignore_ascii_case(b"shortcuts"))? else { return None };
        list.iter().find_map(|(_, item)| {
            let steam::Node::Map(fields) = item else { return None };
            let field = |name: &str| fields.iter().find(|(k, _)| k.eq_ignore_ascii_case(name.as_bytes())).map(|(_, v)| v);
            matches!(field("appid"), Some(steam::Node::Int(id)) if *id == appid).then(|| Shortcut {
                exe: field("Exe").and_then(text).unwrap_or_default(),
                start_dir: field("StartDir").and_then(text).map(PathBuf::from),
                options: field("LaunchOptions").and_then(text).unwrap_or_default(),
            })
        })
    })
}

pub fn shortcuts(home: &Home) -> Vec<Entry> {
    let mut entries = Vec::new();
    for root in roots(home) {
        let played = played(&root);
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
                    (e.last_played, e.playtime) = played.get(id).copied().unwrap_or_default();
                }
                if let Some(steam::Node::Int(t)) = field("LastPlayTime") {
                    e.last_played = e.last_played.max(Some(u64::from(*t)).filter(|&t| t > 0));
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
