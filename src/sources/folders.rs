//! Folders of games: ~/Games, gamefix's games folder, a "Games" folder on each drive, and
//! Downloads. Each folder in them is looked at with gamefix's scan, which picks the program
//! that starts a game; a folder holding several games is looked into.

use super::Home;
use crate::library::{self, Entry, Kind, Source};
use gamefix::binary::Binary;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use gamefix::{disc, scan};
use std::fs;
use std::path::{Path, PathBuf};

/// A drive's folders named like downloads or games, whatever they're called ("Downloads-old",
/// "Old Games"), besides its Games folder.
fn drive_folders(drive: &Path) -> Vec<PathBuf> {
    fs::read_dir(drive)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|top| top.file_type().is_ok_and(|t| t.is_dir()) && crate::places::games_or_downloads(&top.file_name().to_string_lossy()) && !top.file_name().eq_ignore_ascii_case("games"))
        .map(|top| top.path())
        .collect()
}

/// The roots lobby is strict about: a drive's downloads-like folders (an old Downloads is a
/// dumping ground of tools, add-ons and documents), where only what shows a game engine, GOG's
/// record or Steam's library counts (`look`'s `strict`). Not ~/Downloads, Games folders, the
/// games folder or folders the user added.
pub fn strict_roots(home: &Home) -> Vec<PathBuf> {
    [PathBuf::from("/run/media").join(&home.user), PathBuf::from("/media").join(&home.user), PathBuf::from("/mnt")]
        .iter()
        .flat_map(|media| fs::read_dir(media).into_iter().flatten().flatten().flat_map(|d| drive_folders(&d.path())))
        .map(|r| library::norm(&r))
        .collect()
}

pub fn roots(home: &Home) -> Vec<PathBuf> {
    // The home folder's first: a folder reached two ways is known by its first path.
    let mut roots = vec![home.join("Games"), home.join("Downloads")];
    if let Ok(folder) = fs::read_to_string(home.join(".config/gamefix/games-folder")) {
        roots.push(PathBuf::from(folder.trim()));
    }
    for media in [PathBuf::from("/run/media").join(&home.user), PathBuf::from("/media").join(&home.user), PathBuf::from("/mnt")] {
        for drive in fs::read_dir(&media).into_iter().flatten().flatten() {
            let drive = drive.path();
            if drive.file_name().is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case("games")) {
                roots.push(drive.clone());
            }
            for name in ["Games", "games"] {
                roots.push(drive.join(name));
            }
            roots.extend(drive_folders(&drive));
        }
    }
    roots.extend(crate::places::Places::load().added);
    // One folder reached two ways counts once: ~/Downloads here is the Storage drive's
    // Downloads mounted again, a bind mount, the same folder under another path.
    let mut out: Vec<PathBuf> = Vec::new();
    let mut seen: Vec<(u64, u64)> = Vec::new();
    for r in roots {
        let r = library::norm(&r);
        let Ok(meta) = fs::metadata(&r) else { continue };
        let id = (std::os::unix::fs::MetadataExt::dev(&meta), std::os::unix::fs::MetadataExt::ino(&meta));
        if meta.is_dir() && !out.contains(&r) && !seen.contains(&id) {
            out.push(r);
            seen.push(id);
        }
    }
    out
}

/// Whether Steam's library (steam_api.dll, libsteam_api.so) is in the folder of the game's
/// program.
fn steam_api_near(game: &scan::Game) -> bool {
    let program = game.main.or(game.windows).map(|i| game.full(&game.files[i]));
    let dir = program.as_deref().and_then(Path::parent).unwrap_or(&game.root).to_path_buf();
    ["steam_api.dll", "steam_api64.dll", "libsteam_api.so"].iter().any(|f| dir.join(f).is_file())
}

/// Folders in a games folder that aren't games.
fn skipped(name: &str) -> bool {
    let n = name.to_lowercase();
    // Hidden folders, lobby's and gamefix's half-done unpacks and installs among them.
    name.starts_with('.') || n.contains("backup") || ["steamlibrary", "prefixes", "jre", "jdk", "tools", "shots", "screenshots", "saves", "umu", "lost+found", "$recycle.bin", "system volume information"].contains(&n.as_str())
}

enum Found {
    Game(Entry),
    Prefix(PathBuf, Option<Entry>),
    Nothing,
}

