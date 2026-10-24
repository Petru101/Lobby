//! Blizzard's games (StarCraft II, Diablo, Overwatch, Hearthstone ...) only start through the
//! Battle.net app, which signs the player in. Lobby keeps one copy of it in a Wine prefix of its
//! own (~/.local/share/lobby/battlenet), installed the first time such a game is played, and
//! tells it where each game's folder is, so nothing is downloaded again.
//!
//! A Blizzard game's folder holds `.build.info` and `.product.db`: the latter is the game's
//! install record (a protobuf `ProductInstall`: 1 uid, 2 product code, 3 settings whose 1 is the
//! install path), the same record Battle.net's agent keeps for every game in its own
//! `ProgramData/Battle.net/Agent/product.db` (a `Database`: 1 repeated ProductInstall ...).
//! Battle.net's "Locate the game" reads the folder's record; lobby adds it to the agent's
//! database itself, with the folder's path as Wine sees it, while Battle.net is closed.

use crate::library::{Game, Source};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Blizzard's official installer of the Battle.net app (it downloads the app itself).
const SETUP_URL: &str = "https://downloader.battle.net/download/getInstallerForGame?os=win&gameProgram=BATTLENET_APP&version=Live";

/// What Battle.net's `--exec="launch <code>"` calls each game, by its product code.
const LAUNCH_CODES: &[(&str, &str)] = &[
    ("s2", "S2"), ("s1", "S1"), ("pro", "Pro"), ("wow", "WoW"), ("wow_classic", "WoWC"), ("wow_classic_era", "WoWC"), ("d3", "D3"), ("fenris", "Fen"),
    ("osi", "OSI"), ("hs_beta", "WTCG"), ("heroes", "Hero"), ("w3", "W3"), ("anbs", "ANBS"), ("rtro", "RTRO"), ("wlby", "WLBY"), ("gryphon", "GRY"),
    ("viper", "VIPR"), ("odin", "ODIN"), ("lazarus", "LAZR"), ("zeus", "ZEUS"), ("fore", "FORE"), ("spot", "SPOT"), ("auks", "AUKS"),
];

#[derive(Debug, PartialEq)]
pub struct Product {
    /// The product code in its record ("s2").
    pub code: String,
    /// What Battle.net's --exec calls it ("S2").
    pub launch: String,
}

/// The Blizzard game in a folder, from its install record.
pub fn product(folder: &Path) -> Option<Product> {
    if !folder.join(".build.info").is_file() {
        return None;
    }
    let record = fs::read(folder.join(".product.db")).ok()?;
    let code = fields(&record)?.into_iter().find(|f| f.number == 2).and_then(|f| String::from_utf8(f.bytes).ok())?;
    let launch = LAUNCH_CODES.iter().find(|(c, _)| *c == code).map_or_else(|| code.to_uppercase(), |(_, l)| l.to_string());
    Some(Product { code, launch })
}

/// A game Battle.net starts: the folder (of an entry, or up to three above its program) with
/// Blizzard's install record.
pub fn game_folder(game: &Game) -> Option<(PathBuf, Product)> {
    game.entries.iter().filter(|e| e.source != Source::Steam).find_map(|e| {
        let from_program = e.program.as_deref().and_then(Path::parent).into_iter().flat_map(|p| p.ancestors().take(4));
        e.folder.as_deref().into_iter().chain(from_program).find_map(|f| Some((f.to_path_buf(), product(f)?)))
    })
}

pub fn prefix() -> PathBuf {
    crate::store::dir().join("battlenet")
}

fn program() -> PathBuf {
    prefix().join("drive_c/Program Files (x86)/Battle.net/Battle.net.exe")
}

pub fn installed() -> bool {
    program().is_file()
}

