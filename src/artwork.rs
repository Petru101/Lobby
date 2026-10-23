//! Covers (and better names) for games that came with none: games found only as folders on a
//! drive. Looked for, in order, among the games the user owns on GOG, Epic and Amazon (Heroic's
//! and Lutris's store lists), on itch.io for folders named like an itch.io page, and in Steam's
//! store search. A match has to be sure: a sequel, a DLC or a soundtrack is not the game.
//! Results, found or not, are kept in ~/.cache/lobby/artwork.json.

use crate::library::{Entry, Game, Kind, Source};
use crate::sources::Home;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Words that name an edition, not a game ("Game of the Year Edition", "Gold", "HD"), or that
/// some stores add to a name ("Anno 1602 A.D." on GOG).
const EDITION: &[&str] = &["the", "a", "of", "and", "edition", "goty", "game", "year", "definitive", "gold", "platinum", "complete", "collection", "enhanced", "anniversary", "deluxe", "ultimate", "remastered", "director", "directors", "cut", "hd", "classic", "special", "history", "version", "ad"];

/// Words that make a search result something other than the game.
const NOT_THE_GAME: &[&str] = &["soundtrack", "ost", "dlc", "artbook", "pack", "expansion", "season", "pass", "editor", "sdk", "server", "tool", "tools", "demo", "playtest", "bundle", "upgrade", "costume", "skin", "skins"];

fn words(s: &str) -> Vec<String> {
    let s = s.to_lowercase().replace(['™', '®', '©', '\'', '’', '`', '.'], "").replace('&', " and ");
    s.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).map(String::from).collect()
}

fn core(w: &[String]) -> Vec<String> {
    w.iter().filter(|w| !EDITION.contains(&w.as_str())).cloned().collect()
}

/// How sure it is that `candidate` names the game called `query`: 3 the same name, 2 the same
/// once edition words are left out, 1 a series name and a colon or dash before it ("The Elder
/// Scrolls III: Morrowind"; not "LEGO Harry Potter Collection"), 0 not. Words after the name
/// make it another game of the series ("Starship Troopers: Terran Command", "Atomic Heart 2")
/// or a DLC.
pub fn score(query: &str, candidate: &str) -> u8 {
    let (q, c) = (words(query), words(candidate));
    if q.concat().len() < 4 || q.iter().all(|w| EDITION.contains(&w.as_str())) {
        return 0;
    }
    if q.concat() == c.concat() {
        return 3;
    }
    let (qc, cc) = (core(&q), core(&c));
    // The demo, the soundtrack ... unless the folder says so too.
    if c.iter().any(|w| NOT_THE_GAME.contains(&w.as_str()) && !q.contains(w)) {
        return 0;
    }
    if !qc.is_empty() && qc.concat() == cc.concat() {
        return 2;
    }
    // A one-word name after a series name is too loose ("Dungeons" in "Minecraft: Dungeons"),
    // unless it's a long, distinctive word.
    let distinctive = qc.len() >= 2 || qc.first().is_some_and(|w| w.len() >= 9);
    let after_series = [": ", " - "].iter().filter_map(|sep| candidate.split_once(sep)).any(|(series, rest)| words(series).len() <= 4 && score(query, rest) >= 2);
    if distinctive && after_series { 1 } else { 0 }
}

/// A game the user owns in a store, with portrait art.
struct Owned {
    title: String,
    art: String,
}

