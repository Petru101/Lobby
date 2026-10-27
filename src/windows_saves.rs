//! Saves from Windows: a game's saves found in a Windows user folder on the drives (a Windows
//! still installed, or a backup of one: a folder holding NTUSER.DAT), brought into the Wine
//! prefix the game runs in here. Where each game keeps its saves comes from Ludusavi's manifest
//! (github.com/mtkennerly/ludusavi-manifest, from PCGamingWiki: ~53k games), fetched weekly
//! and kept as a small index (~/.cache/lobby/save-paths.json).
//!
//! Saves are copied, never linked (the backup stays a backup, and the game keeps its saves
//! when that drive isn't plugged in). A file is copied when the prefix lacks it or has an older
//! copy, which is kept first in ~/.local/share/lobby/save-backups/<game>/<time>/.

use crate::library::{plain, Game, Key, Source};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

const MANIFEST_URL: &str = "https://raw.githubusercontent.com/mtkennerly/ludusavi-manifest/master/data/manifest.yaml";
const WEEK: u64 = 7 * 24 * 3600;

/// One game's Windows save paths, as the manifest writes them ("<winDocuments>/StarCraft
/// II/Accounts").
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct SavePaths {
    pub name: String,
    #[serde(default)]
    steam: Option<u64>,
    #[serde(default)]
    gog: Option<u64>,
    /// Stores whose cloud keeps its saves ("steam", "gog", "epic").
    #[serde(default)]
    pub(crate) cloud: Vec<String>,
    paths: Vec<String>,
    /// Every save path, for the game's save folders here (`save_folders`): its install folder's
    /// (<base>), Windows' shared folders, Linux's; with the system it applies to, if one.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub all: Vec<(String, Option<String>)>,
}

#[derive(Serialize, Deserialize, Default)]
struct Index {
    fetched: u64,
    games: Vec<SavePaths>,
    /// 2: with `all`.
    #[serde(default)]
    version: u32,
}

const INDEX_VERSION: u32 = 2;

fn index_file() -> PathBuf {
    crate::stores::cache_dir().join("save-paths.json")
}

/// The manifest as written (only what's read here).
#[derive(Deserialize)]
struct ManifestGame {
    #[serde(default)]
    files: HashMap<String, FileRule>,
    #[serde(default)]
    steam: Option<StoreId>,
    #[serde(default)]
    gog: Option<StoreId>,
    #[serde(default)]
    cloud: HashMap<String, bool>,
}

#[derive(Deserialize)]
struct StoreId {
    id: Option<u64>,
}

#[derive(Deserialize, Default)]
struct FileRule {
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    when: Vec<When>,
}

#[derive(Deserialize)]
struct When {
    os: Option<String>,
}

/// A path is one to look for when it's tagged a save, applies to Windows, and starts at a
/// Windows user folder's own folders (not the game's install folder, the registry, Linux's).
fn usable(path: &str, rule: &FileRule) -> bool {
    let windows = rule.when.is_empty() || rule.when.iter().any(|w| w.os.as_deref().is_none_or(|os| os == "windows"));
    let user = ["<home>", "<winDocuments>", "<winAppData>", "<winLocalAppData>"].iter().any(|p| path.starts_with(p));
    rule.tags.iter().any(|t| t == "save") && windows && user
}

fn parse_manifest(text: &str) -> Result<Vec<SavePaths>, String> {
    let manifest: HashMap<String, ManifestGame> = serde_yaml::from_str(text).map_err(|e| format!("Ludusavi's manifest: {e}"))?;
    let mut games: Vec<SavePaths> = manifest
        .into_iter()
        .filter_map(|(name, g)| {
            let mut paths: Vec<String> = g.files.iter().filter(|(p, r)| usable(p, r)).map(|(p, _)| p.clone()).collect();
            paths.sort();
            let mut all: Vec<(String, Option<String>)> = g
                .files
                .iter()
                .filter(|(_, r)| r.tags.iter().any(|t| t == "save"))
                .flat_map(|(p, r)| if r.when.is_empty() { vec![(p.clone(), None)] } else { r.when.iter().map(|w| (p.clone(), w.os.clone())).collect() })
                .collect();
            all.sort();
            all.dedup();
            let cloud = g.cloud.iter().filter(|(_, on)| **on).map(|(s, _)| s.clone()).collect();
            (!all.is_empty()).then(|| SavePaths { name, steam: g.steam.and_then(|s| s.id), gog: g.gog.and_then(|s| s.id), cloud, paths, all })
        })
        .collect();
    games.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(games)
}

