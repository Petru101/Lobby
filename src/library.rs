//! What the sources found (entries) and how they become one list of games: entries naming the
//! same game (a Lutris game that's also a Steam shortcut, a GOG folder Heroic knows) merge.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Where an entry was found. The order is the order names are trusted in: a store's name
/// beats a folder's.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Serialize, Deserialize)]
pub enum Source {
    Steam,
    Epic,
    Gog,
    Amazon,
    Heroic,
    Lutris,
    Faugus,
    /// The itch.io page a game was downloaded from (not a launcher: names and covers).
    Itch,
    Gamefix,
    SteamShortcut,
    AppMenu,
    Prefix,
    Folder,
}

impl Source {
    pub fn label(self) -> &'static str {
        match self {
            Source::Steam => "Steam",
            Source::Epic => "Epic (Heroic)",
            Source::Gog => "GOG (Heroic)",
            Source::Amazon => "Amazon (Heroic)",
            Source::Heroic => "Heroic (added by hand)",
            Source::Lutris => "Lutris",
            Source::Faugus => "Faugus",
            Source::Itch => "itch.io",
            Source::Gamefix => "gamefix",
            Source::SteamShortcut => "Steam shortcut",
            Source::AppMenu => "app menu",
            Source::Prefix => "Windows setup",
            Source::Folder => "folder",
        }
    }
}

