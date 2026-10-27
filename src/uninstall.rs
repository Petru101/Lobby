//! Uninstalling a game lobby (or gamefix) put on the computer: a store game lobby installed or
//! took over from Heroic, or a game lobby unpacked or gamefix installed into the games folder.
//! Its files go, gamefix's setup for it is undone (app-menu entry, Steam shortcut), and, when
//! asked, its Wine prefix: never one another game runs in, and only after its saves are in the
//! store's cloud when the store keeps them. Steam's games are uninstalled by Steam.
//! Games other launchers installed are theirs to uninstall; folders lobby only found aren't
//! deleted (they may be anything).

use crate::library::{Game, Key, Kind, Source};
use crate::stores::saves::{self, Direction};
use crate::stores::Store;
use std::fs;
use std::path::{Path, PathBuf};

pub enum How {
    /// Lobby's own install from a store.
    Store(Store, String),
    /// Steam's game: Steam's own uninstall.
    Steam(u32),
    /// A copy in a Steam library Steam doesn't use: its folder and its app manifest.
    SteamCopy(PathBuf, u32),
    /// A folder lobby unpacked a download into, or gamefix installed a game into.
    Folder(PathBuf),
    /// A game lobby found on a drive: its own folder (checked by `found_folder`).
    Found(PathBuf),
    /// Not lobby's to uninstall: why.
    Not(String),
}

pub fn how(game: &Game) -> How {
    if let Some((store, id)) = crate::stores::installed_in(game) {
        return How::Store(store, id);
    }
    let steam_id = |source: Source| game.entries.iter().filter(|e| e.source == source && e.kind != Kind::Owned).flat_map(|e| e.keys.iter()).find_map(|k| if let Key::SteamApp(id) = k { Some(*id) } else { None });
    if let Some(id) = steam_id(Source::Steam) {
        return How::Steam(id);
    }
    if let (Some(id), Some(folder)) = (steam_id(Source::SteamElsewhere), game.entries.iter().find(|e| e.source == Source::SteamElsewhere).and_then(|e| e.folder.clone())) {
        return How::SteamCopy(folder, id);
    }
    // The folder lobby unpacked into or gamefix installed into: the game's folder or one up to
    // three above it (an archive's top folder holding the game's own: MoleTest/Mole Test/).
    let made = |f: &Path| f.join(crate::install::SOURCE_NOTE).is_file() || f.join("gamefix-install.txt").is_file();
    let places = game.entries.iter().flat_map(|e| [e.folder.clone(), e.program.as_deref().and_then(Path::parent).map(Path::to_path_buf)]).flatten();
    if let Some(folder) = places.flat_map(|f| f.ancestors().take(4).map(Path::to_path_buf).collect::<Vec<_>>()).find(|f| made(f)) {
        return How::Folder(folder);
    }
    // gamefix ran its installer: the note is in the game's Wine prefix, the game itself maybe
    // on another drive the installer was pointed at.
    if let Some(p) = game.entries.iter().filter_map(|e| e.prefix.clone()).chain(prefix(game)).find(|p| p.join("gamefix-install.txt").is_file()) {
        return How::Folder(p);
    }
    // Found on a drive (set up by gamefix or not), or put there by Lutris, Faugus or Heroic
    // (Lobby starts those itself and never hands them back): its own folder, when that's
    // plainly the one game's.
    let launchers = game.entries.iter().any(|e| matches!(e.source, crate::library::Source::Heroic | crate::library::Source::Lutris | crate::library::Source::Faugus));
    if game.unknown_to_launchers() || launchers {
        if let Some(folder) = game.folder().map(Path::to_path_buf) {
            return match found_folder(game, &folder) {
                Ok(()) => How::Found(folder),
                Err(what) => How::Not(format!("Lobby found it on your drive in {}, {what}, so Uninstall can't tell what's the game's: delete its files yourself.", folder.display())),
            };
        }
        // A game in one file at the top of a folder lobby looks in (Downloads/MageBattle.exe).
        if let Some(file) = game.program().filter(|p| p.is_file()).map(Path::to_path_buf) {
            let in_root = crate::sources::Home::find().is_some_and(|home| crate::sources::folders::roots(&home).iter().any(|r| file.parent().is_some_and(|up| real(up) == real(r))));
            if in_root {
                return How::Found(file);
            }
        }
    }
    let menu = || game.entries.iter().filter(|e| e.source == crate::library::Source::AppMenu);
    if let Some(app) = menu().find_map(crate::sources::flatpak_app) {
        return How::Not(format!("It's a Flatpak app: uninstall it in your software app (Discover), or with flatpak uninstall {app}."));
    }
    if menu().flat_map(|e| e.keys.iter()).any(|k| matches!(k, Key::Desktop(p) if p.starts_with("/usr"))) {
        return How::Not("It came with your system or was installed as a system package: remove it with your software app.".into());
    }
    How::Not(if menu().next().is_some() && !launchers {
        "An app-menu entry Lobby didn't make starts it, so Lobby can't tell what's the game's: delete its files and that entry yourself.".into()
    } else {
        "Lobby can't tell which files are the game's: delete them yourself.".into()
    })
}

