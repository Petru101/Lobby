//! itch.io: which page a downloaded game came from, and that page's title and cover.
//!
//! Browsers record, for each download, the page the link was on: Firefox and its relatives
//! (LibreWolf, Zen, Floorp, Waterfox) in places.sqlite, Chrome, Chromium, Brave, Edge, Vivaldi
//! and Opera in each profile's History (`downloads.tab_url`). Only downloads from itch.io game
//! pages (`https://<creator>.itch.io/<game>`) are kept; from Chromium's only those rows are
//! even read. Each page's `data.json` (title, cover) is fetched once and cached in
//! ~/.cache/lobby/itch.json.

use super::Home;
use crate::library::{same_name, Entry, Key, Source};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// A file downloaded from an itch.io game page.
struct Download {
    file: PathBuf,
    page: String,
}

#[derive(Clone, Copy, PartialEq)]
enum Browser {
    /// Firefox and its relatives: places.sqlite.
    Gecko,
    /// Chrome and the browsers built on Chromium: History.
    Chromium,
}

/// Where each browser keeps its profiles: native and flatpak installs.
const GECKO: &[&str] = &[
    ".mozilla/firefox",
    ".var/app/org.mozilla.firefox/.mozilla/firefox",
    ".var/app/org.mozilla.firefox/config/mozilla/firefox",
    ".librewolf",
    ".var/app/io.gitlab.librewolf-community/.librewolf",
    ".zen",
    ".var/app/app.zen_browser.zen/.zen",
    ".floorp",
    ".var/app/one.ablaze.floorp/.floorp",
    ".waterfox",
    ".var/app/net.waterfox.waterfox/.waterfox",
];
const CHROMIUM: &[&str] = &[
    ".config/google-chrome",
    ".config/google-chrome-beta",
    ".config/google-chrome-unstable",
    ".var/app/com.google.Chrome/config/google-chrome",
    ".config/chromium",
    ".var/app/org.chromium.Chromium/config/chromium",
    ".var/app/io.github.ungoogled_software.ungoogled_chromium/config/chromium",
    ".config/BraveSoftware/Brave-Browser",
    ".var/app/com.brave.Browser/config/BraveSoftware/Brave-Browser",
    ".config/microsoft-edge",
    ".var/app/com.microsoft.Edge/config/microsoft-edge",
    ".config/vivaldi",
    ".var/app/com.vivaldi.Vivaldi/config/vivaldi",
    ".config/opera",
    ".var/app/com.opera.Opera/config/opera",
];

/// Every browser profile's history database: a profile folder (Firefox's `<random>.default`,
/// Chromium's `Default`, `Profile 2`), or the browser's folder itself (Opera keeps one there).
fn histories(home: &Home) -> Vec<(Browser, PathBuf)> {
    let mut out = Vec::new();
    for (browser, dirs, file) in [(Browser::Gecko, GECKO, "places.sqlite"), (Browser::Chromium, CHROMIUM, "History")] {
        for dir in dirs.iter().map(|d| home.join(d)) {
            let profiles = fs::read_dir(&dir).into_iter().flatten().flatten().map(|p| p.path()).chain(fs::read_dir(dir.join("Profiles")).into_iter().flatten().flatten().map(|p| p.path())).chain([dir.clone()]);
            for profile in profiles {
                let db = profile.join(file);
                if db.is_file() && !out.iter().any(|(_, p)| *p == db) {
                    out.push((browser, db));
                }
            }
        }
    }
    out
}

/// The game page an address is on: `https://<creator>.itch.io/<game>`, without what follows.
fn game_page(url: &str) -> Option<String> {
    let rest = url.strip_prefix("https://")?;
    let (host, path) = rest.split_once('/')?;
    let creator = host.strip_suffix(".itch.io")?;
    let game: String = path.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').collect();
    let ok = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    (ok(creator) && ok(&game)).then(|| format!("https://{creator}.itch.io/{game}"))
}

