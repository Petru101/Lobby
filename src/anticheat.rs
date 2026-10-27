//! Games whose online play may not work on Linux, said on their page before Play:
//! - anti-cheat a game ships (its files in the game's folder: Ferret's list, from
//!   ~/Projects/ferret helper/src/anticheat.rs, both GPL-3.0 and the user's);
//! - AreWeAntiCheatYet's list (github.com/AreWeAntiCheatYet, MIT): whether a game's
//!   anti-cheat works under Proton ("Supported", "Running"), doesn't ("Broken") or its maker
//!   turned Linux away ("Denied"), by Steam app id or Epic namespace; fetched weekly into
//!   ~/.cache/lobby/areweanticheatyet.json;
//! - games Steam's store lists as online only (`Entry::online_only`).

use crate::library::{Game, Key, Kind};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

/// Anti-cheat by file or folder name: lowercase prefixes, and exact names.
const KNOWN: &[(&str, &[&str], &[&str])] = &[
    ("Easy Anti-Cheat", &["easyanticheat"], &["start_protected_game.exe"]),
    ("BattlEye", &["battleye", "beclient", "beservice", "bedaisy"], &[]),
    ("PunkBuster", &["pnkbstr"], &["pbcl.dll", "pbsv.dll"]),
    ("nProtect GameGuard", &["npgg", "gamemon"], &["gameguard"]),
    ("XIGNCODE3", &["xigncode", "xhunter"], &["x3.xem"]),
    ("Anti-Cheat Expert", &["anticheatexpert", "sguard"], &["ace-base", "ace-base.sys"]),
    ("HoYoverse anti-cheat", &["mhyprot", "hoyokprotect"], &[]),
    ("EQU8", &["equ8"], &[]),
    ("EA Javelin", &["eaanticheat"], &[]),
    ("Denuvo Anti-Cheat", &["denuvo-anti-cheat", "denuvoanticheat"], &[]),
    ("Riot Vanguard", &[], &["vgc", "vgc.exe", "vgk", "vgk.sys", "vgtray.exe"]),
    ("FACEIT Anti-Cheat", &["faceit"], &[]),
    ("ESEA Anti-Cheat", &["eseaclient", "eseadriver"], &[]),
    ("Ricochet", &[], &["randgrid", "randgrid.sys"]),
];

/// The anti-cheat a file or folder name belongs to.
pub fn named(name: &str) -> Option<&'static str> {
    let name = name.to_ascii_lowercase();
    KNOWN.iter().find(|(_, prefixes, exact)| prefixes.iter().any(|p| name.starts_with(p)) || exact.contains(&name.as_str())).map(|(ac, ..)| *ac)
}

/// Any part of a path or command word named like anti-cheat.
pub fn in_path(path: &str) -> Option<&'static str> {
    path.split(['/', '\\']).find_map(named)
}

/// Proton's builds of Easy Anti-Cheat and BattlEye for Linux, which Proton's Wine loads from
/// the folders `PROTON_EAC_RUNTIME` and `PROTON_BATTLEYE_RUNTIME` name (Steam sets them for its
/// own copies, Lutris and Heroic for theirs): without them a game whose anti-cheat works on
/// Linux doesn't start. Lobby keeps its own copies, pinned by version and checksum like its
/// store tools, in ~/.local/share/lobby/tools, and uses only those. They're published on
/// Lutris's build server's GitHub releases (the same files as Steam's runtime tools).
struct Runtime {
    var: &'static str,
    name: &'static str,
    version: &'static str,
    url: &'static str,
    sha256: &'static str,
    /// The folder in the archive, and what's inside it.
    top: &'static str,
    inside: &'static str,
}

const RUNTIMES: &[Runtime] = &[
    Runtime {
        var: "PROTON_EAC_RUNTIME",
        name: "eac_runtime",
        version: "20240114",
        url: "https://github.com/lutris/buildbot/releases/download/2022.06.23/eac_runtime-20240114.tar.xz",
        sha256: "8c711a3dd0906fdedd2fdcdedc7b33e09eff5aa22fe2b2dffcc046f02e194957",
        top: "eac_runtime",
        inside: "v2",
    },
    Runtime {
        var: "PROTON_BATTLEYE_RUNTIME",
        name: "battleye_runtime",
        version: "2024-05-21",
        url: "https://github.com/lutris/buildbot/releases/download/2024-05-21/battleeye_runtime.tar.xz",
        sha256: "229eaedf3bc16d5bf548bc2aed94bc48c44e2038337eedacbd17c6c7244cd212",
        top: "battleeye_runtime",
        inside: "v1",
    },
];

