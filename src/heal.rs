//! Games moved by hand (in a file manager, not with Lobby's Move): what still names the old
//! place (gamefix's records and app-menu entries, Steam shortcuts, Lobby's own files) is pointed
//! at where the game is now, after each scan. Without it the old records stay behind as a
//! second, missing game, or Play aims at the old place while the game sits in the new one.

use crate::library::{Game, Kind};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// A folder (or a one-file game's file) that's gone, where it is now, and the game naming it.
pub struct Moved {
    pub from: PathBuf,
    pub to: PathBuf,
    pub game: usize,
}

/// The folders drives are opened into: a game "gone" from one is on a drive that isn't open.
fn drives_folder(p: &Path) -> bool {
    let parts: Vec<_> = p.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
    matches!(parts.as_slice(), [_, run, media, _] if run == "run" && media == "media") || matches!(parts.as_slice(), [_, media, _] if media == "media") || parts.len() <= 2
}

/// Whether a folder or file is named after the game ("Carnival Zero v1" for Carnival Zero): a
/// folder called Windows or bin says nothing about which game it was.
fn named_after(place: &Path, names: &[&str]) -> bool {
    let slug = |s: &str| gamefix::setup::slug(s);
    let whole = place.file_name().map(|n| slug(&n.to_string_lossy())).unwrap_or_default();
    let stem = place.file_stem().map(|n| slug(&n.to_string_lossy())).unwrap_or_default();
    names.iter().map(|n| slug(n)).any(|n| [&whole, &stem].iter().any(|p| !p.is_empty() && (p.starts_with(&n) || n.starts_with(p.as_str()))))
}

/// What moved: a place some entry names that's gone (the topmost folder of it that is, its
/// parent still there), found under the same name where another entry has the game now, with
/// the files the old entries named inside it. Only when exactly one place fits.
pub fn find(games: &[Game]) -> Vec<Moved> {
    let entries = || games.iter().enumerate().filter(|(_, g)| !g.gone).flat_map(|(i, g)| g.entries.iter().filter(|e| e.kind == Kind::Game).map(move |e| (i, g, e)));
    let live: Vec<&PathBuf> = entries().flat_map(|(_, _, e)| [e.folder.as_ref(), e.program.as_ref()]).flatten().filter(|p| p.exists()).collect();
    // The old place, with the game, the paths inside it that were named, and the names to go by.
    let mut gone: BTreeMap<PathBuf, (usize, Vec<PathBuf>, Vec<&str>)> = BTreeMap::new();
    for (i, g, e) in entries() {
        for p in [e.folder.as_ref(), e.program.as_ref()].into_iter().flatten().filter(|p| p.is_absolute() && !p.exists()) {
            let mut from = p.as_path();
            while let Some(parent) = from.parent().filter(|parent| !parent.exists()) {
                from = parent;
            }
            if from.parent().is_none_or(drives_folder) {
                continue;
            }
            let at = gone.entry(from.to_path_buf()).or_insert((i, Vec::new(), Vec::new()));
            at.1.extend(p.strip_prefix(from).ok().filter(|rel| !rel.as_os_str().is_empty()).map(Path::to_path_buf));
            at.2.extend([g.name.as_str(), e.name.as_str()]);
        }
    }
    let mut moved = Vec::new();
    for (from, (game, inside, names)) in gone {
        if !named_after(&from, &names) {
            continue;
        }
        let mut places: Vec<PathBuf> = live
            .iter()
            .flat_map(|l| l.ancestors())
            .filter(|a| a.file_name() == from.file_name() && *a != from && inside.iter().all(|rel| a.join(rel).exists()))
            .map(Path::to_path_buf)
            .collect();
        places.sort();
        places.dedup();
        if let [to] = places.as_slice() {
            moved.push(Moved { from, to: to.clone(), game });
        }
    }
    moved
}

