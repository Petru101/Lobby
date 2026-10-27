//! Store apps some games only start with: the EA app, Ubisoft Connect, the Rockstar Games
//! Launcher (Battle.net has its own module: its games start through it). A game bought there
//! checks for its app when it starts; outside Steam (whose copies of such games bring the app
//! into their own Proton setup themselves), Lobby keeps one copy of each app in a Wine prefix of
//! its own (~/.local/share/lobby/clients/<id>), installed with the app's official installer the
//! first time such a game is played, and runs the game in that prefix, where its check finds the
//! app. Nothing of another launcher is used (the standing rule).
//!
//! A game that ran in another prefix before (Lutris's EA app setup) has its saves brought over
//! once, by its Ludusavi save paths (`windows_saves::look`), unless the store keeps them in its
//! cloud (the app brings those down).

use crate::library::{Game, Source};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub struct Client {
    pub id: &'static str,
    pub name: &'static str,
    /// The official installer.
    url: &'static str,
    /// The installed app in the prefix's drive_c.
    program: &'static str,
    /// umu's STORE, for protonfixes.
    store: Option<&'static str>,
    /// What Ludusavi's manifest calls the app's cloud saves: those games' saves come down from
    /// it, so local ones aren't brought over.
    cloud: Option<&'static str>,
    /// Files or folders in a game's folder that say it needs the app (lowercase, relative).
    marks: &'static [&'static str],
}

pub const CLIENTS: &[Client] = &[
    Client {
        id: "ea",
        name: "EA app",
        url: "https://origin-a.akamaihd.net/EA-Desktop-Client-Download/installer-releases/EAappInstaller.exe",
        program: "Program Files/Electronic Arts/EA Desktop/EA Desktop/EADesktop.exe",
        store: Some("ea"),
        cloud: Some("origin"),
        marks: &["__installer/installerdata.xml", "core/activation64.dll", "core/activation.dll"],
    },
    Client {
        id: "ubisoft",
        name: "Ubisoft Connect",
        url: "https://static3.cdn.ubi.com/orbit/launcher_installer/UbisoftConnectInstaller.exe",
        program: "Program Files (x86)/Ubisoft/Ubisoft Game Launcher/UbisoftConnect.exe",
        store: Some("ubisoft"),
        cloud: Some("uplay"),
        marks: &["uplay_install.state", "uplay_r1_loader64.dll", "uplay_r1_loader.dll", "uplay_r2_loader64.dll", "upc_r1_loader64.dll", "upc_r2_loader64.dll"],
    },
    Client {
        id: "rockstar",
        name: "Rockstar Games Launcher",
        url: "https://gamedownloads.rockstargames.com/public/installer/Rockstar-Games-Launcher.exe",
        program: "Program Files/Rockstar Games/Launcher/Launcher.exe",
        store: None,
        cloud: None,
        marks: &["playgtav.exe", "playrdr2.exe", "playgtaiv.exe", "launcherpatcher.exe"],
    },
];

/// A cracked copy's own stand-in for the store's library (a Steam/Ubisoft emulator's settings
/// beside the program): it doesn't need the app, and the app would be the wrong thing to run.
const STAND_INS: &[&str] = &["steam_emu.ini", "codex.ini", "rune.ini", "cpy.ini", "uplay.ini", "uplay_emu.ini", "empress.ini", "3dmgame.ini", "onlinefix.ini", "uplay_r1_loader64.ini", "upc.ini"];

pub fn by_id(id: &str) -> Option<&'static Client> {
    CLIENTS.iter().find(|c| c.id == id)
}

impl Client {
    pub fn prefix(&self) -> PathBuf {
        crate::store::dir().join("clients").join(self.id)
    }

    pub fn installed(&self) -> bool {
        self.prefix().join("drive_c").join(self.program).is_file()
    }

    fn umu(&self, verb: &str) -> Vec<(String, String)> {
        let mut env = vec![
            ("WINEPREFIX".to_string(), self.prefix().display().to_string()),
            ("PROTONPATH".to_string(), crate::battlenet::newest_proton(&self.prefix())),
            ("GAMEID".to_string(), "umu-0".to_string()),
            ("PROTON_VERB".to_string(), verb.to_string()),
        ];
        if let Some(s) = self.store {
            env.push(("STORE".into(), s.into()));
        }
        env
    }

    /// The command starting a game in the app's prefix (its check finds the app there).
    pub fn command(&self, program: &Path) -> Vec<String> {
        let mut words = vec!["env".to_string()];
        words.extend(self.umu("waitforexitandrun").into_iter().map(|(k, v)| format!("{k}={v}")));
        words.extend(["umu-run".into(), program.display().to_string()]);
        words
    }

    /// Downloads the official installer and runs it in Lobby's prefix; the player follows it
    /// and signs in. Returns once it's closed.
    pub fn install(&self) -> Result<(), String> {
        let setup = crate::stores::cache_dir().join(format!("{}-installer.exe", self.id));
        fs::create_dir_all(setup.parent().unwrap()).map_err(|e| e.to_string())?;
        let got = Command::new("curl").args(["-fsSL", "--retry", "2", "-o"]).arg(&setup).arg(self.url).stdin(Stdio::null()).status().map_err(|e| format!("curl: {e}"))?;
        if !got.success() || fs::read(&setup).map_or(true, |b| !b.starts_with(b"MZ")) {
            let _ = fs::remove_file(&setup);
            return Err(format!("The {}'s installer couldn't be downloaded.", self.name));
        }
        fs::create_dir_all(self.prefix()).map_err(|e| e.to_string())?;
        let log = crate::store::log_file(&format!("client-{}", self.id));
        fs::create_dir_all(log.parent().unwrap()).map_err(|e| e.to_string())?;
        let out = fs::File::create(&log).map_err(|e| e.to_string())?;
        let status = Command::new("umu-run")
            .envs(self.umu("waitforexitandrun"))
            .arg(&setup)
            .stdin(Stdio::null())
            .stdout(out.try_clone().map_err(|e| e.to_string())?)
            .stderr(out)
            .status()
            .map_err(|e| format!("umu-run: {e}"))?;
        if self.installed() {
            Ok(())
        } else {
            Err(format!("The {} wasn't installed (its installer {}). What it printed is in {}.", self.name, if status.success() { "closed" } else { "failed" }, log.display()))
        }
    }
}

