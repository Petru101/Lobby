//! App-menu and desktop entries (.desktop files) for games: the ones people and programs
//! (gamefix, Steam, Wine) make.

use super::Home;
use crate::library::{Entry, Key, Source};
use std::fs;
use std::path::{Path, PathBuf};

fn find(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if entry.file_type().is_ok_and(|t| t.is_dir()) {
            if depth < 4 {
                find(&path, depth + 1, out);
            }
        } else if path.extension().is_some_and(|e| e == "desktop") {
            out.push(path);
        }
    }
}

/// The folders app menus read entries and icons from, the home's first: Flatpak's (the
/// user's and the system's) and the system's own, as `XDG_DATA_DIRS` names them, else where
/// they usually are.
fn data_dirs(home: &Home) -> Vec<PathBuf> {
    let listed = std::env::var("XDG_DATA_DIRS").unwrap_or_default();
    let usual = [home.join(".local/share/flatpak/exports/share"), PathBuf::from("/var/lib/flatpak/exports/share"), PathBuf::from("/usr/local/share"), PathBuf::from("/usr/share")];
    let mut dirs = vec![home.join(".local/share")];
    for d in listed.split(':').filter(|d| d.starts_with('/')).map(PathBuf::from).chain(usual) {
        if !dirs.contains(&d) {
            dirs.push(d);
        }
    }
    dirs
}

pub fn entries(home: &Home) -> Vec<Entry> {
    entries_in(home, &data_dirs(home))
}

fn entries_in(home: &Home, dirs: &[PathBuf]) -> Vec<Entry> {
    let mut entries = Vec::new();
    // An entry's id is its path under applications/: the first folder's wins, as in app menus
    // (the home's copy of a Flatpak's entry, NoDisplay or Hidden, hides it).
    let mut seen = std::collections::HashSet::new();
    for dir in dirs {
        let apps = dir.join("applications");
        let mut files = Vec::new();
        find(&apps, 0, &mut files);
        for file in files {
            let id = file.strip_prefix(&apps).map(|r| r.to_string_lossy().replace('/', "-")).unwrap_or_default();
            if !seen.insert(id) {
                continue;
            }
            entries.extend(read(&file, dirs));
        }
    }
    let mut desktop = Vec::new();
    find(&home.join("Desktop"), 0, &mut desktop);
    entries.extend(desktop.iter().filter_map(|f| read(f, dirs)));
    entries
}

/// Programs that call themselves games but start other games or set them up: launchers and
/// Proton managers (ProtonPlus says only "Game"), and app-menu categories for emulators, tools
/// and Steam (FileTransfer). Their own entries aren't games.
const NOT_GAME_APPS: &[&str] = &[
    "io.github.Faugus.faugus-launcher", "faugus-launcher", "io.itch.itch", "itch", "io.github.sharkwouter.Minigalaxy", "minigalaxy", "page.kramo.Cartridges", "cartridges",
    "io.github.dummerle.rare", "rare", "com.github.tkashkin.gamehub", "gamehub", "com.vysp3r.ProtonPlus", "protonplus", "net.davidotek.pupgui2", "com.github.Matoking.protontricks",
];
const NOT_GAMES: &[&str] = &["Emulator", "PackageManager", "Utility", "FileTransfer"];

