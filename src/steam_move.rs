//! Moving a game from a Steam library Steam doesn't list (an old Windows install's) into one
//! it does: the game's folder and its app manifest moved into Steam's library on a Linux-format
//! drive with the most room (a Windows-format drive can't hold Proton's prefixes). Steam reads
//! its libraries when it starts, so it's restarted after. It's a move, the user's wish: they
//! couldn't tell where the copy left behind was ("I can't keep track of Linux's drive naming").
//! On the same drive the folder is renamed; across drives it's copied, the copy checked against
//! the original (every file's size), and only then the original deleted.

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

/// What of the original couldn't be deleted (the game is in Steam's library either way).
pub struct Moved {
    pub left: Option<String>,
}

/// Every file under the folder, by path from it, with its size.
fn files_of(dir: &Path, under: &Path, out: &mut Vec<(PathBuf, u64)>) {
    for e in fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = e.path();
        match e.file_type() {
            Ok(t) if t.is_dir() => files_of(&path, under, out),
            Ok(t) => out.push((path.strip_prefix(under).unwrap_or(&path).to_path_buf(), if t.is_file() { e.metadata().map_or(0, |m| m.len()) } else { 0 })),
            Err(_) => {}
        }
    }
}

fn same_files(a: &Path, b: &Path) -> bool {
    let list = |d: &Path| {
        let mut v = Vec::new();
        files_of(d, d, &mut v);
        v.sort();
        v
    };
    list(a) == list(b)
}

/// Moves the game (in `<old library>/steamapps/common/<dir>`), its manifest and its Proton
/// prefix if it has one there into Steam's library; `progress` gets 0 to 100.
pub fn move_into_steam(home: &Home, folder: &Path, appid: u32, progress: &dyn Fn(f64)) -> Result<Moved, String> {
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
    // Its Proton prefix, when it was played from there on Linux: its saves may be in it.
    let (prefix, new_prefix) = (steamapps.join("compatdata").join(appid.to_string()), lib.join("steamapps/compatdata").join(appid.to_string()));
    let prefix = prefix.is_dir().then_some(prefix).filter(|_| !new_prefix.exists());
    // On the same drive a rename is the whole move.
    if fs::rename(folder, &into).is_ok() {
        let left = fs::rename(&manifest, &new_manifest).or_else(|_| fs::copy(&manifest, &new_manifest).map(|_| ()));
        left.map_err(|e| e.to_string())?;
        if let Some(p) = &prefix {
            let _ = fs::create_dir_all(new_prefix.parent().unwrap());
            if fs::rename(p, &new_prefix).is_err() {
                copy_then_delete(p, &new_prefix, &|_| {})?;
            }
        }
        return Ok(Moved { left: None });
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
    if !same_files(folder, &work) {
        let _ = fs::remove_dir_all(&work);
        return Err("The copy didn't come out the same as the original, so nothing was moved.".into());
    }
    fs::rename(&work, &into).map_err(|e| e.to_string())?;
    fs::copy(&manifest, &new_manifest).map_err(|e| e.to_string())?;
    let mut left = Vec::new();
    if let Some(p) = &prefix {
        let _ = fs::create_dir_all(new_prefix.parent().unwrap());
        if let Err(e) = copy_then_delete(p, &new_prefix, &|_| {}) {
            left.push(format!("its Proton settings: {e}"));
        }
    }
    // The original goes only now, with its copy in place and checked.
    if let Err(e) = fs::remove_dir_all(folder) {
        left.push(format!("{} ({e})", folder.display()));
    }
    let _ = fs::remove_file(&manifest);
    Ok(Moved { left: (!left.is_empty()).then(|| left.join("; ")) })
}

/// A folder copied, checked, then deleted.
fn copy_then_delete(from: &Path, to: &Path, copied: &dyn Fn(u64)) -> Result<(), String> {
    copy_dir(from, to, copied)?;
    if !same_files(from, to) {
        let _ = fs::remove_dir_all(to);
        return Err("the copy didn't come out the same".into());
    }
    fs::remove_dir_all(from).map_err(|e| e.to_string())
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

/// Restarts Steam, so it reads its libraries again (when it's running), and starts `play` with
/// it (Steam is started for that when it isn't running).
pub fn restart_steam(play: Option<u32>) -> Result<(), String> {
    let Some(steam) = gamefix::steam::Steam::find() else { return Ok(()) };
    if steam.running() {
        if !steam.shut_down() {
            return Err("Steam didn't close within a minute: restart it yourself to see the game.".into());
        }
    } else if play.is_none() {
        return Ok(());
    }
    let command: Vec<String> = std::iter::once("steam".to_string()).chain(play.map(|id| format!("steam://rungameid/{id}"))).collect();
    crate::launch::start("Steam", &command, None, &crate::store::log_file("steam"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A game in a stray library under `stray`, moved into the Steam library of a home under
    /// `home`: the same drive (a rename) or another (copied, checked, original deleted).
    fn moves(stray: &Path, home: &Path) {
        let _ = (fs::remove_dir_all(stray), fs::remove_dir_all(home));
        let steam = home.join(".local/share/Steam/steamapps");
        fs::create_dir_all(steam.join("common")).unwrap();
        let game = stray.join("SteamLibrary/steamapps/common/Some Game");
        fs::create_dir_all(game.join("data")).unwrap();
        fs::write(game.join("game.exe"), vec![1u8; 300_000]).unwrap();
        fs::write(game.join("data/level.pak"), vec![2u8; 70_000]).unwrap();
        std::os::unix::fs::symlink("level.pak", game.join("data/link")).unwrap();
        fs::write(stray.join("SteamLibrary/steamapps/appmanifest_42.acf"), "\"AppState\" {}").unwrap();
        let prefix = stray.join("SteamLibrary/steamapps/compatdata/42/pfx");
        fs::create_dir_all(&prefix).unwrap();
        fs::write(prefix.join("save.dat"), "saved").unwrap();
        let h = Home { home: home.to_path_buf(), user: "u".into() };
        let moved = move_into_steam(&h, &game, 42, &|_| {}).unwrap();
        assert!(moved.left.is_none());
        let into = fs::canonicalize(&steam).unwrap();
        assert_eq!(fs::read(into.join("common/Some Game/game.exe")).unwrap().len(), 300_000);
        assert_eq!(fs::read_link(into.join("common/Some Game/data/link")).unwrap(), Path::new("level.pak"));
        assert!(into.join("appmanifest_42.acf").is_file());
        assert_eq!(fs::read_to_string(into.join("compatdata/42/pfx/save.dat")).unwrap(), "saved");
        assert!(!game.exists() && !stray.join("SteamLibrary/steamapps/appmanifest_42.acf").exists() && !prefix.exists());
        // Nothing left half-way.
        assert!(!into.join("common/.lobby-moving-42").exists());
        let _ = (fs::remove_dir_all(stray), fs::remove_dir_all(home));
    }

    #[test]
    fn moves_a_game_into_steam_and_leaves_no_copy() {
        let here = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/test-steam-move");
        // Both in the project's folder: one drive.
        moves(&here.join("stray"), &here.join("home"));
        // The game in the temporary folder (another file system here): copied, then deleted.
        moves(&std::env::temp_dir().join(format!("lobby-stray-{}", std::process::id())), &here.join("home2"));
        let _ = fs::remove_dir_all(&here);
    }
}