impl Runtime {
    fn dir(&self) -> PathBuf {
        crate::store::dir().join("tools").join(format!("{}-{}", self.name, self.version))
    }

    fn here(&self) -> bool {
        self.dir().join(self.inside).is_dir()
    }

    /// Downloads, checks and unpacks it into its folder.
    fn fetch(&self) -> Result<(), String> {
        let dir = self.dir();
        let tools = dir.parent().unwrap_or(Path::new("/")).to_path_buf();
        fs::create_dir_all(&tools).map_err(|e| e.to_string())?;
        let failed = |why: String| format!("Couldn't download Proton's {} runtime: {why}", self.name);
        let part = tools.join(format!(".{}-{}.tar.xz", self.name, std::process::id()));
        let out = std::process::Command::new("curl").args(["-sSLf", "--max-time", "120", "-o"]).arg(&part).arg(self.url).stdin(std::process::Stdio::null()).output().map_err(|e| failed(e.to_string()))?;
        let data = fs::read(&part).unwrap_or_default();
        if !out.status.success() || crate::sha256_hex(&data) != self.sha256 {
            let _ = fs::remove_file(&part);
            return Err(failed(if out.status.success() { "it isn't the build Lobby expects (its checksum differs)".into() } else { String::from_utf8_lossy(&out.stderr).trim().to_string() }));
        }
        let unpack = tools.join(format!(".{}-{}", self.name, std::process::id()));
        let _ = fs::remove_dir_all(&unpack);
        fs::create_dir_all(&unpack).map_err(|e| failed(e.to_string()))?;
        let ok = std::process::Command::new("tar").arg("-xJf").arg(&part).arg("-C").arg(&unpack).stdin(std::process::Stdio::null()).status().is_ok_and(|s| s.success());
        let _ = fs::remove_file(&part);
        let got = unpack.join(self.top);
        if !ok || !got.join(self.inside).is_dir() {
            let _ = fs::remove_dir_all(&unpack);
            return Err(failed("it didn't unpack".into()));
        }
        // Another Lobby (the window, a game starting) may have just put it there.
        if self.here() {
            let _ = fs::remove_dir_all(&unpack);
            return Ok(());
        }
        let _ = fs::remove_dir_all(&dir);
        let moved = fs::rename(&got, &dir).map_err(|e| failed(e.to_string()));
        let _ = fs::remove_dir_all(&unpack);
        moved?;
        crate::log::line(&format!("anti-cheat runtime: {}", dir.display()));
        Ok(())
    }
}

/// The anti-cheat runtimes Lobby has, as the variables Proton reads.
pub fn runtimes() -> Vec<(&'static str, PathBuf)> {
    RUNTIMES.iter().filter(|r| r.here()).map(|r| (r.var, r.dir())).collect()
}

/// Downloads the runtimes Lobby doesn't have yet (27 KB and 9 MB). Errors say which failed.
pub fn fetch_runtimes() -> Result<(), String> {
    let errors: Vec<String> = RUNTIMES.iter().filter(|r| !r.here()).filter_map(|r| r.fetch().err()).collect();
    if errors.is_empty() { Ok(()) } else { Err(errors.join(" ")) }
}

const DEPTH: usize = 5;
const MAX_ENTRIES: usize = 50_000;

/// Anti-cheat files in a game's folder, a few levels down (not through links: Wine prefixes
/// link back up to / and the home folder).
fn search(dir: &Path, depth: usize, seen: &mut usize) -> Option<&'static str> {
    let mut subdirs = Vec::new();
    for e in fs::read_dir(dir).ok()?.flatten() {
        *seen += 1;
        if *seen > MAX_ENTRIES {
            return None;
        }
        if let Some(ac) = named(&e.file_name().to_string_lossy()) {
            return Some(ac);
        }
        if e.file_type().is_ok_and(|t| t.is_dir()) {
            subdirs.push(e.path());
        }
    }
    if depth == 0 {
        return None;
    }
    subdirs.iter().find_map(|d| search(d, depth - 1, seen))
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Listed {
    name: String,
    status: String,
    anticheats: Vec<String>,
    steam: Option<u32>,
    epic: Option<String>,
}