/// The store app a game needs, and its program: by its folder's files (not a Steam copy, not
/// a cracked one).
pub fn needed(game: &Game) -> Option<(&'static Client, PathBuf)> {
    if game.entries.iter().any(|e| matches!(e.source, Source::Steam | Source::SteamElsewhere)) {
        return None;
    }
    let program = game.program()?.to_path_buf();
    let dir = program.parent()?;
    let lower = |p: &Path| fs::read_dir(p).into_iter().flatten().flatten().map(|e| e.file_name().to_string_lossy().to_lowercase()).collect::<Vec<_>>();
    if lower(dir).iter().any(|n| STAND_INS.contains(&n.as_str()) || n.ends_with(".cdx")) {
        return None;
    }
    // The game's folder, or the program's folder and up to two above it (Binaries/Win64).
    let folders: Vec<PathBuf> = game.folder().map(Path::to_path_buf).into_iter().chain(dir.ancestors().take(3).map(Path::to_path_buf)).collect();
    let has = |mark: &str| folders.iter().any(|f| exists_ignoring_case(f, mark));
    let client = CLIENTS.iter().find(|c| c.marks.iter().any(|m| has(m)))?;
    Some((client, program))
}

/// A path under `base` whose parts match `rel`'s whatever their case (Windows games name their
/// files as they like: "__Installer", "Core").
fn exists_ignoring_case(base: &Path, rel: &str) -> bool {
    let mut at = base.to_path_buf();
    for part in rel.split('/') {
        let Some(name) = fs::read_dir(&at).into_iter().flatten().flatten().map(|e| e.file_name()).find(|n| n.to_string_lossy().to_lowercase() == part) else { return false };
        at.push(name);
    }
    true
}

/// The prefix the game ran in before (another launcher's setup of the app), when not the app's.
fn old_prefix(game: &Game, client: &Client) -> Option<PathBuf> {
    let ours = client.prefix();
    crate::taken::find(game)
        .and_then(|t| t.prefix)
        .or_else(|| game.entries.iter().find_map(|e| e.prefix.clone()))
        .filter(|p| *p != ours && p.join("drive_c").is_dir())
}

/// Brings the game's saves from the prefix it ran in before into the app's, once (only files
/// missing there or older; replaced ones are kept in save-backups). How many were copied.
pub fn bring_saves(game: &Game, client: &Client) -> Result<usize, String> {
    let marker = client.prefix().join(format!(".lobby-saves-from-old-{}", game.id));
    if marker.exists() {
        return Ok(0);
    }
    let Some(old) = old_prefix(game, client) else { return Ok(0) };
    let index = crate::windows_saves::index();
    let Some(paths) = crate::windows_saves::paths_for(game, &index) else { return Ok(0) };
    // The store keeps them in its cloud (Mass Effect Andromeda's on EA's): the app brings them
    // down, and old local copies could be stale (Ferret's tests changed Andromeda's).
    if client.cloud.is_some_and(|c| paths.cloud.iter().any(|p| p == c)) {
        return Ok(0);
    }
    let found = crate::windows_saves::look(paths, &crate::windows_saves::prefix_user(&old), &crate::windows_saves::prefix_user(&client.prefix()));
    let (copied, _) = crate::windows_saves::bring_over(&game.id, &found)?;
    let _ = fs::write(&marker, old.display().to_string());
    crate::log::line(&format!("{}: {copied} save files from {} into the {}'s prefix", game.name, old.display(), client.name));
    Ok(copied)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::{Entry, Kind};

    fn game(dir: &Path, source: Source) -> Game {
        let mut e = Entry::new(source, "G");
        e.folder = Some(dir.to_path_buf());
        e.program = Some(dir.join("Game.exe"));
        Game { id: "g".into(), name: "G".into(), kind: Kind::Game, entries: vec![e], missing: false, gone: false, added: 0, played: None, chosen_cover: None, launch_options: None, protection: Default::default(), hidden: false, favourite: false, frame_rate: false }
    }

    #[test]
    fn which_app_a_game_needs() {
        let dir = std::env::temp_dir().join(format!("lobby-clients-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("__Installer")).unwrap();
        fs::write(dir.join("Game.exe"), b"MZ").unwrap();
        fs::write(dir.join("__Installer/installerdata.xml"), b"<x/>").unwrap();
        assert_eq!(needed(&game(&dir, Source::Lutris)).map(|(c, _)| c.id), Some("ea"));
        // Steam's copy brings the app itself.
        assert!(needed(&game(&dir, Source::Steam)).is_none());
        fs::remove_dir_all(dir.join("__Installer")).unwrap();
        fs::write(dir.join("uplay_r1_loader64.dll"), b"MZ").unwrap();
        assert_eq!(needed(&game(&dir, Source::Folder)).map(|(c, _)| c.id), Some("ubisoft"));
        // A cracked copy's emulator: not the app.
        fs::write(dir.join("uplay_r1_loader64.ini"), b"").unwrap();
        assert!(needed(&game(&dir, Source::Folder)).is_none());
        let _ = fs::remove_dir_all(&dir);
    }
}
