//! Save backups: copies of a game's save folders (`save_folders::find`) in
//! ~/.local/share/lobby/save-backups/<game>/<time>/, each with a backup.json saying where every
//! folder came from. Taken after each session Lobby times (`lobby sandbox`, `lobby run`), before
//! Uninstall deletes anything, before a restore, and when asked (Back up now on the game's page,
//! `lobby backups <game> now`). One with the same files as the newest isn't taken again; the
//! newest `KEEP` are kept. Restoring copies a backup's files back where they came from (newer
//! files the backup lacks stay, as Ludusavi does).

use crate::library::Game;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

/// Backups kept per game.
const KEEP: usize = 20;
/// More than this isn't a save folder (or not one worth copying after every session).
const LIMIT: u64 = 1 << 30;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Backup {
    pub when: u64,
    /// Why it was taken ("after playing", "before uninstalling" ...).
    pub why: String,
    pub folders: Vec<Saved>,
    pub files: usize,
    pub bytes: u64,
    /// The files' names, sizes and times, to tell whether anything changed since.
    fingerprint: String,
    #[serde(skip)]
    pub dir: PathBuf,
}

/// A save folder in a backup: where it was, and the folder holding its copy ("0", "1" ...).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Saved {
    pub from: PathBuf,
    pub copy: String,
}

pub fn dir(game_id: &str) -> PathBuf {
    crate::store::dir().join("save-backups").join(game_id)
}

/// The game's backups, newest first (Windows saves' replaced copies, kept in the same folder
/// without a backup.json, aren't backups).
pub fn list(game_id: &str) -> Vec<Backup> {
    let mut out: Vec<Backup> = fs::read_dir(dir(game_id))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|d| {
            let mut b: Backup = serde_json::from_str(&fs::read_to_string(d.path().join("backup.json")).ok()?).ok()?;
            b.dir = d.path();
            Some(b)
        })
        .collect();
    out.sort_by(|a, b| b.when.cmp(&a.when).then_with(|| b.dir.cmp(&a.dir)));
    out
}

/// The folders worth backing up: the game's save folders, but not one holding its program
/// (some games save beside it: that would copy the whole game).
fn folders(game: &Game) -> Vec<PathBuf> {
    let program = game.program().map(Path::to_path_buf);
    crate::save_folders::find(game).into_iter().map(|f| f.path).filter(|p| p.is_dir() && !program.as_ref().is_some_and(|x| x.starts_with(p))).collect()
}

/// Every file under a folder (links left out), relative to it, with its size and time.
fn files(root: &Path) -> Vec<(PathBuf, u64, u64)> {
    let mut out = Vec::new();
    let mut todo = vec![root.to_path_buf()];
    while let Some(d) = todo.pop() {
        for e in fs::read_dir(&d).into_iter().flatten().flatten() {
            let Ok(meta) = fs::symlink_metadata(e.path()) else { continue };
            if meta.is_dir() {
                todo.push(e.path());
            } else if meta.is_file() {
                let time = meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs());
                out.push((e.path().strip_prefix(root).unwrap_or(&e.path()).to_path_buf(), meta.len(), time));
            }
        }
    }
    out.sort();
    out
}

/// Copies a file with its time (a game may compare save times).
fn copy(from: &Path, to: &Path) -> std::io::Result<()> {
    if let Some(p) = to.parent() {
        fs::create_dir_all(p)?;
    }
    fs::copy(from, to)?;
    if let Ok(t) = fs::metadata(from).and_then(|m| m.modified()) {
        fs::File::options().write(true).open(to)?.set_modified(t)?;
    }
    Ok(())
}

