//! A game's extras downloaded next to it: MyAbandonware names them after the game with what
//! they are in place of the platform (`Need-for-Speed-Underground_Patch_Win_EN_v14-for-US-version.zip`,
//! `…_NoCD_Win_EN_for-version-14.zip`, `…_Fix_Win_EN_Widescreen-fix.zip`). Install puts them
//! in after the game, as someone on Windows would: the official patch first (its own
//! installer, run in the game's Wine prefix), then a No-CD program, then fixes; files that
//! replace the game's are kept beside them as `<name>.lobby-original`. A No-CD program is
//! often the only way an old disc game starts at all: its disc protection (SafeDisc,
//! SecuROM) needs a Windows driver that Wine doesn't have and Windows 10 dropped too.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    /// The publisher's update, usually its own installer.
    Patch,
    /// A program without the disc check.
    NoCd,
    /// A fan fix (widescreen, a bug): files put into the game's folder.
    Fix,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Extra {
    pub file: PathBuf,
    pub kind: Kind,
    /// What it says it is, from its name ("v14 for US version").
    pub what: String,
}

impl Extra {
    /// "the patch", for a list of what was put in.
    pub fn short(&self) -> &'static str {
        match self.kind {
            Kind::Patch => "the patch",
            Kind::NoCd => "the No-CD program",
            Kind::Fix => "the fix",
        }
    }

    /// "the patch (v14 for US version)".
    pub fn describe(&self) -> String {
        let kind = match self.kind {
            Kind::Patch => "the patch",
            Kind::NoCd => "the No-CD program",
            Kind::Fix => "the fix",
        };
        if self.what.is_empty() { kind.to_string() } else { format!("{kind} ({})", self.what) }
    }
}

/// The kind an extra's second name part says, MyAbandonware's words.
fn kind_of(word: &str) -> Option<Kind> {
    let w = word.to_lowercase().replace('-', "");
    if w == "nocd" || w == "nodvd" || w == "crack" {
        Some(Kind::NoCd)
    } else if w.ends_with("patch") || w == "update" {
        Some(Kind::Patch)
    } else if w.ends_with("fix") {
        Some(Kind::Fix)
    } else {
        None
    }
}

fn stem(name: &str) -> &str {
    let lower = name.to_lowercase();
    [".zip", ".rar", ".7z", ".exe"].iter().find(|e| lower.ends_with(*e)).map_or(name, |e| &name[..name.len() - e.len()])
}

/// The extras beside a download, in the order they go in: patches, then No-CD, then fixes
/// (a No-CD program is made for the patched version).
pub fn for_download(download: &Path) -> Vec<Extra> {
    let Some(name) = download.file_name().map(|n| n.to_string_lossy().into_owned()) else { return Vec::new() };
    if crate::sources::downloads::myabandonware(&name).is_none() {
        return Vec::new();
    }
    let title = stem(&name).split('_').next().unwrap_or_default().to_lowercase();
    let mut found: Vec<Extra> = fs::read_dir(download.parent().unwrap_or(Path::new(".")))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().is_file())
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let parts: Vec<&str> = stem(&name).split('_').collect();
            if parts.len() < 4 || parts[0].to_lowercase() != title {
                return None;
            }
            let kind = kind_of(parts[1])?;
            let what = parts[4..].join(" ").replace('-', " ");
            Some(Extra { file: e.path(), kind, what })
        })
        .collect();
    found.sort_by(|a, b| a.kind.cmp(&b.kind).then(a.file.cmp(&b.file)));
    found
}

/// Readme-like files an extra carries, which don't go into the game.
fn is_doc(name: &str) -> bool {
    let lower = name.to_lowercase();
    [".nfo", ".txt", ".diz", ".url", ".htm", ".html", ".pdf", ".rtf", ".md"].iter().any(|e| lower.ends_with(e))
}

/// The files of an unpacked extra (folder-relative), without a single folder they're all in
/// unless the game has a folder of that name.
fn contents(unpacked: &Path, game_folder: &Path) -> Vec<(PathBuf, PathBuf)> {
    let mut root = unpacked.to_path_buf();
    loop {
        let entries: Vec<PathBuf> = fs::read_dir(&root).into_iter().flatten().flatten().map(|e| e.path()).collect();
        match entries.as_slice() {
            [one] if one.is_dir() && !one.file_name().is_some_and(|n| game_folder.join(n).is_dir()) => root = one.clone(),
            _ => break,
        }
    }
    let mut out = Vec::new();
    let mut stack = vec![root.clone()];
    while let Some(dir) = stack.pop() {
        for e in fs::read_dir(&dir).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if let Ok(rel) = p.strip_prefix(&root) {
                if !(rel.components().count() == 1 && is_doc(&rel.to_string_lossy())) {
                    out.push((p.clone(), rel.to_path_buf()));
                }
            }
        }
    }
    out.sort();
    out
}