/// The index, fetched again when over a week old (the old one serves when that fails).
pub(crate) fn index() -> Vec<SavePaths> {
    let saved: Option<Index> = fs::read_to_string(index_file()).ok().and_then(|t| serde_json::from_str(&t).ok());
    if let Some(i) = saved.as_ref().filter(|i| i.version >= INDEX_VERSION && crate::store::now().saturating_sub(i.fetched) < WEEK) {
        return i.games.clone();
    }
    let fetched = crate::log::timed("windows saves: Ludusavi's manifest", || {
        let out = std::process::Command::new("curl").args(["-fsSL", "--retry", "2", "--max-time", "120", MANIFEST_URL]).stdin(std::process::Stdio::null()).output().map_err(|e| e.to_string())?;
        if !out.status.success() {
            return Err("couldn't fetch Ludusavi's manifest".to_string());
        }
        parse_manifest(&String::from_utf8_lossy(&out.stdout))
    });
    match fetched {
        Ok(games) => {
            let _ = fs::create_dir_all(index_file().parent().unwrap());
            let _ = fs::write(index_file(), serde_json::to_string(&Index { fetched: crate::store::now(), games: games.clone(), version: INDEX_VERSION }).unwrap());
            games
        }
        Err(e) => {
            crate::log::line(&format!("windows saves: {e}"));
            saved.map(|i| i.games).unwrap_or_default()
        }
    }
}

/// The manifest's entry for a game: by Steam or GOG id, else by name.
pub(crate) fn paths_for<'a>(game: &Game, index: &'a [SavePaths]) -> Option<&'a SavePaths> {
    let keys = || game.entries.iter().flat_map(|e| e.keys.iter());
    if let Some(id) = keys().find_map(|k| if let Key::SteamApp(id) = k { Some(u64::from(*id)) } else { None }) {
        if let Some(p) = index.iter().find(|p| p.steam == Some(id)) {
            return Some(p);
        }
    }
    if let Some(id) = keys().find_map(|k| if let Key::Store(s, id) = k { (s == "gog").then(|| id.parse::<u64>().ok()).flatten() } else { None }) {
        if let Some(p) = index.iter().find(|p| p.gog == Some(id)) {
            return Some(p);
        }
    }
    let name = plain(&game.name);
    index.iter().find(|p| plain(&p.name) == name)
}

/// Windows user folders on the drives: a drive's Users/<name>, or a folder up to three down
/// holding NTUSER.DAT (a backup of one: "Windows-backup/Petru").
pub fn user_folders() -> Vec<PathBuf> {
    let user = std::env::var("USER").unwrap_or_default();
    let drives: Vec<PathBuf> = [PathBuf::from("/run/media").join(&user), PathBuf::from("/media").join(&user), PathBuf::from("/mnt")]
        .iter()
        .flat_map(|m| fs::read_dir(m).into_iter().flatten().flatten().map(|d| d.path()))
        .collect();
    let mut out: Vec<PathBuf> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for drive in drives {
        let real = fs::canonicalize(&drive).unwrap_or(drive);
        if seen.insert(real.clone()) {
            walk(&real, 3, &mut out);
        }
    }
    out
}

fn walk(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    let entries: Vec<_> = entries.flatten().collect();
    if entries.iter().any(|e| e.file_name().eq_ignore_ascii_case("ntuser.dat")) {
        let name = dir.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
        if !["default", "default user", "public", "all users"].contains(&name.as_str()) {
            out.push(dir.to_path_buf());
        }
        return;
    }
    if depth == 0 {
        return;
    }
    for e in entries {
        let n = e.file_name().to_string_lossy().to_lowercase();
        let passed = n.starts_with('$') || n.starts_with('.') || ["windows", "program files", "program files (x86)", "programdata", "system volume information", "steamlibrary", "appdata"].contains(&n.as_str());
        if e.file_type().is_ok_and(|t| t.is_dir()) && !passed {
            walk(&e.path(), depth - 1, out);
        }
    }
}

/// Where a manifest path's start is in a Windows user folder, and in the prefix's user folder.
fn bases(path: &str, windows: &Path, prefix_user: &Path) -> Option<(Vec<(PathBuf, PathBuf)>, String)> {
    let (start, rest) = path.split_once('/').unwrap_or((path, ""));
    let docs = |w: &Path| vec![(w.join("Documents"), prefix_user.join("Documents")), (w.join("OneDrive/Documents"), prefix_user.join("Documents"))];
    let pairs = match start {
        "<home>" => vec![(windows.to_path_buf(), prefix_user.to_path_buf())],
        "<winDocuments>" => docs(windows),
        "<winAppData>" => vec![(windows.join("AppData/Roaming"), prefix_user.join("AppData/Roaming"))],
        "<winLocalAppData>" => vec![(windows.join("AppData/Local"), prefix_user.join("AppData/Local"))],
        _ => return None,
    };
    let user = windows.file_name()?.to_string_lossy().to_string();
    Some((pairs, rest.replace("<storeUserId>", "*").replace("<osUserName>", &user)))
}

