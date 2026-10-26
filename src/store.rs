//! lobby's own library, ~/.local/share/lobby/library.json: every game it has found, with an id
//! that stays the same from scan to scan. A game found once is remembered: when no source finds
//! it any more (a drive that isn't plugged in, an uninstall), it's kept, marked gone.

use crate::library::Game;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Default, Serialize, Deserialize)]
pub struct Library {
    pub games: Vec<Game>,
    /// When it was last scanned (Unix time).
    #[serde(default)]
    pub scanned: u64,
}

pub fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

fn data_home() -> PathBuf {
    std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/share"))
}

pub fn dir() -> PathBuf {
    data_home().join("lobby")
}

fn file() -> PathBuf {
    dir().join("library.json")
}

impl Library {
    pub fn load() -> Option<Library> {
        serde_json::from_str(&fs::read_to_string(file()).ok()?).ok()
    }

    /// Written through a temporary file, so a crash never leaves half a library.
    pub fn save(&self) -> Result<(), String> {
        let path = file();
        fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, serde_json::to_string_pretty(self).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        fs::rename(&tmp, &path).map_err(|e| e.to_string())
    }

    /// Takes in a fresh scan: games already known keep their id, when they were added and
    /// played; new ones get an id; known ones the scan didn't find stay, marked gone.
    pub fn update(&mut self, found: Vec<Game>) {
        let now = now();
        let mut old: Vec<Option<Game>> = std::mem::take(&mut self.games).into_iter().map(Some).collect();
        let mut games = Vec::new();
        for mut game in found {
            // When two saved games are now one (a game found on a drive and the same game owned
            // on GOG), the one played, set up or installed keeps its record.
            let worth = |o: &Game| (o.played.is_some(), o.launch_options.is_some() || !o.protection.is_default() || o.chosen_cover.is_some(), o.kind == game.kind);
            let best = old.iter().enumerate().filter_map(|(i, o)| o.as_ref().filter(|o| same(o, &game)).map(|o| (i, worth(o)))).max_by_key(|&(i, w)| (w, std::cmp::Reverse(i))).map(|(i, _)| i);
            match best.and_then(|i| old[i].take()) {
                Some(o) => {
                    game.id = o.id;
                    game.added = o.added;
                    game.played = o.played;
                    game.chosen_cover = o.chosen_cover;
                    game.launch_options = o.launch_options;
                    game.protection = o.protection;
                }
                None => game.added = now,
            }
            games.push(game);
        }
        for mut o in old.into_iter().flatten() {
            // Now part of another game (a download, once what it installed is found).
            if games.iter().any(|g| same(&o, g)) {
                continue;
            }
            o.gone = true;
            games.push(o);
        }
        // New games get ids not already used.
        for i in 0..games.len() {
            if games[i].id.is_empty() {
                let base = gamefix::setup::slug(&games[i].name);
                let mut id = base.clone();
                let mut n = 2;
                while games.iter().any(|g| g.id == id) {
                    id = format!("{base}-{n}");
                    n += 1;
                }
                games[i].id = id;
            }
        }
        games.sort_by_key(|g| g.name.to_lowercase());
        self.games = games;
        self.scanned = now;
    }

    /// A game by its id, else by its name (ignoring case), else the only one whose name holds
    /// the words given.
    pub fn find(&self, query: &str) -> Result<&Game, String> {
        let q = query.to_lowercase();
        if let Some(g) = self.games.iter().find(|g| g.id == query).or_else(|| self.games.iter().find(|g| g.name.to_lowercase() == q)) {
            return Ok(g);
        }
        let matches: Vec<&Game> = self.games.iter().filter(|g| g.name.to_lowercase().contains(&q) || g.id.contains(&q)).collect();
        match matches.as_slice() {
            [one] => Ok(one),
            [] => Err(format!("No game called \"{query}\" (lobby list shows them all).")),
            many => Err(format!("\"{query}\" could be {}: say which (by its id).", many.iter().map(|g| format!("{} ({})", g.name, g.id)).collect::<Vec<_>>().join(", "))),
        }
    }

