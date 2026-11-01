//! Every cover and icon a game shows, kept in Lobby's own folder (~/.local/share/lobby/covers):
//! where it came from (a launcher's cache, a download tied to an itch.io page, a .exe's icon)
//! can go, and the game keeps its picture (`Game::kept_cover`, `Game::kept_icon`).

use crate::library::Game;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub fn dir() -> PathBuf {
    crate::store::dir().join("covers")
}

fn web_dir() -> PathBuf {
    let dir = dir().join("web");
    // Downloaded into the cache folder before: moved over once.
    if !dir.exists() {
        let cache = std::env::var_os("XDG_CACHE_HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".cache"));
        let old = cache.join("lobby/covers");
        if old.is_dir() && fs::create_dir_all(dir.parent().unwrap_or(&dir)).is_ok() {
            let _ = fs::rename(&old, &dir);
        }
    }
    dir
}

/// A web cover's local copy, downloaded the first time (curl, which every desktop has).
pub fn web_copy(url: &str) -> Option<PathBuf> {
    let file = web_dir().join(format!("{}.img", crate::sha256_hex(url.as_bytes())));
    if file.is_file() {
        return Some(file);
    }
    fs::create_dir_all(file.parent()?).ok()?;
    let tmp = file.with_extension("part");
    // Epic's addresses can have spaces in them ("…TOEM_SomethingWeMade_S2 (1)_1200x1600…"),
    // which curl turns away.
    let ok = Command::new("curl").args(["-sfL", "--max-time", "30", "-o"]).arg(&tmp).arg(url.replace(' ', "%20")).stdin(Stdio::null()).status().is_ok_and(|s| s.success());
    if !ok {
        let _ = fs::remove_file(&tmp);
        return None;
    }
    fs::rename(&tmp, &file).ok()?;
    Some(file)
}

/// A picture file in Lobby's folder: itself when it's there already, else a copy named after
/// where it was, copied again when that changes.
fn copy(from: &Path) -> Option<PathBuf> {
    copy_into(from, &dir())
}

fn copy_into(from: &Path, dir: &Path) -> Option<PathBuf> {
    if from.starts_with(dir) {
        return Some(from.to_path_buf());
    }
    let ext = from.extension().map(|x| x.to_string_lossy().to_lowercase()).filter(|x| x.len() <= 4).unwrap_or_else(|| "img".into());
    let to = dir.join("kept").join(format!("{}.{ext}", crate::sha256_hex(from.as_os_str().as_encoded_bytes())));
    let modified = |p: &Path| fs::metadata(p).and_then(|m| m.modified()).ok();
    let size = |p: &Path| fs::metadata(p).map(|m| m.len()).ok();
    if to.is_file() && modified(&to) >= modified(from) && size(&to) == size(from) {
        return Some(to);
    }
    fs::create_dir_all(to.parent()?).ok()?;
    let tmp = to.with_extension("part");
    fs::copy(from, &tmp).ok()?;
    fs::rename(&tmp, &to).ok()?;
    Some(to)
}

/// Each game's cover and icon as found by this scan, kept in Lobby's folder.
pub fn keep(games: &mut [Game]) {
    let threads = 4;
    std::thread::scope(|s| {
        let mut chunks: Vec<Vec<&mut Game>> = (0..threads).map(|_| Vec::new()).collect();
        for (i, g) in games.iter_mut().enumerate() {
            chunks[i % threads].push(g);
        }
        for chunk in chunks {
            s.spawn(move || {
                for g in chunk {
                    g.kept_cover = g.found_cover().and_then(|c| if c.starts_with("https://") { web_copy(c) } else { copy(Path::new(c)) }).map(|p| p.display().to_string());
                    g.kept_icon = g.entries.iter().filter_map(|e| e.icon.as_deref()).find(|p| p.is_file()).and_then(copy);
                }
            });
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kept_copies() {
        let base = std::env::temp_dir().join(format!("lobby-covers-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let dir = base.join("data/lobby/covers");
        let launcher = base.join("launcher/cover.JPG");
        fs::create_dir_all(launcher.parent().unwrap()).unwrap();
        fs::write(&launcher, "picture").unwrap();
        let kept = copy_into(&launcher, &dir).unwrap();
        assert!(kept.starts_with(dir.join("kept")) && kept.extension().unwrap() == "jpg");
        assert_eq!(fs::read(&kept).unwrap(), b"picture");
        // Already Lobby's: itself.
        assert_eq!(copy_into(&kept, &dir).unwrap(), kept);
        // The launcher's picture changed: copied again.
        fs::write(&launcher, "a new picture").unwrap();
        assert_eq!(fs::read(copy_into(&launcher, &dir).unwrap()).unwrap(), b"a new picture");
        fs::remove_dir_all(&base).unwrap();
    }
}
