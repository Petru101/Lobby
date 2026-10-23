//! Lutris: installed games from its database (pga.db), each one's program and prefix from its
//! game config (`games/<configpath>.yml`).

use super::Home;
use crate::library::{Entry, Key, Source};
use std::fs;
use std::path::{Path, PathBuf};

/// The `exe`, `prefix` and `working_dir` of a config's top-level `game:` block.
fn game_block(yml: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut inside = false;
    for line in yml.lines() {
        if !line.starts_with(' ') {
            inside = line.trim_end() == "game:";
            continue;
        }
        if !inside {
            continue;
        }
        // Only the block's own keys, two spaces in.
        let Some(rest) = line.strip_prefix("  ").filter(|r| !r.starts_with(' ')) else { continue };
        if let Some((k, v)) = rest.split_once(':') {
            let v = v.trim().trim_matches(|c| c == '\'' || c == '"');
            if !v.is_empty() {
                out.push((k.trim().to_string(), v.to_string()));
            }
        }
    }
    out
}

pub fn games(home: &Home) -> Vec<Entry> {
    let mut entries = Vec::new();
    for dir in home.app_dirs("net.lutris.Lutris", "data/lutris", ".local/share/lutris") {
        let Ok(db) = rusqlite::Connection::open_with_flags(dir.join("pga.db"), rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY) else { continue };
        let Ok(mut q) = db.prepare("SELECT id, name, directory, configpath FROM games WHERE installed = 1") else { continue };
        let rows = q.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, Option<String>>(2)?, r.get::<_, Option<String>>(3)?)));
        for (id, name, directory, configpath) in rows.into_iter().flatten().flatten() {
            let mut e = Entry::new(Source::Lutris, name);
            e.keys.push(Key::Lutris(id));
            let directory = directory.filter(|d| !d.is_empty()).map(PathBuf::from);
            let config = configpath.and_then(|c| fs::read_to_string(dir.join("games").join(format!("{c}.yml"))).ok()).unwrap_or_default();
            let block = game_block(&config);
            let get = |k: &str| block.iter().find(|(key, _)| key == k).map(|(_, v)| PathBuf::from(v));
            e.prefix = get("prefix");
            // A relative program is in the game's folder (Lutris installs into the prefix).
            e.program = get("exe").map(|exe| match (&directory, exe.is_absolute()) {
                (Some(d), false) => d.join(exe),
                _ => exe,
            });
            e.folder = get("working_dir").or(directory).filter(|d| d != Path::new("/"));
            entries.push(e);
        }
    }
    entries
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_game_block() {
        let yml = "game:\n  exe: drive_c/Games/Forager/Forager.exe\n  prefix: /home/u/Games/gog/forager\nscript:\n  game:\n    exe: other.exe\n";
        assert_eq!(game_block(yml), [("exe".into(), "drive_c/Games/Forager/Forager.exe".into()), ("prefix".into(), "/home/u/Games/gog/forager".into())]);
    }
}
