//! Folders of games: ~/Games, gamefix's games folder, a "Games" folder on each drive, and
//! Downloads. Each folder in them is looked at with gamefix's scan, which picks the program
//! that starts a game; a folder holding several games is looked into.

use super::Home;
use crate::library::{self, Entry, Kind, Source};
use gamefix::binary::Binary;
use gamefix::{disc, scan};
use std::fs;
use std::path::{Path, PathBuf};

pub fn roots(home: &Home) -> Vec<PathBuf> {
    let mut roots = vec![home.join("Games")];
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
        }
    }
    roots.push(home.join("Downloads"));
    let mut out: Vec<PathBuf> = Vec::new();
    for r in roots {
        let r = library::norm(&r);
        if r.is_dir() && !out.contains(&r) {
            out.push(r);
        }
    }
    out
}

/// Folders in a games folder that aren't games.
fn skipped(name: &str) -> bool {
    let n = name.to_lowercase();
    name.starts_with('.') || n.contains("backup") || ["steamlibrary", "prefixes", "jre", "jdk", "tools", "shots", "screenshots", "saves", "umu", "lost+found", "$recycle.bin", "system volume information"].contains(&n.as_str())
}

enum Found {
    Game(Entry),
    Prefix(PathBuf, Option<Entry>),
    Nothing,
}

/// Each folder in the roots, as games, downloads not installed yet, and prefixes. Folders a
/// launcher already names as a game's are left to that launcher.
pub fn scan(roots: &[PathBuf], known: &[Entry]) -> (Vec<Entry>, Vec<PathBuf>) {
    let known: Vec<PathBuf> = known.iter().filter_map(|e| e.folder.as_deref()).map(library::norm).collect();
    let mut candidates = Vec::new();
    for root in roots {
        for entry in fs::read_dir(root).into_iter().flatten().flatten() {
            let path = entry.path();
            if entry.file_type().is_ok_and(|t| t.is_dir()) && !skipped(&entry.file_name().to_string_lossy()) && !roots.contains(&library::norm(&path)) {
                candidates.push(path);
            }
        }
    }
    candidates.sort();
    candidates.dedup_by(|a, b| library::norm(a) == library::norm(b));

    // Folders are independent, and big games take a while to walk: several at once.
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).min(8);
    let chunks: Vec<Vec<PathBuf>> = (0..threads).map(|t| candidates.iter().skip(t).step_by(threads).cloned().collect()).collect();
    let results: Vec<Found> = std::thread::scope(|s| {
        let handles: Vec<_> = chunks.iter().map(|chunk| s.spawn(|| chunk.iter().flat_map(|dir| look(dir, &known, 0)).collect::<Vec<_>>())).collect();
        handles.into_iter().flat_map(|h| h.join().unwrap_or_default()).collect()
    });

    let mut entries = Vec::new();
    let mut prefixes = Vec::new();
    for found in results {
        match found {
            Found::Game(e) => entries.push(e),
            Found::Prefix(p, fallback) => {
                prefixes.push(p);
                entries.extend(fallback);
            }
            Found::Nothing => {}
        }
    }
    (entries, prefixes)
}

fn look(dir: &Path, known: &[PathBuf], depth: usize) -> Vec<Found> {
    let norm = library::norm(dir);
    if known.contains(&norm) {
        return vec![];
    }
    if let Some(pfx) = library::prefix_in(dir) {
        // A prefix whose installer made no shortcuts: the program it put in Program Files.
        let fallback = if disc::shortcuts(&pfx).is_empty() {
            let name = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            match disc::installed_program(&pfx, &name) {
                disc::Installed::Program(exe, label) => {
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
        return if depth < 2 { subs.iter().flat_map(|d| look(d, known, depth + 1)).collect() } else { vec![] };
    }
    // A folder of games has no program of its own at the top, and its folders are games of
    // their own. Not Unreal's layout (Engine beside the game's folder).
    let unreal = subs.iter().any(|d| d.file_name().is_some_and(|n| n == "Engine"));
    if depth == 0 && !unreal && !has_program_at_top(dir) {
        let inner: Vec<Found> = subs.iter().flat_map(|d| look(d, known, 1)).collect();
        let games = inner.iter().filter(|f| !matches!(f, Found::Nothing)).count();
        if games >= 2 {
            return inner;
        }
    }
    let Some(game) = scan::scan(dir) else { return vec![Found::Nothing] };
    // GOG's own idea of what starts the game, over a guess.
    let gog = gamefix::gog::read(&game.root).map(|info| info.program);
    let program = gog.or_else(|| game.main.or(game.windows).or(game.portable).map(|i| game.full(&game.files[i])));
    let download = [game.cd_installer, game.linux_installer, game.packed, game.disc, game.archive].into_iter().flatten().next();
    let mut e = Entry::new(Source::Folder, title(&game.root, &game.name, program.as_deref()));
    e.folder = Some(game.root.clone());
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
        (None, None) => return vec![Found::Nothing],
    }
    vec![Found::Game(e)]
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