/// The Proton that made the prefix, else the newest GE-Proton installed (what Battle.net is
/// known to run on), else umu's own download of it.
fn proton() -> String {
    if let Some(p) = crate::launch::proton_of(&prefix()) {
        return p.display().to_string();
    }
    let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    let mut ge: Vec<PathBuf> = [".local/share/Steam/compatibilitytools.d", ".steam/root/compatibilitytools.d"]
        .iter()
        .flat_map(|d| fs::read_dir(home.join(d)).into_iter().flatten().flatten())
        .map(|e| e.path())
        .filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("GE-Proton")) && p.join("proton").is_file())
        .collect();
    ge.sort_by_key(|p| version(&p.file_name().unwrap().to_string_lossy()));
    ge.pop().map_or("GE-Proton".into(), |p| p.display().to_string())
}

/// "GE-Proton11-11" → [11, 11], so 11-11 comes after 11-7.
fn version(name: &str) -> Vec<u32> {
    name.split(|c: char| !c.is_ascii_digit()).filter_map(|n| n.parse().ok()).collect()
}

fn umu(verb: &str) -> Command {
    let mut c = Command::new("umu-run");
    c.env("WINEPREFIX", prefix()).env("PROTONPATH", proton()).env("GAMEID", "umu-0").env("STORE", "battlenet").env("PROTON_VERB", verb);
    c
}

/// The command starting the game through Battle.net. Proton's `run`, not its default
/// `waitforexitandrun`: Battle.net stays open, and the default would wait for it to close.
pub fn command(launch: &str) -> Vec<String> {
    let mut words = vec!["env".to_string()];
    let c = umu("run");
    words.extend(c.get_envs().filter_map(|(k, v)| Some(format!("{}={}", k.to_string_lossy(), v?.to_string_lossy()))));
    words.extend(["umu-run".into(), program().display().to_string(), format!("--exec=launch {launch}")]);
    words
}

/// Downloads Blizzard's installer and runs it in lobby's prefix: its window asks where to
/// install (the prefix's Program Files is filled in) and then opens Battle.net to sign in.
/// Returns once the installer is closed.
pub fn install() -> Result<(), String> {
    let setup = crate::stores::cache_dir().join("Battle.net-Setup.exe");
    fs::create_dir_all(setup.parent().unwrap()).map_err(|e| e.to_string())?;
    let got = Command::new("curl").args(["-fsSL", "--retry", "2", "-o"]).arg(&setup).arg(SETUP_URL).stdin(Stdio::null()).status().map_err(|e| format!("curl: {e}"))?;
    if !got.success() || fs::read(&setup).map_or(true, |b| !b.starts_with(b"MZ")) {
        let _ = fs::remove_file(&setup);
        return Err("Blizzard's installer couldn't be downloaded.".into());
    }
    fs::create_dir_all(prefix()).map_err(|e| e.to_string())?;
    let log = crate::store::log_file("battlenet");
    fs::create_dir_all(log.parent().unwrap()).map_err(|e| e.to_string())?;
    let out = fs::File::create(&log).map_err(|e| e.to_string())?;
    let status = umu("waitforexitandrun")
        .arg(&setup)
        .args(["--lang=enUS", "--installpath=C:\\Program Files (x86)\\Battle.net"])
        .stdin(Stdio::null())
        .stdout(out.try_clone().map_err(|e| e.to_string())?)
        .stderr(out)
        .status()
        .map_err(|e| format!("umu-run: {e}"))?;
    if installed() {
        Ok(())
    } else {
        Err(format!("Battle.net wasn't installed (the installer {}). What it printed is in {}.", if status.success() { "closed" } else { "failed" }, log.display()))
    }
}

/// The processes running in lobby's prefix (Battle.net, its agent, the wineserver).
fn processes() -> Vec<u32> {
    let prefix = prefix();
    let ours = |v: &str| Path::new(v.trim_end_matches('/')).starts_with(&prefix);
    fs::read_dir("/proc")
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.file_name().to_str()?.parse::<u32>().ok())
        .filter(|pid| {
            let env = fs::read(format!("/proc/{pid}/environ")).unwrap_or_default();
            env.split(|b| *b == 0).filter_map(|kv| std::str::from_utf8(kv).ok()).any(|kv| kv.strip_prefix("WINEPREFIX=").is_some_and(ours) || kv.strip_prefix("STEAM_COMPAT_DATA_PATH=").is_some_and(ours))
        })
        .filter(|pid| *pid != std::process::id())
        .collect()
}

