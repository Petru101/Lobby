//! Downloads whose game is installed: the archive, installer, disc image or repack folder a
//! game came from, still on the drive. Lobby offers to delete them (a game's page, the clean-up
//! list on the Downloads page, after Install; `lobby leftovers`). Where to get one again: an
//! itch.io game's page, or MyAbandonware by its files' naming (`Black-White_Win_EN_ISO.zip`);
//! anything else Lobby can't say, and those start unticked.
//!
//! Deleted to the Trash where the drive has one (GIO's access::can-trash: not on a drive
//! mounted at boot, like an NTFS Downloads drive in fstab), else for good: the window says
//! which before asking.

use crate::library::{self, Game, Key, Kind};
use gtk::gio;
use gtk::prelude::*;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq)]
pub enum Again {
    /// The game's itch.io page.
    Itch(String),
    /// A search for it on MyAbandonware.
    MyAbandonware(String),
    Unknown,
}

impl Again {
    pub fn describe(&self) -> &'static str {
        match self {
            Again::Itch(_) => "on itch.io",
            Again::MyAbandonware(_) => "on MyAbandonware",
            Again::Unknown => "where it's from: unknown",
        }
    }

    pub fn url(&self) -> Option<&str> {
        match self {
            Again::Itch(u) | Again::MyAbandonware(u) => Some(u),
            Again::Unknown => None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Leftover {
    pub id: String,
    pub name: String,
    /// The file, or the download's own folder.
    pub path: PathBuf,
    pub bytes: u64,
    pub again: Again,
    /// Its drive has a Trash: else deleting is for good.
    pub trash: bool,
}

/// MyAbandonware's file naming: `Title-With-Dashes_Platform_Language[_Version]`
/// (`Black-White_Win_EN_ISO-Version.zip`, `Pac-Man_PC-88_EN.zip`,
/// `Harry-Potter-and-the-Sorcerer-s-Stone_Win_EN-ES_installer`): the search for its title.
pub fn myabandonware(name: &str) -> Option<String> {
    let stem = name.split('.').next().unwrap_or(name);
    let parts: Vec<&str> = stem.split('_').collect();
    let platform = parts.get(1)?;
    let language = parts.get(2)?;
    let platform_like = platform.chars().next().is_some_and(|c| c.is_ascii_uppercase()) && platform.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
    let language_like = language.split('-').all(|l| l.len() == 2 && l.chars().all(|c| c.is_ascii_uppercase()));
    if parts[0].is_empty() || !platform_like || !language_like {
        return None;
    }
    let words: Vec<&str> = parts[0].split('-').filter(|w| !w.is_empty()).collect();
    Some(format!("https://www.myabandonware.com/search/q/{}", words.join("+")))
}

/// Every download in the library whose game is installed, safe to delete: not the installed
/// game's files, not holding them or another game's, not a folder of many things.
pub fn find(games: &[Game]) -> Vec<Leftover> {
    let home = crate::sources::Home::find();
    let roots: Vec<PathBuf> = home.as_ref().map(|h| crate::sources::folders::roots(h).iter().map(|r| library::norm(r)).collect()).unwrap_or_default();
    let home = home.map(|h| library::norm(&h.home));
    // Everything installed (any game's), which a download must neither be in nor hold.
    // Only files still on the drive: a game whose files are gone (Carnival Zero here) has its
    // download as the only copy.
    let installed: Vec<(String, PathBuf)> = games
        .iter()
        .filter(|g| !g.gone && !g.missing)
        .flat_map(|g| g.entries.iter().filter(|e| !matches!(e.kind, Kind::Download | Kind::Owned)).flat_map(move |e| [e.program.as_deref(), e.folder.as_deref()].into_iter().flatten().filter(|p| p.exists()).map(move |p| (g.id.clone(), library::norm(p)))))
        .collect();
    let mut out: Vec<Leftover> = Vec::new();
    for g in games.iter().filter(|g| g.kind == Kind::Game && !g.gone) {
        if !installed.iter().any(|(id, _)| *id == g.id) {
            continue;
        }
        let page = g.entries.iter().flat_map(|e| &e.keys).find_map(|k| if let Key::Itch(p) = k { Some(p.clone()) } else { None });
        for e in g.entries.iter().filter(|e| e.kind == Kind::Download) {
            let Some(program) = e.program.as_deref().map(library::norm) else { continue };
            let path = match e.folder.as_deref().map(library::norm) {
                // A folder download: that folder, and the folders above holding only it (an
                // archive's folder around the installer's).
                Some(folder) if program.starts_with(&folder) => {
                    let mut at = folder;
                    while let Some(up) = at.parent().map(Path::to_path_buf) {
                        let alone = fs::read_dir(&up).map(|d| d.flatten().count() == 1).unwrap_or(false);
                        if !alone || wide(&up, &roots, home.as_deref()) {
                            break;
                        }
                        at = up;
                    }
                    at
                }
                _ => program,
            };
            if !path.exists() || out.iter().any(|l| l.path == path) || (path.is_dir() && wide(&path, &roots, home.as_deref())) {
                continue;
            }
            // Never anything installed: in it (Lisparuga's source tarball is in the game's own
            // folder), or holding it.
            if installed.iter().any(|(_, p)| p.starts_with(&path) || path.starts_with(p)) {
                continue;
            }
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let again = match (&page, myabandonware(&name)) {
                (Some(p), _) => Again::Itch(p.clone()),
                (None, Some(search)) => Again::MyAbandonware(search),
                (None, None) => Again::Unknown,
            };
            let bytes = if path.is_dir() { crate::sizes::walk(&path) } else { fs::metadata(&path).map(|m| m.len()).unwrap_or(0) };
            out.push(Leftover { id: g.id.clone(), name: g.name.clone(), trash: can_trash(&path), path, bytes, again });
        }
    }
    out.sort_by(|a, b| b.bytes.cmp(&a.bytes));
    out
}

/// A folder of many things, never one download's: the home folder or above, a drive's top, a
/// folder Lobby looks in, one named like downloads or games.
fn wide(dir: &Path, roots: &[PathBuf], home: Option<&Path>) -> bool {
    let named = dir.file_name().is_some_and(|n| crate::places::games_or_downloads(&n.to_string_lossy()));
    let dev = |p: &Path| fs::metadata(p).map(|m| std::os::unix::fs::MetadataExt::dev(&m)).ok();
    let drive_top = dir.parent().is_none_or(|up| dev(up) != dev(dir));
    named || drive_top || roots.iter().any(|r| r == dir) || home.is_some_and(|h| h.starts_with(dir))
}

fn can_trash(path: &Path) -> bool {
    gio::File::for_path(path).query_info("access::can-trash", gio::FileQueryInfoFlags::NOFOLLOW_SYMLINKS, gio::Cancellable::NONE).is_ok_and(|i| i.boolean("access::can-trash"))
}

/// Deletes one, only if it's still one of the library's leftovers (checked again now): to the
/// Trash when its drive has one, else for good. Returns whether it went to the Trash.
pub fn delete(path: &Path, games: &[Game]) -> Result<bool, String> {
    let Some(l) = find(games).into_iter().find(|l| l.path == path) else {
        return Err(format!("{} isn't a download of an installed game any more.", path.display()));
    };
    remove(&l, l.trash)
}

fn remove(l: &Leftover, to_trash: bool) -> Result<bool, String> {
    if to_trash {
        gio::File::for_path(&l.path).trash(gio::Cancellable::NONE).map_err(|e| format!("{}: {e}", l.path.display()))?;
        crate::log::line(&format!("leftovers: to the Trash: {}", l.path.display()));
        return Ok(true);
    }
    let gone = if l.path.is_dir() { fs::remove_dir_all(&l.path) } else { fs::remove_file(&l.path) };
    gone.map_err(|e| format!("{}: {e}", l.path.display()))?;
    crate::log::line(&format!("leftovers: deleted {}", l.path.display()));
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::{Entry, Source};

    #[test]
    fn myabandonware_names() {
        assert_eq!(myabandonware("Black-White_Win_EN_ISO-Version.zip").as_deref(), Some("https://www.myabandonware.com/search/q/Black+White"));
        assert_eq!(myabandonware("Pac-Man_PC-88_EN.zip").as_deref(), Some("https://www.myabandonware.com/search/q/Pac+Man"));
        assert!(myabandonware("Harry-Potter-and-the-Sorcerer-s-Stone_Win_EN-ES_installer").is_some());
        assert_eq!(myabandonware("NecroLore_Apeirophobia.zip"), None);
        assert_eq!(myabandonware("Project_P.I.T.T_DEMO_v0.2.8.1_linux.zip"), None);
        assert_eq!(myabandonware("mobility-windows.zip"), None);
    }

    fn game(id: &str, entries: Vec<Entry>) -> Game {
        Game { id: id.into(), name: id.into(), kind: Kind::Game, entries, missing: false, gone: false, added: 0, played: None, chosen_cover: None, launch_options: None, protection: Default::default(), hidden: false, favourite: false, frame_rate: false }
    }

    #[test]
    fn what_can_go() {
        let dir = std::env::temp_dir().join(format!("lobby-leftovers-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        // A loose zip, a repack folder holding only its setup, the installed games, and a
        // tarball inside a game's own folder.
        let drop = dir.join("drop");
        fs::create_dir_all(drop.join("Repack [X]")).unwrap();
        fs::write(drop.join("Black-White_Win_EN_ISO.zip"), b"zip").unwrap();
        fs::write(drop.join("Repack [X]/setup.exe"), b"setup").unwrap();
        fs::create_dir_all(dir.join("installed/bw")).unwrap();
        fs::write(dir.join("installed/bw/bw.exe"), b"x").unwrap();
        fs::create_dir_all(dir.join("installed/repack")).unwrap();
        fs::write(dir.join("installed/repack/game.exe"), b"x").unwrap();
        fs::create_dir_all(dir.join("installed/own/source")).unwrap();
        fs::write(dir.join("installed/own/play.sh"), b"x").unwrap();
        fs::write(dir.join("installed/own/source/own.tar.gz"), b"x").unwrap();
        let entry = |kind: Kind, folder: Option<PathBuf>, program: PathBuf| {
            let mut e = Entry::new(Source::Folder, "x");
            e.kind = kind;
            e.folder = folder;
            e.program = Some(program);
            e
        };
        let games = vec![
            game("bw", vec![entry(Kind::Game, Some(dir.join("installed/bw")), dir.join("installed/bw/bw.exe")), entry(Kind::Download, None, drop.join("Black-White_Win_EN_ISO.zip"))]),
            game("repack", vec![entry(Kind::Game, Some(dir.join("installed/repack")), dir.join("installed/repack/game.exe")), entry(Kind::Download, Some(drop.join("Repack [X]")), drop.join("Repack [X]/setup.exe"))]),
            game("own", vec![entry(Kind::Game, Some(dir.join("installed/own")), dir.join("installed/own/play.sh")), entry(Kind::Download, Some(dir.join("installed/own")), dir.join("installed/own/source/own.tar.gz"))]),
            // Not installed: its download stays.
            game("later", vec![entry(Kind::Download, None, drop.join("Black-White_Win_EN_ISO.zip"))]),
        ];
        let found = find(&games);
        let paths: Vec<&Path> = found.iter().map(|l| l.path.as_path()).collect();
        assert_eq!(found.len(), 2, "{paths:?}");
        let bw = found.iter().find(|l| l.id == "bw").unwrap();
        assert_eq!(bw.path, library::norm(&drop.join("Black-White_Win_EN_ISO.zip")));
        assert!(matches!(bw.again, Again::MyAbandonware(_)));
        let repack = found.iter().find(|l| l.id == "repack").unwrap();
        assert_eq!(repack.path, library::norm(&drop.join("Repack [X]")));
        assert_eq!(repack.again, Again::Unknown);
        // Deleting: the folder goes (for good here: a test mustn't fill the user's Trash);
        // something that isn't a leftover is refused.
        remove(repack, false).unwrap();
        assert!(!drop.join("Repack [X]").exists());
        assert!(delete(&dir.join("installed/own/source/own.tar.gz"), &games).is_err());
        assert!(dir.join("installed/own/source/own.tar.gz").exists());
        let _ = fs::remove_dir_all(&dir);
    }
}