fn file_from_uri(uri: &str) -> Option<PathBuf> {
    let path = uri.strip_prefix("file://")?;
    // Percent-decoding: Firefox writes file URIs encoded.
    let bytes = path.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(b) = u8::from_str_radix(std::str::from_utf8(&bytes[i + 1..i + 3]).ok()?, 16) {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    Some(PathBuf::from(String::from_utf8_lossy(&out).into_owned()))
}

/// The browsers' downloads from itch.io pages. Browsers keep their databases locked, so each is
/// read from a copy, deleted right after: lobby keeps only the itch.io downloads found
/// (~/.cache/lobby/itch-downloads.json), read again when the browser changes the database.
fn downloads(home: &Home) -> Vec<Download> {
    let cache = cache_dir();
    let saved_file = cache.join("itch-downloads.json");
    let mut saved: HashMap<String, Saved> = fs::read_to_string(&saved_file).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    let mut out = Vec::new();
    for (browser, places) in histories(home) {
        let key = places.display().to_string();
        let mtime = |p: &Path| fs::metadata(p).and_then(|m| m.modified()).ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs());
        let side = |ext: &str| PathBuf::from(format!("{}-{ext}", places.display()));
        let stamp = mtime(&places).max(mtime(&side("wal"))).max(mtime(&side("journal")));
        if saved.get(&key).is_none_or(|s| s.stamp != stamp) {
            let found = match browser {
                Browser::Gecko => read_firefox(&places, &cache),
                Browser::Chromium => read_chromium(&places, &cache),
            };
            saved.insert(key.clone(), Saved { stamp, downloads: found });
        }
        out.extend(saved[&key].downloads.iter().map(|(file, page)| Download { file: file.clone(), page: page.clone() }));
    }
    let _ = fs::create_dir_all(&cache);
    if let Ok(text) = serde_json::to_string(&saved) {
        let _ = fs::write(&saved_file, text);
    }
    out
}

#[derive(Serialize, Deserialize)]
struct Saved {
    stamp: u64,
    downloads: Vec<(PathBuf, String)>,
}

fn read_firefox(places: &Path, cache: &Path) -> Vec<(PathBuf, String)> {
    let copy = cache.join(format!("firefox-{}.sqlite", std::process::id()));
    let side = |ext: &str| copy.with_extension(format!("sqlite-{ext}"));
    let _ = fs::create_dir_all(cache);
    let mut out = Vec::new();
    if fs::copy(places, &copy).is_ok() {
        let wal = places.with_extension("sqlite-wal");
        if wal.is_file() {
            let _ = fs::copy(&wal, side("wal"));
        }
        if let Ok(db) = rusqlite::Connection::open(&copy) {
            // A download is a visit of type 7 whose from_visit is the page the link was on.
            if let Ok(mut q) = db.prepare(
                "SELECT a.content, (SELECT pp.url FROM moz_historyvisits v JOIN moz_historyvisits fv ON fv.id = v.from_visit JOIN moz_places pp ON pp.id = fv.place_id WHERE v.place_id = p.id AND v.visit_type = 7 LIMIT 1)
                 FROM moz_annos a JOIN moz_anno_attributes n ON n.id = a.anno_attribute_id JOIN moz_places p ON p.id = a.place_id
                 WHERE n.name = 'downloads/destinationFileURI'",
            ) {
                let rows = q.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?)));
                for (dest, from) in rows.into_iter().flatten().flatten() {
                    if let (Some(file), Some(page)) = (file_from_uri(&dest), from.as_deref().and_then(game_page)) {
                        out.push((file, page));
                    }
                }
            }
        }
    }
    for f in [copy.clone(), side("wal"), side("shm")] {
        let _ = fs::remove_file(f);
    }
    out
}

