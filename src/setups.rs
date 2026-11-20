//! Where games' Windows setups (Wine prefixes) are kept, the user's pick ("Windows Setups" in
//! the menu, `lobby windows-setups`): in one folder in the home (gamefix's
//! ~/.local/share/gamefix/prefixes/<game>, where it makes them), or beside each game, as
//! "<game's folder> (Windows setup)" next to its folder, so a game's settings and saves are on
//! its drive and go where it goes. After each scan the setups not where the pick says are moved
//! (a link left in gamefix's folder, which everything that names the setup keeps finding).
//!
//! Only setups gamefix made for a game run from a folder of its own: not an installer's
//! (the game is inside it, or it's already beside the game), Steam's, a store's or a
//! launcher's, one that several games share, a game on a Windows-format drive (Proton needs a
//! Linux one), nor a one-file game in a folder of downloads (nothing there is only its own).

use crate::library::{self, Game, Kind};
use crate::places::Setups;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const ENDING: &str = " (Windows setup)";

/// Where gamefix makes them.
fn central() -> PathBuf {
    crate::store::data_home().join("gamefix/prefixes")
}

/// A game folder's setup beside it, hidden (`hide`).
pub fn beside(folder: &Path) -> Option<PathBuf> {
    Some(folder.parent()?.join(format!(".{}{ENDING}", folder.file_name()?.to_string_lossy())))
}

/// A setup to move, and the game it's for.
pub struct Move {
    pub from: PathBuf,
    pub to: PathBuf,
    pub name: String,
    /// Back into the one folder: `to` is the link left there.
    back: bool,
}

fn device(p: &Path) -> Option<u64> {
    fs::metadata(p).ok().map(|m| std::os::unix::fs::MetadataExt::dev(&m))
}

/// The links in gamefix's folder and where each points.
fn links() -> Vec<(PathBuf, PathBuf)> {
    fs::read_dir(central()).into_iter().flatten().flatten().map(|e| e.path()).filter_map(|link| Some((fs::read_link(&link).ok()?, link))).map(|(target, link)| (target, link)).collect()
}

/// The setups that aren't where the pick says.
pub fn moves(games: &[Game], pick: Setups) -> Vec<Move> {
    let name_of = |p: &Path| p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    if pick == Setups::Central {
        return links().into_iter().filter(|(target, _)| name_of(target).ends_with(ENDING) && library::prefix_in(target).is_some()).map(|(from, to)| Move { name: name_of(&to), from, to, back: true }).collect();
    }
    let central = library::norm(&central());
    let mounts = crate::drives::mounts();
    let mut out: Vec<Move> = Vec::new();
    for g in games.iter().filter(|g| g.kind == Kind::Game && !g.gone && !g.missing) {
        let Some(from) = g.entries.iter().filter_map(|e| e.prefix.as_deref()).map(crate::import::unit).find(|p| fs::symlink_metadata(p).is_ok_and(|m| m.is_dir()) && library::norm(p).parent() == Some(central.as_path())) else { continue };
        let from = library::norm(&from);
        let Some(folder) = g.folder().map(library::norm) else { continue };
        if folder.starts_with(&from) || crate::uninstall::found_folder(g, &folder).is_err() || !crate::uninstall::prefix_shared_with(&from, g, games).is_empty() {
            continue;
        }
        if crate::drives::mount_of(&folder, &mounts).is_none_or(|m| m.windows_format() || m.read_only) {
            continue;
        }
        let Some(to) = beside(&folder).filter(|to| fs::symlink_metadata(to).is_err()) else { continue };
        if !out.iter().any(|m| m.from == from || m.to == to) {
            out.push(Move { from, to, name: g.name.clone(), back: false });
        }
    }
    out
}

/// Moves a folder: renamed on the same drive, else copied whole and only then deleted.
pub fn carry(from: &Path, to: &Path) -> Result<(), String> {
    if device(from) == to.parent().and_then(device) {
        return fs::rename(from, to).map_err(|e| e.to_string());
    }
    let tmp = to.with_file_name(format!(".lobby-moving-{}", to.file_name().unwrap_or_default().to_string_lossy()));
    let _ = fs::remove_dir_all(&tmp);
    let copied = Command::new("cp").args(["-a", "--reflink=auto"]).arg(from).arg(&tmp).stdin(Stdio::null()).status().is_ok_and(|s| s.success());
    if !copied || fs::rename(&tmp, to).is_err() {
        let _ = fs::remove_dir_all(&tmp);
        return Err(format!("it couldn't be copied to {}", to.display()));
    }
    fs::remove_dir_all(from).map_err(|e| format!("it's in {} now, but the old copy couldn't be deleted: {e}", to.display()))
}

