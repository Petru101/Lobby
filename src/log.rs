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
pub fn timed<T>(what: &str, f: impl FnOnce() -> T) -> T {
    let start = Instant::now();
    let out = f();
    line(&format!("{what}: {} ms", start.elapsed().as_millis()));
    out
}