fn owned(home: &Home) -> Vec<Owned> {
    let mut out = Vec::new();
    for heroic in home.app_dirs("com.heroicgameslauncher.hgl", "config/heroic", ".config/heroic") {
        for (file, list) in [("gog_library.json", "games"), ("legendary_library.json", "library"), ("nile_library.json", "library")] {
            let Some(v) = fs::read_to_string(heroic.join("store_cache").join(file)).ok().and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok()) else { continue };
            for g in v.get(list).and_then(|l| l.as_array()).into_iter().flatten() {
                let text = |k: &str| g.get(k).and_then(|x| x.as_str()).filter(|s| !s.is_empty()).map(String::from);
                if let (Some(title), Some(art)) = (text("title"), text("art_square")) {
                    out.push(Owned { title, art });
                }
            }
        }
    }
    // The accounts signed in to in lobby.
    for store in crate::stores::Store::ALL {
        out.extend(crate::stores::owned(store).into_iter().filter_map(|o| Some(Owned { title: o.title, art: o.cover? })));
    }
    // Lutris's Epic list: the tall box art among the key images.
    for dir in home.app_dirs("net.lutris.Lutris", "data/lutris", ".local/share/lutris") {
        let Ok(db) = rusqlite::Connection::open_with_flags(dir.join("pga.db"), rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY) else { continue };
        let Ok(mut q) = db.prepare("SELECT name, details FROM service_games WHERE service = 'egs' AND details IS NOT NULL") else { continue };
        let rows = q.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)));
        for (title, details) in rows.into_iter().flatten().flatten() {
            let Ok(d) = serde_json::from_str::<serde_json::Value>(&details) else { continue };
            let art = d.get("keyImages").and_then(|k| k.as_array()).into_iter().flatten().find(|i| i.get("type").and_then(|t| t.as_str()) == Some("DieselGameBoxTall")).and_then(|i| i.get("url")?.as_str().map(String::from));
            if let Some(art) = art {
                out.push(Owned { title, art });
            }
        }
    }
    out
}

#[derive(Serialize, Deserialize, Clone, Default)]
struct Found {
    /// Set when the match is sure enough to name the game by it.
    name: Option<String>,
    cover: Option<String>,
    from: String,
    /// Unix time: a search that found nothing is tried again after a month.
    when: u64,
}

/// A web request that failed (no network, or the site turned it away: Steam limits how often
/// it's asked), as opposed to one that found nothing. A failure isn't remembered.
struct Failed;

/// LOBBY_DEBUG_ARTWORK=1 prints each step of a lookup.
fn debug(what: impl FnOnce() -> String) {
    if std::env::var_os("LOBBY_DEBUG_ARTWORK").is_some() {
        eprintln!("artwork: {}", what());
    }
}

fn curl(url: &str) -> Result<Vec<u8>, Failed> {
    let out = Command::new("curl").args(["-sfL", "--max-time", "20"]).arg(url).stdin(Stdio::null()).output().map_err(|_| Failed)?;
    if out.status.success() { Ok(out.stdout) } else { Err(Failed) }
}

fn query_arg(s: &str) -> String {
    s.bytes().map(|b| if b.is_ascii_alphanumeric() { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
}

/// Steam's store search: the surest result's portrait art (its header, wider, when an old game
/// has none).
fn steam(query: &str, folder: Option<&Path>) -> Result<Option<Found>, Failed> {
    // Plain words: punctuation ("Starship Troopers - Ultimate Bug War!") makes it find nothing.
    let term = words(query).join(" ");
    let json: serde_json::Value = serde_json::from_slice(&curl(&format!("https://store.steampowered.com/api/storesearch/?term={}&l=english&cc=US", query_arg(&term)))?).map_err(|_| Failed)?;
    let mut results: Vec<(u8, u64, String)> = json
        .get("items")
        .and_then(|i| i.as_array())
        .into_iter()
        .flatten()
        .filter_map(|i| Some((score(query, i.get("name")?.as_str()?), i.get("id")?.as_u64()?, i.get("name")?.as_str()?.to_string())))
        .filter(|(s, ..)| *s > 0)
        .collect();
    // The surest first; among equals Steam's own order.
    results.sort_by_key(|(s, ..)| std::cmp::Reverse(*s));
    debug(|| format!("Steam search {term:?} (folder {folder:?}): {results:?}"));
    for (s, id, name) in results {
        let item = steam_item(id)?;
        if !same_maker_or_era(folder, &item) {
            continue;
        }
        let Some(cover) = item.cover else { continue };
        // Named after it only when it's the same name: "Anno 1602" stays itself, not the History Edition.
        return Ok(Some(Found { name: (s >= 3).then_some(name.replace(['™', '®'], "")), cover: Some(cover), from: format!("Steam store, app {id}"), when: 0 }));
    }
    Ok(None)
}

/// Makers that are an engine's default, not the game's ("DefaultCompany" is Unity's).
const NO_MAKER: &[&str] = &["defaultcompany", "unity technologies", "epic games", "epic games, inc.", "microsoft corporation", "godot engine", "yoyo games", "installshield", "nullsoft", "inno setup"];

/// The makers the game's biggest programs name (Windows' CompanyName), and the year of the
/// newest of those.
fn programs_info(folder: &Path) -> (Vec<String>, Option<u64>) {
    let mut exes: Vec<(u64, PathBuf, u64)> = Vec::new();
    let mut dirs = vec![(folder.to_path_buf(), 0)];
    while let Some((dir, depth)) = dirs.pop() {
        for e in fs::read_dir(&dir).into_iter().flatten().flatten() {
            let p = e.path();
            let Ok(meta) = e.metadata() else { continue };
            if meta.is_dir() {
                let name = e.file_name().to_string_lossy().to_lowercase();
                // Wine's own programs say nothing about the game (and are new).
                let wine = ["windows", "internet explorer", "windows media player", "windows nt", "common files", "microsoft.net"].contains(&name.as_str());
                if depth < 6 && !wine && !name.starts_with('.') && exes.len() < 400 {
                    dirs.push((p, depth + 1));
                }
            } else if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("exe")) && !gamefix::scan::not_the_game(&p) {
                let secs = meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs());
                exes.push((meta.len(), p, secs));
            }
        }
    }
    exes.sort_by_key(|(size, ..)| std::cmp::Reverse(*size));
    let makers = exes.iter().take(5).filter_map(|(_, p, _)| gamefix::binary::company(p)).filter(|c| !NO_MAKER.contains(&c.to_lowercase().trim())).collect();
    let year = exes.iter().take(5).map(|(.., s)| 1970 + s / 31_556_952).max();
    (makers, year)
}

