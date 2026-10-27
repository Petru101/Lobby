//! How much room each installed game takes: what its launcher records (Steam's SizeOnDisk,
//! legendary's install_size), else its folder measured once and kept in
//! ~/.cache/lobby/sizes.json while the folder's own change time stays the same (an update or
//! a removed file usually changes something at its top; measured again after 30 days anyway).

use crate::library::{Game, Kind};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Serialize, Deserialize)]
struct Measured {
    bytes: u64,
    mtime: u64,
    when: u64,
}

fn file() -> PathBuf {
    crate::stores::cache_dir().join("sizes.json")
}

fn load() -> HashMap<PathBuf, Measured> {
    fs::read_to_string(file()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

fn mtime(path: &Path) -> u64 {
    fs::metadata(path).and_then(|m| m.modified()).ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs())
}

/// The folder to measure: the installed game's own, unless it's a folder of many things (the
/// home folder or above, a drive's top, a folder Lobby looks in, one named like games or
/// downloads), whose size wouldn't be the game's.
fn folder(game: &Game, wide: &[PathBuf]) -> Option<PathBuf> {
    if game.kind != Kind::Game || game.gone {
        return None;
    }
    let f = crate::library::norm(game.folder()?);
    let home = crate::library::norm(&PathBuf::from(std::env::var_os("HOME")?));
    let drive_top = f.parent().and_then(Path::parent).is_some_and(|p| p == Path::new("/run/media")) || f.components().count() <= 4 && f.starts_with("/run/media");
    let named = f.file_name().is_some_and(|n| crate::places::games_or_downloads(&n.to_string_lossy()));
    (f.is_dir() && !home.starts_with(&f) && !drive_top && !named && !wide.contains(&f)).then_some(f)
}

/// The folders Lobby looks in: never one game's.
fn wide() -> Vec<PathBuf> {
    crate::sources::Home::find().map(|h| crate::sources::folders::roots(&h).iter().map(|r| crate::library::norm(r)).collect()).unwrap_or_default()
}

/// Every game's size Lobby knows now, by id (nothing is measured here).
pub fn known(games: &[Game]) -> HashMap<String, u64> {
    let cache = load();
    let wide = wide();
    games
        .iter()
        .filter_map(|g| {
            let recorded = g.entries.iter().filter_map(|e| e.size).max();
            let measured = || folder(g, &wide).and_then(|f| cache.get(&f).map(|m| m.bytes));
            Some((g.id.clone(), recorded.or_else(measured)?))
        })
        .collect()
}

/// Measures the installed games whose size isn't known or is out of date. Takes a while the
/// first time (every file of every game); returns how many were measured.
pub fn measure(games: &[Game]) -> usize {
    let mut cache = load();
    let wide = wide();
    let now = crate::store::now();
    let todo: Vec<PathBuf> = games
        .iter()
        .filter(|g| g.entries.iter().all(|e| e.size.is_none()))
        .filter_map(|g| folder(g, &wide))
        .filter(|f| cache.get(f).is_none_or(|m| m.mtime != mtime(f) || now.saturating_sub(m.when) > 30 * 86400))
        .collect();
    let save = |cache: &HashMap<PathBuf, Measured>| {
        if let Ok(text) = serde_json::to_string(cache) {
            let _ = fs::create_dir_all(crate::stores::cache_dir());
            let _ = fs::write(file(), text);
        }
    };
    // Saved every 20 folders too: Lobby closed halfway keeps what it measured.
    for (n, f) in todo.iter().enumerate() {
        let bytes = crate::log::timed(&format!("size of {}", f.display()), || walk(f));
        cache.insert(f.clone(), Measured { bytes, mtime: mtime(f), when: now });
        if n % 20 == 19 {
            save(&cache);
        }
    }
    if !todo.is_empty() {
        save(&cache);
    }
    todo.len()
}

/// Every file's size under a folder, links not followed.
pub(crate) fn walk(dir: &Path) -> u64 {
    let mut total = 0;
    let mut dirs = vec![dir.to_path_buf()];
    while let Some(d) = dirs.pop() {
        for entry in fs::read_dir(&d).into_iter().flatten().flatten() {
            let Ok(meta) = entry.path().symlink_metadata() else { continue };
            if meta.is_dir() {
                dirs.push(entry.path());
            } else if meta.is_file() {
                total += meta.len();
            }
        }
    }
    total
}