/// Backs up the game's saves. None when it has none here, or nothing changed since the
/// newest backup.
pub fn take(game: &Game, why: &str) -> Result<Option<Backup>, String> {
    let folders = folders(game);
    let listed: Vec<Vec<(PathBuf, u64, u64)>> = folders.iter().map(|f| files(f)).collect();
    let count: usize = listed.iter().map(Vec::len).sum();
    if count == 0 {
        return Ok(None);
    }
    let bytes: u64 = listed.iter().flatten().map(|(_, size, _)| size).sum();
    if bytes > LIMIT {
        return Err(format!("its save folders hold {}, more than Lobby backs up", crate::windows_saves::size_text(bytes)));
    }
    let mut text = String::new();
    for (folder, list) in folders.iter().zip(&listed) {
        text.push_str(&format!("{}\n", folder.display()));
        for (path, size, time) in list {
            text.push_str(&format!("{}\t{size}\t{time}\n", path.display()));
        }
    }
    let fingerprint = crate::sha256_hex(text.as_bytes());
    let old = list(&game.id);
    if old.first().is_some_and(|b| b.fingerprint == fingerprint) {
        return Ok(None);
    }

    let when = crate::store::now();
    let base = dir(&game.id);
    let name = (1..).map(|n| if n == 1 { when.to_string() } else { format!("{when}-{n}") }).find(|n| !base.join(n).exists()).unwrap();
    let taking = base.join(format!(".taking-{name}"));
    let _ = fs::remove_dir_all(&taking);
    let result = (|| -> Result<Vec<Saved>, String> {
        let mut saved = Vec::new();
        for (i, (folder, list)) in folders.iter().zip(&listed).enumerate() {
            let copy_dir = taking.join(i.to_string());
            for (path, ..) in list {
                copy(&folder.join(path), &copy_dir.join(path)).map_err(|e| format!("couldn't copy {}: {e}", folder.join(path).display()))?;
            }
            saved.push(Saved { from: folder.clone(), copy: i.to_string() });
        }
        Ok(saved)
    })();
    let saved = match result {
        Ok(s) => s,
        Err(e) => {
            let _ = fs::remove_dir_all(&taking);
            return Err(e);
        }
    };
    let backup = Backup { when, why: why.to_string(), folders: saved, files: count, bytes, fingerprint, dir: base.join(&name) };
    fs::write(taking.join("backup.json"), serde_json::to_string_pretty(&backup).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    fs::rename(&taking, &backup.dir).map_err(|e| e.to_string())?;
    crate::log::line(&format!("saves backed up: {} ({why}, {count} files)", game.name));
    for b in list(&game.id).into_iter().skip(KEEP) {
        let _ = fs::remove_dir_all(&b.dir);
    }
    Ok(Some(backup))
}

/// Puts a backup's files back where they came from, after backing up the saves there now.
/// Returns how many files it put back.
pub fn restore(game: &Game, backup: &Backup) -> Result<usize, String> {
    take(game, "before restoring an older backup").map_err(|e| format!("The saves there now couldn't be backed up first ({e}), so nothing was restored."))?;
    let mut count = 0;
    for f in &backup.folders {
        let from = backup.dir.join(&f.copy);
        for (path, ..) in files(&from) {
            copy(&from.join(&path), &f.from.join(&path)).map_err(|e| format!("couldn't put back {}: {e}", f.from.join(&path).display()))?;
            count += 1;
        }
    }
    crate::log::line(&format!("saves restored: {} ({count} files from {})", game.name, backup.dir.display()));
    Ok(count)
}

/// When a backup was taken, as a person reads it ("27 Oct 2026, 14:05").
pub fn time_text(t: u64) -> String {
    gtk::glib::DateTime::from_unix_local(t as i64).ok().and_then(|d| d.format("%-d %b %Y, %H:%M").ok()).map_or_else(|| t.to_string(), |s| s.to_string())
}

/// After a session: the game's saves backed up (by its id; failures only logged).
pub fn after_playing(game_id: &str) {
    let Some(game) = crate::store::Library::load().and_then(|lib| lib.games.into_iter().find(|g| g.id == game_id)) else { return };
    if let Err(e) = take(&game, "after playing") {
        crate::log::line(&format!("saves not backed up: {}: {e}", game.name));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_files_with_sizes() {
        let root = std::env::temp_dir().join(format!("lobby-backups-{}", std::process::id()));
        fs::create_dir_all(root.join("slot")).unwrap();
        fs::write(root.join("slot/1.sav"), "abc").unwrap();
        fs::write(root.join("options.ini"), "x").unwrap();
        let found: Vec<(PathBuf, u64)> = files(&root).into_iter().map(|(p, s, _)| (p, s)).collect();
        assert_eq!(found, [(PathBuf::from("options.ini"), 1), (PathBuf::from("slot/1.sav"), 3)]);
        let copied = root.join("copy/slot/1.sav");
        copy(&root.join("slot/1.sav"), &copied).unwrap();
        assert_eq!(fs::metadata(&copied).unwrap().modified().unwrap(), fs::metadata(root.join("slot/1.sav")).unwrap().modified().unwrap());
        fs::remove_dir_all(&root).unwrap();
    }
}