fn move_one(m: &Move) -> Result<(), String> {
    if m.back {
        fs::remove_file(&m.to).map_err(|e| e.to_string())?;
        if let Err(e) = carry(&m.from, &m.to) {
            let _ = std::os::unix::fs::symlink(&m.from, &m.to);
            return Err(e);
        }
    } else {
        carry(&m.from, &m.to)?;
        std::os::unix::fs::symlink(&m.to, &m.from).map_err(|e| format!("it's in {} now, but no link could be left at {}: {e}", m.to.display(), m.from.display()))?;
    }
    crate::import::point_records(&m.from, &m.to)
}

/// A setup in plain view (no hidden folder on its path) that holds no game, to be given a
/// hidden name: a file search going into it follows its drive links (Z: is the whole system,
/// gamefix's D: to G: each drive) and so searches every drive. Dolphin's search never goes
/// into hidden folders. Only gamefix's setups (an installer's, with its install note) and
/// Lobby's beside a game: not Steam's, a store's or another launcher's, whose records Lobby
/// doesn't keep; not one with a game inside (hiding it would hide the game), nor in use.
pub fn to_hide(games: &[Game]) -> Vec<Move> {
    let mut out: Vec<Move> = Vec::new();
    let named: Vec<(PathBuf, &Game)> = games.iter().flat_map(|g| g.entries.iter().filter_map(|e| e.prefix.as_deref()).map(move |p| (library::norm(&crate::import::unit(p)), g))).collect();
    for (from, g) in &named {
        let Some(name) = from.file_name().map(|n| n.to_string_lossy().into_owned()) else { continue };
        if out.iter().any(|m| m.from == *from) || from.components().any(|c| c.as_os_str().to_string_lossy().starts_with('.')) {
            continue;
        }
        if !fs::symlink_metadata(from).is_ok_and(|m| m.is_dir()) || library::prefix_in(from).is_none() || !(from.join("gamefix-install.txt").is_file() || name.ends_with(ENDING)) {
            continue;
        }
        // Every game it's named for keeps its folder and program outside it.
        let inside = |p: &Path| library::norm(p).starts_with(from);
        if named.iter().filter(|(p, _)| p == from).any(|(_, h)| h.folder().is_none() || h.folder().is_some_and(inside) || h.entries.iter().filter_map(|e| e.program.as_deref()).any(inside)) {
            continue;
        }
        let to = from.with_file_name(format!(".{name}"));
        if fs::symlink_metadata(&to).is_err() {
            out.push(Move { from: from.clone(), to, name: g.name.clone(), back: false });
        }
    }
    out
}

/// Gives a setup its hidden name (renamed in place) and points everything naming it there:
/// gamefix's records, app-menu entries and Steam shortcuts (`setup::relocate`), the link in
/// gamefix's folder to a setup Lobby put beside its game, and Lobby's own files. Put back as
/// it was when gamefix's part can't be done (Steam open with a shortcut to change).
fn hide_one(m: &Move) -> Result<(), String> {
    fs::rename(&m.from, &m.to).map_err(|e| e.to_string())?;
    if let Err(e) = gamefix::setup::relocate(&m.from, &m.to) {
        let _ = fs::rename(&m.to, &m.from);
        return Err(e);
    }
    for (_, link) in links().into_iter().filter(|(target, _)| library::norm(target) == m.from || *target == m.from) {
        fs::remove_file(&link).and_then(|()| std::os::unix::fs::symlink(&m.to, &link)).map_err(|e| format!("{}: {e}", link.display()))?;
    }
    crate::import::point_records(&m.from, &m.to)
}