/// Whether a folder is plainly the game's own (safe to delete or move as that game's): its
/// program is in it, and it isn't a drive's top, the home folder or one above it, a folder
/// lobby looks in (Downloads, Games ...), nor one holding another game of the library. Err:
/// what it is instead ("the top of a drive").
pub(crate) fn found_folder(game: &Game, folder: &Path) -> Result<(), String> {
    let refuse = |what: &str| Err(what.to_string());
    let dir = real(folder);
    if !dir.is_dir() {
        return refuse("which isn't there");
    }
    if game.program().is_none_or(|p| !real(p).starts_with(&dir)) {
        return refuse("which doesn't hold its program");
    }
    let dev = |p: &Path| fs::metadata(p).map(|m| std::os::unix::fs::MetadataExt::dev(&m)).ok();
    if dir.parent().is_none_or(|up| dev(up) != dev(&dir)) {
        return refuse("the top of a drive");
    }
    if let Some(home) = crate::sources::Home::find() {
        if real(&home.home).starts_with(&dir) {
            return refuse("which holds your home folder");
        }
        if crate::sources::folders::roots(&home).iter().any(|r| real(r) == dir) {
            return refuse("one of the folders Lobby looks in for games");
        }
    }
    // A folder of downloads or games (lobby took Downloads/Downloaded Games for one game: a
    // loose .exe at its top).
    let name = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    if crate::places::games_or_downloads(&name) {
        return refuse("which is named like a folder of downloads or games");
    }
    let packed = |n: &str| [".zip", ".rar", ".7z", ".iso", ".tar", ".gz", ".xz"].iter().any(|x| n.ends_with(x)) || ((n.contains("setup") || n.contains("install")) && n.ends_with(".exe"));
    let archives = fs::read_dir(&dir).into_iter().flatten().flatten().filter(|e| packed(&e.file_name().to_string_lossy().to_lowercase())).count();
    if archives >= 2 {
        return refuse(&format!("which holds {archives} archives or installers"));
    }
    let others = crate::store::Library::load().map(|l| l.games).unwrap_or_default();
    if let Some(other) = others.iter().filter(|g| g.id != game.id && !g.gone).find(|g| g.entries.iter().flat_map(|e| [e.folder.as_deref(), e.program.as_deref()]).flatten().any(|p| real(p).starts_with(&dir))) {
        return refuse(&format!("which holds {} too", other.name));
    }
    Ok(())
}