pub fn running() -> bool {
    !processes().is_empty()
}

/// Closes Battle.net (and everything in its prefix), waiting up to 10 s.
fn stop() {
    for signal in ["-TERM", "-KILL"] {
        let pids = processes();
        if pids.is_empty() {
            return;
        }
        let _ = Command::new("kill").arg(signal).args(pids.iter().map(u32::to_string)).stderr(Stdio::null()).status();
        for _ in 0..20 {
            if !running() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(250));
        }
    }
}

fn database() -> PathBuf {
    prefix().join("drive_c/ProgramData/Battle.net/Agent/product.db")
}

/// The folder as Battle.net writes paths: Wine's drive Z: is /, forward slashes.
fn windows_path(folder: &Path) -> String {
    format!("Z:{}", folder.display())
}

/// Whether Battle.net knows the game is in that folder.
pub fn knows(folder: &Path) -> bool {
    let db = fs::read(database()).unwrap_or_default();
    let want = folder.display().to_string().to_lowercase();
    String::from_utf8_lossy(&db).to_lowercase().contains(&want)
}

/// Adds the folder's game to Battle.net's agent database (as "Locate the game" does), in place
/// of any record of the same game. Battle.net must be closed. The database before is kept as
/// product.db.lobby-backup.
fn add(folder: &Path) -> Result<(), String> {
    let record = fs::read(folder.join(".product.db")).map_err(|e| format!("{}: {e}", folder.display()))?;
    let path = database();
    let old = fs::read(&path).unwrap_or_default();
    let new = with_game(&old, &record, folder)?;
    fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
    if !old.is_empty() {
        fs::write(path.with_extension("db.lobby-backup"), &old).map_err(|e| e.to_string())?;
    }
    fs::write(&path, new).map_err(|e| format!("{}: {e}", path.display()))
}

/// The agent's database with the game's record (its install path set to the folder) first.
fn with_game(db: &[u8], record: &[u8], folder: &Path) -> Result<Vec<u8>, String> {
    let fields_of = |b: &[u8]| fields(b).ok_or_else(|| format!("{}: not a Battle.net record lobby can read", folder.display()));
    let mut install = fields_of(record)?;
    let uid = install.iter().find(|f| f.number == 1).map(|f| f.bytes.clone()).ok_or("the game's record has no id")?;
    for f in install.iter_mut().filter(|f| f.number == 3) {
        let mut settings = fields_of(&f.bytes)?;
        settings.retain(|s| s.number != 1);
        settings.insert(0, Field { number: 1, wire: 2, bytes: windows_path(folder).into_bytes() });
        f.bytes = encode(&settings);
    }
    let mut db = fields(db).ok_or("Battle.net's database isn't one lobby can read")?;
    db.retain(|f| !(f.number == 1 && fields(&f.bytes).is_some_and(|i| i.iter().any(|x| x.number == 1 && x.bytes == uid))));
    db.insert(0, Field { number: 1, wire: 2, bytes: encode(&install) });
    Ok(encode(&db))
}

/// Makes sure Battle.net knows the game (closing Battle.net first when it's open, then it's
/// started again by the game's command), then starts it.
pub fn start(folder: &Path, launch: &str, log: &Path) -> Result<(), String> {
    if !knows(folder) {
        stop();
        add(folder)?;
    }
    crate::launch::start(&command(launch), None, log)
}

/// A protobuf field: its number, wire type and contents (a varint's bytes as written).
#[derive(Debug, Clone, PartialEq)]
struct Field {
    number: u64,
    wire: u8,
    bytes: Vec<u8>,
}