#[derive(Default, Serialize, Deserialize)]
struct ListCache {
    when: u64,
    games: Vec<Listed>,
}

#[derive(Default, Serialize, Deserialize)]
struct FolderCache {
    /// Folder -> (its mtime, the anti-cheat found or "").
    folders: HashMap<PathBuf, (u64, String)>,
}

fn list_file() -> PathBuf {
    crate::stores::cache_dir().join("areweanticheatyet.json")
}

fn folder_file() -> PathBuf {
    crate::stores::cache_dir().join("anticheat-folders.json")
}

/// AreWeAntiCheatYet's list, fetched again when a week old (kept when it can't be).
fn listed() -> Vec<Listed> {
    let cached: ListCache = fs::read_to_string(list_file()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    if crate::store::now().saturating_sub(cached.when) < 7 * 86_400 && !cached.games.is_empty() {
        return cached.games;
    }
    let fetched = crate::log::timed("anti-cheat list", || crate::stores::get_json("https://raw.githubusercontent.com/AreWeAntiCheatYet/AreWeAntiCheatYet/HEAD/games.json", None));
    let Ok(serde_json::Value::Array(all)) = fetched else { return cached.games };
    let games: Vec<Listed> = all
        .iter()
        .filter_map(|g| {
            let text = |k: &str| g.get(k).and_then(|v| v.as_str()).map(String::from);
            let ids = g.get("storeIds");
            Some(Listed {
                name: text("name")?,
                status: text("status")?,
                anticheats: g.get("anticheats").and_then(|a| a.as_array()).map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default(),
                steam: ids.and_then(|i| i.get("steam")).and_then(|s| s.as_str()?.parse().ok()),
                epic: ids.and_then(|i| i.get("epic")).and_then(|e| e.get("namespace")).and_then(|n| n.as_str()).map(String::from),
            })
        })
        .collect();
    if games.is_empty() {
        return cached.games;
    }
    let _ = fs::create_dir_all(crate::stores::cache_dir());
    let _ = fs::write(list_file(), serde_json::to_string(&ListCache { when: crate::store::now(), games: games.clone() }).unwrap_or_default());
    games
}

fn mtime(p: &Path) -> u64 {
    fs::metadata(p).and_then(|m| m.modified()).ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs())
}

