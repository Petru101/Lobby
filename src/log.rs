//! lobby's own log, ~/.local/state/lobby/lobby.log: what it did and how long each step took,
//! for finding what's slow. Started over when it passes 1 MB.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

pub fn file() -> PathBuf {
    std::env::var_os("XDG_STATE_HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/state")).join("lobby/lobby.log")
}

/// One line: the time of day (UTC), the process, what happened.
pub fn line(what: &str) {
    let file = file();
    let _ = fs::create_dir_all(file.parent().unwrap());
    if fs::metadata(&file).is_ok_and(|m| m.len() > 1_000_000) {
        let _ = fs::remove_file(&file);
    }
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    let (s, ms) = (now.as_secs() % 86_400, now.subsec_millis());
    let stamp = format!("{:02}:{:02}:{:02}.{ms:03}", s / 3600, s / 60 % 60, s % 60);
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(&file) {
        let _ = writeln!(f, "{stamp} [{}] {what}", std::process::id());
    }
}

/// Runs `f`, logging how long it took.
/// Like `timed`, with the processor time this thread spent on it too (waiting costs none).
pub fn timed_cpu<T>(what: &str, f: impl FnOnce() -> T) -> T {
    let cpu = || fs::read_to_string("/proc/thread-self/schedstat").ok().and_then(|s| s.split(' ').next()?.parse::<u64>().ok()).unwrap_or(0);
    let (start, used) = (Instant::now(), cpu());
    let out = f();
    line(&format!("{what}: {} ms, {} ms of processor time", start.elapsed().as_millis(), cpu().saturating_sub(used) / 1_000_000));
    out
}

pub fn timed<T>(what: &str, f: impl FnOnce() -> T) -> T {
    let start = Instant::now();
    let out = f();
    line(&format!("{what}: {} ms", start.elapsed().as_millis()));
    out
}

/// Milliseconds since this process started (from /proc: the kernel's start time against the
/// time since boot), for timing the window's start from the very beginning.
pub fn since_start() -> u64 {
    let start = fs::read_to_string("/proc/self/stat").ok().and_then(|s| s.rsplit(')').next()?.split_whitespace().nth(19)?.parse::<u64>().ok());
    let up = fs::read_to_string("/proc/uptime").ok().and_then(|s| s.split_whitespace().next()?.parse::<f64>().ok());
    match (start, up) {
        (Some(start), Some(up)) => ((up * 1000.0) as u64).saturating_sub(start * 10),
        _ => 0,
    }
}
