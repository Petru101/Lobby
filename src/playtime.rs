//! When each game was last played and for how long, by game id, in
//! ~/.local/share/lobby/playtime.json: Lobby's own sessions, timed where Lobby waits for the
//! game (`lobby sandbox`, which every start Lobby makes and every shortcut it wraps goes
//! through, and `lobby run`), plus what the launchers recorded (Steam, Heroic, Lutris, Faugus:
//! `Entry::playtime`, `Entry::last_played`), copied in at each scan so it stays when the
//! launcher is removed. Lobby's sessions and the launchers' never overlap: a game Lobby starts
//! itself isn't seen by its launcher, and one Lobby hands to Steam isn't timed by Lobby.

use crate::library::Game;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct Played {
    /// Seconds played through Lobby.
    #[serde(default)]
    pub own: u64,
    /// Seconds the launchers counted (the most any one did: a Steam shortcut starting a Lutris
    /// game counts the same time twice).
    #[serde(default)]
    pub launchers: u64,
    /// Unix time it was last started (0: never).
    #[serde(default)]
    pub last: u64,
}

impl Played {
    pub fn seconds(&self) -> u64 {
        self.own + self.launchers
    }
}

fn file() -> PathBuf {
    crate::store::dir().join("playtime.json")
}

pub fn load() -> HashMap<String, Played> {
    fs::read_to_string(file()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

/// Changes the record under a lock: a game's `lobby sandbox` ending and a scan can save at the
/// same moment.
fn change(f: impl FnOnce(&mut HashMap<String, Played>)) -> Result<(), String> {
    let path = file();
    fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
    let lock = fs::File::create(path.with_extension("lock")).map_err(|e| e.to_string())?;
    lock.lock().map_err(|e| e.to_string())?;
    let mut map = load();
    f(&mut map);
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, serde_json::to_string_pretty(&map).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    fs::rename(&tmp, &path).map_err(|e| e.to_string())
}

/// A session Lobby timed, from its start to its end (Unix seconds).
pub fn session(id: &str, start: u64, end: u64) {
    let result = change(|map| {
        let p = map.entry(id.to_string()).or_default();
        p.own += end.saturating_sub(start);
        p.last = p.last.max(start);
    });
    if let Err(e) = result {
        crate::log::line(&format!("play time of {id}: {e}"));
    }
}

/// What the launchers recorded, taken in after a scan (only ever growing: a launcher removed,
/// or a game it forgot, keeps what it had counted).
pub fn take_in(games: &[Game]) {
    let found: Vec<(String, u64, u64)> = games
        .iter()
        .filter_map(|g| {
            let seconds = g.entries.iter().filter_map(|e| e.playtime).max().unwrap_or(0);
            let last = g.entries.iter().filter_map(|e| e.last_played).max().unwrap_or(0);
            (seconds > 0 || last > 0).then(|| (g.id.clone(), seconds, last))
        })
        .collect();
    let old = load();
    let new = |(id, s, l): &(String, u64, u64)| old.get(id).is_none_or(|p| *s > p.launchers || *l > p.last);
    if !found.iter().any(new) {
        return;
    }
    let result = change(|map| {
        for (id, seconds, last) in found {
            let p = map.entry(id).or_default();
            p.launchers = p.launchers.max(seconds);
            p.last = p.last.max(last);
        }
    });
    if let Err(e) = result {
        crate::log::line(&format!("play times: {e}"));
    }
}

/// The game's record, with the last time Lobby started it (Play) counted too.
pub fn of(map: &HashMap<String, Played>, game: &Game) -> Played {
    let mut p = map.get(&game.id).copied().unwrap_or_default();
    p.last = p.last.max(game.played.unwrap_or(0));
    p
}

/// "3 h 20 min", "45 min", "under a minute".
pub fn text(seconds: u64) -> String {
    let minutes = seconds / 60;
    match (minutes / 60, minutes % 60) {
        (0, 0) => "under a minute".into(),
        (0, m) => format!("{m} min"),
        (h, 0) => format!("{h} h"),
        (h, m) if h < 10 => format!("{h} h {m} min"),
        (h, _) => format!("{h} h"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn texts() {
        assert_eq!(text(20), "under a minute");
        assert_eq!(text(45 * 60), "45 min");
        assert_eq!(text(3 * 3600 + 20 * 60), "3 h 20 min");
        assert_eq!(text(25 * 3600 + 59), "25 h");
    }
}