/// Whether a name matches a pattern with * and ?, ignoring case (Windows paths do).
fn matches(pattern: &str, name: &str) -> bool {
    let (p, n): (Vec<char>, Vec<char>) = (pattern.to_lowercase().chars().collect(), name.to_lowercase().chars().collect());
    let (mut pi, mut ni, mut star, mut back) = (0, 0, None, 0);
    while ni < n.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == n[ni]) {
            pi += 1;
            ni += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            back = ni;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            back += 1;
            ni = back;
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|c| *c == '*')
}

/// The paths under `dir` matching the pattern's parts, each part ignoring case ("**" any depth).
pub(crate) fn expand(dir: &Path, parts: &[&str], out: &mut Vec<PathBuf>) {
    let Some((first, rest)) = parts.split_first() else {
        out.push(dir.to_path_buf());
        return;
    };
    if *first == "**" {
        expand(dir, rest, out);
        for e in fs::read_dir(dir).into_iter().flatten().flatten().filter(|e| e.file_type().is_ok_and(|t| t.is_dir())) {
            expand(&e.path(), parts, out);
        }
        return;
    }
    for e in fs::read_dir(dir).into_iter().flatten().flatten() {
        if matches(first, &e.file_name().to_string_lossy()) {
            expand(&e.path(), rest, out);
        }
    }
}

/// Every file under a path (itself when it's a file).
fn files_under(path: &Path, out: &mut Vec<PathBuf>) {
    match fs::symlink_metadata(path) {
        Ok(m) if m.is_dir() => {
            for e in fs::read_dir(path).into_iter().flatten().flatten() {
                files_under(&e.path(), out);
            }
        }
        Ok(m) if m.is_file() => out.push(path.to_path_buf()),
        _ => {}
    }
}

/// One file to bring over.
#[derive(Debug, Clone)]
pub struct Copy {
    pub from: PathBuf,
    pub to: PathBuf,
    /// The prefix has an older copy (kept before it's replaced).
    pub replaces: bool,
}

#[derive(Debug, Default)]
pub struct Found {
    /// The Windows user folder they're in.
    pub windows: PathBuf,
    /// Files found there.
    pub files: usize,
    pub bytes: u64,
    pub newest: Option<SystemTime>,
    /// What isn't here yet, or is older here.
    pub copies: Vec<Copy>,
}

