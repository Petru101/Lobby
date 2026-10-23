//! Lutris: installed games from its database (pga.db), each one's program and prefix from its
//! game config (`games/<configpath>.yml`).

use super::Home;
use crate::library::{Entry, Key, Source};
use std::fs;
use std::path::{Path, PathBuf};

/// The `exe`, `prefix` and `working_dir` of a config's top-level `game:` block. Lutris wraps
/// long values onto lines indented further, which continue the value after a space.
fn game_block(yml: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    let mut inside = false;
    let mut continues = false;
    for line in yml.lines() {
        if !line.starts_with(' ') {
            inside = line.trim_end() == "game:";
            continues = false;
            continue;
        }
        if !inside {
            continue;
        }
        // Only the block's own keys, two spaces in.
        match line.strip_prefix("  ") {
            Some(rest) if !rest.starts_with(' ') => {
                continues = false;
                if let Some((k, v)) = rest.split_once(':') {
                    let v = v.trim();
                    if !v.is_empty() {
                        out.push((k.trim().to_string(), v.to_string()));
                        continues = true;
                    }
                }
            }
            Some(_) if continues => {
                let last = out.last_mut().unwrap();
                last.1 = format!("{} {}", last.1, line.trim());
            }
            _ => continues = false,
        }
    }
    for (_, v) in &mut out {
        *v = v.trim_matches(|c| c == '\'' || c == '"').to_string();
    }
    out
}

pub fn games(home: &Home) -> Vec<Entry> {
    let mut entries = Vec::new();
    for dir in home.app_dirs("net.lutris.Lutris", "data/lutris", ".local/share/lutris") {
        let Ok(db) = rusqlite::Connection::open_with_flags(dir.join("pga.db"), rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY) else { continue };
        let Ok(mut q) = db.prepare("SELECT id, name, directory, configpath, slug FROM games WHERE installed = 1") else { continue };
        let rows = q.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, Option<String>>(2)?, r.get::<_, Option<String>>(3)?, r.get::<_, Option<String>>(4)?)));
        for (id, name, directory, configpath, slug) in rows.into_iter().flatten().flatten() {
            let mut e = Entry::new(Source::Lutris, name);
            e.keys.push(Key::Lutris(id));
            e.cover = slug.and_then(|slug| ["coverart", "banners"].iter().map(|d| dir.join(d).join(format!("{slug}.jpg"))).find(|p| p.is_file())).map(|p| p.display().to_string());
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

    #[test]
    fn long_values_wrap() {
        let yml = "game:\n  exe: /home/u/Games/gog/ee/drive_c/GOG Games/Empire Earth\n    Gold Edition/Empire Earth/Empire Earth.exe\n  prefix: /home/u/Games/gog/ee\n";
        assert_eq!(game_block(yml)[0].1, "/home/u/Games/gog/ee/drive_c/GOG Games/Empire Earth Gold Edition/Empire Earth/Empire Earth.exe");
        assert_eq!(game_block(yml)[1].1, "/home/u/Games/gog/ee");
    }
}
