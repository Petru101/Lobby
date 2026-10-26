//! lobby's own library, ~/.local/share/lobby/library.json: every game it has found, with an id
//! that stays the same from scan to scan. A game found once is remembered: when no source finds
//! it any more (a drive that isn't plugged in, an uninstall), it's kept, marked gone.

use crate::library::{Game, Key};
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
        let mut games = found;
        for game in &mut games {
            for e in &mut game.entries {
                e.keys.retain(|k| !matches!(k, Key::Name(_)));
            }
        }
        // Every new game against every saved record, the best pairs first: a record of the same
        // kind (when a game that had its download in it splits from it, the installed part
        // keeps the record), then the surest match, then, between saved games now one as
        // surely (a game found on a drive and the same game owned on GOG), the one played or
        // set up.
        let worth = |o: &Game| (o.played.is_some(), o.launch_options.is_some() || !o.protection.is_default() || o.chosen_cover.is_some());
        let mut pairs: Vec<(usize, usize, (bool, u8, (bool, bool)))> = Vec::new();
        for (n, game) in games.iter().enumerate() {
            for (i, o) in old.iter().enumerate() {
                let Some(o) = o else { continue };
                let s = same(o, game);
                if s > 0 {
                    pairs.push((n, i, (o.kind == game.kind, s, worth(o))));
                }
            }
        }
        pairs.sort_by(|a, b| b.2.cmp(&a.2).then(a.0.cmp(&b.0)).then(a.1.cmp(&b.1)));
        let mut matched = vec![false; games.len()];
        for (n, i, _) in pairs {
            if matched[n] || old[i].is_none() {
                continue;
            }
            let o = old[i].take().unwrap();
            let game = &mut games[n];
            matched[n] = true;
            game.id = o.id;
            game.added = o.added;
            game.played = o.played;
            game.chosen_cover = o.chosen_cover;
            game.launch_options = o.launch_options;
            game.protection = o.protection;
        }
        for (game, matched) in games.iter_mut().zip(matched) {
            if !matched {
                game.added = now;
            }
        }
        for mut o in old.into_iter().flatten() {
            // Now part of another game (a download, once what it installed is found).
            if games.iter().any(|g| same(&o, g) > 0) {
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

/// How surely an old record and a new scan's game are the same game: 3 a shared launcher's id,
/// 2 the same program, 1 the same folder, 0 not. Merging's `Name` ties don't count: they're
/// numbered by where entries were in one scan, so another scan's same number is another game
/// (saved records still have them from before they were stripped).
fn same(old: &Game, new: &Game) -> u8 {
    let keys = |g: &Game| -> Vec<Key> { g.entries.iter().flat_map(|e| e.keys.iter()).filter(|k| !matches!(k, Key::Name(_))).cloned().collect() };
    let old_keys = keys(old);
    if keys(new).iter().any(|k| old_keys.contains(k)) {
        return 3;
    }
    let paths = |g: &Game, f: fn(&crate::library::Entry) -> Option<PathBuf>| -> Vec<PathBuf> { g.entries.iter().filter_map(f).collect() };
    let programs = paths(old, |e| e.program.clone());
    if paths(new, |e| e.program.clone()).iter().any(|p| programs.contains(p)) {
        return 2;
    }
    let folders = paths(old, |e| e.folder.clone());
    u8::from(paths(new, |e| e.folder.clone()).iter().any(|p| folders.contains(p)))
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
    fn a_played_record_never_goes_to_another_game() {
        // Saved: Borderlands 4 (played, with options) and Apeirophobia, each with merging's
        // numbered tie from the scan they were saved in.
        let with = |name: &str, id: &str, program: &str, tie: &str| {
            let mut g = game(name, Key::Name(tie.into()));
            g.id = id.into();
            g.entries[0].program = Some(program.into());
            g
        };
        let mut bl4 = with("Borderlands 4", "borderlands-4", "/g/bl4.exe", "856");
        bl4.played = Some(9);
        bl4.launch_options = Some("MANGOHUD=1 %command%".into());
        let mut lib = Library { games: vec![with("Apeirophobia", "apeirophobia", "/g/apeiro.exe", "967"), bl4], ..Default::default() };
        // A new scan numbers its ties anew: Apeirophobia's is now Borderlands 4's old number.
        lib.update(vec![with("Apeirophobia", "", "/g/apeiro.exe", "856"), with("Borderlands 4", "", "/g/bl4.exe", "12")]);
        let a = lib.games.iter().find(|g| g.name == "Apeirophobia").unwrap();
        let b = lib.games.iter().find(|g| g.name == "Borderlands 4").unwrap();
        assert_eq!((a.id.as_str(), a.played), ("apeirophobia", None));
        assert_eq!((b.id.as_str(), b.played, b.launch_options.as_deref()), ("borderlands-4", Some(9), Some("MANGOHUD=1 %command%")));
        assert!(lib.games.iter().all(|g| g.entries.iter().all(|e| e.keys.iter().all(|k| !matches!(k, Key::Name(_))))));
    }

    #[test]
    fn a_split_games_installed_part_keeps_its_record() {
        // Creeper World 3's download had been taken for Creeper World 4's (a "3" dropped as a
        // version); now named right, it splits off.
        let mut merged = game("Creeper World 4", Key::Download("/d/cw3.rar".into()));
        merged.id = "creeper-world-4".into();
        merged.played = Some(5);
        merged.entries[0].kind = Kind::Download;
        let mut installed = Entry::new(Source::Folder, "Creeper World 4");
        installed.program = Some("/g/CW4.exe".into());
        merged.entries.push(installed.clone());
        let mut lib = Library { games: vec![merged], ..Default::default() };
        let mut download = game("Creeper World 3", Key::Download("/d/cw3.rar".into()));
        download.kind = Kind::Download;
        let mut cw4 = game("Creeper World 4", Key::SteamApp(1));
        cw4.entries = vec![installed];
        lib.update(vec![download, cw4]);
        let cw4 = lib.games.iter().find(|g| g.name == "Creeper World 4").unwrap();
        assert_eq!((cw4.id.as_str(), cw4.played), ("creeper-world-4", Some(5)));
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
