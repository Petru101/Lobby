//! Cloud saves for games lobby installed from Epic and GOG: before a game starts, saves newer
//! in the cloud come down; after it quits, saves newer here go up. legendary and gogdl move the
//! files; lobby works out where the game keeps them inside its Wine prefix and remembers when
//! each GOG location was last synced (gogdl decides by that time).

use super::{epic, gog, output, read_json, tools, Store};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Direction {
    /// Before playing: only saves newer in the cloud come down.
    Down,
    /// After playing: only saves newer here go up.
    Up,
    /// The user chose the cloud's saves over this computer's.
    KeepCloud,
    /// The user chose this computer's saves over the cloud's.
    KeepLocal,
}

#[derive(Clone, PartialEq, Debug)]
pub enum Synced {
    /// Nothing needed moving.
    Nothing,
    /// What moved, in a sentence.
    Moved(String),
    /// Saves changed here and in the cloud since the last sync: the user picks (GOG).
    Conflict,
}

/// The Wine prefix (the folder holding drive_c) a game runs in, from the environment its
/// gamefix entry gives umu-run.
pub fn prefix_of(env: &[String]) -> Option<PathBuf> {
    let var = |name: &str| env.iter().find_map(|w| w.strip_prefix(name)?.strip_prefix('=').map(PathBuf::from));
    var("WINEPREFIX").or_else(|| var("STEAM_COMPAT_DATA_PATH").map(|p| p.join("pfx")))
}

/// Whether the store keeps the game's saves in the cloud.
pub fn supported(store: Store, id: &str) -> bool {
    match store {
        Store::Epic => epic::cloud_save_folder(id).is_some(),
        Store::Gog => !gog_locations(id).is_empty(),
    }
}

/// Moves saves between the game's Wine prefix and the store's cloud.
pub fn sync(store: Store, id: &str, env: &[String], direction: Direction) -> Result<Synced, String> {
    let prefix = prefix_of(env).ok_or("the game's Wine prefix isn't known")?;
    match store {
        Store::Epic => sync_epic(id, env, direction),
        Store::Gog => sync_gog(id, &prefix, direction),
    }
}

/// legendary works out the save folder from the game's CloudSaveFolder and the prefix's
/// registry (WINEPREFIX in its environment), and keeps it: newest wins, by the files' times.
fn sync_epic(app: &str, env: &[String], direction: Direction) -> Result<Synced, String> {
    let mut command = Command::new(tools::LEGENDARY.path()?);
    command.env("LEGENDARY_CONFIG_PATH", epic::config());
    for (k, v) in env.iter().filter_map(|w| w.split_once('=')) {
        command.env(k, v);
    }
    command.args(["-y", "sync-saves", app, "--accept-path"]);
    command.arg(match direction {
        Direction::Down => "--skip-upload",
        Direction::Up => "--skip-download",
        Direction::KeepCloud => "--force-download",
        Direction::KeepLocal => "--force-upload",
    });
    // What it did is in its log, on stderr.
    let out = command.stdin(std::process::Stdio::null()).env("NO_COLOR", "1").output().map_err(|e| e.to_string())?;
    let log = String::from_utf8_lossy(&out.stderr);
    if !out.status.success() {
        let last = log.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("it stopped with an error");
        return Err(format!("Epic's cloud saves: {}", last.split("ERROR:").last().unwrap_or(last).trim()));
    }
    Ok(if log.contains("Downloading remote savegame") {
        Synced::Moved("Brought its saves down from Epic's cloud.".into())
    } else if log.contains("Uploading local savegame") {
        Synced::Moved("Put its saves in Epic's cloud.".into())
    } else {
        Synced::Nothing
    })
}

/// When each GOG save location was last synced (gogdl prints it after a sync): game id to
/// location name to time.
fn sync_times_file() -> PathBuf {
    crate::store::dir().join("gog-save-sync.json")
}

