//! Loose downloads in a games folder or Downloads: archives, installers and disc images not
//! unpacked or installed yet. An archive is a game's when it holds a program (an .exe, a Linux
//! build, a .love ...) and isn't a mod (Fallout's F4SE plugins and .esp files) or a patch. Its
//! contents are listed without unpacking it: 7-Zip for zip, 7z, tar and iso; unrar or lsar for
//! rar (Fedora's 7-Zip leaves rar out).

use crate::library::{not_a_game, Entry, Key, Kind, Source};
use std::path::Path;
use std::process::{Command, Stdio};

const ARCHIVES: &[&str] = &["zip", "rar", "7z", "gz", "tgz", "xz", "bz2", "iso"];

/// Whether lobby looks at the file at all.
pub fn wanted(path: &Path) -> bool {
    let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    ext == "exe" || ext == "jar" || ARCHIVES.contains(&ext.as_str())
}

/// Files in a .jar that make it a Minecraft mod or a server plugin, not a game.
const JAR_MODS: &[&str] = &["fabric.mod.json", "quilt.mod.json", "meta-inf/mods.toml", "meta-inf/neoforge.mods.toml", "mcmod.info", "plugin.yml", "bungee.yml", "paper-plugin.yml"];

/// Whether a .jar is a game: it runs by itself (its manifest names a Main-Class) and isn't a
/// mod or a plugin.
fn runnable_jar(file: &Path) -> bool {
    let Some(names) = contents(file) else { return false };
    if names.iter().any(|n| JAR_MODS.contains(&n.to_lowercase().replace('\\', "/").as_str())) {
        return false;
    }
    let manifest = Command::new("timeout").args(["60", "7z", "e", "-so", "-pX"]).arg(file).arg("META-INF/MANIFEST.MF").stdin(Stdio::null()).stderr(Stdio::null()).output();
    manifest.is_ok_and(|o| String::from_utf8_lossy(&o.stdout).lines().any(|l| l.starts_with("Main-Class:")))
}

/// Words in a download's name that make it something for a game, not one.
const NOT_A_GAME: &[&str] = &["patch", "fix", "mod", "mods", "trainer", "soundtrack", "ost", "wallpaper", "wallpapers", "manual", "save", "saves", "update", "redist", "dlc", "checksum", "plugin"];

/// The command's output lines, or None if it failed or took over 60 s. Never waits for input
/// (an encrypted archive would otherwise ask for a password).
fn lines(program: &str, args: &[&str], file: &Path) -> Option<Vec<String>> {
    let out = Command::new("timeout").arg("60").arg(program).args(args).arg(file).stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).lines().map(String::from).collect())
}

/// The paths inside an archive or disc image.
fn contents(file: &Path) -> Option<Vec<String>> {
    let ext = file.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    // 7-Zip only lists the .tar inside a .tar.gz.
    let name = file.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
    if [".tar.gz", ".tar.xz", ".tar.bz2", ".tgz"].iter().any(|e| name.ends_with(e)) {
        return lines("tar", &["-tf"], file);
    }
    if ext == "rar" {
        if let Some(names) = lines("unrar", &["lb", "-p-", "-y"], file) {
            return Some(names);
        }
        // lsar's first line names the archive.
        return lines("lsar", &[], file).map(|l| l.into_iter().skip(1).collect());
    }
    let listing = lines("7z", &["l", "-ba", "-slt", "-pX"], file)?;
    Some(listing.iter().filter_map(|l| l.strip_prefix("Path = ")).map(String::from).collect())
}

/// What an archive holds: a game, one for another system, a mod, or neither.
#[derive(Debug, PartialEq)]
enum Holds {
    Game,
    Mac,
    Android,
    Mod,
    Other,
}

