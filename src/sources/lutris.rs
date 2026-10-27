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

/// The store a Lutris service is, as `Key::Store` names it.
fn store_of(service: &str) -> Option<&'static str> {
    match service {
        "gog" => Some("gog"),
        "egs" => Some("epic"),
        "amazon" => Some("amazon"),
        _ => None,
    }
}

/// How Lutris starts a game, read from its database and config so Lobby can start it the same
/// way itself: the runner ("wine", "linux" ...), program, arguments, prefix, working folder,
/// environment (system: env:, and the wine runner's DLL overrides as WINEDLLOVERRIDES) and the
/// wine build it picked (wine: version:), if any.
#[derive(Debug, Default, PartialEq)]
pub struct Launch {
    pub runner: String,
    pub program: Option<PathBuf>,
    pub args: String,
    pub prefix: Option<PathBuf>,
    pub cwd: Option<PathBuf>,
    pub env: Vec<(String, String)>,
    pub wine: Option<PathBuf>,
}

pub fn launch(id: i64) -> Option<Launch> {
    let home = Home::find()?;
    home.app_dirs("net.lutris.Lutris", "data/lutris", ".local/share/lutris").into_iter().find_map(|dir| {
        let db = rusqlite::Connection::open_with_flags(dir.join("pga.db"), rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).ok()?;
        let (runner, directory, configpath): (Option<String>, Option<String>, Option<String>) = db.query_row("SELECT runner, directory, configpath FROM games WHERE id = ?1", [id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).ok()?;
        let config = fs::read_to_string(dir.join("games").join(format!("{}.yml", configpath?))).ok()?;
        let mut l = read_launch(&config, directory.filter(|d| !d.is_empty()).map(PathBuf::from));
        l.runner = runner.unwrap_or_default();
        // A wine build of Lutris's own (runners/wine/<version>).
        l.wine = l.wine.take().map(|v| dir.join("runners/wine").join(v)).filter(|d| d.is_dir());
        Some(l)
    })
}

fn read_launch(config: &str, directory: Option<PathBuf>) -> Launch {
    let doc: serde_yaml::Value = serde_yaml::from_str(config).unwrap_or_default();
    let text = |v: &serde_yaml::Value| match v {
        serde_yaml::Value::String(s) => Some(s.clone()),
        serde_yaml::Value::Number(n) => Some(n.to_string()),
        serde_yaml::Value::Bool(b) => Some(b.to_string()),
        _ => None,
    };
    let game = &doc["game"];
    let path = |k: &str| text(&game[k]).filter(|s| !s.is_empty()).map(PathBuf::from);
    let prefix = path("prefix");
    // A relative program is in the prefix (wine) or the game's folder.
    let program = path("exe").map(|exe| match (prefix.as_ref().or(directory.as_ref()), exe.is_absolute()) {
        (Some(base), false) => base.join(exe),
        _ => exe,
    });
    let mut env: Vec<(String, String)> = doc["system"]["env"].as_mapping().into_iter().flatten().filter_map(|(k, v)| Some((text(k)?, text(v).unwrap_or_default()))).collect();
    let overrides: Vec<String> = doc["wine"]["overrides"].as_mapping().into_iter().flatten().filter_map(|(k, v)| Some(format!("{}={}", text(k)?, text(v)?))).collect();
    if !overrides.is_empty() {
        match env.iter_mut().find(|(k, _)| k == "WINEDLLOVERRIDES") {
            Some((_, v)) => *v = format!("{v};{}", overrides.join(";")),
            None => env.push(("WINEDLLOVERRIDES".into(), overrides.join(";"))),
        }
    }
    Launch {
        runner: String::new(),
        cwd: path("working_dir").or_else(|| program.as_ref().and_then(|p| p.parent().map(Path::to_path_buf))),
        program,
        args: text(&game["args"]).unwrap_or_default(),
        prefix,
        env,
        wine: text(&doc["wine"]["version"]).map(PathBuf::from),
    }
}

pub fn games(home: &Home) -> Vec<Entry> {
    let mut entries = Vec::new();
    for dir in home.app_dirs("net.lutris.Lutris", "data/lutris", ".local/share/lutris") {
        let Ok(db) = rusqlite::Connection::open_with_flags(dir.join("pga.db"), rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY) else { continue };
        let Ok(mut q) = db.prepare("SELECT id, name, directory, configpath, slug, service, service_id, lastplayed, playtime, installed_at FROM games WHERE installed = 1") else { continue };
        let rows = q.query_map([], |r| {
            // Lutris's play time is in hours.
            let played = (r.get::<_, Option<i64>>(7)?, r.get::<_, Option<f64>>(8)?, r.get::<_, Option<i64>>(9)?);
            Ok(((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, Option<String>>(2)?, r.get::<_, Option<String>>(3)?, r.get::<_, Option<String>>(4)?, r.get::<_, Option<String>>(5)?, r.get::<_, Option<String>>(6)?), played))
        });
        for ((id, name, directory, configpath, slug, service, service_id), (last, hours, installed)) in rows.into_iter().flatten().flatten() {
            let mut e = Entry::new(Source::Lutris, name);
            e.keys.push(Key::Lutris(id));
            e.last_played = last.filter(|&t| t > 0).map(|t| t as u64);
            e.playtime = hours.filter(|&h| h > 0.0).map(|h| (h * 3600.0) as u64);
            e.since = installed.filter(|&t| t > 0).map(|t| t as u64);
            // Installed from a store through Lutris (its GOG or Epic integration): the store's
            // id ties it to the game the account owns there.
            if let (Some(store), Some(sid)) = (service.as_deref().and_then(store_of), service_id.filter(|s| !s.is_empty())) {
                e.keys.push(Key::Store(store.into(), sid));
            }
            e.cover = slug.and_then(|slug| ["coverart", "banners"].iter().map(|d| dir.join(d).join(format!("{slug}.jpg"))).find(|p| p.is_file())).map(|p| p.display().to_string());
            let directory = directory.filter(|d| !d.is_empty()).map(PathBuf::from);
            let config = configpath.and_then(|c| fs::read_to_string(dir.join("games").join(format!("{c}.yml"))).ok()).unwrap_or_default();
            let block = game_block(&config);
            let get = |k: &str| block.iter().find(|(key, _)| key == k).map(|(_, v)| PathBuf::from(v));
            e.prefix = get("prefix");
            // A relative program is in the game's folder (Lutris installs into the prefix).
            // Runners other than Wine name the game's file otherwise (DOSBox's main_file, an
            // emulator's rom or iso) and ScummVM its folder (path).
            e.program = ["exe", "main_file", "rom", "iso"].into_iter().find_map(get).map(|exe| match (&directory, exe.is_absolute()) {
                (Some(d), false) => d.join(exe),
                _ => exe,
            });
            e.folder = get("working_dir").or(get("path")).or(directory).filter(|d| d != Path::new("/"));
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
    fn how_lutris_starts_it() {
        let yml = "game:\n  exe: drive_c/GOG Games/EE/Empire Earth.exe\n  args: -windowed\n  prefix: /g/ee\nsystem:\n  env:\n    WINEDLLOVERRIDES: ddraw=n,b\n    LC_ALL: ''\nwine:\n  overrides:\n    dsound: native\n  version: wine-ge-8-26-x86_64\n";
        let l = read_launch(yml, None);
        assert_eq!(l.program, Some(PathBuf::from("/g/ee/drive_c/GOG Games/EE/Empire Earth.exe")));
        assert_eq!(l.cwd, Some(PathBuf::from("/g/ee/drive_c/GOG Games/EE")));
        assert_eq!(l.args, "-windowed");
        assert_eq!(l.env, [("WINEDLLOVERRIDES".into(), "ddraw=n,b;dsound=native".into()), ("LC_ALL".into(), String::new())]);
        assert_eq!(l.wine, Some(PathBuf::from("wine-ge-8-26-x86_64")));
    }

    #[test]
    fn long_values_wrap() {
        let yml = "game:\n  exe: /home/u/Games/gog/ee/drive_c/GOG Games/Empire Earth\n    Gold Edition/Empire Earth/Empire Earth.exe\n  prefix: /home/u/Games/gog/ee\n";
        assert_eq!(game_block(yml)[0].1, "/home/u/Games/gog/ee/drive_c/GOG Games/Empire Earth Gold Edition/Empire Earth/Empire Earth.exe");
        assert_eq!(game_block(yml)[1].1, "/home/u/Games/gog/ee");
    }
}
