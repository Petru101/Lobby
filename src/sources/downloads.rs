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
    // A split archive is looked at by its first part.
    let name = path.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
    if name.rsplit_once(".part").is_some_and(|(_, part)| part.trim_end_matches(".rar").parse::<u32>().is_ok_and(|n| n > 1)) {
        return false;
    }
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
const NOT_A_GAME: &[&str] = &["patch", "fix", "mod", "mods", "trainer", "soundtrack", "ost", "wallpaper", "wallpapers", "manual", "save", "saves", "update", "redist", "dlc", "checksum", "plugin", "plugins", "tool", "tools", "addon", "addons", "asset", "assets", "assetpack", "blender", "texture", "textures", "documentation", "exiftool", "sdk"];

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

/// Whether an archive's files show a game engine: Unity, Unreal, Godot, GameMaker, RPG Maker,
/// Ren'Py, LÖVE, NW.js, MonoGame/FNA, Steam's or GOG's files.
fn engine_files(names: &[String]) -> bool {
    names.iter().map(|n| n.to_lowercase().replace('\\', "/")).any(|n| {
        let file = n.rsplit('/').next().unwrap_or(&n).to_string();
        ["unityplayer.dll", "unityplayer.so", "data.win", "game.ini", "nw.pak", "monogame.framework.dll", "fna.dll", "steam_api.dll", "steam_api64.dll", "libsteam_api.so"].contains(&file.as_str())
            || [".pck", ".love", ".rgssad", ".rgss2a", ".rgss3a", ".rpa"].iter().any(|e| file.ends_with(e))
            || file.starts_with("goggame-")
            // Unity's data folder by Unity's own files in it (plenty of tools have a "*_data/").
            || (n.contains("_data/") && ["globalgamemanagers", "resources.assets", "level0", "data.unity3d", "maindata"].contains(&file.as_str()))
            || n.contains("/engine/binaries/") || n.starts_with("engine/binaries/") || n.contains("/renpy/") || n.starts_with("renpy/")
    })
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

/// MyAbandonware's naming, for a file or folder: `Title-With-Dashes_Platform_Language[_…]`
/// (`Black-White_Win_EN_ISO-Version.zip`, `Pac-Man_PC-88_EN.zip`, `SimAnt_Win-3x_EN_…`): the
/// title (dashes as spaces; "Sorcerer s Stone" is "Sorcerer's Stone") and the platform as
/// people say it ("Windows", "Windows 3.x", "DOS", "Commodore 64").
pub fn myabandonware(name: &str) -> Option<(String, String)> {
    let lower = name.to_lowercase();
    let stem = [".tar.gz", ".zip", ".rar", ".7z", ".iso", ".exe"].iter().find(|e| lower.ends_with(*e)).map_or(name, |e| &name[..name.len() - e.len()]);
    let parts: Vec<&str> = stem.split('_').collect();
    let (title, platform, language) = (parts.first()?, parts.get(1)?, parts.get(2)?);
    let platform_like = platform.chars().next().is_some_and(|c| c.is_ascii_uppercase() || c.is_ascii_digit()) && platform.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
    let language_like = language.split('-').all(|l| l.len() == 2 && l.chars().all(|c| c.is_ascii_uppercase()));
    if title.is_empty() || !platform_like || !language_like {
        return None;
    }
    let mut words: Vec<String> = Vec::new();
    for w in title.split('-').filter(|w| !w.is_empty()) {
        match words.last_mut() {
            Some(last) if w == "s" => last.push_str("'s"),
            _ => words.push(w.to_string()),
        }
    }
    let platform = match *platform {
        "Win" => "Windows".to_string(),
        "Win-3x" => "Windows 3.x".to_string(),
        "C64" => "Commodore 64".to_string(),
        "Atari-8-bit" => "Atari 8-bit".to_string(),
        "VIC-20" | "PC-88" | "PC-98" => platform.to_string(),
        p => p.replace('-', " "),
    };
    Some((words.join(" "), platform))
}

/// A MyAbandonware download's name: its title, with its platform when that isn't Windows
/// ("Pac-Man (DOS)": one site lists a game once per system).
pub fn myabandonware_name(name: &str) -> Option<String> {
    let (title, platform) = myabandonware(name)?;
    Some(if platform == "Windows" { title } else { format!("{title} ({platform})") })
}

/// A download's name as a game's: its file name without the extension, copy numbers
/// ("(1)"), and versions or platforms at the end ("Ghost_Defender", "XCOW 64Bits",
/// "Dungeon Dash", "corrupted-boundary-win64-prealpha").
pub fn title(file: &Path) -> String {
    let mut stem = file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    if let Some(name) = myabandonware_name(&stem) {
        return name;
    }
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
    // Dotted versions stay whole ("1.0.2"), other dots part words ("Ghost.Defender").
    let spaced = stem.replace(['_', '-'], " ");
    let version = |w: &str| {
        let w = w.trim_start_matches(['v', 'V']);
        !w.is_empty() && w.contains('.') && w.chars().all(|c| c.is_ascii_digit() || c == '.')
    };
    let words: Vec<String> = spaced
        .split_whitespace()
        .flat_map(|w| {
            // "Ghost.Defender.v1.3": the words, then the version as one.
            let parts: Vec<&str> = w.split('.').filter(|p| !p.is_empty()).collect();
            let tail = (0..parts.len()).find(|&i| version(&parts[i..].join(".")) || i + 1 == parts.len() && version(parts[i])).unwrap_or(parts.len());
            let mut out: Vec<String> = parts[..tail].iter().map(|p| p.to_string()).collect();
            if tail < parts.len() {
                out.push(parts[tail..].join("."));
            }
            out
        })
        .collect();
    let mut words: Vec<&str> = words.iter().map(String::as_str).collect();
    let drop = |w: &str| {
        let l = w.to_lowercase();
        let l = l.trim_matches(|c| c == '(' || c == ')' || c == '[' || c == ']');
        l.is_empty()
            || version(l)
            // A build's number ("20240312", "002"); a short one is a sequel's ("Kingdom
            // Hearts 4", "Anno 1602").
            || (l.chars().all(|c| c.is_ascii_digit()) && (l.len() >= 5 || l.starts_with('0')))
            || (l.starts_with('v') && l.len() > 1 && l[1..].chars().all(|c| c.is_ascii_digit()))
            // "beta5", "build002": a stage and its number.
            || ["steamrip", "com", "part"].contains(&l.trim_end_matches(|c: char| c.is_ascii_digit()))
            || ["win", "win32", "win64", "windows", "windows64", "linux", "x64", "x86", "x86_64", "64bit", "32bit", "64bits", "32bits", "bit", "bits", "build", "release", "final", "prealpha", "alpha", "beta", "setup", "installer", "pc", "ver", "en", "iso", "version", "mac", "osx", "macos", "android"].iter().any(|t| *t == l || *t == l.trim_end_matches(|c: char| c.is_ascii_digit()))
    };
    let number = |w: &str| w.chars().all(|c| c.is_ascii_digit());
    // Two numbers or more at the end are a version written without dots ("spelunky_1_1"); one
    // alone is a sequel's.
    loop {
        let n = words.len();
        if n > 2 && number(words[n - 1]) && number(words[n - 2]) {
            while words.len() > 1 && number(words[words.len() - 1]) {
                words.pop();
            }
        } else if n > 1 && drop(words[n - 1]) {
            words.pop();
        } else {
            break;
        }
    }
    let t = words.join(" ");
    if t.is_empty() { stem } else { t }
}

/// A loose file as a game download, or nothing.
/// `strict` (a dumping ground of a folder: an old Downloads): only an archive showing a game
/// engine counts, and no loose .exe (tools and installers of all sorts).
pub fn look(file: &Path, strict: bool) -> Option<Entry> {
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
        if strict {
            return None;
        }
        // An installer, or a game in one program that runs as it is.
        let installer = match gamefix::binary::identify(file)? {
            gamefix::binary::Binary::Pe(pe) => pe.installer.is_some() || plain.contains("setup") || plain.contains("install"),
            _ => return None,
        };
        e.kind = if installer { Kind::Download } else { Kind::Game };
        return Some(e);
    }
    let names = contents(file)?;
    if strict && !engine_files(&names) {
        return None;
    }
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
    fn myabandonware_names() {
        assert_eq!(title(Path::new("/d/Bookworm-Deluxe_Win_EN_ISO.zip")), "Bookworm Deluxe");
        assert_eq!(title(Path::new("/d/Pac-Man_DOS_EN.zip")), "Pac Man (DOS)");
        assert_eq!(title(Path::new("/d/SimAnt_Win-3x_EN_Win3xO-release.zip")), "SimAnt (Windows 3.x)");
        assert_eq!(title(Path::new("/d/Harry-Potter-and-the-Sorcerer-s-Stone_Win_EN-ES_installer.zip")), "Harry Potter and the Sorcerer's Stone");
        assert_eq!(title(Path::new("/d/Pac-Man_Atari-8-bit_EN.zip")), "Pac Man (Atari 8-bit)");
        assert_eq!(myabandonware("NecroLore_Apeirophobia.zip"), None);
        assert_eq!(myabandonware("Project_P.I.T.T_DEMO_v0.2.8.1_linux.zip"), None);
    }

    #[test]
    fn titles() {
        assert_eq!(title(Path::new("/d/Ghost_Defender.zip")), "Ghost Defender");
        assert_eq!(title(Path::new("/d/XCOW 64Bits.zip")), "XCOW");
        assert_eq!(title(Path::new("/d/Kingdom Hearts 4 - Linux.tar.gz")), "Kingdom Hearts 4");
        assert_eq!(title(Path::new("/d/Anno 1602.zip")), "Anno 1602");
        assert_eq!(title(Path::new("/d/spelunky_1_1.zip")), "spelunky");
        assert_eq!(title(Path::new("/d/Zombielicious x64.zip")), "Zombielicious");
        assert_eq!(title(Path::new("/d/Game_1.0.2.zip")), "Game");
        assert_eq!(title(Path::new("/d/cool-game-20240312-win64.zip")), "cool game");
        assert_eq!(title(Path::new("/d/Ghost.Defender.v1.3.zip")), "Ghost Defender");
        assert_eq!(title(Path::new("/d/DataShift(1).zip")), "DataShift");
        assert_eq!(title(Path::new("/d/corrupted-boundary-win64-prealpha.zip")), "corrupted boundary");
        assert_eq!(title(Path::new("/d/Shoot'n'Slaughter 0.0.0.4.rar")), "Shoot'n'Slaughter");
        assert_eq!(title(Path::new("/d/StarshipAssassinSetup_beta5.exe")), "StarshipAssassinSetup");
        assert_eq!(title(Path::new("/d/minato-linux.tar.gz")), "minato");
        assert_eq!(title(Path::new("/d/Black-White_Win_EN_ISO-Version.zip")), "Black White");
        assert_eq!(title(Path::new("/d/Plague-Inc-Evolved-SteamRIP.com.rar")), "Plague Inc Evolved");
        assert_eq!(title(Path::new("/d/Some.Game.part1.rar")), "Some Game");
    }

    #[test]
    fn split_archives() {
        assert!(wanted(Path::new("/d/x.part1.rar")));
        assert!(!wanted(Path::new("/d/x.part2.rar")));
        assert!(wanted(Path::new("/d/Partition Wars.zip")));
    }

    #[test]
    fn engines_in_archives() {
        let names = |l: &[&str]| l.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(engine_files(&names(&["Spilled/game/UnityPlayer.dll", "Spilled/game/Spilled!.exe"])));
        assert!(engine_files(&names(&["Old/Old.exe", "Old/Old_Data/mainData"])));
        assert!(!engine_files(&names(&["HOps/operators/misc/pizza_ops_data/__init__.py", "retopoflow/docs/_data/keymaps.yml"])));
        assert!(engine_files(&names(&["G/Engine/Binaries/Win64/x.exe", "G/G.exe"])));
        assert!(engine_files(&names(&["game.pck", "game.exe"])));
        assert!(!engine_files(&names(&["rufus-4.6.exe"])));
        assert!(!engine_files(&names(&["exiftool(-k).exe", "exiftool_files/perl.exe"])));
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