fn holds(names: &[String]) -> Holds {
    let lower: Vec<String> = names.iter().map(|n| n.to_lowercase().replace('\\', "/")).collect();
    let ends = |exts: &[&str]| lower.iter().any(|n| exts.iter().any(|e| n.ends_with(e)));
    if ends(&[".esp", ".esm", ".esl", ".ba2", ".bsa", ".pex"]) || lower.iter().any(|n| n.starts_with("f4se/") || n.starts_with("skse/") || n.contains("/f4se/") || n.contains("/skse/")) {
        return Holds::Mod;
    }
    // Linux builds' programs have no extension, but they bring libraries (.so).
    let program = ends(&[".exe", ".x86_64", ".x86", ".appimage", ".love", ".swf", ".jar", ".iso", ".pck", ".so"]) || lower.iter().any(|n| n.contains("_data/") || n.contains(".so."));
    // A download packed twice: only archives inside ("InsertPointless.zip" holds a zip).
    let nested = !lower.is_empty() && lower.iter().all(|n| [".zip", ".rar", ".7z"].iter().any(|e| n.ends_with(e)));
    if program || nested {
        Holds::Game
    } else if lower.iter().any(|n| n.contains(".app/contents/macos/")) {
        Holds::Mac
    } else if ends(&[".apk"]) {
        Holds::Android
    } else {
        Holds::Other
    }
}

/// A disc image is a game's when it installs one on Windows: an autorun.inf or a program at
/// its top (not a Linux system's disc: Fedora's, Rescuezilla's).
fn game_disc(names: &[String]) -> bool {
    // A live Linux system (Ubuntu's casper, Fedora's LiveOS, isolinux) may still carry an
    // autorun.inf for Windows users.
    let linux = names.iter().any(|n| {
        let n = n.to_lowercase();
        ["casper", "liveos", "isolinux", "live"].iter().any(|d| n == *d || n.starts_with(&format!("{d}/")))
    });
    !linux && names.iter().any(|n| {
        let n = n.to_lowercase().replace('\\', "/");
        n == "autorun.inf" || (!n.contains('/') && n.ends_with(".exe"))
    })
}

/// A download's name as a game's: its file name without the extension, copy numbers
/// ("(1)"), and versions or platforms at the end ("Ghost_Defender", "XCOW 64Bits",
/// "Dungeon Dash", "corrupted-boundary-win64-prealpha").
pub fn title(file: &Path) -> String {
    let mut stem = file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    for ext in [".tar.gz", ".tar.xz", ".tar.bz2", ".tgz", ".zip", ".rar", ".7z", ".exe", ".iso"] {
        if stem.to_lowercase().ends_with(ext) {
            stem.truncate(stem.len() - ext.len());
            break;
        }
    }
    // A browser's copy number: "DataShift(1)".
    if let Some(open) = stem.rfind('(').filter(|_| stem.ends_with(')')) {
        if stem[open + 1..stem.len() - 1].chars().all(|c| c.is_ascii_digit()) && open > 0 {
            stem.truncate(open);
        }
    }
    let spaced = stem.replace(['_', '-', '.'], " ");
    let mut words: Vec<&str> = spaced.split_whitespace().collect();
    let drop = |w: &str| {
        let l = w.to_lowercase();
        let l = l.trim_matches(|c| c == '(' || c == ')' || c == '[' || c == ']');
        l.is_empty()
            || l.chars().all(|c| c.is_ascii_digit())
            || (l.starts_with('v') && l.len() > 1 && l[1..].chars().all(|c| c.is_ascii_digit()))
            // "beta5", "build002": a stage and its number.
            || ["win", "win32", "win64", "windows", "windows64", "linux", "x64", "x86", "64bit", "32bit", "64bits", "32bits", "bit", "bits", "build", "release", "final", "prealpha", "alpha", "beta", "setup", "installer", "pc", "ver", "en", "iso", "version", "mac", "osx", "macos", "android"].contains(&l.trim_end_matches(|c: char| c.is_ascii_digit()))
    };
    while words.len() > 1 && drop(words[words.len() - 1]) {
        words.pop();
    }
    let t = words.join(" ");
    if t.is_empty() { stem } else { t }
}