/// A file of the game's, found whatever its case (Windows games' files: "SPEED.EXE").
fn in_game(game_folder: &Path, rel: &Path) -> PathBuf {
    let mut at = game_folder.to_path_buf();
    for part in rel.components() {
        let want = part.as_os_str().to_string_lossy().to_lowercase();
        let found = fs::read_dir(&at).into_iter().flatten().flatten().map(|e| e.file_name()).find(|n| n.to_string_lossy().to_lowercase() == want);
        at = at.join(found.as_deref().unwrap_or(part.as_os_str()));
    }
    at
}

/// Copies an extra's files into the game's folder; a file it replaces is kept as
/// `<name>.lobby-original` (once: the first one is the game's own). Returns how many files.
fn copy_in(files: &[(PathBuf, PathBuf)], game_folder: &Path) -> Result<usize, String> {
    for (from, rel) in files {
        let to = in_game(game_folder, rel);
        if let Some(parent) = to.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        if to.is_file() {
            let mut kept = to.clone().into_os_string();
            kept.push(".lobby-original");
            let kept = PathBuf::from(kept);
            if !kept.exists() {
                fs::rename(&to, &kept).map_err(|e| format!("{}: {e}", to.display()))?;
            }
        }
        fs::copy(from, &to).map_err(|e| format!("{} → {}: {e}", from.display(), to.display()))?;
    }
    Ok(files.len())
}

/// A file's size and time, to tell whether a patch changed it.
fn stamp(p: &Path) -> Option<(u64, std::time::SystemTime)> {
    let m = fs::metadata(p).ok()?;
    Some((m.len(), m.modified().ok()?))
}

/// The installed game's program, folder and how it runs, from its app-menu entry.
pub struct Target {
    pub program: PathBuf,
    /// umu-run and the environment its entry gives it (WINEPREFIX, PROTONPATH ...).
    pub umu: String,
    pub env: Vec<(String, String)>,
}

