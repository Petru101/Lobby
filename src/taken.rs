//! Games Lobby took over from Lutris, Faugus and Steam shortcuts: their name, program, prefix,
//! folder, cover and the exact command Lobby starts them with (environment, Proton, arguments),
//! recorded once in ~/.local/share/lobby/taken-over.json. From then on Lobby starts them from
//! its own record and lists them as its own (Source::Lobby), whatever happens to the launcher:
//! it can be removed. Its settings are read only the first time; launch options and protection
//! on the game's page change how Lobby starts it after that.

use crate::library::{Entry, Game, Key, Kind, Source};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Taken {
    /// The launcher's id for the game (it merges the launcher's entry while there is one).
    pub key: Key,
    /// The launcher entry's other ids (a Lutris GOG install's store id ties it to the game
    /// owned on GOG): without them, the game split from it once Lutris was gone.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub keys: Vec<Key>,
    /// Which launcher it came from ("Lutris").
    pub from: String,
    pub name: String,
    pub command: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub program: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prefix: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<PathBuf>,
    /// The launcher's cover, copied into Lobby's own covers (or a web address).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cover: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<PathBuf>,
}

fn data() -> PathBuf {
    std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/share")).join("lobby")
}

fn file() -> PathBuf {
    data().join("taken-over.json")
}