/// A loose file as a game download, or nothing.
pub fn look(file: &Path) -> Option<Entry> {
    let name = title(file);
    let words: Vec<String> = name.to_lowercase().split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).map(String::from).collect();
    let plain: String = words.concat();
    if not_a_game(&name) || words.iter().any(|w| NOT_A_GAME.contains(&w.as_str())) || ["patch", "trainer", "cheatengine"].iter().any(|w| plain.contains(w)) {
        return None;
    }
    let ext = file.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    let mut e = Entry::new(Source::Folder, name);
    e.program = Some(file.to_path_buf());
    e.keys.push(Key::Download(file.to_path_buf()));
    if ext == "jar" {
        // A Java game in one file (an installer's .jar too, by its name).
        if !runnable_jar(file) {
            return None;
        }
        e.kind = if plain.contains("install") || plain.contains("setup") { Kind::Download } else { Kind::Game };
        return Some(e);
    }
    if ext == "exe" {
        // An installer, or a game in one program that runs as it is.
        let installer = match gamefix::binary::identify(file)? {
            gamefix::binary::Binary::Pe(pe) => pe.installer.is_some() || plain.contains("setup") || plain.contains("install"),
            _ => return None,
        };
        e.kind = if installer { Kind::Download } else { Kind::Game };
        return Some(e);
    }
    let names = contents(file)?;
    match holds(&names) {
        Holds::Game if ext != "iso" || game_disc(&names) => e.kind = Kind::Download,
        // Downloaded by mistake, most likely: say so rather than leave it out.
        Holds::Mac => {
            e.kind = Kind::WrongSystem;
            e.note = Some("Mac version: won't run here".into());
        }
        Holds::Android => {
            e.kind = Kind::WrongSystem;
            e.note = Some("Android version: won't run here".into());
        }
        _ => return None,
    }
    Some(e)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles() {
        assert_eq!(title(Path::new("/d/Ghost_Defender.zip")), "Ghost Defender");
        assert_eq!(title(Path::new("/d/XCOW 64Bits.zip")), "XCOW");
        assert_eq!(title(Path::new("/d/DataShift(1).zip")), "DataShift");
        assert_eq!(title(Path::new("/d/corrupted-boundary-win64-prealpha.zip")), "corrupted boundary");
        assert_eq!(title(Path::new("/d/Shoot'n'Slaughter 0.0.0.4.rar")), "Shoot'n'Slaughter");
        assert_eq!(title(Path::new("/d/StarshipAssassinSetup_beta5.exe")), "StarshipAssassinSetup");
        assert_eq!(title(Path::new("/d/minato-linux.tar.gz")), "minato");
        assert_eq!(title(Path::new("/d/Black-White_Win_EN_ISO-Version.zip")), "Black White");
    }

    #[test]
    fn contents_decide() {
        let names = |l: &[&str]| l.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(holds(&names(&["F4SE/Plugins/Buffout4.dll", "F4SE/Plugins/Buffout4.toml"])), Holds::Mod);
        assert_eq!(holds(&names(&["Endless_Runner/Content/Paks/x.pak", "Endless_Runner.exe"])), Holds::Game);
        assert_eq!(holds(&names(&["Minato.x86_64", "Minato_Data/"])), Holds::Game);
        assert_eq!(holds(&names(&["hooks/readonly-guard.py", "README.md"])), Holds::Other);
        assert_eq!(holds(&names(&["EFI/BOOT/BOOTX64.EFI", "LiveOS/squashfs.img"])), Holds::Other);
        assert_eq!(holds(&names(&["SlimeTime.app/Contents/MacOS/SlimeTime"])), Holds::Mac);
        assert_eq!(holds(&names(&["Neuron_Game_arm64-v8a.apk"])), Holds::Android);
        assert_eq!(holds(&names(&["InsertPointlessTitle.zip"])), Holds::Game);
        assert_eq!(holds(&names(&["KH4 - Linux/lib/libnode.so", "KH4 - Linux/nw"])), Holds::Game);
        assert!(game_disc(&names(&["AUTORUN.INF", "Setup.exe"])));
        assert!(!game_disc(&names(&["casper/vmlinuz", "EFI/boot/grubx64.efi"])));
        assert!(!game_disc(&names(&["autorun.inf", "autorun.exe", "casper", "casper/vmlinuz"])));
    }
}