/// Words that say what kind of company, not which.
const COMPANY_WORDS: &[&str] = &["ltd", "inc", "llc", "gmbh", "studio", "studios", "games", "game", "entertainment", "interactive", "software", "productions", "co", "corp", "corporation", "the", "limited", "sa", "ab", "srl", "digital", "publishing"];

/// What Steam's store says about an app: who made it, when it first came out (the original
/// release, not its Steam one: Morrowind's 2002), and its portrait art (newer apps keep it at a
/// hashed path only the store's item list gives), else its header.
struct Item {
    id: u64,
    makers: Vec<String>,
    released: Option<u64>,
    cover: Option<String>,
}

fn steam_item(id: u64) -> Result<Item, Failed> {
    let input = format!(r#"{{"ids":[{{"appid":{id}}}],"context":{{"language":"english","country_code":"US"}},"data_request":{{"include_assets":true,"include_release":true,"include_basic_info":true}}}}"#);
    let json: serde_json::Value = serde_json::from_slice(&curl(&format!("https://api.steampowered.com/IStoreBrowseService/GetItems/v1/?input_json={}", query_arg(&input)))?).map_err(|_| Failed)?;
    let item = &json["response"]["store_items"][0];
    let makers = ["developers", "publishers"].iter().flat_map(|k| item["basic_info"][*k].as_array().cloned().unwrap_or_default()).filter_map(|m| m["name"].as_str().map(String::from)).collect();
    let release = &item["release"];
    let released = release["original_release_date"].as_u64().or_else(|| release["steam_release_date"].as_u64()).map(|t| 1970 + t / 31_556_952);
    let assets = &item["assets"];
    let cover = ["library_capsule", "header"].iter().find_map(|k| assets[*k].as_str()).zip(assets["asset_url_format"].as_str()).map(|(file, format)| format!("https://shared.steamstatic.com/store_item_assets/{}", format.replace("${FILENAME}", file)));
    Ok(Item { id, makers, released, cover })
}

/// Whether the Steam app can be the game in the folder. A same-named game from another maker
/// many years later isn't it (Black & White: Lionhead's, Steam's from another studio in 2023).
/// A maker named like a company decides: the app's developer or publisher, or not the game.
/// With no maker, or only a project's name (Unreal games often name that instead of their
/// studio), the years decide: the app at most 10 years newer than the game's programs (an
/// itch.io build from 2020 can be a 2026 Steam release's early version; installed files carry
/// the install's date, so years alone can't tell an old game from a new one).
fn same_maker_or_era(folder: Option<&Path>, item: &Item) -> bool {
    let Some(folder) = folder else { return true };
    let (makers, newest) = programs_info(folder);
    let words_of = |s: &str| words(s).into_iter().filter(|w| !COMPANY_WORDS.contains(&w.as_str()) && w.len() > 1).collect::<Vec<_>>();
    let steam_makers: Vec<String> = item.makers.iter().flat_map(|m| words_of(m)).collect();
    let gap = newest.zip(item.released).map(|(files, released)| released.saturating_sub(files));
    debug(|| format!("app {}: its makers {steam_makers:?}, released {:?}; the game's makers {makers:?}, newest program {newest:?}", item.id, item.released));
    if steam_makers.is_empty() || makers.is_empty() {
        return gap.is_none_or(|g| g <= 10);
    }
    if makers.iter().any(|m| words_of(m).iter().any(|w| steam_makers.contains(w))) {
        return true;
    }
    // A maker that reads like a company ("LionHead Studios Ltd.", "MAX DESIGN") that isn't the
    // app's is another game; a lone project name (Unreal's) says too little: the years decide.
    let company = makers.iter().any(|m| m.trim().contains(' ') || words(m).iter().any(|w| COMPANY_WORDS.contains(&w.as_str())));
    !company && gap.is_none_or(|g| g <= 10)
}

/// itch.io's search, for a folder named like an itch.io page (`astro-defend`): only a result
/// whose page has exactly that name.
fn itch(slug: &str) -> Result<Option<Found>, Failed> {
    let html = String::from_utf8_lossy(&curl(&format!("https://itch.io/search?q={}", query_arg(&slug.replace('-', " "))))?).into_owned();
    let page = html.split("href=\"").skip(1).filter_map(|s| s.split('"').next()).find(|u| {
        u.strip_prefix("https://").and_then(|r| r.split_once(".itch.io/")).is_some_and(|(creator, game)| !creator.contains('/') && game == slug)
    });
    let Some(page) = page else { return Ok(None) };
    let json: serde_json::Value = serde_json::from_slice(&curl(&format!("{page}/data.json"))?).map_err(|_| Failed)?;
    let text = |k: &str| json.get(k).and_then(|v| v.as_str()).filter(|s| !s.is_empty()).map(String::from);
    Ok(Some(Found { name: text("title"), cover: text("cover_image"), from: page.to_string(), when: 0 }))
}

/// A folder name that is an itch.io page's: lowercase words joined by dashes.
fn slug_like(s: &str) -> bool {
    s.contains('-') && s.split('-').all(|w| !w.is_empty() && w.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()))
}

