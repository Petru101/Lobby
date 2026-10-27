//! Starts that failed: the game ended before anything of it showed (no GPU driver or display
//! connection loaded: `running::shows_window`), within a couple of minutes. Seen by `lobby
//! sandbox`, which every protected start goes through (Play and the shortcuts), and kept in
//! ~/.local/share/lobby/starts.json by game id until a start works. The likely reason is read
//! from the game's log (~/.local/state/lobby/logs/<id>.log) and its command.

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

/// How soon it ended, for a sentence: "right away", "after 4 s".
pub fn after(seconds: u64) -> String {
    if seconds < 2 { "right away".into() } else { format!("after {seconds} s") }
}

/// The likely reason, in plain words, from the game's command and what it printed (the last
/// lines win: Wine prints warnings all along).
pub fn why(command: &[String], log: &str) -> Option<String> {
    if let Some(ac) = command.iter().find_map(|w| crate::anticheat::in_path(w)) {
        return Some(format!("It starts through {ac}, which runs on Linux only for games whose makers allow it."));
    }
    let lines: Vec<&str> = log.lines().collect();
    let tail = &lines[lines.len().saturating_sub(400)..];
    let find = |needle: &str| tail.iter().rev().find(|l| l.contains(needle)).copied();
    for l in tail.iter().rev() {
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
    fn reasons() {
        let cmd = |w: &[&str]| w.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let plain = cmd(&["umu-run", "/g/game.exe"]);
        assert!(why(&cmd(&["umu-run", "/g/start_protected_game.exe"]), "").unwrap().contains("Easy Anti-Cheat"));
        assert_eq!(why(&plain, "0024:err:module:import_dll Library MSVCP140.dll (which is needed by L\"Z:\\\\g\\\\game.exe\") not found\n").as_deref(), Some("A Windows file it needs is missing: MSVCP140.dll."));
        assert_eq!(why(&plain, "./game: error while loading shared libraries: libssl.so.1.1: cannot open shared object file\n").as_deref(), Some("A Linux library it needs is missing: libssl.so.1.1."));
        assert!(why(&plain, "info: Game version 1.0\n[EOS SDK] EasyAntiCheat: integrity violation\n").unwrap().starts_with("Easy Anti-Cheat stopped it"));
        assert_eq!(why(&plain, "all fine\n"), None);
    }
}
