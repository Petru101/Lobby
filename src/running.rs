//! Which games are running and stopping them, from the processes (/proc), whoever started
//! them:
//! - Lobby's own wrappers: `lobby sandbox <id> --` (every Play, and the app-menu shortcuts it
//!   wraps) and `lobby run <id>` (its store games).
//! - Steam's: `reaper SteamLaunch AppId=<id>` holds each game Steam starts.
//! - Anything else (Heroic, Lutris, the game started by hand): a process running the game's
//!   program: a Wine process names it as `Z:\…` or as `C:\…` in its WINEPREFIX, a Linux one is
//!   it (/proc/<pid>/exe).
//!
//! Stopping ends what's under the wrapper (the sandbox goes with its bubblewrap, a Wine game with
//! its process), never the wrapper itself: `lobby sandbox` then ends as when the game quits
//! (play time counted) and `lobby run` still puts the saves in the cloud. SIGTERM first, SIGKILL
//! to what's left after a few seconds (a hanging game ignores the first).

use crate::library::{Game, Key, Kind};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

struct Process {
    pid: i32,
    ppid: i32,
    args: Vec<String>,
}

fn processes() -> Vec<Process> {
    let me = std::process::id() as i32;
    fs::read_dir("/proc")
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.file_name().to_str()?.parse::<i32>().ok())
        .filter(|&pid| pid != me)
        .filter_map(|pid| {
            let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
            // pid (comm) state ppid ...: comm may hold spaces and parentheses.
            let ppid = stat.rsplit_once(')')?.1.split_whitespace().nth(1)?.parse().ok()?;
            let raw = fs::read(format!("/proc/{pid}/cmdline")).ok()?;
            let args = raw.split(|&b| b == 0).filter(|a| !a.is_empty()).map(|a| String::from_utf8_lossy(a).into_owned()).collect();
            Some(Process { pid, ppid, args })
        })
        .collect()
}

/// A Wine process's program as a path here: `Z:\run\media\…` is the path itself, `C:\…` is in
/// its prefix's drive_c.
fn wine_path(arg: &str, pid: i32) -> Option<PathBuf> {
    let a = arg.replace('\\', "/");
    let (drive, rest) = a.split_once(":/")?;
    match drive.to_ascii_lowercase().as_str() {
        "z" => Some(PathBuf::from(format!("/{rest}"))),
        "c" => {
            let env = fs::read(format!("/proc/{pid}/environ")).ok()?;
            let prefix = env.split(|&b| b == 0).find_map(|v| v.strip_prefix(b"WINEPREFIX=").or_else(|| v.strip_prefix(b"STEAM_COMPAT_DATA_PATH=")).map(|p| String::from_utf8_lossy(p).into_owned()))?;
            let prefix = PathBuf::from(prefix);
            let prefix = if prefix.join("pfx").is_dir() { prefix.join("pfx") } else { prefix };
            Some(prefix.join("drive_c").join(rest))
        }
        _ => None,
    }
}

fn same_file(a: &Path, b: &Path) -> bool {
    let norm = |p: &Path| crate::library::norm(p).to_string_lossy().to_lowercase();
    norm(a) == norm(b)
}

