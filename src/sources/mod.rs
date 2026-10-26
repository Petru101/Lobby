//! The places games are found: launchers' own lists, app-menu entries, folders of games and
//! Wine prefixes. Each source reads what's there and never changes anything.

mod downloads;
pub mod faugus;
pub mod folders;
mod gamefix;
mod heroic;
mod itch;
pub mod lutris;
mod menu;
mod prefixes;
pub mod steam;
mod stores;

use crate::library::{self, Entry};

pub use faugus::plain_settings as faugus_plain_settings;
use std::path::{Path, PathBuf};

pub struct Home {
    pub home: PathBuf,
    pub user: String,
}

impl Home {
    pub fn find() -> Option<Home> {
        let home = PathBuf::from(std::env::var_os("HOME")?);
        let user = std::env::var("USER").ok().or_else(|| home.file_name().map(|n| n.to_string_lossy().into_owned()))?;
        Some(Home { home, user })
    }

    pub fn join(&self, rel: &str) -> PathBuf {
        self.home.join(rel)
    }

    /// A flatpak's data or config folder, or the same for the program installed normally:
    /// `flatpak("com.heroicgameslauncher.hgl", "config/heroic", ".config/heroic")`.
    pub fn app_dirs(&self, flatpak: &str, in_flatpak: &str, native: &str) -> Vec<PathBuf> {
        [self.home.join(".var/app").join(flatpak).join(in_flatpak), self.home.join(native)].into_iter().filter(|p| p.exists()).collect()
    }
}

pub struct Found {
    pub entries: Vec<Entry>,
    /// Folders that hold many games (a games drive, ~/Games): never one game's folder.
    pub roots: Vec<PathBuf>,
}

pub fn scan(home: &Home) -> Found {
    let mut entries = Vec::new();
    entries.extend(crate::log::timed("scan: Steam games", || steam::apps(home)));
    entries.extend(crate::log::timed("scan: Steam libraries Steam doesn't list", || steam::stray_apps(home)));
    entries.extend(crate::log::timed("scan: Steam shortcuts", || steam::shortcuts(home)));
    entries.extend(crate::log::timed("scan: lobby's store games", || stores::games()));
    entries.extend(crate::log::timed("scan: Heroic", || heroic::games(home)));
    entries.extend(crate::log::timed("scan: Lutris", || lutris::games(home)));
    entries.extend(crate::log::timed("scan: Faugus", || faugus::games(home)));
    entries.extend(crate::log::timed("scan: gamefix records", || gamefix::records(home)));
    entries.extend(crate::log::timed("scan: app menu", || menu::entries(home)));

    let roots = folders::roots(home);
    let games = crate::places::Places::load().games;
    let strict = folders::strict_roots(home);
    let (found, folder_prefixes, collections) = crate::log::timed("scan: folders", || folders::scan(&roots, &strict, &games, &entries));
    entries.extend(found);
    // Names and covers for what came from itch.io.
    let itch = crate::log::timed("scan: itch.io", || itch::entries(home, &entries));
    entries.extend(itch);

    // Every prefix any source mentions, the usual places prefixes are kept, and the ones the
    // folder scan came across.
    let mut pfxs: Vec<PathBuf> = entries.iter().filter_map(|e| e.prefix.clone()).collect();
    pfxs.extend(entries.iter().filter_map(|e| e.folder.as_deref().and_then(library::prefix_in)));
    pfxs.extend(folder_prefixes);
    pfxs.extend(prefixes::usual(home));
    entries.extend(crate::log::timed("scan: Wine prefixes", || prefixes::scan(pfxs)));

    let mut wide = roots.clone();
    wide.extend(collections);
    wide.push(home.home.clone());
    for d in ["Downloads", "Desktop", "Documents", "Games"] {
        wide.push(home.join(d));
    }
    Found { entries, roots: wide.iter().map(|p| library::norm(p)).collect() }
}

/// Reads a JSON file, or nothing.
fn json(path: &Path) -> Option<serde_json::Value> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

fn str_of<'a>(v: &'a serde_json::Value, key: &str) -> Option<&'a str> {
    v.get(key)?.as_str().filter(|s| !s.is_empty())
}