impl Target {
    pub fn from_entry(entry: &Path) -> Option<Target> {
        let (words, _) = crate::launch::desktop_exec(&fs::read_to_string(entry).ok()?)?;
        let umu = words.iter().position(|w| w.ends_with("umu-run"))?;
        let env = words[..umu].iter().filter_map(|w| w.split_once('=')).filter(|(k, _)| !k.is_empty() && k.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')).map(|(k, v)| (k.to_string(), v.to_string())).collect();
        let program = PathBuf::from(words.get(umu + 1)?);
        Some(Target { program, umu: words[umu].clone(), env })
    }

    fn folder(&self) -> &Path {
        self.program.parent().unwrap_or(Path::new("/"))
    }

    fn prefix(&self) -> Option<PathBuf> {
        crate::stores::saves::prefix_of(&self.env.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>())
    }

    /// Runs a patch's installer in the game's prefix with the game's Proton, and waits until
    /// everything it started has finished.
    fn run(&self, program: &Path) -> Result<(), String> {
        let mut c = Command::new(&self.umu);
        c.arg(program).current_dir(program.parent().unwrap_or(Path::new("/"))).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
        for (k, v) in &self.env {
            c.env(k, v);
        }
        c.status().map_err(|e| format!("umu-run: {e}"))?;
        if let Some(prefix) = self.prefix() {
            gamefix::disc::wait_for_prefix(&prefix);
        }
        Ok(())
    }
}

/// Puts the extras into the installed game, in order. A line for each (✓ done, ✗ not), and
/// the short names of those put in ("the patch"), for the message that it's installed. Stops
/// at a patch that changed nothing (cancelled): what comes after it is made for the patched
/// game.
pub fn put_in(extras: &[Extra], target: &Target) -> (Vec<String>, Vec<&'static str>) {
    let mut lines = Vec::new();
    let mut put = Vec::new();
    let work = std::env::temp_dir().join(format!("lobby-extras-{}", std::process::id()));
    for extra in extras {
        let _ = fs::remove_dir_all(&work);
        let _ = fs::create_dir_all(&work);
        let unpacked = if crate::install::archive_kind(&extra.file).is_some() {
            crate::install::unpack(&extra.file, &work).map(|_| work.clone())
        } else {
            // A patch downloaded as its own program.
            fs::copy(&extra.file, work.join(extra.file.file_name().unwrap_or_default())).map(|_| work.clone()).map_err(|e| e.to_string())
        };
        let files = match unpacked {
            Ok(dir) => contents(&dir, target.folder()),
            Err(e) => {
                lines.push(format!("✗ Couldn't unpack {}: {e}", extra.describe()));
                continue;
            }
        };
        // A patch that's one program the game doesn't have is the patch's installer.
        let installer = match files.as_slice() {
            [(exe, rel)] if extra.kind == Kind::Patch && rel.extension().is_some_and(|e| e.eq_ignore_ascii_case("exe")) && !in_game(target.folder(), rel).exists() => Some(exe.clone()),
            _ => None,
        };
        match installer {
            Some(exe) => {
                let before = stamp(&target.program);
                if let Err(e) = target.run(&exe) {
                    lines.push(format!("✗ Couldn't start {}: {e}", extra.describe()));
                    break;
                }
                if stamp(&target.program) == before {
                    lines.push(format!("✗ {} didn't change the game (was it cancelled?), so what comes after it wasn't put in either. Install again to have another go.", capitalize(&extra.describe())));
                    break;
                }
                put.push(extra.short());
                lines.push(format!("✓ Installed {}.", extra.describe()));
            }
            None => match copy_in(&files, target.folder()) {
                Ok(n) => {
                    put.push(extra.short());
                    let s = if n == 1 { "" } else { "s" };
                    lines.push(format!("✓ Put in {}: {n} file{s} in the game's folder (any it replaced are kept as .lobby-original).", extra.describe()));
                }
                Err(e) => lines.push(format!("✗ Couldn't put in {}: {e}", extra.describe())),
            },
        }
    }
    let _ = fs::remove_dir_all(&work);
    (lines, put)
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_games_extras_beside_it() {
        let dir = std::env::temp_dir().join(format!("lobby-extras-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        for f in [
            "Need-for-Speed-Underground_Win_EN_ISO-images.zip",
            "Need-for-Speed-Underground_Fix_Win_EN_Widescreen-fix.zip",
            "Need-for-Speed-Underground_NoCD_Win_EN_for-version-14.zip",
            "Need-for-Speed-Underground_Patch_Win_EN_v14-for-US-version.zip",
            "Need-for-Speed-Underground-2_Patch_Win_EN_v12.zip",
            "Need-for-Speed-Underground_Manual_Win_EN.zip",
        ] {
            fs::write(dir.join(f), b"").unwrap();
        }
        let found = for_download(&dir.join("Need-for-Speed-Underground_Win_EN_ISO-images.zip"));
        let kinds: Vec<(Kind, &str)> = found.iter().map(|e| (e.kind, e.what.as_str())).collect();
        assert_eq!(kinds, [(Kind::Patch, "v14 for US version"), (Kind::NoCd, "for version 14"), (Kind::Fix, "Widescreen fix")]);
        assert_eq!(found[0].describe(), "the patch (v14 for US version)");
        // Only MyAbandonware's downloads have them.
        fs::write(dir.join("game.zip"), b"").unwrap();
        assert!(for_download(&dir.join("game.zip")).is_empty());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn files_go_into_the_game_with_the_originals_kept() {
        let dir = std::env::temp_dir().join(format!("lobby-extras-copy-{}", std::process::id()));
        let (game, extra) = (dir.join("game"), dir.join("extra"));
        fs::create_dir_all(game.join("Scripts")).unwrap();
        fs::write(game.join("SPEED.EXE"), b"original").unwrap();
        fs::create_dir_all(extra.join("NFSU/scripts")).unwrap();
        fs::write(extra.join("NFSU/Speed.exe"), b"no-cd").unwrap();
        fs::write(extra.join("NFSU/dinput8.dll"), b"loader").unwrap();
        fs::write(extra.join("NFSU/scripts/fix.asi"), b"fix").unwrap();
        fs::write(extra.join("NFSU/Readme.txt"), b"read me").unwrap();
        let files = contents(&extra, &game);
        let rels: Vec<String> = files.iter().map(|(_, r)| r.display().to_string()).collect();
        assert_eq!(rels, ["Speed.exe", "dinput8.dll", "scripts/fix.asi"]);
        assert_eq!(copy_in(&files, &game).unwrap(), 3);
        assert_eq!(fs::read(game.join("SPEED.EXE")).unwrap(), b"no-cd");
        assert_eq!(fs::read(game.join("SPEED.EXE.lobby-original")).unwrap(), b"original");
        assert_eq!(fs::read(game.join("Scripts/fix.asi")).unwrap(), b"fix");
        assert!(!game.join("Readme.txt").exists());
        // Again: the game's own original stays the one kept.
        copy_in(&files, &game).unwrap();
        assert_eq!(fs::read(game.join("SPEED.EXE.lobby-original")).unwrap(), b"original");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_games_command_from_its_entry() {
        let dir = std::env::temp_dir().join(format!("lobby-extras-entry-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let entry = dir.join("g.desktop");
        fs::write(&entry, "[Desktop Entry]\nExec=env \"WINEPREFIX=/g/Need For Speed Underground\" PROTONPATH=/s/GE GAMEID=umu-0 /usr/bin/umu-run \"/g/Need For Speed Underground/drive_c/Program Files/EA GAMES/Need For Speed Underground/Speed.exe\"\n").unwrap();
        let t = Target::from_entry(&entry).unwrap();
        assert_eq!(t.umu, "/usr/bin/umu-run");
        assert_eq!(t.prefix(), Some(PathBuf::from("/g/Need For Speed Underground")));
        assert_eq!(t.folder(), Path::new("/g/Need For Speed Underground/drive_c/Program Files/EA GAMES/Need For Speed Underground"));
        assert!(t.env.contains(&("GAMEID".into(), "umu-0".into())));
        fs::remove_dir_all(&dir).unwrap();
    }
}