/// Each folder in the roots, as games, downloads not installed yet, and prefixes. Folders a
/// launcher already names as a game's are left to that launcher.
/// Returns the entries found, the Wine prefixes come across, and the folders that collect
/// downloads or games (too wide to be any one game's folder when merging).
pub fn scan(roots: &[PathBuf], strict: &[PathBuf], games: &[PathBuf], known: &[Entry]) -> (Vec<Entry>, Vec<PathBuf>, Vec<PathBuf>) {
    let known: Vec<PathBuf> = known.iter().filter_map(|e| e.folder.as_deref()).map(library::norm).collect();
    // Game folders taken from the search of all drives: looked at as they are.
    let mut candidates: Vec<PathBuf> = games.iter().filter(|g| g.is_dir()).cloned().collect();
    let strict_dirs: Vec<PathBuf> = roots.iter().filter(|r| strict.contains(&library::norm(r))).cloned().collect();
    // A root's folders and loose downloads, and those of folders in it that collect downloads
    // or games (Downloads/Downloaded Games), two deep.
    let mut dirs: Vec<(PathBuf, usize)> = roots.iter().map(|r| (r.clone(), 0)).collect();
    let mut collections = Vec::new();
    while let Some((root, depth)) = dirs.pop() {
        if depth > 0 {
            collections.push(library::norm(&root));
        }
        for entry in fs::read_dir(&root).into_iter().flatten().flatten() {
            let path = entry.path();
            let dir = entry.file_type().is_ok_and(|t| t.is_dir());
            if dir && depth < 2 && collection(&path) && !roots.contains(&library::norm(&path)) {
                dirs.push((path, depth + 1));
            } else if (dir && !skipped(&entry.file_name().to_string_lossy()) && !roots.contains(&library::norm(&path))) || (!dir && super::downloads::wanted(&path)) {
                candidates.push(path);
            }
        }
    }
    candidates.sort();
    candidates.dedup_by(|a, b| library::norm(a) == library::norm(b));

    let mut cache = Cache::load();
    let (fresh, stale): (Vec<_>, Vec<_>) = candidates.into_iter().map(|dir| {
        let key = Looked::key(&dir, &known);
        (dir, key)
    }).partition(|(dir, key)| cache.folders.get(&dir.display().to_string()).is_some_and(|c| c.mtime == key.0 && c.known_inside == key.1));

    // Folders are independent, and big games take a while to walk: several at once.
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).min(8);
    let chunks: Vec<Vec<&(PathBuf, (u64, Vec<PathBuf>))>> = (0..threads).map(|t| stale.iter().skip(t).step_by(threads).collect()).collect();
    let looked: Vec<(String, Looked)> = std::thread::scope(|s| {
        let handles: Vec<_> = chunks
            .iter()
            .map(|chunk| s.spawn(|| chunk.iter().map(|(dir, (mtime, inside))| (dir.display().to_string(), Looked::from(look(dir, &known, 0, strict_dirs.iter().any(|r| dir.parent().is_some_and(|up| up.starts_with(r)))), *mtime, inside.clone()))).collect::<Vec<_>>()))
            .collect();
        handles.into_iter().flat_map(|h| h.join().unwrap_or_default()).collect()
    });
    let mut entries = Vec::new();
    let mut prefixes = Vec::new();
    for (dir, _) in &fresh {
        let c = &cache.folders[&dir.display().to_string()];
        entries.extend(c.entries.iter().cloned());
        prefixes.extend(c.prefixes.iter().cloned());
    }
    for (dir, l) in looked {
        entries.extend(l.entries.iter().cloned());
        prefixes.extend(l.prefixes.iter().cloned());
        cache.folders.insert(dir, l);
    }
    // Folders gone from the roots leave the cache.
    let wanted: Vec<String> = fresh.iter().chain(&stale).map(|(d, _)| d.display().to_string()).collect();
    cache.folders.retain(|k, _| wanted.contains(k));
    cache.save();
    (entries, prefixes, collections)
}

/// What looking at one folder in a root found, kept between scans in
/// ~/.cache/lobby/folders.json: walking big games is most of a scan's time.
#[derive(Serialize, Deserialize)]
struct Looked {
    /// The folder's own change time: a game added, removed or unpacked at its top changes it.
    mtime: u64,
    /// Launcher-known game folders inside it, which change what's looked at.
    known_inside: Vec<PathBuf>,
    entries: Vec<Entry>,
    prefixes: Vec<PathBuf>,
}