fn read(file: &Path, dirs: &[PathBuf]) -> Option<Entry> {
    let text = fs::read_to_string(file).ok()?;
    let mut fields = Vec::new();
    let mut main = false;
    for line in text.lines() {
        if line.starts_with('[') {
            main = line.trim() == "[Desktop Entry]";
        } else if main {
            if let Some((k, v)) = line.split_once('=') {
                fields.push((k.trim().to_string(), v.trim().to_string()));
            }
        }
    }
    let get = |k: &str| fields.iter().find(|(key, _)| key == k).map(|(_, v)| v.as_str());
    let name = get("Name")?;
    let exec = get("Exec").unwrap_or_default();
    let wine = file.components().any(|c| c.as_os_str() == "wine");
    let categories: Vec<&str> = get("Categories").unwrap_or_default().split(';').collect();
    let stem = file.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let game = (categories.contains(&"Game") && !categories.iter().any(|c| NOT_GAMES.contains(c)) && !NOT_GAME_APPS.contains(&stem.as_str()))
        || file.file_name().is_some_and(|n| n.to_string_lossy().starts_with("gamefix-"))
        || exec.contains("steam://rungameid/")
        || wine;
    // lobby's own entry is in Games too.
    let lobby = file.file_stem().is_some_and(|s| s == "io.github.Petru101.Lobby");
    if !game || lobby || get("Type").is_some_and(|t| t != "Application") || get("NoDisplay") == Some("true") || get("Hidden") == Some("true") {
        return None;
    }
    let mut e = Entry::new(Source::AppMenu, name);
    e.keys.push(Key::Desktop(file.to_path_buf()));
    e.icon = get("Icon").and_then(|i| icon_file(i, dirs));
    if let Some(id) = exec.split("steam://rungameid/").nth(1).and_then(|s| s.split(|c: char| !c.is_ascii_digit()).next()).and_then(|s| s.parse::<u64>().ok()) {
        // A non-Steam game's run id is its shortcut id shifted up (with 0x02000000 below).
        e.keys.push(if id > u32::MAX as u64 { Key::SteamShortcut((id >> 32) as u32) } else { Key::SteamApp(id as u32) });
    }
    // The last file the command names: the game's program, or what a runner opens. Not
    // flatpak itself: a Flatpak app's entry names no file of its own, and every one would be
    // the same game (and /usr/bin its folder).
    e.program = crate::launch::desktop_exec(&text).map(|(w, _)| w).unwrap_or_default().into_iter().rev().map(|w| PathBuf::from(w.rsplit_once('=').map_or(w.as_str(), |(_, v)| v))).find(|p| p.is_absolute() && p.is_file() && p.file_name().is_none_or(|n| n != "flatpak"));
    // Its working folder is the game's when the program is in it, or when no program could be
    // found (a runner's command naming a folder that was since deleted).
    e.folder = get("Path").filter(|p| !p.is_empty()).map(PathBuf::from).filter(|p| e.program.as_ref().is_none_or(|prog| prog.starts_with(p)));
    // gamefix's entry names the game's own folder; its Path is where the program runs, often
    // deep inside (Binaries/Win64), which made the game's top folder look like a folder of
    // games and its Engine and _Redist folders games of their own. Not for a game gamefix
    // installed: there it names the installer's folder (Mycopunk's repack in Downloads).
    let path = get("Path").map(PathBuf::from);
    if let Some(root) = get("X-Gamefix-Game").map(PathBuf::from).filter(|r| r.is_absolute() && path.as_ref().is_some_and(|p| p.starts_with(r))) {
        e.folder = Some(game_root(&root, path.as_deref().unwrap_or(&root)));
    }
    // Wine's entries: `env WINEPREFIX="/path" wine C:\\...\\game.exe`, or with the whole
    // `"WINEPREFIX=/a path"` quoted (gamefix's): the command's own words, as the desktop-entry
    // spec splits them.
    let words = crate::launch::desktop_exec(&text).map(|(w, _)| w).unwrap_or_default();
    e.prefix = words.iter().find_map(|w| w.strip_prefix("WINEPREFIX=")).filter(|p| p.starts_with('/')).map(PathBuf::from);
    Some(e)
}

/// An entry's icon as a file: its Icon= when that's a path, else the icon of that name in the
/// app menus' icon folders, biggest first.
fn icon_file(icon: &str, dirs: &[PathBuf]) -> Option<PathBuf> {
    let path = PathBuf::from(icon);
    if path.is_absolute() {
        return Some(path);
    }
    if icon.is_empty() || icon.contains('/') {
        return None;
    }
    let sizes = ["512x512", "256x256", "scalable", "192x192", "128x128", "96x96", "64x64", "48x48"];
    dirs.iter()
        .flat_map(|d| sizes.iter().flat_map(move |s| ["png", "svg"].map(|x| d.join(format!("icons/hicolor/{s}/apps/{icon}.{x}")))).chain(["png", "svg"].map(|x| d.join(format!("pixmaps/{icon}.{x}")))))
        .find(|p| p.is_file())
}

/// The Flatpak app an app-menu entry starts, when that's all it starts (X-Flatpak=, which
/// Flatpak writes in the entries it exports).
pub fn flatpak_app(e: &Entry) -> Option<String> {
    let file = e.keys.iter().find_map(|k| if let Key::Desktop(p) = k { Some(p) } else { None })?;
    let text = fs::read_to_string(file).ok()?;
    e.program.is_none().then_some(())?;
    text.lines().take_while(|l| !l.starts_with('[') || l.trim() == "[Desktop Entry]").find_map(|l| l.strip_prefix("X-Flatpak=")).map(|id| id.trim().to_string())
}

