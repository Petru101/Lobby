//! Uninstalling a game lobby (or gamefix) put on the computer: a store game lobby installed or
//! took over from Heroic, or a game lobby unpacked or gamefix installed into the games folder.
//! Its files go, gamefix's setup for it is undone (app-menu entry, Steam shortcut), and, when
//! asked, its Wine prefix: never one another game runs in, and only after its saves are in the
//! store's cloud when the store keeps them. Steam's games are uninstalled by Steam.
//! Games other launchers installed are theirs to uninstall; folders lobby only found aren't
//! deleted (they may be anything).

use crate::library::{Game, Key, Kind};
use crate::stores::saves::{self, Direction};
use crate::stores::Store;
use std::fs;
use std::path::{Path, PathBuf};

pub enum How {
    /// Lobby's own install from a store.
    Store(Store, String),
    /// Steam's game: Steam's own uninstall.
    Steam(u32),
    /// A folder lobby unpacked a download into, or gamefix installed a game into.
    Folder(PathBuf),
    /// Not lobby's to uninstall: why.
    Not(String),
}

pub fn how(game: &Game) -> How {
    if let Some((store, id)) = crate::stores::installed_in(game) {
        return How::Store(store, id);
    }
    let keys = || game.entries.iter().flat_map(|e| e.keys.iter());
    if let Some(id) = keys().find_map(|k| if let Key::SteamApp(id) = k { Some(*id) } else { None }) {
        return How::Steam(id);
    }
    let made = |f: &Path| f.join(crate::install::SOURCE_NOTE).is_file() || f.join("gamefix-install.txt").is_file();
    if let Some(folder) = game.entries.iter().filter_map(|e| e.folder.clone().or_else(|| e.program.as_deref().and_then(Path::parent).map(Path::to_path_buf))).find(|f| made(f)) {
        return How::Folder(folder);
    }
    let launcher = game.entries.iter().find(|e| matches!(e.source, crate::library::Source::Heroic | crate::library::Source::Lutris | crate::library::Source::Faugus)).map(|e| e.source.label());
    How::Not(match launcher {
        Some(l) => format!("{l} installed it: uninstall it there."),
        None => "Lobby found it on your drive but didn't install it, so it doesn't delete it: delete its folder yourself if you're sure.".into(),
    })
}

/// The Wine prefix the game runs in (from how lobby starts it), when lobby knows one.
pub fn prefix(game: &Game) -> Option<PathBuf> {
    let wine = crate::launch::wine_command(game)?;
    saves::prefix_of(&wine.env).map(|p| real(&p))
}