impl Looked {
    fn key(dir: &Path, known: &[PathBuf]) -> (u64, Vec<PathBuf>) {
        let mtime = fs::metadata(dir).and_then(|m| m.modified()).ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs());
        let norm = library::norm(dir);
        (mtime, known.iter().filter(|k| k.starts_with(&norm)).cloned().collect())
    }

    fn from(found: Vec<Found>, mtime: u64, known_inside: Vec<PathBuf>) -> Looked {
        let mut l = Looked { mtime, known_inside, entries: Vec::new(), prefixes: Vec::new() };
        for f in found {
            match f {
                Found::Game(e) => l.entries.push(e),
                Found::Prefix(p, fallback) => {
                    l.prefixes.push(p);
                    l.entries.extend(fallback);
                }
                Found::Nothing => {}
            }
        }
        l
    }
}

/// Bumped when the cache's format changes.
const CACHE_VERSION: u32 = 3;

/// When the running lobby was built: a rebuild (lobby's look or gamefix's scan may pick
/// differently now) starts the cache over.
fn build_time() -> u64 {
    std::env::current_exe().and_then(fs::metadata).and_then(|m| m.modified()).ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs())
}

#[derive(Serialize, Deserialize, Default)]
struct Cache {
    version: u32,
    #[serde(default)]
    build: u64,
    folders: HashMap<String, Looked>,
}

