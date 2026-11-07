//! gamefix's records (`~/.local/share/gamefix/games/<name>.log`): games it installed, and the
//! Steam shortcuts and app-menu entries it made, which tie its fixes to those entries. A game
//! installed from a download carries the download's path, so the two are one game.

use super::Home;
use crate::library::{Entry, Key, Source};
use std::fs;
use std::path::{Path, PathBuf};

pub fn records(home: &Home) -> Vec<Entry> {
    let mut entries = Vec::new();
    for file in fs::read_dir(home.join(".local/share/gamefix/games")).into_iter().flatten().flatten() {
        let path = file.path();
        if path.extension().is_none_or(|e| e != "log") {
            continue;
        }
        let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let mut e = Entry::new(Source::Gamefix, stem);
        e.annotation = true;
        for line in fs::read_to_string(&path).unwrap_or_default().lines() {
            let f: Vec<&str> = line.split('\t').collect();
            match f.as_slice() {
                ["installed-game", prefix, program, name, ..] => {
                    // The installer or disc it was installed from (gamefix's note in the
                    // prefix): the download it belongs to.
                    let note = fs::read_to_string(PathBuf::from(prefix).join("gamefix-install.txt")).unwrap_or_default();
                    if let Some(source) = note.lines().find_map(|l| l.strip_prefix("source\t")) {
                        e.keys.push(Key::Download(PathBuf::from(source)));
                    }
                    // Installed on another drive: the whole of the game, not the program's
                    // folder (RoadCraft runs in root/bin/pc, its data is in root/paks).
                    if let Some(Ok(top)) = crate::uninstall::installed_elsewhere(Path::new(prefix)) {
                        e.folder = Some(top);
                    }
                    e.prefix = Some(PathBuf::from(prefix));
                    e.program = Some(PathBuf::from(program));
                    e.name = name.to_string();
                    e.annotation = false;
                }
                ["steam-shortcut", id, ..] | ["steam-compat", id, ..] => {
                    if let Ok(id) = id.parse::<u32>() {
                        // Non-Steam games' ids have the top bit set.
                        e.keys.push(if id & 0x8000_0000 != 0 { Key::SteamShortcut(id) } else { Key::SteamApp(id) });
                    }
                }
                ["created", file, ..] if file.ends_with(".desktop") => e.keys.push(Key::Desktop(PathBuf::from(file))),
                _ => {}
            }
        }
        entries.push(e);
    }
    entries
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_installed_game_is_its_whole_folder() {
        let dir = std::env::temp_dir().join(format!("lobby-whole-folder-{}", std::process::id()));
        let (prefix, top, records_dir) = (dir.join("d/RoadCraft 2"), dir.join("d/RoadCraft"), dir.join("home/.local/share/gamefix/games"));
        for d in [&prefix, &records_dir, &top.join("root/bin/pc"), &top.join("root/paks")] {
            fs::create_dir_all(d).unwrap();
        }
        let program = top.join("root/bin/pc/game.exe");
        fs::write(&program, "").unwrap();
        fs::write(top.join("unins000.exe"), "").unwrap();
        fs::write(prefix.join("gamefix-install.txt"), format!("program\t{}\n", program.display())).unwrap();
        fs::write(records_dir.join("roadcraft.log"), format!("installed-game\t{}\t{}\tRoadCraft\n", prefix.display(), program.display())).unwrap();
        let found = records(&Home { home: dir.join("home"), user: "p".into() });
        assert_eq!(found[0].folder.as_deref(), Some(top.as_path()));
        // A program in its prefix keeps Lobby's own guess at its folder.
        fs::write(prefix.join("gamefix-install.txt"), "program\tdrive_c/Games/X/x.exe\n").unwrap();
        assert_eq!(records(&Home { home: dir.join("home"), user: "p".into() })[0].folder, None);
        fs::remove_dir_all(&dir).unwrap();
    }
}