/// The game's own folder from what gamefix's entry names: a folder named like a folder of games
/// or downloads ("Downloaded Games/Games", set up when it was taken for a game called "Games")
/// isn't one game's, so the folder below it on the way to the program is.
fn game_root(root: &Path, path: &Path) -> PathBuf {
    let named = root.file_name().is_some_and(|n| crate::places::games_or_downloads(&n.to_string_lossy()));
    match path.strip_prefix(root).ok().and_then(|rest| rest.components().next()) {
        Some(first) if named => root.join(first),
        _ => root.to_path_buf(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_games_folder_isnt_the_games_own() {
        let p = |s: &str| PathBuf::from(s);
        assert_eq!(game_root(&p("/d/Downloaded Games/Games"), &p("/d/Downloaded Games/Games/Minecraft Dungeons II/Dungeons/Binaries/WinGDK")), p("/d/Downloaded Games/Games/Minecraft Dungeons II"));
        assert_eq!(game_root(&p("/g/Woodo"), &p("/g/Woodo/Binaries/Win64")), p("/g/Woodo"));
        assert_eq!(game_root(&p("/d/Games"), &p("/d/Games")), p("/d/Games"));
    }

    #[test]
    fn flatpak_and_system_games_not_launchers() {
        let dir = std::env::temp_dir().join(format!("lobby-menu-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let home = Home { home: dir.join("home"), user: "u".into() };
        let (local, flatpak, system) = (dir.join("home/.local/share"), dir.join("flatpak"), dir.join("usr"));
        let write = |d: &Path, id: &str, body: &str| {
            fs::create_dir_all(d.join("applications")).unwrap();
            fs::write(d.join("applications").join(format!("{id}.desktop")), format!("[Desktop Entry]\nType=Application\n{body}\n")).unwrap();
        };
        write(&flatpak, "org.supertuxproject.SuperTux", "Name=SuperTux 2\nExec=/usr/bin/flatpak run --command=supertux2 org.supertuxproject.SuperTux\nIcon=org.supertuxproject.SuperTux\nCategories=Game;ArcadeGame;\nX-Flatpak=org.supertuxproject.SuperTux");
        write(&flatpak, "io.github.Faugus.faugus-launcher", "Name=Faugus\nExec=/usr/bin/flatpak run io.github.Faugus.faugus-launcher\nCategories=Game;");
        write(&flatpak, "com.vysp3r.ProtonPlus", "Name=ProtonPlus\nExec=/usr/bin/flatpak run com.vysp3r.ProtonPlus\nCategories=Game;");
        write(&flatpak, "io.github.dosbox-staging", "Name=DOSBox Staging\nExec=/usr/bin/flatpak run io.github.dosbox-staging\nCategories=Game;Emulator;");
        write(&flatpak, "com.usebottles.bottles", "Name=Bottles\nExec=/usr/bin/flatpak run com.usebottles.bottles\nCategories=Utility;Game;");
        write(&flatpak, "net.unvanquished.Unvanquished", "Name=Unvanquished\nExec=/usr/bin/flatpak run net.unvanquished.Unvanquished\nCategories=Game;");
        write(&local, "net.unvanquished.Unvanquished", "Name=Unvanquished\nExec=/usr/bin/flatpak run net.unvanquished.Unvanquished\nCategories=Game;\nHidden=true");
        write(&system, "steam", "Name=Steam\nExec=steam %U\nCategories=Network;FileTransfer;Game;");
        write(&system, "net.lutris.Lutris", "Name=Lutris\nExec=lutris %U\nCategories=Game;PackageManager;");
        write(&system, "tux-racer", "Name=Tux Racer\nExec=/bin/sh\nIcon=tux-racer\nCategories=Game;");
        fs::create_dir_all(flatpak.join("icons/hicolor/scalable/apps")).unwrap();
        fs::write(flatpak.join("icons/hicolor/scalable/apps/org.supertuxproject.SuperTux.svg"), "<svg/>").unwrap();
        fs::create_dir_all(system.join("pixmaps")).unwrap();
        fs::write(system.join("pixmaps/tux-racer.png"), "").unwrap();

        let found = entries_in(&home, &[local, flatpak.clone(), system.clone()]);
        let names: Vec<&str> = found.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["SuperTux 2", "Tux Racer"]);
        let (tux, racer) = (&found[0], &found[1]);
        assert_eq!(tux.program, None);
        assert_eq!(tux.icon, Some(flatpak.join("icons/hicolor/scalable/apps/org.supertuxproject.SuperTux.svg")));
        assert_eq!(flatpak_app(tux).as_deref(), Some("org.supertuxproject.SuperTux"));
        assert_eq!(racer.program, Some(PathBuf::from("/bin/sh")));
        assert_eq!(racer.icon, Some(system.join("pixmaps/tux-racer.png")));
        assert_eq!(flatpak_app(racer), None);
        let _ = fs::remove_dir_all(&dir);
    }
}