fn varint(b: &[u8], i: &mut usize) -> Option<u64> {
    let mut value = 0u64;
    for shift in (0..64).step_by(7) {
        let byte = *b.get(*i)?;
        *i += 1;
        value |= u64::from(byte & 0x7f) << shift;
        if byte < 0x80 {
            return Some(value);
        }
    }
    None
}

fn put_varint(out: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        out.push((v as u8) | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

/// A message's fields in order, or None when it isn't one.
fn fields(b: &[u8]) -> Option<Vec<Field>> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let key = varint(b, &mut i)?;
        let (number, wire) = (key >> 3, (key & 7) as u8);
        let start = i;
        let end = match wire {
            0 => {
                varint(b, &mut i)?;
                i
            }
            1 => i + 8,
            2 => {
                let n = varint(b, &mut i)? as usize;
                i.checked_add(n)?
            }
            5 => i + 4,
            _ => return None,
        };
        let bytes = if wire == 2 { b.get(i..end)? } else { b.get(start..end)? }.to_vec();
        i = end;
        out.push(Field { number, wire, bytes });
    }
    Some(out)
}

fn encode(fields: &[Field]) -> Vec<u8> {
    let mut out = Vec::new();
    for f in fields {
        put_varint(&mut out, f.number << 3 | u64::from(f.wire));
        if f.wire == 2 {
            put_varint(&mut out, f.bytes.len() as u64);
        }
        out.extend_from_slice(&f.bytes);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A game folder's record like StarCraft II's (uid, code, settings with a Windows path).
    fn record(path: &str) -> Vec<u8> {
        let settings = encode(&[Field { number: 1, wire: 2, bytes: path.into() }, Field { number: 2, wire: 2, bytes: b"eu".to_vec() }, Field { number: 3, wire: 0, bytes: vec![2] }]);
        encode(&[Field { number: 1, wire: 2, bytes: b"s2".to_vec() }, Field { number: 2, wire: 2, bytes: b"s2".to_vec() }, Field { number: 3, wire: 2, bytes: settings }, Field { number: 7, wire: 0, bytes: vec![0] }])
    }

    #[test]
    fn reads_and_writes_records() {
        let r = record("D:/Games/StarCraft II");
        assert_eq!(encode(&fields(&r).unwrap()), r);
        let dir = std::env::temp_dir().join(format!("lobby-bnet-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(".product.db"), &r).unwrap();
        assert_eq!(product(&dir), None, "no .build.info: not an install");
        fs::write(dir.join(".build.info"), "Branch!STRING:0").unwrap();
        assert_eq!(product(&dir), Some(Product { code: "s2".into(), launch: "S2".into() }));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn adds_the_game_in_place_of_its_old_record() {
        let other = encode(&[Field { number: 1, wire: 2, bytes: b"bna".to_vec() }]);
        let db = encode(&[Field { number: 1, wire: 2, bytes: record("C:/Old/StarCraft II") }, Field { number: 1, wire: 2, bytes: other.clone() }, Field { number: 5, wire: 0, bytes: vec![1] }]);
        let new = fields(&with_game(&db, &record("D:/Games/StarCraft II"), Path::new("/run/media/u/Games/StarCraft II")).unwrap()).unwrap();
        assert_eq!(new.len(), 3);
        assert_eq!(new[1].bytes, other);
        let settings = fields(&fields(&new[0].bytes).unwrap()[2].bytes).unwrap();
        assert_eq!(settings[0].bytes, b"Z:/run/media/u/Games/StarCraft II");
        assert_eq!(settings[1].bytes, b"eu");
    }

    #[test]
    fn newest_ge_proton() {
        let mut v = vec!["GE-Proton11-7", "GE-Proton11-11", "GE-Proton9-27"];
        v.sort_by_key(|n| version(n));
        assert_eq!(v, ["GE-Proton9-27", "GE-Proton11-7", "GE-Proton11-11"]);
    }
}