    pub fn get_mut(&mut self, id: &str) -> Option<&mut Game> {
        self.games.iter_mut().find(|g| g.id == id)
    }
}

/// An old record and a new scan's game are the same game when they share a launcher's id, a
/// program or a folder.
fn same(old: &Game, new: &Game) -> bool {
    let paths = |g: &Game| -> Vec<PathBuf> { g.entries.iter().flat_map(|e| [e.program.clone(), e.folder.clone()]).flatten().collect() };
    let old_paths = paths(old);
    old.entries.iter().any(|a| new.entries.iter().any(|b| a.keys.iter().any(|k| b.keys.contains(k)))) || paths(new).iter().any(|p| old_paths.contains(p))
}

/// Where a game's log goes when lobby starts it.
pub fn log_file(id: &str) -> PathBuf {
    let state = std::env::var_os("XDG_STATE_HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/state"));
    state.join("lobby/logs").join(format!("{id}.log"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::{Entry, Key, Kind, Source};

    fn game(name: &str, key: Key) -> Game {
        let mut e = Entry::new(Source::Steam, name);
        e.keys.push(key);
        Game { id: String::new(), name: name.into(), kind: Kind::Game, entries: vec![e], missing: false, gone: false, added: 0, played: None, chosen_cover: None, launch_options: None, protection: Default::default() }
    }

    #[test]
    fn ids_stay_and_lost_games_are_kept() {
        let mut lib = Library::default();
        lib.update(vec![game("Valheim", Key::SteamApp(892970)), game("Brotato", Key::Heroic("legendary".into(), "b".into()))]);
        let valheim = lib.games.iter().find(|g| g.name == "Valheim").unwrap().id.clone();
        assert_eq!(valheim, "valheim");
        lib.get_mut("valheim").unwrap().played = Some(5);
        lib.get_mut("valheim").unwrap().chosen_cover = Some("/covers/valheim.png".into());
        // Renamed by its store, Brotato's drive unplugged.
        lib.update(vec![game("Valheim (Beta)", Key::SteamApp(892970))]);
        let v = lib.games.iter().find(|g| g.name == "Valheim (Beta)").unwrap();
        assert_eq!((v.id.as_str(), v.played, v.gone), ("valheim", Some(5), false));
        assert_eq!(v.cover(), Some("/covers/valheim.png"));
        assert!(lib.games.iter().any(|g| g.name == "Brotato" && g.gone));
    }

    #[test]
    fn merged_games_keep_the_played_ones_record() {
        // Owned on GOG (listed first) and found on a drive, saved apart; now one game.
        let mut owned = game("Alien Breed 2", Key::Store("gog".into(), "1".into()));
        owned.kind = Kind::Owned;
        owned.id = "alien-breed-2".into();
        let mut found = game("Alien Breed 2", Key::Download("/g/ab2".into()));
        found.id = "alien-breed-2-2".into();
        found.played = Some(7);
        let mut lib = Library { games: vec![owned.clone(), found.clone()], ..Default::default() };
        let mut both = found.clone();
        both.entries.extend(owned.entries);
        both.played = None;
        lib.update(vec![both]);
        assert_eq!(lib.games.len(), 1);
        assert_eq!((lib.games[0].id.as_str(), lib.games[0].played), ("alien-breed-2-2", Some(7)));
    }

    #[test]
    fn ids_are_unique() {
        let mut lib = Library::default();
        lib.update(vec![game("the-mammoth", Key::SteamApp(1)), game("The Mammoth", Key::SteamApp(2))]);
        let mut ids: Vec<_> = lib.games.iter().map(|g| g.id.clone()).collect();
        ids.sort();
        assert_eq!(ids, ["the-mammoth", "the-mammoth-2"]);
    }

    #[test]
    fn finding_by_name() {
        let mut lib = Library::default();
        lib.update(vec![game("Wolfenstein: The New Order", Key::SteamApp(1)), game("Wolfenstein II: The New Colossus", Key::SteamApp(2))]);
        assert_eq!(lib.find("new order").unwrap().id, "wolfenstein-the-new-order");
        assert!(lib.find("wolfenstein").is_err());
        assert!(lib.find("nothing like it").is_err());
    }
}