/// Names to look a game up by: its own, then its folder's and the folders between that and
/// its program ("Serious Sam TFE/Serious Sam The First Encounter/Bin").
fn names(g: &Game) -> Vec<String> {
    let mut out = vec![g.name.clone()];
    if let (Some(folder), Some(program)) = (g.folder(), g.program()) {
        out.extend(folder.file_name().map(|n| n.to_string_lossy().into_owned()).filter(|n| words(n).len() >= 2));
        if let Ok(rel) = program.parent().unwrap_or(program).strip_prefix(folder) {
            out.extend(rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).filter(|n| words(n).len() >= 2));
        }
    }
    out.dedup();
    out
}

fn cache_file() -> PathBuf {
    let cache = std::env::var_os("XDG_CACHE_HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".cache"));
    cache.join("lobby/artwork.json")
}

/// Adds a cover (and, when sure, the right name) to every game that has none.
pub fn fill(home: &Home, games: &mut [Game]) {
    let wanted: Vec<usize> = (0..games.len()).filter(|&i| matches!(games[i].kind, Kind::Game | Kind::Download) && games[i].cover().is_none() && !games[i].gone).collect();
    if wanted.is_empty() {
        return;
    }
    let file = cache_file();
    let mut cache: HashMap<String, Found> = fs::read_to_string(&file).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    let owned = owned(home);
    let now = crate::store::now();
    let stale = |f: &Found| f.cover.is_none() && now.saturating_sub(f.when) > 30 * 86_400;

    // What to look up for each game: its key in the cache, or an owned game's art.
    let mut lookups: Vec<(usize, String)> = Vec::new();
    let mut results: Vec<(usize, Found)> = Vec::new();
    for &i in &wanted {
        let names = names(&games[i]);
        let own = names.iter().flat_map(|n| owned.iter().map(move |o| (score(n, &o.title), o))).filter(|(s, _)| *s >= 2).max_by_key(|(s, _)| *s);
        if let Some((s, o)) = own {
            results.push((i, Found { name: (s >= 2).then(|| o.title.replace(['™', '®'], "")), cover: Some(o.art.clone()), from: "a game you own".into(), when: now }));
            continue;
        }
        lookups.push((i, names[0].clone()));
    }
    // Searches not done yet (or that found nothing a month ago), two at a time: Steam turns
    // away a client that asks too often.
    let mut todo: Vec<(String, Vec<String>, Option<PathBuf>)> = Vec::new();
    for (i, key) in &lookups {
        if cache.get(key).is_none_or(stale) && !todo.iter().any(|(k, ..)| k == key) {
            let folder = games[*i].folder().and_then(Path::file_name).map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let mut names = names(&games[*i]);
            // Only a game found on a drive alone: "black-white" is also some itch.io page, but
            // the Black & White the app menu starts isn't that game.
            if slug_like(&folder) && games[*i].unknown_to_launchers() {
                names.insert(0, format!("itch:{folder}"));
            }
            todo.push((key.clone(), names, games[*i].folder().map(Path::to_path_buf)));
        }
    }
    if !todo.is_empty() {
        let found: Vec<(String, Found)> = std::thread::scope(|s| {
            let handles: Vec<_> = todo
                .chunks(todo.len().div_ceil(2))
                .map(|chunk| {
                    s.spawn(move || {
                        chunk
                            .iter()
                            .filter_map(|(key, names, folder)| {
                                let mut failed = false;
                                for n in names {
                                    let result = match n.strip_prefix("itch:") {
                                        Some(slug) => itch(slug),
                                        // An itch.io page's name isn't a store game's.
                                        None if names.iter().any(|n| n.starts_with("itch:")) => Ok(None),
                                        None => steam(n, folder.as_deref()),
                                    };
                                    match result {
                                        Ok(Some(hit)) => return Some((key.clone(), Found { when: now, ..hit })),
                                        Ok(None) => {}
                                        Err(Failed) => failed = true,
                                    }
                                }
                                // Nothing found, unless a request failed: then ask again next scan.
                                (!failed).then(|| (key.clone(), Found { when: now, ..Found::default() }))
                            })
                            .collect::<Vec<_>>()
                    })
                })
                .collect();
            handles.into_iter().flat_map(|h| h.join().unwrap_or_default()).collect()
        });
        cache.extend(found);
        let _ = fs::create_dir_all(file.parent().unwrap());
        if let Ok(text) = serde_json::to_string(&cache) {
            let _ = fs::write(&file, text);
        }
    }
    results.extend(lookups.into_iter().filter_map(|(i, key)| cache.get(&key).filter(|f| f.cover.is_some()).map(|f| (i, f.clone()))));

    let covered: Vec<usize> = results.iter().map(|(i, _)| *i).collect();
    // Still nothing: the icon in the game's own Windows program, for its placeholder card.
    for &i in wanted.iter().filter(|i| !covered.contains(i)) {
        let g = &mut games[i];
        if g.icon().is_some() {
            continue;
        }
        let Some(program) = g.program().filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("exe"))).map(Path::to_path_buf) else { continue };
        if let (Some(icon), Some(e)) = (icon_file(&program), g.entries.first_mut()) {
            e.icon = Some(icon);
        }
    }
    for (i, f) in results {
        let g = &mut games[i];
        let mut e = Entry::new(Source::Artwork, f.name.clone().unwrap_or_else(|| g.name.clone()));
        e.cover = f.cover;
        e.program = g.program().map(Path::to_path_buf);
        e.folder = g.folder().map(Path::to_path_buf);
        if let Some(name) = f.name {
            g.name = name;
        }
        g.entries.push(e);
    }
}