pub enum Saves {
    /// The manifest doesn't know the game, or none of its saves are in a Windows user folder.
    None,
    /// The store's cloud keeps them (named), so it brings them back itself.
    Cloud(&'static str, Found),
    /// Found, but there's no Wine prefix yet: it's made the first time the game is played.
    NoPrefix(Found),
    Found { found: Found, prefix: PathBuf },
}

/// The Wine prefix the game runs in here, whether or not it exists yet.
pub fn prefix(game: &Game) -> Option<PathBuf> {
    if crate::battlenet::game_folder(game).is_some() && matches!(crate::launch::plan(game), crate::launch::Plan::BattleNet { .. }) {
        return Some(crate::battlenet::prefix());
    }
    // A store app's game runs in Lobby's prefix of the app (`clients`).
    if let Some((client, _)) = crate::clients::needed(game) {
        return Some(client.prefix());
    }
    // Steam's: compatdata/<appid>/pfx in the game's library (made by Steam on the first play).
    for e in game.entries.iter().filter(|e| e.source == Source::Steam) {
        let app = e.keys.iter().find_map(|k| if let Key::SteamApp(id) = k { Some(*id) } else { None });
        if let (Some(app), Some(steamapps)) = (app, e.folder.as_deref().and_then(Path::parent).and_then(Path::parent)) {
            return Some(steamapps.join("compatdata").join(app.to_string()).join("pfx"));
        }
    }
    crate::uninstall::prefix(game).or_else(|| game.entries.iter().find_map(|e| e.prefix.clone()))
}

/// The prefix's user folder (Proton's steamuser, else Wine's own user).
pub(crate) fn prefix_user(prefix: &Path) -> PathBuf {
    let users = prefix.join("drive_c/users");
    if users.join("steamuser").is_dir() {
        return users.join("steamuser");
    }
    fs::read_dir(&users)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .find(|p| p.is_dir() && !p.ends_with("Public"))
        .unwrap_or_else(|| users.join("steamuser"))
}

/// What a Windows user folder holds of the game's saves, and what of it to copy.
pub(crate) fn look(paths: &SavePaths, windows: &Path, prefix_user: &Path) -> Found {
    let mut found = Found { windows: windows.to_path_buf(), ..Default::default() };
    let mut seen = std::collections::HashSet::new();
    for path in &paths.paths {
        let Some((pairs, rest)) = bases(path, windows, prefix_user) else { continue };
        let parts: Vec<&str> = rest.split('/').filter(|p| !p.is_empty()).collect();
        for (from_base, to_base) in pairs {
            let mut matched = Vec::new();
            expand(&from_base, &parts, &mut matched);
            let mut files = Vec::new();
            for m in matched {
                files_under(&m, &mut files);
            }
            for file in files {
                if !seen.insert(file.clone()) {
                    continue;
                }
                let Ok(meta) = fs::metadata(&file) else { continue };
                let modified = meta.modified().ok();
                found.files += 1;
                found.bytes += meta.len();
                found.newest = found.newest.max(modified);
                let to = to_base.join(file.strip_prefix(&from_base).unwrap());
                let here = fs::metadata(&to).ok().and_then(|m| m.modified().ok());
                match (here, modified) {
                    (None, _) => found.copies.push(Copy { from: file, to, replaces: false }),
                    // Older here by more than FAT's 2 s rounding.
                    (Some(h), Some(m)) if m.duration_since(h).is_ok_and(|d| d.as_secs() > 2) => found.copies.push(Copy { from: file, to, replaces: true }),
                    _ => {}
                }
            }
        }
    }
    found
}

/// The game's saves on Windows: in the Windows user folder holding the newest of them.
pub fn find(game: &Game) -> Saves {
    let index = index();
    let Some(paths) = paths_for(game, &index) else { return Saves::None };
    let prefix = prefix(game);
    let user = prefix.as_deref().map(prefix_user).unwrap_or_else(|| PathBuf::from("/nonexistent/drive_c/users/steamuser"));
    let Some(found) = user_folders().iter().map(|w| look(paths, w, &user)).filter(|f| f.files > 0).max_by_key(|f| f.newest) else { return Saves::None };
    let store_cloud = |store: &str| paths.cloud.iter().any(|c| c == store);
    let from = |s: Source| game.entries.iter().any(|e| e.source == s);
    if from(Source::Steam) && store_cloud("steam") {
        return Saves::Cloud("Steam Cloud", found);
    }
    // lobby syncs Epic's and GOG's cloud saves for the games it installed (not Lutris's).
    if let Some((store, _)) = crate::stores::installed_in(game).filter(|(s, _)| store_cloud(s.id())) {
        return Saves::Cloud(if store == crate::stores::Store::Epic { "Epic's cloud saves" } else { "GOG's cloud saves" }, found);
    }
    match prefix {
        Some(p) if p.join("drive_c").is_dir() => Saves::Found { found, prefix: p },
        _ => Saves::NoPrefix(found),
    }
}

/// Copies what's missing or older in the prefix, keeping the files it replaces. Returns how
/// many it copied and replaced.
pub fn bring_over(game_id: &str, found: &Found) -> Result<(usize, usize), String> {
    let backup = crate::store::dir().join("save-backups").join(game_id).join(crate::store::now().to_string());
    let (mut copied, mut replaced) = (0, 0);
    for c in &found.copies {
        if c.replaces {
            let kept = backup.join(c.to.strip_prefix("/").unwrap_or(&c.to));
            fs::create_dir_all(kept.parent().unwrap()).map_err(|e| e.to_string())?;
            fs::copy(&c.to, &kept).map_err(|e| format!("{}: {e}", c.to.display()))?;
            replaced += 1;
        }
        fs::create_dir_all(c.to.parent().unwrap()).map_err(|e| format!("{}: {e}", c.to.display()))?;
        fs::copy(&c.from, &c.to).map_err(|e| format!("{}: {e}", c.from.display()))?;
        // The save's own time, so newer-or-older stays right (and games that look at it agree).
        if let Ok(t) = fs::metadata(&c.from).and_then(|m| m.modified()) {
            let _ = fs::File::options().write(true).open(&c.to).and_then(|f| f.set_modified(t));
        }
        copied += 1;
    }
    crate::log::line(&format!("windows saves: {game_id}: copied {copied} from {}, {replaced} replaced (kept in {})", found.windows.display(), backup.display()));
    Ok((copied, replaced))
}

pub fn size_text(bytes: u64) -> String {
    match bytes {
        b if b >= 1 << 40 => format!("{:.1} TB", b as f64 / (1u64 << 40) as f64),
        b if b >= 1 << 30 => format!("{:.1} GB", b as f64 / (1u64 << 30) as f64),
        b if b >= 1 << 20 => format!("{:.0} MB", b as f64 / (1u64 << 20) as f64),
        b => format!("{} KB", b.div_ceil(1024)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wildcards_ignore_case() {
        assert!(matches("*.sav", "SLOT1.SAV"));
        assert!(matches("starcraft ii", "StarCraft II"));
        assert!(matches("save?", "save1"));
        assert!(!matches("*.sav", "slot1.cfg"));
    }

    #[test]
    fn reads_the_manifest() {
        let text = r#"
StarCraft II:
  files:
    "<home>/Library/Application Support/Blizzard/StarCraft II/Accounts":
      tags:
        - save
      when:
        - os: mac
    "<winDocuments>/StarCraft II/Accounts":
      tags:
        - config
        - save
      when:
        - os: windows
  id:
    lutris: starcraft-ii
Some Game:
  cloud:
    steam: true
  files:
    "<base>/saves":
      tags:
        - save
    "<home>/Saved Games/Some Game":
      tags:
        - save
    "<winAppData>/Some Game/settings.ini":
      tags:
        - config
  steam:
    id: 123
"#;
        let games = parse_manifest(text).unwrap();
        assert_eq!(games.len(), 2);
        assert_eq!(games[0].name, "Some Game");
        assert_eq!(games[0].paths, ["<home>/Saved Games/Some Game"]);
        assert_eq!((games[0].steam, games[0].cloud.clone()), (Some(123), vec!["steam".to_string()]));
        assert_eq!(games[1].paths, ["<winDocuments>/StarCraft II/Accounts"]);
    }

    #[test]
    fn finds_and_copies() {
        let dir = std::env::temp_dir().join(format!("lobby-winsaves-{}", std::process::id()));
        let windows = dir.join("backup/Petru");
        let user = dir.join("pfx/drive_c/users/steamuser");
        fs::create_dir_all(windows.join("Documents/StarCraft II/Accounts/1/2-S2")).unwrap();
        fs::create_dir_all(user.join("Documents/StarCraft II/Accounts/1/2-S2")).unwrap();
        fs::write(windows.join("NTUSER.DAT"), b"").unwrap();
        fs::write(windows.join("Documents/StarCraft II/Accounts/1/2-S2/a.SC2Bank"), b"old pc").unwrap();
        fs::write(windows.join("Documents/StarCraft II/Accounts/1/2-S2/b.SC2Bank"), b"old pc").unwrap();
        // Already here and newer: left alone.
        std::thread::sleep(std::time::Duration::from_millis(10));
        fs::write(user.join("Documents/StarCraft II/Accounts/1/2-S2/b.SC2Bank"), b"played here").unwrap();
        let paths = SavePaths { name: "StarCraft II".into(), paths: vec!["<winDocuments>/starcraft ii/Accounts".into()], ..Default::default() };
        let found = look(&paths, &windows, &user);
        assert_eq!(found.files, 2);
        assert_eq!(found.copies.len(), 1);
        assert_eq!(found.copies[0].to, user.join("Documents/StarCraft II/Accounts/1/2-S2/a.SC2Bank"));
        let mut game = crate::library::Game { id: "t".into(), name: "T".into(), kind: crate::library::Kind::Game, entries: vec![], missing: false, gone: false, added: 0, played: None, chosen_cover: None, launch_options: None, protection: Default::default(), hidden: false, favourite: false, frame_rate: false };
        game.id = format!("test-{}", std::process::id());
        let (copied, replaced) = bring_over(&game.id, &found).unwrap();
        assert_eq!((copied, replaced), (1, 0));
        assert_eq!(fs::read(user.join("Documents/StarCraft II/Accounts/1/2-S2/a.SC2Bank")).unwrap(), b"old pc");
        assert_eq!(fs::read(user.join("Documents/StarCraft II/Accounts/1/2-S2/b.SC2Bank")).unwrap(), b"played here");
        assert!(look(&paths, &windows, &user).copies.is_empty());
        fs::remove_dir_all(&dir).unwrap();
    }
}