/// GOG's save locations for a game: Galaxy's settings for the game's client (named in gogdl's
/// manifest or the game's .info), fetched once into ~/.cache/lobby/gog-saves.json.
/// A location is a name and a path with Galaxy's folder variables (<?SAVED_GAMES?> ...).
fn gog_locations(id: &str) -> Vec<(String, String)> {
    let cache_file = super::cache_dir().join("gog-saves.json");
    let mut cache: BTreeMap<String, Vec<(String, String)>> = read_json(&cache_file);
    if let Some(found) = cache.get(id) {
        return found.clone();
    }
    let Some(client) = gog::client_id(id) else { return Vec::new() };
    let Ok(config) = super::get_json(&format!("https://remote-config.gog.com/components/galaxy_client/clients/{client}?component_version=2.0.45"), None) else { return Vec::new() };
    let storage = config.get("content").and_then(|c| c.get("Windows")).and_then(|w| w.get("cloudStorage"));
    let found = locations(storage, &client);
    cache.insert(id.to_string(), found.clone());
    let _ = fs::write(&cache_file, serde_json::to_string(&cache).unwrap());
    found
}

/// The locations in Galaxy's cloudStorage settings: none when it's off; Galaxy's own default
/// folder when it's on with none named.
fn locations(storage: Option<&serde_json::Value>, client: &str) -> Vec<(String, String)> {
    let Some(storage) = storage.filter(|s| s.get("enabled").and_then(|e| e.as_bool()) == Some(true)) else { return Vec::new() };
    let named: Vec<(String, String)> = storage.get("locations").and_then(|l| l.as_array()).into_iter().flatten().filter_map(|l| Some((l.get("name")?.as_str()?.to_string(), l.get("location")?.as_str()?.to_string()))).collect();
    if named.is_empty() {
        return vec![("__default".into(), format!("<?APPLICATION_DATA_LOCAL?>/GOG.com/Galaxy/Applications/{client}/Storage/Shared/Files"))];
    }
    named
}

/// Where GOG's cloud keeps a game's saves in its prefix (Galaxy's locations, resolved).
pub fn gog_folders(id: &str, prefix: &Path, game_folder: &Path) -> Vec<PathBuf> {
    gog_locations(id).iter().map(|(_, l)| resolve(l, prefix, game_folder)).collect()
}

/// A Galaxy location as a folder in the game's Proton prefix (its user is steamuser).
fn resolve(location: &str, prefix: &Path, game_folder: &Path) -> PathBuf {
    let user = prefix.join("drive_c/users/steamuser");
    let path = location.replace('\\', "/");
    let vars: [(&str, PathBuf); 6] = [
        ("<?INSTALL?>", game_folder.to_path_buf()),
        ("<?DOCUMENTS?>", user.join("Documents")),
        ("<?SAVED_GAMES?>", user.join("Saved Games")),
        ("<?APPLICATION_DATA_ROAMING?>", user.join("AppData/Roaming")),
        ("<?APPLICATION_DATA_LOCAL_LOW?>", user.join("AppData/LocalLow")),
        ("<?APPLICATION_DATA_LOCAL?>", user.join("AppData/Local")),
    ];
    for (var, dir) in vars {
        if let Some(rest) = path.strip_prefix(var) {
            return dir.join(rest.trim_start_matches('/'));
        }
    }
    PathBuf::from(path)
}

#[derive(Serialize, Deserialize, Default)]
struct SyncTimes(BTreeMap<String, BTreeMap<String, String>>);