/// Ids launchers use to point at each other's games.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum Key {
    SteamApp(u32),
    SteamShortcut(u32),
    Lutris(i64),
    Desktop(PathBuf),
    /// Heroic's runner (legendary, gog, nile, sideload) and app name.
    Heroic(String, String),
    /// Faugus's game id.
    Faugus(String),
    /// An itch.io game page.
    Itch(String),
    /// A download and an installed game of the same name.
    Name(String),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum Kind {
    Game,
    /// A store's own program installed in Wine (EA app, Epic Games Launcher).
    StoreApp,
    /// An installer, disc image or archive: downloaded, not installed yet.
    Download,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Entry {
    pub source: Source,
    pub name: String,
    pub kind: Kind,
    /// The game's own folder.
    pub folder: Option<PathBuf>,
    /// The program that starts it.
    pub program: Option<PathBuf>,
    /// The Wine prefix it runs in (the folder holding drive_c).
    pub prefix: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub keys: Vec<Key>,
    /// Portrait cover art: a file, or a web address to fetch it from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cover: Option<String>,
    /// A small icon, for games without a cover.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<PathBuf>,
    /// Only says something about a game another entry found (gamefix's record of a shortcut it
    /// made): dropped when nothing else matches it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub annotation: bool,
}

impl Entry {
    pub fn new(source: Source, name: impl Into<String>) -> Entry {
        Entry { source, name: name.into(), kind: Kind::Game, folder: None, program: None, prefix: None, keys: Vec::new(), cover: None, icon: None, annotation: false }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Game {
    /// lobby's own id for it, kept from scan to scan (set by the saved library).
    #[serde(default)]
    pub id: String,
    pub name: String,
    pub kind: Kind,
    pub entries: Vec<Entry>,
    /// Nothing it names is on disk (uninstalled behind a launcher's back, or on a drive that
    /// isn't plugged in).
    pub missing: bool,
    /// No source found it in the last scan (uninstalled, or its drive isn't plugged in): kept so
    /// it isn't lost.
    #[serde(default)]
    pub gone: bool,
    /// Unix times.
    #[serde(default)]
    pub added: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub played: Option<u64>,
}

impl Game {
    pub fn sources(&self) -> Vec<Source> {
        let mut s: Vec<_> = self.entries.iter().map(|e| e.source).collect();
        s.sort();
        s.dedup();
        s
    }

    /// Only found by looking through folders: no launcher knows it.
    pub fn unknown_to_launchers(&self) -> bool {
        self.entries.iter().all(|e| matches!(e.source, Source::Folder | Source::Prefix | Source::Itch))
    }

    /// Entries for the game itself first, then downloads of it.
    fn installed_first(&self) -> impl Iterator<Item = &Entry> {
        self.entries.iter().filter(|e| e.kind != Kind::Download).chain(self.entries.iter().filter(|e| e.kind == Kind::Download))
    }

    pub fn folder(&self) -> Option<&Path> {
        self.installed_first().find_map(|e| e.folder.as_deref())
    }

    pub fn program(&self) -> Option<&Path> {
        self.installed_first().find_map(|e| e.program.as_deref())
    }

    /// Its cover: a file on disk first, else a web address.
    pub fn cover(&self) -> Option<&str> {
        let covers = || self.entries.iter().filter_map(|e| e.cover.as_deref());
        covers().find(|c| c.starts_with('/') && Path::new(c).is_file()).or_else(|| covers().find(|c| c.starts_with("https://")))
    }

    pub fn icon(&self) -> Option<&Path> {
        self.entries.iter().filter_map(|e| e.icon.as_deref()).find(|p| p.is_file())
    }
}

/// The same path written the same way: /home is a link to /var/home on Fedora's atomic
/// desktops, and launchers write either.
pub fn norm(path: &Path) -> PathBuf {
    let p = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    match p.strip_prefix("/var/home") {
        Ok(rest) => Path::new("/home").join(rest),
        Err(_) => p,
    }
}

/// A Wine prefix: the folder holding drive_c, or Proton's `<folder>/pfx`.
pub fn prefix_in(dir: &Path) -> Option<PathBuf> {
    [dir.to_path_buf(), dir.join("pfx")].into_iter().find(|p| p.join("drive_c").is_dir())
}

/// Folders inside a game that are about its build, not the game (Unreal's Binaries/Win64).
const BUILD_DIRS: &[&str] = &["bin", "bin32", "bin64", "binaries", "win32", "win64", "wingdk", "x86", "x64", "x86_64", "system", "retail", "release", "shipping", "exe", "game"];

/// The game's folder around its program: the program's own, or the one above folders named
/// for the build.
pub fn game_dir(program: &Path) -> Option<PathBuf> {
    program.ancestors().skip(1).find(|d| d.file_name().is_some_and(|n| !BUILD_DIRS.contains(&n.to_string_lossy().to_lowercase().as_str()))).map(Path::to_path_buf)
}

/// Programs of stores and launchers, installed in Wine to get at their games.
const STORE_APPS: &[&str] = &["eadesktop", "ealauncher", "eabackgroundservice", "origin.exe", "epicgameslauncher", "upc.exe", "ubisoftconnect", "uplay", "battle.net", "galaxyclient", "steam.exe", "rockstarlauncher", "amazon games", "itch.exe"];

pub fn store_app(program: &Path) -> bool {
    let name = program.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
    STORE_APPS.iter().any(|s| name.contains(s))
}

/// Programs that come with games or are kept beside them, but aren't games.
const NOT_GAMES: &[&str] = &["cheat engine", "cheatengine", "uninstall", "readme", "manual", "website", "error report", "crash report", "kernel module", "settings", "configurat", "support", "register"];

pub fn not_a_game(name: &str) -> bool {
    let name = name.to_lowercase();
    NOT_GAMES.iter().any(|n| name.contains(n))
}

/// A shortcut an installer made to something other than the game.
fn tool_shortcut(e: &Entry) -> bool {
    e.source == Source::Prefix && e.program.as_ref().is_some_and(|p| not_a_game(&p.file_name().unwrap_or_default().to_string_lossy()))
}

/// A name with only its letters and digits, for telling copies of one game apart from others.
fn plain(name: &str) -> String {
    name.to_lowercase().chars().filter(|c| c.is_alphanumeric()).collect()
}

/// Tidies entries (paths written one way, prefixes told apart from game folders) and merges
/// those naming the same game.
pub fn merge(mut entries: Vec<Entry>, too_wide: &[PathBuf]) -> Vec<Game> {
    entries.retain(|e| !not_a_game(&e.name) && !tool_shortcut(e));
    // A download of a game that is installed belongs with it (the-mammoth's zip beside its
    // unpacked folder).
    let installed: Vec<String> = entries.iter().filter(|e| e.kind == Kind::Game).map(|e| plain(&e.name)).collect();
    let downloaded: Vec<String> = entries.iter().filter(|e| e.kind == Kind::Download).map(|e| plain(&e.name)).collect();
    for e in &mut entries {
        let name = plain(&e.name);
        if (e.kind == Kind::Download && installed.contains(&name)) || (e.kind == Kind::Game && downloaded.contains(&name)) {
            e.keys.push(Key::Name(name));
        }
    }
    for e in &mut entries {
        e.folder = e.folder.as_deref().map(norm);
        e.program = e.program.as_deref().map(norm);
        e.prefix = e.prefix.as_deref().map(norm);
        // A shortcut starting a runner (/usr/bin/flatpak, umu-run) names no game's program.
        if e.program.as_ref().is_some_and(|p| ["/usr", "/bin", "/opt", "/app", "/etc"].iter().any(|s| p.starts_with(s))) {
            e.program = None;
        }
        // A launcher's "game folder" that is the prefix the game was installed into.
        if let Some(pfx) = e.folder.as_deref().and_then(prefix_in) {
            e.prefix = Some(norm(&pfx));
            e.folder = None;
        }
        if e.folder.is_none() {
            e.folder = e.program.as_deref().and_then(game_dir);
        }
        // A home folder, a drive or a folder of games isn't any one game's.
        if e.folder.as_ref().is_some_and(|f| too_wide.contains(f) || f.parent().is_none()) {
            e.folder = None;
        }
        if e.program.as_deref().is_some_and(store_app) {
            e.kind = Kind::StoreApp;
        }
    }

    let n = entries.len();
    let mut parent: Vec<usize> = (0..n).collect();
    fn root(parent: &mut [usize], mut i: usize) -> usize {
        while parent[i] != i {
            parent[i] = parent[parent[i]];
            i = parent[i];
        }
        i
    }
    for a in 0..n {
        for b in a + 1..n {
            if same_game(&entries[a], &entries[b]) {
                let (ra, rb) = (root(&mut parent, a), root(&mut parent, b));
                parent[ra] = rb;
            }
        }
    }
    let mut groups: Vec<Vec<Entry>> = Vec::new();
    let mut slot = vec![usize::MAX; n];
    for (i, e) in entries.into_iter().enumerate() {
        let r = root(&mut parent, i);
        if slot[r] == usize::MAX {
            slot[r] = groups.len();
            groups.push(Vec::new());
        }
        groups[slot[r]].push(e);
    }

    let mut games: Vec<Game> = groups
        .into_iter()
        .filter(|g| !g.iter().all(|e| e.annotation))
        .map(|mut entries| {
            entries.sort_by_key(|e| e.source);
            let named = entries.iter().find(|e| !e.annotation && e.kind != Kind::Download).or_else(|| entries.iter().find(|e| !e.annotation)).unwrap_or(&entries[0]);
            let name = named.name.clone();
            let kind = if entries.iter().any(|e| e.kind == Kind::StoreApp) {
                Kind::StoreApp
            } else if entries.iter().all(|e| e.kind == Kind::Download) {
                Kind::Download
            } else {
                Kind::Game
            };
            let paths: Vec<&PathBuf> = entries.iter().flat_map(|e| [e.folder.as_ref(), e.program.as_ref()]).flatten().collect();
            let missing = !paths.is_empty() && !paths.iter().any(|p| p.exists());
            Game { id: String::new(), name, kind, entries, missing, gone: false, added: 0, played: None }
        })
        .collect();
    games.sort_by_key(|g| g.name.to_lowercase());
    games
}

fn same_game(a: &Entry, b: &Entry) -> bool {
    if a.keys.iter().any(|k| b.keys.contains(k)) {
        return true;
    }
    if a.program.is_some() && a.program == b.program {
        return true;
    }
    // One launcher listing two programs lists two games (Black & White and Creature Isle,
    // two scripts in one folder); two Steam apps are two games. A store's programs (the EA
    // app and its launcher) are one store.
    let stores = a.kind == Kind::StoreApp && b.kind == Kind::StoreApp;
    if a.source == b.source && a.program.is_some() && b.program.is_some() && !stores {
        return false;
    }
    let steam_app = |e: &Entry| e.keys.iter().find_map(|k| if let Key::SteamApp(id) = k { Some(*id) } else { None });
    if let (Some(x), Some(y)) = (steam_app(a), steam_app(b)) {
        if x != y {
            return false;
        }
    }
    // A store app's prefix holds the games installed through it: being in it says nothing,
    // except about the store's other programs.
    if (a.kind == Kind::StoreApp) != (b.kind == Kind::StoreApp) {
        return false;
    }
    let holds = |x: &Entry, y: &Entry| match (&x.folder, &y.folder, &y.program) {
        (Some(f), Some(g), _) if f == g => true,
        (Some(f), _, Some(p)) => p.starts_with(f),
        _ => false,
    };
    holds(a, b) || holds(b, a)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(source: Source, name: &str, folder: Option<&str>, program: Option<&str>) -> Entry {
        let mut e = Entry::new(source, name);
        e.folder = folder.map(PathBuf::from);
        e.program = program.map(PathBuf::from);
        e
    }

    fn names(games: &[Game]) -> Vec<(String, Vec<Source>)> {
        games.iter().map(|g| (g.name.clone(), g.sources())).collect()
    }

    #[test]
    fn a_launchers_game_and_its_folder_are_one_game() {
        let games = merge(
            vec![
                entry(Source::Gog, "Dishonored - Definitive Edition", Some("/g/Dishonored"), None),
                entry(Source::Folder, "Dishonored", Some("/g/Dishonored"), Some("/g/Dishonored/Binaries/Win32/Dishonored.exe")),
            ],
            &[],
        );
        assert_eq!(names(&games), [("Dishonored - Definitive Edition".into(), vec![Source::Gog, Source::Folder])]);
    }

    #[test]
    fn two_programs_from_one_launcher_are_two_games() {
        let games = merge(
            vec![
                entry(Source::AppMenu, "Black & White", Some("/g/bw"), Some("/g/bw/play.sh")),
                entry(Source::AppMenu, "Black & White: Creature Isle", Some("/g/bw"), Some("/g/bw/play-creature-isle.sh")),
            ],
            &[],
        );
        assert_eq!(games.len(), 2);
    }

    #[test]
    fn keys_tie_launchers_together() {
        let mut lutris = entry(Source::Lutris, "Infested Planet", None, Some("/p/drive_c/Games/IP/InfestedPlanet.exe"));
        lutris.keys.push(Key::Lutris(2));
        let mut steam = entry(Source::SteamShortcut, "Infested Planet", None, None);
        steam.keys.push(Key::Lutris(2));
        let mut gamefix = entry(Source::Gamefix, "infested-planet", None, None);
        gamefix.annotation = true;
        let games = merge(vec![lutris, steam, gamefix], &[]);
        // gamefix's record matches nothing here: dropped, not listed as a game.
        assert_eq!(names(&games), [("Infested Planet".into(), vec![Source::Lutris, Source::SteamShortcut])]);
    }

    #[test]
    fn a_store_apps_prefix_says_nothing_about_its_games() {
        let games = merge(
            vec![
                entry(Source::Prefix, "EA app", None, Some("/p/drive_c/Program Files/EA/EADesktop.exe")),
                entry(Source::Prefix, "EA launcher", None, Some("/p/drive_c/Program Files/EA/EALauncher.exe")),
                entry(Source::Folder, "A game", Some("/p/drive_c/Program Files"), Some("/p/drive_c/Program Files/Game/game.exe")),
            ],
            &[],
        );
        assert_eq!(games.len(), 2);
        assert!(games.iter().any(|g| g.kind == Kind::StoreApp && g.entries.len() == 2));
    }

    #[test]
    fn a_download_belongs_with_its_installed_game() {
        let mut zip = entry(Source::Folder, "the-mammoth", Some("/w/dl/the-mammoth"), Some("/w/dl/the-mammoth/mammoth.zip"));
        zip.kind = Kind::Download;
        let games = merge(vec![zip, entry(Source::Folder, "The Mammoth", Some("/g/the-mammoth"), Some("/g/the-mammoth/TheMammoth.exe"))], &[]);
        assert_eq!(games.len(), 1);
        assert_eq!(games[0].kind, Kind::Game);
        assert_eq!(games[0].program(), Some(Path::new("/g/the-mammoth/TheMammoth.exe")));
    }

    #[test]
    fn a_folder_of_games_is_no_ones_folder() {
        let games = merge(
            vec![
                entry(Source::Steam, "A", Some("/home/u"), None),
                entry(Source::SteamShortcut, "B", None, Some("/home/u/b.exe")),
            ],
            &[PathBuf::from("/home/u")],
        );
        assert_eq!(games.len(), 2);
    }

    #[test]
    fn build_folders_are_skipped_for_the_games_folder() {
        assert_eq!(game_dir(Path::new("/g/MD2/Dungeons/Binaries/WinGDK/x.exe")), Some(PathBuf::from("/g/MD2/Dungeons")));
    }
}
