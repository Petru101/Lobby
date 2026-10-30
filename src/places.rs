//! Where lobby looks for games besides the usual places: folders the user added (looked into
//! like ~/Games: each folder in them is a game or a folder of games), game folders the user
//! took from the search of all drives (each one game), games the user added by their program
//! (Add a game), and the search's suggestions turned down. Kept in ~/.config/lobby/places.json.

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Serialize, Deserialize, Default, Clone)]
pub struct Places {
    /// Folders the user added: looked into.
    #[serde(default)]
    pub added: Vec<PathBuf>,
    /// Game folders taken from the search: each one game.
    #[serde(default)]
    pub games: Vec<PathBuf>,
    /// Games the user added by picking the program that starts them (or their download).
    #[serde(default)]
    pub programs: Vec<PathBuf>,
    /// The search's suggestions the user turned down: not suggested again.
    #[serde(default)]
    pub dismissed: Vec<PathBuf>,
    /// Steam libraries the search found that Steam doesn't list.
    #[serde(default)]
    pub libraries: Vec<PathBuf>,
}

fn file() -> PathBuf {
    crate::stores::config_home().join("lobby/places.json")
}

impl Places {
    pub fn load() -> Places {
        fs::read_to_string(file()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }

    pub fn save(&self) -> Result<(), String> {
        let file = file();
        fs::create_dir_all(file.parent().unwrap()).map_err(|e| e.to_string())?;
        fs::write(&file, serde_json::to_string_pretty(self).unwrap()).map_err(|e| e.to_string())
    }

    /// Adds a folder to look in (not twice).
    pub fn add(&mut self, folder: &Path) {
        let folder = folder.to_path_buf();
        if !self.added.contains(&folder) {
            self.added.push(folder);
        }
    }

    /// Adds a game by its program (not twice).
    pub fn add_program(&mut self, program: &Path) {
        let program = crate::library::norm(program);
        if !self.programs.contains(&program) {
            self.programs.push(program);
        }
    }
}

/// Whether a folder at the top of a drive looks like downloads or games ("Downloads-old",
/// "Old Games", "Game"), so lobby looks into it like ~/Downloads. Not a game maker's folders
/// ("GameDev", "Unreal Projects") or backups.
pub fn games_or_downloads(name: &str) -> bool {
    let n = name.to_lowercase();
    let words: Vec<&str> = n.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect();
    let about = words.iter().any(|w| w.starts_with("download") || *w == "game" || *w == "games" || *w == "gaming");
    let not = ["dev", "development", "project", "projects", "backup", "backups", "engine", "assets", "source", "sources", "server", "servers", "mods", "saves"];
    about && !words.iter().any(|w| not.contains(w)) && !n.contains("gamedev")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drive_folders() {
        for yes in ["Downloads-old", "Downloads", "Old Games", "Game", "games", "My Downloads 2019"] {
            assert!(games_or_downloads(yes), "{yes}");
        }
        for no in ["GameDev", "Unreal Projects", "Game Backups", "Movies", "Windows Archive", "game-server", "Gameplay Videos"] {
            assert!(!games_or_downloads(no), "{no}");
        }
    }
}