/// Moves the setups to where the pick says; says what it did (`dry`: what it would do). One
/// something runs in waits for the next time.
pub fn run(games: &[Game], dry: bool) -> Vec<String> {
    let pick = crate::places::Places::load().windows_setups;
    let mut done = Vec::new();
    for m in to_hide(games) {
        if dry {
            done.push(format!("{}: its Windows setup {} gets the hidden name {}.", m.name, m.from.display(), m.to.display()));
        } else if crate::import::in_use(&m.from) {
            done.push(format!("{}: its Windows setup is in use; it's hidden next time.", m.name));
        } else {
            match hide_one(&m) {
                Ok(()) => {
                    crate::log::line(&format!("windows setups: hidden {} → {} ({})", m.from.display(), m.to.display(), m.name));
                    done.push(format!("✓ {}: Windows setup hidden as {}.", m.name, m.to.display()));
                }
                Err(e) => {
                    crate::log::line(&format!("windows setups: {} not hidden: {e}", m.from.display()));
                    done.push(format!("{}: its Windows setup couldn't be hidden: {e}", m.name));
                }
            }
        }
    }
    // Hidden ones aren't moved again this time: the library still names the old places.
    if done.iter().any(|l| l.starts_with('✓')) && !dry {
        return done;
    }
    for m in moves(games, pick) {
        if dry {
            done.push(format!("{}: its Windows setup {} moves to {}.", m.name, m.from.display(), m.to.display()));
        } else if crate::import::in_use(&m.from) {
            done.push(format!("{}: its Windows setup is in use; it moves next time.", m.name));
        } else {
            match move_one(&m) {
                Ok(()) => {
                    crate::log::line(&format!("windows setups: {} → {} ({})", m.from.display(), m.to.display(), m.name));
                    done.push(format!("✓ {}: Windows setup moved to {}.", m.name, m.to.display()));
                }
                Err(e) => {
                    crate::log::line(&format!("windows setups: {} not moved: {e}", m.from.display()));
                    done.push(format!("{}: its Windows setup couldn't be moved: {e}", m.name));
                }
            }
        }
    }
    done
}

/// A game's folder went from `from` to `to`, its setup beside it too (moved by hand with it, or
/// by `take_along`): the link in gamefix's folder and Lobby's records point at it again.
/// Whether anything changed.
pub fn follow(from: &Path, to: &Path) -> bool {
    let (Some(old), Some(new)) = (beside(from), beside(to)) else { return false };
    if old.exists() || library::prefix_in(&new).is_none() {
        return false;
    }
    let mut changed = false;
    for (_, link) in links().into_iter().filter(|(target, _)| *target == old || library::norm(target) == library::norm(&old)) {
        if fs::remove_file(&link).and_then(|()| std::os::unix::fs::symlink(&new, &link)).is_ok() {
            crate::log::line(&format!("windows setups: {} now points at {}", link.display(), new.display()));
            changed = true;
        }
    }
    let _ = crate::import::point_records(&old, &new);
    changed
}

/// Lobby moved a game's folder: the setup beside it goes along (not onto a Windows-format
/// drive, where it stays and still works). What it did, if anything.
pub fn take_along(from: &Path, to: &Path) -> Option<String> {
    let (old, new) = (beside(from)?, beside(to)?);
    if fs::symlink_metadata(&old).is_ok_and(|m| !m.is_dir()) || library::prefix_in(&old).is_none() || fs::symlink_metadata(&new).is_ok() {
        return None;
    }
    let mounts = crate::drives::mounts();
    if crate::drives::mount_of(to, &mounts).is_none_or(|m| m.windows_format() || m.read_only) {
        return Some(format!("Its Windows setup stays in {}: Proton needs it on a Linux-format drive.", old.display()));
    }
    Some(match carry(&old, &new) {
        Ok(()) => {
            follow(from, to);
            format!("Moved its Windows setup to {}.", new.display())
        }
        Err(e) => format!("Its Windows setup stays in {}: {e}.", old.display()),
    })
}

/// A setup was deleted (Uninstall): the links to it in gamefix's folder go too.
pub fn forget(setup: &Path) {
    for (_, link) in links().into_iter().filter(|(target, _)| target == setup && !target.exists()) {
        let _ = fs::remove_file(link);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_setup_is_named_after_the_games_folder() {
        assert_eq!(beside(Path::new("/run/media/p/Games/Cookie Clicker")), Some(PathBuf::from("/run/media/p/Games/.Cookie Clicker (Windows setup)")));
        assert_eq!(beside(Path::new("/")), None);
    }

    #[test]
    fn a_folder_is_carried_whole() {
        let dir = std::env::temp_dir().join(format!("lobby-setups-{}", std::process::id()));
        fs::create_dir_all(dir.join("a/drive_c/users")).unwrap();
        fs::write(dir.join("a/drive_c/users/save.dat"), "1").unwrap();
        carry(&dir.join("a"), &dir.join("b (Windows setup)")).unwrap();
        assert!(!dir.join("a").exists());
        assert_eq!(fs::read_to_string(dir.join("b (Windows setup)/drive_c/users/save.dat")).unwrap(), "1");
        fs::remove_dir_all(&dir).unwrap();
    }
}