/// What may keep each game from being played online here, by game id, as sentences for its
/// page. Searches installed games' folders (kept while a folder's mtime holds) and reads the
/// list: a few seconds the first time, then quick.
pub fn warnings(games: &[Game]) -> HashMap<String, Vec<String>> {
    let list = listed();
    let mut cache: FolderCache = fs::read_to_string(folder_file()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    let wide = crate::sizes::wide();
    let mut out = HashMap::new();
    for g in games.iter().filter(|g| matches!(g.kind, Kind::Game | Kind::Owned) && !g.gone) {
        // The list, by store id (Epic's namespace is in Lobby's owned list as the app, so
        // its name is matched there too).
        let steam = g.entries.iter().flat_map(|e| &e.keys).find_map(|k| if let Key::SteamApp(id) = k { Some(*id) } else { None });
        let listed = list.iter().find(|l| steam.is_some() && l.steam == steam).or_else(|| {
            let epic = g.entries.iter().any(|e| e.keys.iter().any(|k| matches!(k, Key::Store(s, _) if s == "epic")));
            let plain = |s: &str| s.to_lowercase().chars().filter(|c| c.is_alphanumeric()).collect::<String>();
            list.iter().find(|l| l.epic.is_some() && epic && plain(&l.name) == plain(&g.name))
        });
        // The game's own folder, searched once per change.
        let shipped = crate::sizes::folder(g, &wide).and_then(|f| {
            let m = mtime(&f);
            let found = match cache.folders.get(&f) {
                Some((t, found)) if *t == m => found.clone(),
                _ => {
                    let found = crate::log::timed(&format!("anti-cheat in {}", f.display()), || search(&f, DEPTH, &mut 0)).unwrap_or_default().to_string();
                    cache.folders.insert(f.clone(), (m, found.clone()));
                    found
                }
            };
            (!found.is_empty()).then_some(found)
        });
        let online_only = g.entries.iter().any(|e| e.online_only);
        let lines = sentences(listed.map(|l| (l.status.as_str(), l.anticheats.as_slice())), shipped.as_deref(), online_only);
        if !lines.is_empty() {
            out.insert(g.id.clone(), lines);
        }
    }
    let _ = fs::create_dir_all(crate::stores::cache_dir());
    let _ = fs::write(folder_file(), serde_json::to_string(&cache).unwrap_or_default());
    out
}

/// The page's sentences from what's known: the list's status and anti-cheat, the anti-cheat
/// its folder ships, and whether it's online only.
fn sentences(listed: Option<(&str, &[String])>, shipped: Option<&str>, online_only: bool) -> Vec<String> {
    let mut lines = Vec::new();
    let named = |acs: &[String]| if acs.is_empty() { shipped.unwrap_or("Its anti-cheat").to_string() } else { acs.join(" and ") };
    match listed {
        // Easy Anti-Cheat and BattlEye run on Linux (through Proton) for the games whose makers
        // turned that on: the list says which.
        Some(("Denied", acs)) => lines.push(format!("{}: its makers don't allow it on Linux, so its online play won't work here (AreWeAntiCheatYet).", named(acs))),
        Some(("Broken", acs)) => lines.push(format!("{} isn't allowed on Linux for this game, so its online play likely won't work (AreWeAntiCheatYet).", named(acs))),
        Some(("Planned", acs)) => lines.push(format!("{} isn't allowed on Linux for this game yet; its makers plan to (AreWeAntiCheatYet).", named(acs))),
        // Supported or Running: works.
        Some(_) => {}
        None => {
            // Only these two have Linux builds a game's makers can turn on.
            if let Some(ac) = shipped.filter(|ac| ["Easy Anti-Cheat", "BattlEye"].contains(ac)) {
                lines.push(format!("It ships {ac}, which works on Linux only for games whose makers allow it; AreWeAntiCheatYet doesn't say for this one."));
            } else if let Some(ac) = shipped {
                lines.push(format!("It ships {ac}, which doesn't run on Linux: its online play likely won't work here."));
            }
        }
    }
    if online_only {
        lines.push("Online only, Steam says: it's played on its servers, with other people.".into());
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Downloads both runtimes (network): `cargo test -- --ignored`, with XDG_DATA_HOME set to
    /// a scratch folder.
    #[test]
    #[ignore]
    fn runtimes_download() {
        fetch_runtimes().unwrap();
        assert_eq!(runtimes().len(), 2);
    }

    #[test]
    fn names_and_sentences() {
        assert_eq!(named("EasyAntiCheat_EOS"), Some("Easy Anti-Cheat"));
        assert_eq!(named("BEService_x64.exe"), Some("BattlEye"));
        assert_eq!(in_path("Z:\\g\\start_protected_game.exe"), Some("Easy Anti-Cheat"));
        assert_eq!(named("vgcore.dll"), None);
        let eac = vec!["Easy Anti-Cheat".to_string()];
        assert!(sentences(Some(("Denied", &eac)), None, false)[0].starts_with("Easy Anti-Cheat: its makers don't allow it"));
        assert!(sentences(Some(("Supported", &eac)), Some("Easy Anti-Cheat"), false).is_empty());
        assert!(sentences(None, Some("BattlEye"), false)[0].contains("only for games whose makers allow it"));
        assert!(sentences(None, Some("PunkBuster"), false)[0].contains("doesn't run on Linux"));
        assert_eq!(sentences(None, None, true).len(), 1);
    }

    #[test]
    fn finds_shipped_files() {
        let dir = std::env::temp_dir().join(format!("lobby-ac-{}", std::process::id()));
        fs::create_dir_all(dir.join("Game/Binaries/Win64/EasyAntiCheat")).unwrap();
        assert_eq!(search(&dir, DEPTH, &mut 0), Some("Easy Anti-Cheat"));
        fs::remove_dir_all(dir.join("Game/Binaries/Win64/EasyAntiCheat")).unwrap();
        assert_eq!(search(&dir, DEPTH, &mut 0), None);
        let _ = fs::remove_dir_all(&dir);
    }
}