/// Chromium's downloads from itch.io pages: the query asks only for rows whose tab, the tab's
/// referrer or the download's referrer is on itch.io; the first that's a game page counts (a
/// game's download page, `<game>/download/<key>`, is that game's).
fn read_chromium(history: &Path, cache: &Path) -> Vec<(PathBuf, String)> {
    let copy = cache.join(format!("chromium-{}.sqlite", std::process::id()));
    let _ = fs::create_dir_all(cache);
    let mut out = Vec::new();
    if fs::copy(history, &copy).is_ok() {
        if let Ok(db) = rusqlite::Connection::open(&copy) {
            out = itch_downloads(&db);
        }
    }
    let _ = fs::remove_file(&copy);
    out
}

fn itch_downloads(db: &rusqlite::Connection) -> Vec<(PathBuf, String)> {
    let Ok(mut q) = db.prepare(
        "SELECT target_path, tab_url, tab_referrer_url, referrer FROM downloads
         WHERE tab_url LIKE '%itch.io/%' OR tab_referrer_url LIKE '%itch.io/%' OR referrer LIKE '%itch.io/%'",
    ) else {
        return Vec::new();
    };
    let rows = q.query_map([], |r| Ok((r.get::<_, String>(0)?, [r.get::<_, Option<String>>(1)?, r.get::<_, Option<String>>(2)?, r.get::<_, Option<String>>(3)?])));
    rows.into_iter().flatten().flatten().filter_map(|(file, pages)| Some((PathBuf::from(file).clone(), pages.iter().flatten().find_map(|p| game_page(p))?))).filter(|(f, _)| !f.as_os_str().is_empty()).collect()
}

#[derive(Serialize, Deserialize, Clone, Default)]
struct Page {
    title: Option<String>,
    cover: Option<String>,
    /// Unix time of the last try: a page that failed is tried again after a day.
    fetched: u64,
}

