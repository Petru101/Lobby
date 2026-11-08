//! Starts that failed: the game ended before anything of it showed (no GPU driver or display
//! connection loaded: `running::shows_window`), within a couple of minutes. Seen by `lobby
//! sandbox`, which every protected start goes through (Play and the shortcuts), and kept in
//! ~/.local/share/lobby/starts.json by game id until a start works. The likely reason is read
//! from the game's log (~/.local/state/lobby/logs/<id>.log) and its command.
//!
//! Steam starts its games itself, so those are read from Steam's own log of them
//! (logs/console_log.txt, `Game process added` and `removed`), whoever started them, once they
//! end (`judge_steam`). While the window is open it also watched for a picture, which makes the
//! verdict sure; a start it didn't see is only a guess, from how soon it ended.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

/// Longer than this and it's a game that was played (or hung, and was stopped), not one that
/// didn't start.
pub const WITHIN: u64 = 120;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Failed {
    pub when: u64,
    pub seconds: u64,
    /// Why, when the log or the command says.
    pub why: Option<String>,
    /// Seen ending without showing anything. Not: a Steam game that ended very soon while the
    /// window wasn't open, which may have been someone quitting.
    #[serde(default = "sure")]
    pub sure: bool,
}

fn sure() -> bool {
    true
}

fn file() -> PathBuf {
    crate::store::dir().join("starts.json")
}