/// The program's own icon (gamefix's reader), saved as a PNG in lobby's cache once, for the
/// window to show.
fn icon_file(program: &Path) -> Option<PathBuf> {
    let file = cache_file().with_file_name("icons").join(format!("{}.png", crate::sha256_hex(program.as_os_str().as_encoded_bytes())));
    if file.is_file() {
        return Some(file);
    }
    let png = gamefix::peicon::icon_png(program)?;
    fs::create_dir_all(file.parent()?).ok()?;
    fs::write(&file, png).ok()?;
    Some(file)
}

/// A cover to choose from, for the cover picker.
pub struct Candidate {
    pub title: String,
    pub cover: String,
    /// Where it's from, in a few words.
    pub from: String,
}

/// Covers for the user to choose from: their own games named like the search, then Steam's and
/// itch.io's results. Looser than the automatic match: the user picks.
pub fn candidates(query: &str) -> Vec<Candidate> {
    let mut out = Vec::new();
    let Some(home) = Home::find() else { return out };
    let q = words(query);
    if q.is_empty() {
        return out;
    }
    for o in owned(&home) {
        let t = words(&o.title);
        if score(query, &o.title) > 0 || q.iter().all(|w| t.contains(w)) {
            out.push(Candidate { title: o.title, cover: o.art, from: "Your games".into() });
        }
    }
    out.truncate(12);
    let term = q.join(" ");
    if let Ok(body) = curl(&format!("https://store.steampowered.com/api/storesearch/?term={}&l=english&cc=US", query_arg(&term))) {
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap_or_default();
        let ids: Vec<(u64, String)> = json["items"].as_array().into_iter().flatten().filter_map(|i| Some((i["id"].as_u64()?, i["name"].as_str()?.to_string()))).take(8).collect();
        for (id, name) in ids {
            if let Ok(Item { cover: Some(cover), .. }) = steam_item(id) {
                out.push(Candidate { title: name, cover, from: "Steam".into() });
            }
        }
    }
    if let Ok(body) = curl(&format!("https://itch.io/search?q={}", query_arg(&term))) {
        let html = String::from_utf8_lossy(&body).into_owned();
        let mut pages: Vec<String> = Vec::new();
        for u in html.split("href=\"").skip(1).filter_map(|s| s.split('"').next()) {
            let page = u.strip_prefix("https://").and_then(|r| r.split_once(".itch.io/")).filter(|(c, g)| !c.contains('/') && !g.is_empty() && !g.contains('/') && !g.contains('?'));
            if page.is_some() && !pages.iter().any(|p| p == u) && pages.len() < 6 {
                pages.push(u.to_string());
            }
        }
        for page in pages {
            let Ok(body) = curl(&format!("{page}/data.json")) else { continue };
            let json: serde_json::Value = serde_json::from_slice(&body).unwrap_or_default();
            if let (Some(title), Some(cover)) = (json["title"].as_str(), json["cover_image"].as_str()) {
                out.push(Candidate { title: title.to_string(), cover: cover.to_string(), from: "itch.io".into() });
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scores() {
        assert_eq!(score("Atomic Heart", "Atomic Heart"), 3);
        assert_eq!(score("Atomic Heart", "Atomic Heart 2"), 0);
        assert_eq!(score("Atomic Heart", "Atomic Heart - Annihilation Instinct"), 0);
        assert_eq!(score("Starship Troopers", "Starship Troopers: Terran Command"), 0);
        assert_eq!(score("Dungeons", "Minecraft Dungeons"), 0);
        assert_eq!(score("Harry Potter", "LEGO® Harry Potter™ Collection"), 0);
        assert_eq!(score("Morrowind", "The Elder Scrolls III: Morrowind® Game of the Year Edition"), 1);
        assert_eq!(score("Morrowind", "The Elder Scrolls III: Morrowind Soundtrack"), 0);
        assert_eq!(score("Anno 1602", "Anno 1602 History Edition"), 2);
        assert_eq!(score("Anno 1602", "Anno 1602 A.D."), 2);
        assert_eq!(score("FEAR Platinum", "F.E.A.R. Platinum"), 3);
        assert_eq!(score("Cossacks Back To War", "Cossacks - Back To War"), 3);
        assert_eq!(score("Empire Earth", "Empire Earth II Gold Edition"), 0);
        assert_eq!(score("Empire Earth Gold", "Empire Earth"), 2);
        assert_eq!(score("Serious Sam The First Encounter", "Serious Sam HD: The First Encounter"), 2);
        assert_eq!(score("Unreal Tournament", "Unreal Tournament 2004: Editor's Choice Edition"), 0);
        assert_eq!(score("Unreal Tournament", "Unreal Tournament: Game of the Year Edition"), 2);
        assert_eq!(score("game", "Game"), 0);
        assert_eq!(score("Hell Pie Demo", "Hell Pie Demo"), 3);
    }

    #[test]
    fn slugs() {
        assert!(slug_like("astro-defend"));
        assert!(slug_like("the-mammoth"));
        assert!(!slug_like("Atomic Heart"));
        assert!(!slug_like("mercapocalypse_v1.1"));
    }
}
