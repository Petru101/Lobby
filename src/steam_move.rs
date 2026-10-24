//! Moving a game from a Steam library Steam doesn't list (an old Windows install's) into one
//! it does: the game's folder and its app manifest copied into Steam's library on a Linux-format
//! drive with the most room (a Windows-format drive can't hold Proton's prefixes). Steam reads
//! its libraries when it starts, so it's restarted after. The old copy is left where it was.

use crate::sources::Home;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

/// Free bytes on the drive holding the folder.
fn free_space(path: &Path) -> u64 {
    let out = std::process::Command::new("df").args(["-B1", "--output=avail"]).arg(path).stdin(std::process::Stdio::null()).output();
    out.ok().and_then(|o| String::from_utf8_lossy(&o.stdout).lines().nth(1).and_then(|l| l.trim().parse().ok())).unwrap_or(0)
}

fn size_of(dir: &Path) -> u64 {
    fs::read_dir(dir).into_iter().flatten().flatten().map(|e| match e.file_type() {
        Ok(t) if t.is_dir() => size_of(&e.path()),
        Ok(t) if t.is_file() => e.metadata().map_or(0, |m| m.len()),
        _ => 0,
    }).sum()
}

/// Steam's library to move into: on a Linux-format drive, with room for the game, the one with
/// the most room.
pub fn target(home: &Home, needs: u64) -> Result<PathBuf, String> {
    let mounts = crate::drives::mounts();
    let libs: Vec<PathBuf> = crate::sources::steam::known_libraries(home).into_iter().filter(|l| l.join("steamapps").is_dir() && !crate::drives::mount_of(l, &mounts).is_some_and(|m| m.windows_format())).collect();
    let best = libs.into_iter().map(|l| (free_space(&l), l)).max_by_key(|(free, _)| *free).ok_or("Steam has no library on a Linux-format drive to move it into (add one in Steam: Settings → Storage).")?;
    if best.0 < needs + (1 << 30) {
        return Err(format!("Steam's library {} doesn't have room for it ({} GB needed).", best.1.display(), needs / (1 << 30) + 1));
    }
    Ok(best.1)
}

/// The game's size, for the question.
pub fn size_text(folder: &Path) -> String {
    let gb = size_of(folder) as f64 / 1e9;
    if gb >= 1.0 { format!("{gb:.1} GB") } else { format!("{:.0} MB", gb * 1000.0) }
}

/// Copies the game (in `<old library>/steamapps/common/<dir>`) and its manifest into Steam's
/// library; `progress` gets 0 to 100. Returns the library it went to.
pub fn move_into_steam(home: &Home, folder: &Path, appid: u32, progress: &dyn Fn(f64)) -> Result<PathBuf, String> {
    let steamapps = folder.parent().and_then(Path::parent).filter(|p| p.ends_with("steamapps")).ok_or("It isn't in a Steam library's steamapps/common.")?;
    let manifest = steamapps.join(format!("appmanifest_{appid}.acf"));
    if !manifest.is_file() {
        return Err("Its app manifest isn't there, so Steam couldn't take it.".into());
    }
    let total = size_of(folder);
    let lib = target(home, total)?;
    let into = lib.join("steamapps/common").join(folder.file_name().ok_or("no folder name")?);
    let new_manifest = lib.join("steamapps").join(format!("appmanifest_{appid}.acf"));
    if new_manifest.exists() || into.exists() {
        return Err(format!("Steam's library {} has it already.", lib.display()));
    }
    let done = std::cell::Cell::new(0u64);
    let work = into.with_file_name(format!(".lobby-moving-{appid}"));
    let _ = fs::remove_dir_all(&work);
    copy_dir(folder, &work, &|n| {
        done.set(done.get() + n);
        progress(done.get() as f64 * 100.0 / total.max(1) as f64);
    })
    .inspect_err(|_| {
        let _ = fs::remove_dir_all(&work);
    })?;
    fs::rename(&work, &into).map_err(|e| e.to_string())?;
    fs::copy(&manifest, &new_manifest).map_err(|e| e.to_string())?;
    Ok(lib)
}

fn copy_dir(from: &Path, to: &Path, copied: &dyn Fn(u64)) -> Result<(), String> {
    fs::create_dir_all(to).map_err(|e| format!("{}: {e}", to.display()))?;
    for entry in fs::read_dir(from).map_err(|e| format!("{}: {e}", from.display()))?.flatten() {
        let (src, dst) = (entry.path(), to.join(entry.file_name()));
        let kind = entry.file_type().map_err(|e| e.to_string())?;
        if kind.is_dir() {
            copy_dir(&src, &dst, copied)?;
        } else if kind.is_symlink() {
            let target = fs::read_link(&src).map_err(|e| e.to_string())?;
            std::os::unix::fs::symlink(target, &dst).map_err(|e| e.to_string())?;
        } else {
            let mut input = fs::File::open(&src).map_err(|e| format!("{}: {e}", src.display()))?;
            let mut output = fs::File::create(&dst).map_err(|e| format!("{}: {e}", dst.display()))?;
            let mut buf = vec![0u8; 1 << 20];
            loop {
                let n = input.read(&mut buf).map_err(|e| format!("{}: {e}", src.display()))?;
                if n == 0 {
                    break;
                }
                output.write_all(&buf[..n]).map_err(|e| format!("{}: {e}", dst.display()))?;
                copied(n as u64);
            }
        }
    }
    Ok(())
}

/// Restarts Steam, so it reads its libraries again (when it's running).
pub fn restart_steam() -> Result<(), String> {
    let Some(steam) = gamefix::steam::Steam::find() else { return Ok(()) };
    if !steam.running() {
        return Ok(());
    }
    if !steam.shut_down() {
        return Err("Steam didn't close within a minute: restart it yourself to see the game.".into());
    }
    std::os::unix::process::CommandExt::process_group(&mut std::process::Command::new("steam"), 0).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn().map(|_| ()).map_err(|e| e.to_string())
}