impl Cache {
    fn file() -> PathBuf {
        let cache = std::env::var_os("XDG_CACHE_HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".cache"));
        cache.join("lobby/folders.json")
    }

    fn load() -> Cache {
        let cache: Cache = fs::read_to_string(Self::file()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        let build = build_time();
        if cache.version == CACHE_VERSION && cache.build == build {
            cache
        } else {
            Cache { version: CACHE_VERSION, build, folders: HashMap::new() }
        }
    }

    fn save(&self) {
        let file = Self::file();
        let _ = fs::create_dir_all(file.parent().unwrap());
        if let Ok(text) = serde_json::to_string(self) {
            let _ = fs::write(&file, text);
        }
    }
}

fn look(dir: &Path, known: &[PathBuf], depth: usize, strict: bool) -> Vec<Found> {
    // A loose download: an archive, an installer, a disc image.
    if depth == 0 && dir.is_file() {
        return vec![super::downloads::look(dir, strict).map_or(Found::Nothing, Found::Game)];
    }
    let norm = library::norm(dir);
    if known.contains(&norm) {
        return vec![];
    }
    // Unreal Engine itself (Epic's launcher puts it in "Epic Games" beside its games): tens of
    // thousands of source files, and no game.
    if unreal_engine(dir) {
        return vec![Found::Nothing];
    }
    if let Some(pfx) = library::prefix_in(dir) {
        // A prefix whose installer made no shortcuts: the program it put in Program Files.
        let fallback = if disc::shortcuts(&pfx).is_empty() {
            let name = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            match disc::installed_program(&pfx, &name) {
                // Only a program in the prefix's own C:. gamefix also looks for new programs on
                // the prefix's other drive letters (right for its own install run), which in an
                // old prefix with v: = /run/media found Borderlands 4 for dome-keeper's prefix.
                disc::Installed::Program(exe, label) if fs::canonicalize(&exe).is_ok_and(|e| e.starts_with(fs::canonicalize(pfx.join("drive_c")).unwrap_or_default())) => {
                    let mut e = Entry::new(Source::Folder, label);
                    e.program = Some(exe);
                    e.prefix = Some(pfx.clone());
                    Some(e)
                }
                _ => None,
            }
        } else {
            None
        };
        return vec![Found::Prefix(pfx, fallback)];
    }
    // A folder holding a game a launcher knows (Heroic's ~/Games/Heroic, a GOG game one folder
    // down) is a folder of games: only its other folders are looked at.
    let subs = subdirs(dir);
    if known.iter().any(|k| k.starts_with(&norm) && k != &norm) {
        return if depth < 2 { subs.iter().flat_map(|d| look(d, known, depth + 1, strict)).collect() } else { vec![] };
    }
    // A folder of games has no program of its own at the top, and its folders are games of
    // their own. Not Unreal's layout (Engine beside the game's folder).
    let unreal = subs.iter().any(|d| d.file_name().is_some_and(|n| n == "Engine"));
    if depth == 0 && !unreal && !has_program_at_top(dir) {
        let inner: Vec<Found> = subs.iter().flat_map(|d| look(d, known, 1, strict)).collect();
        let games = inner.iter().filter(|f| !matches!(f, Found::Nothing)).count();
        if games >= 2 {
            return inner;
        }
    }
    let Some(game) = scan::scan(dir) else { return vec![Found::Nothing] };
    // GOG's own idea of what starts the game, over a guess.
    let gog = gamefix::gog::read(&game.root).map(|info| info.program);
    // In a dumping ground, only what shows it's a game: an engine gamefix knows, GOG's record,
    // or Steam's library beside its program.
    if strict && game.engine.is_none() && gog.is_none() && !steam_api_near(&game) {
        return vec![Found::Nothing];
    }
    let program = gog.or_else(|| game.main.or(game.windows).or(game.portable).map(|i| game.full(&game.files[i])));
    let download = [game.cd_installer, game.linux_installer, game.packed, game.disc, game.archive].into_iter().flatten().next();
    let mut e = Entry::new(Source::Folder, title(&game.root, &game.name, program.as_deref()));
    e.folder = Some(game.root.clone());
    // Unpacked by lobby: tied to its download.
    if let Ok(source) = fs::read_to_string(dir.join(crate::install::SOURCE_NOTE)) {
        e.keys.push(crate::library::Key::Download(PathBuf::from(source.trim())));
    }
    match (program, download) {
        (Some(p), _) => {
            // A Python script is only taken for a game when it uses pygame: Downloads holds
            // plenty of other scripts.
            let python = game.files.iter().find(|f| game.full(f) == p).and_then(|f| if let Binary::Python(py) = &f.binary { Some(py.pygame) } else { None });
            if python == Some(false) {
                return vec![Found::Nothing];
            }
            e.program = Some(p);
        }
        (None, Some(i)) => {
            // Saves kept as archives aren't games waiting to be installed.
            if game.name.to_lowercase().contains("save") {
                return vec![Found::Nothing];
            }
            e.kind = Kind::Download;
            e.program = Some(game.full(&game.files[i]));
        }
        (None, None) => match top_installer(dir) {
            // A game waiting in its installer (a repack's setup.exe beside its .bin files).
            Some(setup) => {
                e.kind = Kind::Download;
                // What gamefix's install note names as its source, and a name without the
                // packer's tag ("Mycopunk [FitGirl Repack]").
                e.keys.push(crate::library::Key::Download(setup.clone()));
                e.name = without_tags(&e.name);
                e.program = Some(setup);
            }
            None => return vec![Found::Nothing],
        },
    }
    vec![Found::Game(e)]
}

/// An install of Unreal Engine (the launcher's, or built from source), not a game made with it:
/// packaged games ship neither the engine's source nor its editor.
fn unreal_engine(dir: &Path) -> bool {
    let engine = dir.join("Engine");
    engine.join("Build/InstalledBuild.txt").is_file()
        || engine.join("Source").is_dir()
        || ["UnrealEditor.exe", "UE4Editor.exe", "UnrealEditor"].iter().any(|e| engine.join("Binaries/Win64").join(e).is_file() || engine.join("Binaries/Linux").join(e).is_file())
}

/// A folder collecting downloads or games rather than being one: named so ("Downloaded Games",
/// "Old Games") and holding two or more downloads (archives, installers, programs) at its top.
fn collection(dir: &Path) -> bool {
    let name = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    crate::places::games_or_downloads(&name) && fs::read_dir(dir).into_iter().flatten().flatten().filter(|e| e.file_type().is_ok_and(|t| t.is_file()) && super::downloads::wanted(&e.path())).take(2).count() >= 2
}

/// A name without bracketed tags: "Mycopunk [FitGirl Repack]", "Game (DODI Repack)".
fn without_tags(name: &str) -> String {
    let mut out = String::new();
    let mut depth = 0;
    for c in name.chars() {
        match c {
            '[' | '(' => depth += 1,
            ']' | ')' if depth > 0 => depth -= 1,
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    let out = out.split_whitespace().collect::<Vec<_>>().join(" ");
    if out.is_empty() { name.to_string() } else { out }
}

/// A Windows installer at the top of the folder (Inno Setup, NSIS ... or named setup/install).
fn top_installer(dir: &Path) -> Option<PathBuf> {
    let mut exes: Vec<PathBuf> = fs::read_dir(dir).ok()?.flatten().map(|e| e.path()).filter(|p| p.is_file() && p.extension().is_some_and(|x| x.eq_ignore_ascii_case("exe"))).collect();
    exes.sort();
    exes.into_iter().find(|p| {
        let name = p.file_stem().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
        match gamefix::binary::identify(p) {
            Some(gamefix::binary::Binary::Pe(pe)) => pe.installer.is_some() || name.contains("setup") || name.contains("install"),
            _ => false,
        }
    })
}

/// Folders between a game's folder and its program that say nothing about the game.
const PACKAGING: &[&str] = &["windowsnoeditor", "windows", "win32", "win64", "engine", "binaries", "bin", "game", "build", "release", "x64", "x86", "data"];

/// A folder name that reads like a title ("Gnome Issues") rather than a download's file name
/// ("gnomeissues", "pb_ver.1.03").
fn reads_like_title(name: &str) -> bool {
    name.contains(' ') || (name.chars().any(|c| c.is_uppercase()) && name.chars().any(|c| c.is_lowercase()) && !name.contains('_'))
}

/// The game's name: what scan found (a Unity game's product name), else the folder's name, or
/// the folder inside it holding the program when that one reads like a title and the outer one
/// doesn't (`gnomeissues/Gnome Issues/...`).
fn title(root: &Path, scanned: &str, program: Option<&Path>) -> String {
    let folder = root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    if scanned != folder || reads_like_title(&folder) {
        return scanned.to_string();
    }
    let inner = program.and_then(|p| p.strip_prefix(root).ok()).and_then(|rel| {
        let mut parts = rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect::<Vec<_>>();
        parts.pop();
        parts.into_iter().find(|p| {
            let p = p.to_lowercase();
            // WindowsNoEditor32-Bit too.
            !PACKAGING.contains(&p.as_str()) && !p.starts_with("windowsnoeditor")
        })
    });
    match inner {
        Some(inner) if reads_like_title(&inner) => inner,
        _ => scanned.to_string(),
    }
}

fn subdirs(dir: &Path) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()) && !skipped(&e.file_name().to_string_lossy()))
        .map(|e| e.path())
        .collect();
    dirs.sort();
    dirs
}

fn has_program_at_top(dir: &Path) -> bool {
    fs::read_dir(dir).into_iter().flatten().flatten().any(|e| {
        let p = e.path();
        let ext = p.extension().map(|x| x.to_string_lossy().to_lowercase()).unwrap_or_default();
        e.file_type().is_ok_and(|t| t.is_file())
            && (["exe", "sh", "x86_64", "x86", "love", "jar", "swf", "appimage", "bat"].contains(&ext.as_str())
                || (ext.is_empty() && gamefix::binary::identify(&p).is_some_and(|b| matches!(b, Binary::Elf(_)))))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unreal_engine_isnt_a_game() {
        let dir = std::env::temp_dir().join(format!("lobby-ue-{}", std::process::id()));
        fs::create_dir_all(dir.join("UE_5.8/Engine/Build")).unwrap();
        fs::write(dir.join("UE_5.8/Engine/Build/InstalledBuild.txt"), "").unwrap();
        fs::create_dir_all(dir.join("Earth Rush/Engine/Binaries/ThirdParty")).unwrap();
        assert!(unreal_engine(&dir.join("UE_5.8")));
        assert!(!unreal_engine(&dir.join("Earth Rush")));
        assert_eq!(without_tags("Mycopunk [FitGirl Repack]"), "Mycopunk");
        let coll = dir.join("Downloaded Games");
        fs::create_dir_all(&coll).unwrap();
        for f in ["a.zip", "b.rar"] {
            fs::write(coll.join(f), "").unwrap();
        }
        assert!(collection(&coll));
        assert!(!collection(&dir.join("UE_5.8")));
        assert_eq!(without_tags("Some Game (DODI Repack) v1.2"), "Some Game v1.2");
        assert_eq!(without_tags("[tags only]"), "[tags only]");
        fs::remove_dir_all(&dir).unwrap();
    }
}