fn load() -> HashMap<String, Failed> {
    fs::read_to_string(file()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

/// Every game's last start that failed, by id.
pub fn all() -> HashMap<String, Failed> {
    load()
}

/// The game's last start, when it failed.
pub fn failed(id: &str) -> Option<Failed> {
    load().remove(id)
}

/// Records how a start went: a failure, or none (it worked).
pub fn record(id: &str, failed: Option<Failed>) {
    let mut all = load();
    let changed = match failed {
        Some(f) => {
            all.insert(id.to_string(), f);
            true
        }
        None => all.remove(id).is_some(),
    };
    if changed {
        let _ = fs::create_dir_all(crate::store::dir());
        let _ = fs::write(file(), serde_json::to_string_pretty(&all).unwrap_or_default());
    }
}

/// Unwatched, a Steam game that ended sooner than this may not have started; any later and it
/// counts as played.
const UNWATCHED_WITHIN: u64 = 30;

/// A start and end of a game in Steam's log.
#[derive(Debug, PartialEq)]
struct SteamRun {
    app: u32,
    started: u64,
    ended: u64,
}

/// Steam's log: the runs that ended, in order, and the apps running at its end.
fn steam_runs(log: &str) -> (Vec<SteamRun>, std::collections::HashSet<u32>) {
    let mut open: HashMap<u32, u64> = HashMap::new();
    let mut runs = Vec::new();
    for line in log.lines() {
        // [2026-11-08 15:38:39] Game process added : AppID 422900 "..."
        let Some((time, rest)) = line.strip_prefix('[').and_then(|l| l.split_once("] Game process ")) else { continue };
        let (added, rest) = if let Some(r) = rest.strip_prefix("added") {
            (true, r)
        } else if let Some(r) = rest.strip_prefix("removed") {
            (false, r)
        } else {
            continue;
        };
        let Some(app) = rest.split("AppID ").nth(1).and_then(|r| r.split_whitespace().next()).and_then(|a| a.parse().ok()) else { continue };
        let Some(at) = crate::library::unix_time(&time.replacen(' ', "T", 1)) else { continue };
        if added {
            open.insert(app, at);
        } else if let Some(started) = open.remove(&app) {
            runs.push(SteamRun { app, started, ended: at });
        }
    }
    (runs, open.into_keys().collect())
}

/// What a Steam run says: Some(failure) or None (it worked, or can't be called a failure).
fn verdict(run: &SteamRun, watched: bool, shown: bool) -> Option<Failed> {
    let seconds = run.ended.saturating_sub(run.started);
    let failed = |sure| Some(Failed { when: run.ended, seconds, why: None, sure });
    match (watched, shown) {
        (_, true) => None,
        (true, false) if seconds < WITHIN => failed(true),
        (false, false) if seconds < UNWATCHED_WITHIN => failed(false),
        _ => None,
    }
}

fn steam_seen_file() -> PathBuf {
    crate::store::dir().join("steam-runs-seen")
}

/// Steam restarts a game by itself sometimes (a first process that ends for the real one): a run
/// is judged once this long has passed with nothing after it.
const SETTLE: u64 = 10;

/// Records how the Steam games that ended since the last look went. `games` by Steam app id,
/// `since`: when the window opened (a start before that wasn't watched), `shown`: games it saw
/// show a picture. Returns the games judged, whose `shown` is used up.
pub fn judge_steam(games: &HashMap<u32, String>, logs: &[PathBuf], since: u64, shown: &std::collections::HashSet<String>) -> Vec<String> {
    let log: Vec<u8> = logs.iter().flat_map(|f| fs::read(f).unwrap_or_default()).collect();
    let (runs, running) = steam_runs(&String::from_utf8_lossy(&log));
    let seen: u64 = fs::read_to_string(steam_seen_file()).ok().and_then(|t| t.trim().parse().ok()).unwrap_or(0);
    let now = crate::store::now();
    let mut latest = seen;
    let mut judged = Vec::new();
    // The next run of the same game tells whether this one was only Steam restarting it.
    for (i, run) in runs.iter().enumerate().filter(|(_, r)| r.ended > seen) {
        if running.contains(&run.app) || (now.saturating_sub(run.ended) < SETTLE && !runs[i + 1..].iter().any(|r| r.app == run.app)) {
            continue;
        }
        latest = latest.max(run.ended);
        let Some(id) = games.get(&run.app) else { continue };
        record(id, verdict(run, run.started >= since, shown.contains(id)));
        judged.push(id.clone());
    }
    if latest > seen {
        let _ = fs::create_dir_all(crate::store::dir());
        let _ = fs::write(steam_seen_file(), latest.to_string());
    }
    judged
}

/// How soon it ended, for a sentence: "right away", "after 4 s".
pub fn after(seconds: u64) -> String {
    if seconds < 2 { "right away".into() } else { format!("after {seconds} s") }
}

/// The likely reason, in plain words, from the game's command and what it printed (the last
/// lines win: Wine prints warnings all along).
pub fn why(command: &[String], log: &str) -> Option<String> {
    // Not the anti-cheat runtimes Lobby offers every Proton game (PROTON_BATTLEYE_RUNTIME=…):
    // they named BattlEye for every failed start.
    let runtime = |w: &str| w.contains("_RUNTIME=") || w.contains("/lobby/tools/");
    if let Some(ac) = command.iter().filter(|w| !runtime(w)).find_map(|w| crate::anticheat::in_path(w)) {
        return Some(format!("It starts through {ac}, which runs on Linux only for games whose makers allow it."));
    }
    let lines: Vec<&str> = log.lines().collect();
    let tail = &lines[lines.len().saturating_sub(400)..];
    let find = |needle: &str| tail.iter().rev().find(|l| l.contains(needle)).copied();
    // umu: "PROTONPATH '…' is not valid, toolmanifest.vdf not found".
    if let Some(l) = find("PROTONPATH '") {
        let path = l.split('\'').nth(1).unwrap_or("its Proton");
        return Some(format!("Its Proton isn't where its setup says ({path}): it was moved or uninstalled. Install it again in Steam (Lobby finds a Proton moved to another Steam library by itself)."));
    }
    for l in tail.iter().rev().filter(|l| !runtime(l)) {
        if let Some(ac) = ["EasyAntiCheat", "Easy Anti-Cheat", "BattlEye", "BEClient", "EOS_AntiCheat", "AntiCheat"].iter().find(|a| l.contains(**a)) {
            let name = crate::anticheat::named(&ac.replace(' ', "")).unwrap_or("Its anti-cheat");
            return Some(format!("{name} stopped it: on Linux it runs only for games whose makers allow it."));
        }
    }
    // Wine: `err:module:import_dll Library MSVCP140.dll (which is needed by L"...") not found`.
    if let Some(l) = find("err:module:import_dll Library ") {
        let dll = l.split("Library ").nth(1).and_then(|r| r.split_whitespace().next()).unwrap_or("a file");
        return Some(format!("A Windows file it needs is missing: {dll}."));
    }
    if let Some(l) = find("error while loading shared libraries: ") {
        let lib = l.split("shared libraries: ").nth(1).and_then(|r| r.split(':').next()).unwrap_or("a library");
        return Some(format!("A Linux library it needs is missing: {lib}."));
    }
    if find("err:mscoree").is_some() || find("CLR error").is_some() {
        return Some("It needs .NET, which its Wine setup doesn't have.".into());
    }
    if ["vkCreateInstance", "VK_ERROR_", "DXVK: No adapters", "Failed to create Vulkan", "vkCreateDevice failed"].iter().any(|n| find(n).is_some()) {
        return Some("Vulkan, which draws it, didn't start: a graphics driver problem.".into());
    }
    if let Some(l) = find("Exception in thread \"main\"") {
        return Some(format!("Java stopped with an error: {}", l.trim().chars().take(160).collect::<String>()));
    }
    if find("Unhandled exception").is_some() || find("Segmentation fault").is_some() || find("page fault on").is_some() {
        return Some("It crashed as it started.".into());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steam_log() {
        let log = "[2026-11-08 15:38:38] GameAction [AppID 422900, ActionID 1] : LaunchApp changed task to CreatingProcess with \"\"\n\
                   [2026-11-08 15:38:39] Game process added : AppID 422900 \"WINEDLLOVERRIDES=x /steam-launch-wrapper -- /g.exe\", ProcID 1, IP 0.0.0.0:0\n\
                   [2026-11-08 15:38:42] Game process updated : AppID 422900 \"x\"\n\
                   [2026-11-08 15:38:50] Game process removed: AppID 422900 \"x\", ProcID 1\n\
                   [2026-11-08 16:00:00] Game process added : AppID 570 \"x\"\n\
                   [2026-11-08 16:01:00] Game process removed: AppID 9 \"never added\"\n";
        let (runs, running) = steam_runs(log);
        let at = |t: &str| crate::library::unix_time(t).unwrap();
        assert_eq!(runs, [SteamRun { app: 422900, started: at("2026-11-08T15:38:39"), ended: at("2026-11-08T15:38:50") }]);
        assert!(running.contains(&570) && running.len() == 1);
    }

    #[test]
    fn steam_verdicts() {
        let run = |seconds| SteamRun { app: 1, started: 1000, ended: 1000 + seconds };
        // Watched: anything short with no picture failed; a picture means it started.
        assert!(verdict(&run(90), true, false).is_some_and(|f| f.sure && f.seconds == 90));
        assert_eq!(verdict(&run(5), true, true), None);
        assert_eq!(verdict(&run(WITHIN), true, false), None);
        // Not watched: only a very short one is a guess, never sure.
        assert!(verdict(&run(5), false, false).is_some_and(|f| !f.sure));
        assert_eq!(verdict(&run(UNWATCHED_WITHIN), false, false), None);
    }

    #[test]
    fn old_records_were_sure() {
        let f: Failed = serde_json::from_str(r#"{"when":1,"seconds":2,"why":null}"#).unwrap();
        assert!(f.sure);
    }

    #[test]
    fn reasons() {
        let cmd = |w: &[&str]| w.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let proton = cmd(&["env", "PROTON_BATTLEYE_RUNTIME=/h/.local/share/lobby/tools/battleye_runtime-2024-05-21", "PROTONPATH=/s/Proton - Experimental", "umu-run", "/g/CoreBreaker.exe"]);
        let log = "FileNotFoundError: PROTONPATH '/s/Proton - Experimental' is not valid, toolmanifest.vdf not found";
        assert!(why(&proton, log).unwrap().contains("Its Proton isn't where its setup says (/s/Proton - Experimental)"));
        assert_eq!(why(&proton, ""), None);
        let plain = cmd(&["umu-run", "/g/game.exe"]);
        assert!(why(&cmd(&["umu-run", "/g/start_protected_game.exe"]), "").unwrap().contains("Easy Anti-Cheat"));
        assert_eq!(why(&plain, "0024:err:module:import_dll Library MSVCP140.dll (which is needed by L\"Z:\\\\g\\\\game.exe\") not found\n").as_deref(), Some("A Windows file it needs is missing: MSVCP140.dll."));
        assert_eq!(why(&plain, "./game: error while loading shared libraries: libssl.so.1.1: cannot open shared object file\n").as_deref(), Some("A Linux library it needs is missing: libssl.so.1.1."));
        assert!(why(&plain, "info: Game version 1.0\n[EOS SDK] EasyAntiCheat: integrity violation\n").unwrap().starts_with("Easy Anti-Cheat stopped it"));
        assert_eq!(why(&plain, "all fine\n"), None);
    }
}