fn cache_dir() -> PathBuf {
    let cache = std::env::var_os("XDG_CACHE_HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".cache"));
    cache.join("lobby")
}

fn fetch(page: &str) -> Page {
    let fetched = crate::store::now();
    let out = Command::new("curl").args(["-sfL", "--max-time", "20"]).arg(format!("{page}/data.json")).stdin(Stdio::null()).output();
    let Some(json) = out.ok().filter(|o| o.status.success()).and_then(|o| serde_json::from_slice::<serde_json::Value>(&o.stdout).ok()) else {
        return Page { fetched, ..Page::default() };
    };
    let text = |k: &str| json.get(k).and_then(|v| v.as_str()).filter(|s| !s.is_empty()).map(String::from);
    Page { title: text("title"), cover: text("cover_image"), fetched }
}

/// Titles and covers for the pages, from the cache or fetched (several at once).
fn pages(wanted: &[String]) -> HashMap<String, Page> {
    let file = cache_dir().join("itch.json");
    let mut cache: HashMap<String, Page> = fs::read_to_string(&file).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    let now = crate::store::now();
    let mut missing: Vec<String> = wanted.iter().filter(|p| cache.get(*p).is_none_or(|c| c.title.is_none() && now.saturating_sub(c.fetched) > 86_400)).cloned().collect();
    missing.sort();
    missing.dedup();
    if !missing.is_empty() {
        // Four at a time: itch.io turns some away when asked for many at once.
        let mut fetched: Vec<(String, Page)> = std::thread::scope(|s| {
            let handles: Vec<_> = missing.chunks(missing.len().div_ceil(4)).map(|chunk| s.spawn(move || chunk.iter().map(|p| (p.clone(), fetch(p))).collect::<Vec<_>>())).collect();
            handles.into_iter().flat_map(|h| h.join().unwrap_or_default()).collect()
        });
        // Those turned away, once more, one by one.
        for (page, got) in fetched.iter_mut().filter(|(_, got)| got.title.is_none()) {
            *got = fetch(page);
        }
        cache.extend(fetched);
        let _ = fs::create_dir_all(cache_dir());
        if let Ok(text) = serde_json::to_string(&cache) {
            let _ = fs::write(&file, text);
        }
    }
    cache
}

/// Entries for the folders and downloads that came from an itch.io page: the page's title and
/// cover, tied to the folder's program so they merge with it.
pub fn entries(home: &Home, found: &[Entry]) -> Vec<Entry> {
    let downloads = downloads(home);
    if downloads.is_empty() {
        return Vec::new();
    }
    let stem = |p: &Path| p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let mut matched: Vec<(&Entry, &Download)> = Vec::new();
    // Folders, and entries named after one: gamefix's app-menu entries and Steam shortcuts for a
    // downloaded game (its folder then isn't scanned as a folder: the entry has it).
    for e in found.iter().filter(|e| matches!(e.source, Source::Folder | Source::AppMenu | Source::Gamefix | Source::SteamShortcut) && e.program.is_some()) {
        let folder_name = e.folder.as_deref().map(stem).unwrap_or_default();
        let hit = downloads
            .iter()
            .find(|d| e.program.as_deref() == Some(d.file.as_path()))
            .or_else(|| downloads.iter().find(|d| same_name(&stem(&d.file), &folder_name) || same_name(&stem(&d.file), &e.name)));
        if let Some(d) = hit {
            matched.push((e, d));
        }
    }
    let wanted: Vec<String> = matched.iter().map(|(_, d)| d.page.clone()).collect();
    let pages = pages(&wanted);
    matched
        .into_iter()
        .map(|(e, d)| {
            let page = pages.get(&d.page).cloned().unwrap_or_default();
            let mut itch = Entry::new(Source::Itch, page.title.unwrap_or_else(|| e.name.clone()));
            // A download's page names it, but doesn't install it (or make a Mac version run).
            itch.kind = e.kind;
            itch.note = e.note.clone();
            itch.program = e.program.clone();
            itch.folder = e.folder.clone();
            itch.cover = page.cover;
            itch.keys.push(Key::Itch(d.page.clone()));
            itch
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chromium_downloads() {
        let db = rusqlite::Connection::open_in_memory().unwrap();
        db.execute_batch(
            "CREATE TABLE downloads (target_path TEXT, tab_url TEXT, tab_referrer_url TEXT, referrer TEXT);
             INSERT INTO downloads VALUES ('/home/u/Downloads/Game.zip', 'https://maker.itch.io/cool-game/download/abc123', 'https://itch.io/games', '');
             INSERT INTO downloads VALUES ('/home/u/Downloads/tax.pdf', 'https://bank.example/statements', '', '');
             INSERT INTO downloads VALUES ('/home/u/Downloads/other.zip', 'https://itch.io/my-purchases', 'https://maker.itch.io/other-game', '');
             INSERT INTO downloads VALUES ('', 'https://maker.itch.io/cancelled', '', '');",
        )
        .unwrap();
        assert_eq!(
            itch_downloads(&db),
            vec![(PathBuf::from("/home/u/Downloads/Game.zip"), "https://maker.itch.io/cool-game".to_string()), (PathBuf::from("/home/u/Downloads/other.zip"), "https://maker.itch.io/other-game".to_string())]
        );
    }

    #[test]
    fn game_pages() {
        assert_eq!(game_page("https://portfiend.itch.io/get-killed-by-circles?download").as_deref(), Some("https://portfiend.itch.io/get-killed-by-circles"));
        assert_eq!(game_page("https://fr0ke.itch.io/project-pitt-demo#download").as_deref(), Some("https://fr0ke.itch.io/project-pitt-demo"));
        assert_eq!(game_page("https://www.reddit.com/r/witcher/"), None);
        assert_eq!(game_page("https://itch.io/games"), None);
    }

    #[test]
    fn file_uris() {
        assert_eq!(file_from_uri("file:///home/u/Downloads/ULTRAKILL%20Prelude.zip"), Some(PathBuf::from("/home/u/Downloads/ULTRAKILL Prelude.zip")));
    }
}