pub fn load() -> Vec<Taken> {
    let mut list: Vec<Taken> = fs::read_to_string(file()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    for t in &mut list {
        t.keys.retain(lasting);
    }
    list
}

/// An id that means the same game in every scan: not merging's own `Name` ties, numbered by
/// where entries were in one scan (kept, they tied X-Moon to another game the next time).
fn lasting(k: &Key) -> bool {
    !matches!(k, Key::Name(_))
}

fn save(list: &[Taken]) -> Result<(), String> {
    let path = file();
    fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, serde_json::to_string_pretty(list).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    fs::rename(&tmp, &path).map_err(|e| e.to_string())
}

/// Lobby's record for the game, when it took it over.
pub fn find(game: &Game) -> Option<Taken> {
    let keys: Vec<&Key> = game.entries.iter().flat_map(|e| e.keys.iter()).collect();
    load().into_iter().find(|t| keys.contains(&&t.key))
}

/// The games Lobby took over, as a source: still on the drive (a game whose files are gone,
/// uninstalled or on a drive not plugged in, is the library's "gone" one).
pub fn entries() -> Vec<Entry> {
    load()
        .into_iter()
        .filter(|t| t.program.as_ref().or(t.prefix.as_ref()).is_some_and(|p| p.exists()))
        .map(|t| {
            let mut e = Entry::new(Source::Lobby, t.name);
            e.keys.push(t.key);
            e.keys.extend(t.keys);
            // Lutris names no prefix when the program is in one (Infested Planet's): the one
            // holding it.
            e.prefix = t.prefix.or_else(|| t.program.as_deref()?.ancestors().find_map(crate::library::prefix_in));
            e.program = t.program;
            e.folder = t.folder;
            e.cover = t.cover;
            e.icon = t.icon;
            e
        })
        .collect()
}

/// Takes over every game that Lobby starts from Lutris's, Faugus's, Heroic's or a Steam
/// shortcut's settings (not one a gamefix entry, a store install or Steam itself starts):
/// records how, and copies its cover and icon. Heroic's Epic and GOG installs become Lobby's
/// store installs (`stores::takeover`). Returns the names taken over.
pub fn take_over(games: &[Game]) -> Vec<String> {
    let mut taken = heroic_installs();
    let mut list = load();
    let mut changed = false;
    for game in games.iter().filter(|g| g.kind == Kind::Game && !g.gone) {
        if let Some(t) = list.iter_mut().find(|t| game.entries.iter().any(|e| e.keys.contains(&t.key))) {
            // Where it is, for records that took it from an entry without it (gamefix's record
            // of a Steam shortcut it made has the shortcut's id but no program).
            if t.program.is_none() {
                if let Some(e) = launcher_entry(game, &t.key, &t.from) {
                    (t.program, t.prefix, t.folder) = (e.program.clone(), e.prefix.clone(), e.folder.clone());
                    changed |= t.program.is_some();
                }
            }
            // Ids the launcher's entry has that the record lacks (records from before `keys`).
            let more: Vec<Key> = game.entries.iter().filter(|e| e.keys.contains(&t.key) && e.source != Source::Lobby).flat_map(|e| e.keys.iter()).filter(|k| lasting(k) && **k != t.key && !t.keys.contains(k)).cloned().collect();
            if !more.is_empty() {
                t.keys.extend(more);
                changed = true;
            }
            continue;
        }
        let Some((key, from, command, cwd)) = crate::launch::launcher_command(game) else { continue };
        if !matches!(crate::launch::plan(game), crate::launch::Plan::Run { command: ref c, .. } if *c == command) {
            continue;
        }
        let Some(e) = launcher_entry(game, &key, from) else { continue };
        let cover = game.entries.iter().find_map(|e| e.cover.clone()).and_then(|c| keep(&c, &game.id, "cover"));
        let icon = e.icon.as_ref().and_then(|i| keep(&i.display().to_string(), &game.id, "icon")).map(PathBuf::from);
        let keys = e.keys.iter().filter(|k| lasting(k) && **k != key).cloned().collect();
        list.push(Taken { key: key.clone(), keys, from: from.to_string(), name: game.name.clone(), command, cwd, program: e.program.clone(), prefix: e.prefix.clone(), folder: e.folder.clone(), cover, icon });
        taken.push(game.name.clone());
        crate::log::line(&format!("taken over from {from}: {}", game.name));
    }
    // Records saved with merging's own ties lose them.
    changed |= fs::read_to_string(file()).is_ok_and(|t| t.contains("\"Name\""));
    if !taken.is_empty() || changed {
        if let Err(e) = save(&list) {
            crate::log::line(&format!("taking games over: {e}"));
            return Vec::new();
        }
    }
    taken
}

/// Heroic's Epic and GOG installs Lobby hasn't taken over yet, taken over.
fn heroic_installs() -> Vec<String> {
    let Some(home) = crate::sources::Home::find() else { return Vec::new() };
    if crate::stores::takeover::heroic_games(&home).is_empty() {
        return Vec::new();
    }
    match crate::stores::takeover::take_over(&home) {
        Ok(done) => {
            for l in &done {
                crate::log::line(&format!("taken over from Heroic: {l}"));
            }
            done
        }
        Err(e) => {
            crate::log::line(&format!("taking Heroic's games over: {e}"));
            Vec::new()
        }
    }
}

/// The launcher's own entry for the game: the one with its id from that launcher (gamefix's
/// record of a Steam shortcut has the shortcut's id too, but not the program), else any with it.
fn launcher_entry<'a>(game: &'a Game, key: &Key, from: &str) -> Option<&'a Entry> {
    let source = match from {
        "Lutris" => Source::Lutris,
        "Faugus" => Source::Faugus,
        "Heroic" => Source::Heroic,
        _ => Source::SteamShortcut,
    };
    let with_key = || game.entries.iter().filter(|e| e.keys.contains(key));
    with_key().find(|e| e.source == source).or_else(|| with_key().find(|e| e.program.is_some())).or_else(|| with_key().next())
}

/// A launcher's cover or icon file copied into Lobby's own folder (the launcher's goes with
/// it); a web address as it is.
fn keep(source: &str, id: &str, what: &str) -> Option<String> {
    if source.starts_with("https://") || source.starts_with("http://") {
        return Some(source.to_string());
    }
    let from = Path::new(source);
    let ext = from.extension().map(|x| x.to_string_lossy().into_owned()).unwrap_or_else(|| "png".into());
    let to = data().join("covers/taken").join(format!("{id}-{what}.{ext}"));
    fs::create_dir_all(to.parent()?).ok()?;
    if from == to || fs::copy(from, &to).is_ok() {
        Some(to.display().to_string())
    } else {
        None
    }
}