/// Points everything at the games' new places; says what it did (`dry`: what it would do).
pub fn run(games: &[Game], dry: bool) -> Vec<String> {
    let mut done = Vec::new();
    for m in find(games) {
        let name = &games[m.game].name;
        if dry {
            done.push(format!("{name} moved from {} to {}", m.from.display(), m.to.display()));
            continue;
        }
        // Its Windows setup, when that was beside it and came along.
        let setup = crate::setups::follow(&m.from, &m.to) as usize;
        match crate::moving::follow(&m.from, &m.to, &games[m.game]).map(|n| n + setup) {
            Ok(0) => {}
            Ok(n) => {
                crate::log::line(&format!("found again: {name} moved from {} to {}: {n} records pointed there", m.from.display(), m.to.display()));
                done.push(format!("{name} moved from {} to {}: pointed {n} records there", m.from.display(), m.to.display()));
            }
            Err(e) => crate::log::line(&format!("found again: {name} in {}: {e}", m.to.display())),
        }
    }
    done
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::{Entry, Source};
    use std::fs;

    fn game(name: &str, entries: Vec<Entry>) -> Game {
        Game { id: name.into(), name: name.into(), kind: Kind::Game, entries, missing: false, gone: false, added: 0, played: None, chosen_cover: None, launch_options: None, protection: Default::default(), hidden: false, favourite: false, frame_rate: false, proton: None, kept_cover: None, kept_icon: None, controller: None }
    }

    fn entry(source: Source, name: &str, folder: Option<&Path>, program: Option<PathBuf>) -> Entry {
        let mut e = Entry::new(source, name);
        e.folder = folder.map(Path::to_path_buf);
        e.program = program;
        e
    }

    #[test]
    fn finds_where_a_game_moved_by_hand_went() {
        let dir = std::env::temp_dir().join(format!("lobby-heal-{}", std::process::id()));
        let (old, new) = (dir.join("Downloads"), dir.join("Downloads/Kept"));
        for f in ["Carnival Zero v1/Carnival.exe", "simant-colony/SIMANT.EXE", "Shield.exe", "Other/Windows/Game.exe"] {
            fs::create_dir_all(new.join(f).parent().unwrap()).unwrap();
            fs::write(new.join(f), "").unwrap();
        }
        let games = vec![
            // One game, known in both places: the old one is gone.
            game("Carnival Zero", vec![
                entry(Source::SteamShortcut, "Carnival Zero", Some(&old.join("Carnival Zero v1")), Some(old.join("Carnival Zero v1/Carnival.exe"))),
                entry(Source::Folder, "Carnival Zero", Some(&new.join("Carnival Zero v1")), Some(new.join("Carnival Zero v1/Carnival.exe"))),
            ]),
            // Old records left as a game of their own: a folder only, and a one-file game.
            game("simant-colony", vec![entry(Source::Gamefix, "simant-colony", Some(&old.join("simant-colony")), None)]),
            game("SimAnt Colony (DOS)", vec![entry(Source::AppMenu, "SimAnt", Some(&new.join("simant-colony")), Some(new.join("simant-colony/SIMANT.EXE")))]),
            game("Shield", vec![entry(Source::SteamShortcut, "Shield", None, Some(old.join("Shield.exe")))]),
            game("Shield", vec![entry(Source::Folder, "Shield", None, Some(new.join("Shield.exe")))]),
            // Deleted, not moved: another game's folder of the same plain name isn't it.
            game("Deleted", vec![entry(Source::Gamefix, "Deleted", Some(&old.join("Windows")), Some(old.join("Windows/Game.exe")))]),
            game("Other", vec![entry(Source::Folder, "Other", Some(&new.join("Other/Windows")), Some(new.join("Other/Windows/Game.exe")))]),
            // On a drive that isn't open: left alone.
            game("Away", vec![entry(Source::Folder, "Away", Some(Path::new("/run/media/nobody/Away")), None)]),
        ];
        let found: Vec<(PathBuf, PathBuf)> = find(&games).into_iter().map(|m| (m.from, m.to)).collect();
        assert_eq!(found, vec![
            (old.join("Carnival Zero v1"), new.join("Carnival Zero v1")),
            (old.join("Shield.exe"), new.join("Shield.exe")),
            (old.join("simant-colony"), new.join("simant-colony")),
        ]);
        fs::remove_dir_all(&dir).unwrap();
    }
}