/// For a gamefix install (its note in `folder`) whose program is outside it, on another
/// drive: the game's own folder there, the one holding the installer's uninstaller (Inno
/// Setup's unins000.exe, NSIS's uninstall.exe), else Err(the program).
fn installed_elsewhere(folder: &Path) -> Option<Result<PathBuf, PathBuf>> {
    let note = fs::read_to_string(folder.join("gamefix-install.txt")).ok()?;
    let program = PathBuf::from(note.lines().find_map(|l| l.strip_prefix("program\t"))?);
    if !program.is_absolute() || program.starts_with(folder) {
        return None;
    }
    let uninstaller = |dir: &Path| {
        fs::read_dir(dir).into_iter().flatten().flatten().any(|e| {
            let n = e.file_name().to_string_lossy().to_lowercase();
            n.ends_with(".exe") && (n.starts_with("unins") || n.starts_with("uninstall"))
        })
    };
    let dev = |p: &Path| fs::metadata(p).map(|m| std::os::unix::fs::MetadataExt::dev(&m)).ok();
    // Up from the program, never to a drive's top (a folder whose parent is another device).
    let found = program.ancestors().skip(1).take(6).take_while(|d| d.parent().is_some_and(|up| dev(up) == dev(d))).find(|d| uninstaller(d));
    Some(found.map(Path::to_path_buf).ok_or(program))
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
    // Never a store app's (Battle.net's, the EA app's ...): every game of that store runs there.
    let store_app = real(p) == real(&crate::battlenet::prefix()) || crate::clients::CLIENTS.iter().any(|c| real(p) == real(&c.prefix()));
    (p.join("drive_c").is_dir() || empty) && p.components().count() > 4 && Some(p.to_path_buf()) != home && !p.ends_with("Prefixes") && !p.ends_with("prefixes") && !store_app
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn games_installed_on_another_drive() {
        let dir = std::env::temp_dir().join(format!("lobby-elsewhere-{}", std::process::id()));
        let (prefix, game) = (dir.join("Games/Borderlands 4"), dir.join("Storage/Games/Borderlands 4"));
        fs::create_dir_all(prefix.join("drive_c")).unwrap();
        fs::create_dir_all(game.join("OakGame/Binaries/Win64")).unwrap();
        let program = game.join("OakGame/Binaries/Win64/Borderlands4.exe");
        fs::write(&program, "").unwrap();
        fs::write(prefix.join("gamefix-install.txt"), format!("source\t/d/setup.exe\nprogram\t{}\nname\tBorderlands 4\n", program.display())).unwrap();
        assert_eq!(installed_elsewhere(&prefix), Some(Err(program.clone())));
        fs::write(game.join("unins000.exe"), "").unwrap();
        assert_eq!(installed_elsewhere(&prefix), Some(Ok(game.clone())));
        fs::write(prefix.join("gamefix-install.txt"), "source\t/d/setup.exe\nprogram\tdrive_c/Games/X/x.exe\n").unwrap();
        assert_eq!(installed_elsewhere(&prefix), None);
        fs::remove_dir_all(&dir).unwrap();
    }

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
pub fn gamefix_record(game: &Game) -> Option<PathBuf> {
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
        How::Folder(f) | How::SteamCopy(f, _) | How::Found(f) => Some(f.clone()),
        How::Steam(id) => {
            crate::launch::start(&["steam".into(), format!("steam://uninstall/{id}")], None, &crate::store::log_file(&game.id))?;
            return Ok(vec!["Steam asks you to confirm, then uninstalls it.".into()]);
        }
        How::Not(why) => return Err(why.clone()),
    };
    match crate::log::timed("uninstall: backing up its saves", || crate::backups::take(game, "before uninstalling")) {
        Ok(Some(b)) => done.push(format!("✓ Backed up its saves ({} files) in {}.", b.files, b.dir.display())),
        Ok(None) => {}
        Err(e) => done.push(format!("Its saves weren't backed up: {e}.")),
    }

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
            // A game gamefix's installer run put on another drive: its own folder there.
            match installed_elsewhere(f) {
                Some(Ok(dir)) => {
                    crate::log::timed("uninstall: deleting the game's folder", || fs::remove_dir_all(&dir)).map_err(|e| format!("Couldn't delete {}: {e}", dir.display()))?;
                    done.push(format!("✓ Deleted {}.", dir.display()));
                }
                Some(Err(program)) => done.push(format!("Kept the game's files on the drive it was installed to: no uninstaller beside {} tells where its folder starts. Delete that folder yourself.", program.display())),
                None => {}
            }
            crate::log::timed("uninstall: deleting the folder", || fs::remove_dir_all(f)).map_err(|e| format!("Couldn't delete {}: {e}", f.display()))?;
            done.push(format!("✓ Deleted {}.", f.display()));
        }
        How::Found(f) => {
            crate::log::timed("uninstall: deleting the folder", || if f.is_dir() { fs::remove_dir_all(f) } else { fs::remove_file(f) }).map_err(|e| format!("Couldn't delete {}: {e}", f.display()))?;
            done.push(format!("✓ Deleted {}.", f.display()));
        }
        How::SteamCopy(f, id) => {
            crate::log::timed("uninstall: deleting the folder", || fs::remove_dir_all(f)).map_err(|e| format!("Couldn't delete {}: {e}", f.display()))?;
            if let Some(steamapps) = f.parent().and_then(Path::parent) {
                let _ = fs::remove_file(steamapps.join(format!("appmanifest_{id}.acf")));
            }
            done.push(format!("✓ Deleted {} and its record in that Steam library.", f.display()));
        }
        _ => {}
    }
    // Gone with the folder already when the folder was the prefix (gamefix's installs).
    if let Some(p) = delete_prefix.filter(|p| p.exists()) {
        crate::log::timed("uninstall: deleting the prefix", || fs::remove_dir_all(&p)).map_err(|e| format!("Couldn't delete its Wine prefix {}: {e}", p.display()))?;
        done.push(format!("✓ Deleted its Wine prefix {} (its settings and saves on this computer).", p.display()));
    } else if let (false, Some(p)) = (with_prefix, &prefix) {
        done.push(format!("Kept its Wine prefix {} (its settings and saves).", p.display()));
    }
    Ok(done)
}