/// The running games, by id, each with the processes holding it (a wrapper, Steam's reaper, or
/// the game's own process).
pub fn find(games: &[Game]) -> HashMap<String, Vec<i32>> {
    let procs = processes();
    let mut out: HashMap<String, Vec<i32>> = HashMap::new();
    let steam_app = |g: &Game| g.entries.iter().flat_map(|e| e.keys.iter()).find_map(|k| if let Key::SteamApp(id) = k { Some(*id) } else { None });
    let programs: Vec<(&Game, &Path)> = games.iter().filter(|g| g.kind == Kind::Game && !g.gone).flat_map(|g| g.entries.iter().filter_map(move |e| e.program.as_deref().map(|p| (g, p)))).collect();
    for p in &procs {
        let lobby = p.args.first().is_some_and(|a| Path::new(a).file_name().is_some_and(|n| n == "lobby"));
        // `lobby sandbox <id> -- …` (not Battle.net's own: it isn't a game) and `lobby run <id>`.
        if lobby && p.args.get(1).is_some_and(|a| a == "sandbox") && p.args.get(3).is_some_and(|a| a == "--") && !p.args.iter().any(|a| a.contains("Battle.net.exe")) {
            out.entry(p.args[2].clone()).or_default().push(p.pid);
            continue;
        }
        if lobby && p.args.get(1).is_some_and(|a| a == "run") && p.args.len() > 2 {
            let id = p.args[2..].join(" ");
            if let Some(g) = games.iter().find(|g| g.id == id) {
                out.entry(g.id.clone()).or_default().push(p.pid);
            }
            continue;
        }
        if let Some(app) = p.args.iter().position(|a| a == "SteamLaunch").and_then(|_| p.args.iter().find_map(|a| a.strip_prefix("AppId=")?.parse::<u32>().ok())) {
            if let Some(g) = games.iter().find(|g| steam_app(g) == Some(app)) {
                out.entry(g.id.clone()).or_default().push(p.pid);
            }
            continue;
        }
        // The game's own process.
        let Some(first) = p.args.first() else { continue };
        let name = first.replace('\\', "/").rsplit('/').next().unwrap_or_default().to_lowercase();
        let candidates: Vec<&(&Game, &Path)> = programs.iter().filter(|(_, prog)| prog.file_name().is_some_and(|n| n.to_string_lossy().to_lowercase() == name)).collect();
        if candidates.is_empty() {
            continue;
        }
        let path = wine_path(first, p.pid).or_else(|| fs::read_link(format!("/proc/{}/exe", p.pid)).ok());
        if let Some(path) = path {
            if let Some((g, _)) = candidates.iter().find(|(_, prog)| same_file(prog, &path)) {
                out.entry(g.id.clone()).or_default().push(p.pid);
            }
        }
    }
    // A game seen both through its wrapper and its own process: the wrapper says it all.
    for pids in out.values_mut() {
        let all = pids.clone();
        pids.retain(|&pid| !all.iter().any(|&other| other != pid && descends(&procs, pid, other)));
    }
    out
}

/// Whether `pid` is under `ancestor`.
fn descends(procs: &[Process], pid: i32, ancestor: i32) -> bool {
    let mut at = pid;
    for _ in 0..64 {
        let Some(p) = procs.iter().find(|p| p.pid == at) else { return false };
        if p.ppid == ancestor {
            return true;
        }
        if p.ppid <= 1 {
            return false;
        }
        at = p.ppid;
    }
    false
}

/// Ends a running game: what's under its wrappers, or its own processes and theirs. Waits up to
/// `patience` for them to quit after SIGTERM, then SIGKILLs the rest. Whether it's gone.
pub fn stop(game: &Game, all: &[Game], patience: std::time::Duration) -> Result<bool, String> {
    let roots = find(all).remove(&game.id).ok_or_else(|| format!("{} isn't running.", game.name))?;
    let targets = |procs: &[Process]| -> Vec<i32> {
        let wrapper = |pid: i32| procs.iter().find(|p| p.pid == pid).is_some_and(|p| p.args.first().is_some_and(|a| Path::new(a).file_name().is_some_and(|n| n == "lobby")) || p.args.iter().any(|a| a == "SteamLaunch"));
        let mut t: Vec<i32> = procs.iter().filter(|p| roots.iter().any(|&r| descends(procs, p.pid, r))).map(|p| p.pid).collect();
        // A game's own process (no wrapper) goes too.
        t.extend(roots.iter().filter(|&&r| !wrapper(r)));
        t
    };
    let first = targets(&processes());
    for &pid in &first {
        unsafe { libc::kill(pid, libc::SIGTERM) };
    }
    let start = std::time::Instant::now();
    loop {
        let left: Vec<i32> = first.iter().copied().filter(|&pid| alive(pid)).collect();
        if left.is_empty() {
            return Ok(true);
        }
        if start.elapsed() >= patience {
            // Hanging: what's left, and anything it started meanwhile.
            let mut rest = targets(&processes());
            rest.extend(left);
            for pid in rest {
                unsafe { libc::kill(pid, libc::SIGKILL) };
            }
            std::thread::sleep(std::time::Duration::from_millis(500));
            return Ok(first.iter().all(|&pid| !alive(pid)));
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
}

/// Still running (a process that ended but wasn't collected yet is a zombie: gone).
fn alive(pid: i32) -> bool {
    fs::read_to_string(format!("/proc/{pid}/stat")).ok().and_then(|s| s.rsplit_once(')').and_then(|(_, rest)| rest.split_whitespace().next().map(|st| st != "Z"))).unwrap_or(false)
}