fn real(p: &Path) -> PathBuf {
    fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

/// The other games in the library that run in the prefix too (it's kept for them).
pub fn prefix_shared_with(prefix: &Path, game: &Game, all: &[Game]) -> Vec<String> {
    all.iter()
        .filter(|g| g.id != game.id && !g.gone && g.kind == Kind::Game)
        .filter(|g| g.entries.iter().filter_map(|e| e.prefix.as_deref()).map(real).any(|p| p == prefix) || self::prefix(g).as_deref() == Some(prefix))
        .map(|g| g.name.clone())
        .collect()
}

/// Whether a folder is a Wine prefix lobby may delete: it holds drive_c (or is still empty:
/// made for a game never played) and isn't a folder of prefixes or the home folder.
fn deletable_prefix(p: &Path) -> bool {
    let home = std::env::var_os("HOME").map(|h| real(Path::new(&h)));
    let empty = fs::read_dir(p).is_ok_and(|mut d| d.next().is_none());
    (p.join("drive_c").is_dir() || empty) && p.components().count() > 4 && Some(p.to_path_buf()) != home && !p.ends_with("Prefixes") && !p.ends_with("prefixes")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefixes_to_delete() {
        let dir = std::env::temp_dir().join(format!("lobby-uninstall-{}", std::process::id()));
        let (own, empty, other, prefixes) = (dir.join("a/b/game"), dir.join("a/b/new"), dir.join("a/b/stuff"), dir.join("a/b/Prefixes"));
        for d in [own.join("drive_c"), empty.clone(), other.join("saves"), prefixes.join("drive_c")] {
            fs::create_dir_all(d).unwrap();
        }
        assert!(deletable_prefix(&own));
        assert!(deletable_prefix(&empty));
        assert!(!deletable_prefix(&other));
        assert!(!deletable_prefix(&prefixes));
        fs::remove_dir_all(&dir).unwrap();
    }
}

/// gamefix's record of its setup for the game: the one that made its app-menu entry.
fn gamefix_record(game: &Game) -> Option<PathBuf> {
    let entries: Vec<&PathBuf> = game.entries.iter().flat_map(|e| e.keys.iter()).filter_map(|k| if let Key::Desktop(p) = k { Some(p) } else { None }).collect();
    let dir = PathBuf::from(std::env::var_os("HOME")?).join(".local/share/gamefix/games");
    fs::read_dir(dir).ok()?.flatten().map(|e| e.path()).find(|record| {
        fs::read_to_string(record).unwrap_or_default().lines().any(|l| l.strip_prefix("created\t").is_some_and(|f| entries.iter().any(|e| e.as_path() == Path::new(f))))
    })
}

/// Undoes gamefix's setup (its app-menu entry, Steam shortcut, icons). gamefix finds its
/// record by the game's name, so the game is given the record's name.
fn undo_gamefix(game: &Game, folder: &Path, done: &mut Vec<String>) -> Result<(), String> {
    let Some(record) = crate::log::timed("uninstall: finding gamefix's record", || gamefix_record(game)) else { return Ok(()) };
    let Some(mut g) = crate::log::timed("uninstall: gamefix scan", || gamefix::scan::scan(folder)) else { return Ok(()) };
    g.name = record.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let undone = crate::log::timed("uninstall: gamefix's undo", || gamefix::setup::undo(&g))?;
    done.extend(undone.into_iter().filter(|l| !l.contains("Wine prefix")).map(|l| format!("✓ {l}")));
    let _ = fs::remove_file(&record);
    Ok(())
}

/// Uninstalls the game; with `with_prefix`, its prefix too when no other game runs in it.
/// Returns what it did, a line each.
pub fn uninstall(game: &Game, all: &[Game], with_prefix: bool) -> Result<Vec<String>, String> {
    crate::log::line(&format!("uninstall {}", game.name));
    let mut done = Vec::new();
    let how = crate::log::timed("uninstall: what kind of install", || how(game));
    let prefix = crate::log::timed("uninstall: its prefix", || prefix(game));
    let folder = match &how {
        How::Store(store, id) => store.installed().into_iter().find(|g| &g.id == id).map(|g| g.folder),
        How::Folder(f) => Some(f.clone()),
        How::Steam(id) => {
            crate::launch::start(&["steam".into(), format!("steam://uninstall/{id}")], None, &crate::store::log_file(&game.id))?;
            return Ok(vec!["Steam asks you to confirm, then uninstalls it.".into()]);
        }
        How::Not(why) => return Err(why.clone()),
    };

    // The prefix goes only when nothing else uses it, and once its saves are safe in the cloud
    // (when the store keeps them).
    let mut delete_prefix = None;
    if let (true, Some(p)) = (with_prefix, &prefix) {
        let shared = crate::log::timed("uninstall: other games in its prefix", || prefix_shared_with(p, game, all));
        if !shared.is_empty() {
            done.push(format!("Kept its Wine prefix {}: {} run in it too.", p.display(), shared.join(", ")));
        } else if !deletable_prefix(p) {
            done.push(format!("Kept {}: it doesn't look like a game's own Wine prefix.", p.display()));
        } else {
            let mut safe = true;
            if let How::Store(store, id) = &how {
                if crate::log::timed("uninstall: cloud saves?", || saves::supported(*store, id)) {
                    let env = crate::launch::wine_command(game).map(|w| w.env).unwrap_or_default();
                    match crate::log::timed("uninstall: saves up", || saves::sync(*store, id, &env, Direction::Up)) {
                        Ok(saves::Synced::Conflict) => {
                            safe = false;
                            done.push(format!("Kept its Wine prefix {}: its saves there and in {}'s cloud both changed since the last sync. Play it once from lobby to choose which to keep.", p.display(), store.name()));
                        }
                        Ok(_) => done.push(format!("✓ Its saves are in {}'s cloud.", store.name())),
                        Err(e) => {
                            safe = false;
                            done.push(format!("Kept its Wine prefix {}: its saves couldn't be put in the cloud ({e}).", p.display()));
                        }
                    }
                }
            }
            if safe {
                delete_prefix = Some(p.clone());
            }
        }
    }

    if let Some(folder) = &folder {
        crate::log::timed("uninstall: gamefix undo", || undo_gamefix(game, folder, &mut done))?;
    }
    match &how {
        How::Store(store, id) => {
            crate::log::timed("uninstall: store's uninstall", || store.uninstall(id))?;
            done.push(format!("✓ Removed its files ({}).", folder.as_ref().map_or_else(String::new, |f| f.display().to_string())));
        }
        How::Folder(f) => {
            crate::log::timed("uninstall: deleting the folder", || fs::remove_dir_all(f)).map_err(|e| format!("Couldn't delete {}: {e}", f.display()))?;
            done.push(format!("✓ Deleted {}.", f.display()));
        }
        _ => {}
    }
    if let Some(p) = delete_prefix {
        crate::log::timed("uninstall: deleting the prefix", || fs::remove_dir_all(&p)).map_err(|e| format!("Couldn't delete its Wine prefix {}: {e}", p.display()))?;
        done.push(format!("✓ Deleted its Wine prefix {} (its settings and saves on this computer).", p.display()));
    } else if let (false, Some(p)) = (with_prefix, &prefix) {
        done.push(format!("Kept its Wine prefix {} (its settings and saves).", p.display()));
    }
    Ok(done)
}