fn sync_gog(id: &str, prefix: &Path, direction: Direction) -> Result<Synced, String> {
    let folder = gog::installed().into_iter().find(|g| g.id == id).map(|g| g.folder).ok_or("Lobby didn't install this game.")?;
    let mut times: SyncTimes = read_json(&sync_times_file());
    let mut moved = false;
    let mut conflict = false;
    for (name, location) in gog_locations(id) {
        let path = resolve(&location, prefix, &folder);
        let ts = times.0.get(id).and_then(|t| t.get(&name)).cloned().unwrap_or_else(|| "0".into());
        let mut command = gog::gogdl()?;
        command.arg("save-sync").arg(&path).args([id, "--os", "windows", "--ts", &ts, "--name", &name]);
        command.arg(match direction {
            Direction::Down => "--skip-upload",
            Direction::Up => "--skip-download",
            Direction::KeepCloud => "--force-download",
            Direction::KeepLocal => "--force-upload",
        });
        let out = command.stdin(std::process::Stdio::null()).output().map_err(|e| e.to_string())?;
        let log = String::from_utf8_lossy(&out.stderr);
        if !out.status.success() {
            let last = log.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("it stopped with an error");
            return Err(format!("GOG's cloud saves: {}", last.split("ERROR:").last().unwrap_or(last).trim()));
        }
        if log.contains("Files in conflict") {
            conflict = true;
            continue;
        }
        moved |= ["Uploading files", "Downloading files", "No files in cloud, uploading", "No files locally, downloading", "Forcing"].iter().any(|m| log.contains(m));
        // The time it synced at, which the next sync compares the files' times with.
        let printed = String::from_utf8_lossy(&out.stdout);
        if let Some(t) = printed.split_whitespace().last().filter(|t| t.parse::<f64>().is_ok()) {
            times.0.entry(id.to_string()).or_default().insert(name, t.to_string());
        }
    }
    fs::create_dir_all(crate::store::dir()).map_err(|e| e.to_string())?;
    fs::write(sync_times_file(), serde_json::to_string_pretty(&times).unwrap()).map_err(|e| e.to_string())?;
    Ok(if conflict {
        Synced::Conflict
    } else if moved {
        Synced::Moved(if matches!(direction, Direction::Up | Direction::KeepLocal) { "Put its saves in GOG's cloud.".into() } else { "Brought its saves down from GOG's cloud.".into() })
    } else {
        Synced::Nothing
    })
}

/// A prefix umu hasn't made yet has no registry, which legendary needs to find Windows'
/// folders: made first (`umu-run wineboot -u` with the game's own environment).
pub fn make_prefix(env: &[String], umu: &str) -> Result<(), String> {
    let Some(prefix) = prefix_of(env) else { return Ok(()) };
    if prefix.join("user.reg").is_file() {
        return Ok(());
    }
    fs::create_dir_all(&prefix).map_err(|e| e.to_string())?;
    let mut command = Command::new(umu);
    for (k, v) in env.iter().filter_map(|w| w.split_once('=')) {
        command.env(k, v);
    }
    command.args(["wineboot", "-u"]);
    output(command).map(|_| ()).map_err(|e| format!("Couldn't make the game's Wine prefix: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn galaxy_locations() {
        let off = serde_json::json!({"enabled": false, "locations": [{"name": "cloud_saves", "location": "<?INSTALL?>\\\\cloud_saves"}]});
        assert!(locations(Some(&off), "1").is_empty());
        let on = serde_json::json!({"enabled": true, "locations": [{"name": "saves", "location": "<?APPLICATION_DATA_LOCAL_LOW?>/ClockStone Software GmbH/Bridge Constructor Portal"}]});
        assert_eq!(locations(Some(&on), "1"), [("saves".to_string(), "<?APPLICATION_DATA_LOCAL_LOW?>/ClockStone Software GmbH/Bridge Constructor Portal".to_string())]);
        let default = serde_json::json!({"enabled": true, "locations": []});
        assert_eq!(locations(Some(&default), "55").first().map(|l| l.0.as_str()), Some("__default"));
    }

    #[test]
    fn folders_in_the_prefix() {
        let (pfx, game) = (Path::new("/p"), Path::new("/g/Bio Menace"));
        assert_eq!(resolve("<?APPLICATION_DATA_LOCAL_LOW?>/ClockStone/Portal", pfx, game), PathBuf::from("/p/drive_c/users/steamuser/AppData/LocalLow/ClockStone/Portal"));
        assert_eq!(resolve("<?APPLICATION_DATA_LOCAL?>/x", pfx, game), PathBuf::from("/p/drive_c/users/steamuser/AppData/Local/x"));
        assert_eq!(resolve("<?INSTALL?>\\cloud_saves", pfx, game), PathBuf::from("/g/Bio Menace/cloud_saves"));
        assert_eq!(resolve("<?DOCUMENTS?>/Grey Alien Games/Ancient Enemy", pfx, game), PathBuf::from("/p/drive_c/users/steamuser/Documents/Grey Alien Games/Ancient Enemy"));
    }

    #[test]
    fn prefixes() {
        assert_eq!(prefix_of(&["WINEPREFIX=/p/x".into(), "GAMEID=umu-0".into()]), Some(PathBuf::from("/p/x")));
        assert_eq!(prefix_of(&["STEAM_COMPAT_DATA_PATH=/c/1".into()]), Some(PathBuf::from("/c/1/pfx")));
    }
}
