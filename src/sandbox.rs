//! Protection for the games Lobby starts itself (its store installs, gamefix's set-ups, games
//! found in a prefix): the game runs in a bubblewrap sandbox, at the level picked on its page.
//!
//! - Standard: the home folder and drives stay, minus what a stealer is after (keys, browser
//!   profiles, chat and store sign-ins, password managers: `HIDDEN`, plus names like *token*),
//!   with the files that run code at login read-only (`LOCKED`).
//! - Isolated: a home of its own (~/.local/share/lobby/sandboxes/<id>), and only the game's
//!   folder, its prefix and folders the user shares with it; no internet unless allowed.
//! - Off: as before.
//!
//! Both: a PID namespace of its own (ptrace and /proc/<pid>/root would get around the rest),
//! the session bus only through xdg-dbus-proxy (notifications, screensaver and sleep inhibit,
//! the portal's settings, GameMode: no systemd, keyring, Flatpak or URI opener, each a way out),
//! no system bus, a /tmp and /run/user of its own with only the display, sound and Discord
//! sockets. Steam's folder is hidden but for its client libraries (Proton's steam.exe loads
//! them through ~/.steam/sdk64); umu's runtime and Proton get a throwaway writable layer
//! (they take lock files there, and a game must not change what every game runs).
//!
//! A game runs as `lobby sandbox <id> -- <command>`, which looks the level up at each start.
//! gamefix puts the same first in the game's app-menu entry (`sync_shortcut`), so the
//! shortcut starts it protected too.

use crate::library::{Game, Key, Kind, Source};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    Standard,
    Isolated,
    Off,
}

impl Level {
    pub const ALL: [Level; 3] = [Level::Standard, Level::Isolated, Level::Off];

    pub fn name(self) -> &'static str {
        match self {
            Level::Standard => "Standard",
            Level::Isolated => "Isolated",
            Level::Off => "Off",
        }
    }

    pub fn describe(self) -> &'static str {
        match self {
            Level::Standard => "Your files stay, but not your passwords, keys, browser profiles or sign-ins, and the game can't make itself start with your computer.",
            Level::Isolated => "The game sees only its own folder and Wine prefix, with a home folder of its own: nothing else of yours.",
            Level::Off => "The game can do anything you can.",
        }
    }

    pub fn parse(s: &str) -> Option<Level> {
        Level::ALL.into_iter().find(|l| l.name().eq_ignore_ascii_case(s))
    }
}

/// What the user chose for a game (none of it: the defaults).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Protection {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub level: Option<Level>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub internet: Option<bool>,
    /// Folders an isolated game may use too (mods, a level editor's files).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub folders: Vec<PathBuf>,
}

impl Protection {
    pub fn is_default(&self) -> bool {
        *self == Protection::default()
    }
}

/// The game's level: the user's, else Isolated for a game from itch.io or a download not played
/// yet (played ones keep their saves where they are), else Standard.
pub fn level(game: &Game) -> Level {
    game.protection.level.unwrap_or_else(|| if untrusted(game) && game.played.is_none() { Level::Isolated } else { Level::Standard })
}

/// Internet: the user's choice, else on unless isolated.
pub fn internet(game: &Game) -> bool {
    game.protection.internet.unwrap_or(level(game) != Level::Isolated)
}

/// From itch.io or a download, and no store or launcher vouches for it.
fn untrusted(game: &Game) -> bool {
    let from_download = game.entries.iter().any(|e| e.source == Source::Itch || e.keys.iter().any(|k| matches!(k, Key::Download(_))) || e.folder.as_ref().is_some_and(|f| f.join(".lobby-download").is_file()));
    game.unknown_to_launchers() && from_download
}

/// The level and internet written down, so a later default (the game has been played now)
/// doesn't change them. Returns whether anything changed.
pub fn settle(game: &mut Game) -> bool {
    let (level, internet) = (level(game), internet(game));
    let changed = game.protection.level.is_none() || game.protection.internet.is_none();
    game.protection.level = Some(level);
    game.protection.internet = Some(internet);
    changed
}

fn lobby_exe() -> String {
    crate::me().display().to_string()
}

/// Whether a command's first word is Lobby: `lobby`, its AppImage (this one, or a file named
/// Lobby….AppImage: an older download a shortcut still names), or Lobby running inside an
/// AppImage (its mount's AppRun, as a process names itself).
pub fn is_lobby(word: &str) -> bool {
    let path = Path::new(word);
    let name = path.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
    name == "lobby" || (name.starts_with("lobby") && name.ends_with(".appimage")) || path == crate::me() || (word.contains("/.mount_Lobby") && name.starts_with("apprun"))
}

/// `<lobby> sandbox <id> --` before a command: how lobby (and gamefix's entries, once they
/// carry it) runs a game protected. The words after it, when there.
pub fn unwrapped(words: &[String]) -> Option<&[String]> {
    let lobby = words.first().is_some_and(|w| is_lobby(w));
    (lobby && words.get(1).is_some_and(|w| w == "sandbox") && words.get(3).is_some_and(|w| w == "--")).then(|| &words[4..])
}

/// The command Play runs: the game's own inside the sandbox, unless it's off (or `lobby run`,
/// which sandboxes the game itself and needs the store's sign-in outside).
pub fn wrap(game: &Game, command: Vec<String>) -> Vec<String> {
    let lobby_run = command.first().is_some_and(|w| is_lobby(w)) && command.get(1).is_some_and(|w| w == "run");
    // Off too: `lobby sandbox` then runs it as it is, and times it (`playtime`).
    if lobby_run || unwrapped(&command).is_some() {
        return command;
    }
    [vec![lobby_exe(), "sandbox".into(), game.id.clone(), "--".into()], command].concat()
}

/// The installed lobby (run/install.sh's), which app-menu entries name: not a build being tried.
/// An AppImage names itself.
fn installed_lobby() -> String {
    let installed = PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/bin/lobby");
    if installed.is_file() && std::env::var_os("APPIMAGE").is_none() { installed.display().to_string() } else { lobby_exe() }
}

/// Makes gamefix's app-menu entry for the game start it as Play does: `lobby sandbox <id> --`
/// first in its Exec, or nothing when it's Off (or the entry runs a Flatpak, which has a
/// sandbox of its own). Only when that changes (gamefix rebuilds the menu each time). Whether
/// it did.
pub fn sync_shortcut(game: &Game) -> Result<bool, String> {
    let mut changed = false;
    for file in game.entries.iter().filter(|e| e.source == Source::AppMenu && !e.made_by_gamefix()).flat_map(|e| e.keys.iter()).filter_map(|k| if let Key::Desktop(p) = k { Some(p) } else { None }) {
        changed |= sync_entry(game, file)?;
    }
    let Some(record) = crate::uninstall::gamefix_record(game) else { return Ok(changed) };
    let text = fs::read_to_string(&record).unwrap_or_default();
    let current: Option<Vec<String>> = text.lines().find_map(|l| l.strip_prefix("wrapper\t")).map(|w| w.split('\t').map(String::from).collect());
    let entry = text.lines().filter_map(|l| l.strip_prefix("created\t")).filter(|p| p.ends_with(".desktop")).find_map(|p| crate::launch::desktop_exec(&fs::read_to_string(p).ok()?));
    let Some((words, _)) = entry else { return Ok(changed) };
    let wanted = (shortcut_wrapped(game) && crate::launch::hands_over(&words).is_none()).then(|| wrapper(game));
    if current == wanted {
        return Ok(changed);
    }
    gamefix::setup::set_wrapper(&record, wanted.as_deref()).map(|()| true)
}

/// Whether the game's shortcuts start it through `lobby sandbox`: unless it's Off, and then too
/// where only programs AppArmor names may make sandboxes (Proton's too): Lobby's profile is
/// what lets them.
fn shortcut_wrapped(game: &Game) -> bool {
    level(game) != Level::Off || crate::apparmor::restricted()
}

fn wrapper(game: &Game) -> Vec<String> {
    vec![installed_lobby(), "sandbox".into(), game.id.clone(), "--".into()]
}

/// The key that keeps an entry's own command while Lobby's wrapper is in front of it.
const ORIGINAL: &str = "X-Lobby-Original-Exec";

/// Another program's (or your own) app-menu entry or desktop shortcut for the game: its Exec
/// gets the wrapper in front, the command it had kept beside it (`ORIGINAL`), put back for
/// Off. Only files in the home you can write (a system entry stays as it is: Play is still
/// protected); not an entry that hands the game to another launcher or a container.
fn sync_entry(game: &Game, file: &Path) -> Result<bool, String> {
    let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    let Ok(text) = fs::read_to_string(file) else { return Ok(false) };
    // A link is another program's file: a Flatpak's entry links into the app's installed files,
    // which Flatpak shares with its own store of them (writing it would change both).
    if fs::symlink_metadata(file).is_ok_and(|m| m.file_type().is_symlink()) || file.components().any(|c| c.as_os_str() == "flatpak") {
        return Ok(false);
    }
    if !(file.starts_with(&home) || real(file).starts_with(real(&home))) || fs::metadata(file).is_ok_and(|m| m.permissions().readonly()) {
        return Ok(false);
    }
    let rewritten = entry_text(&text, shortcut_wrapped(game).then(|| wrapper(game)).as_deref());
    if rewritten == text {
        return Ok(false);
    }
    fs::write(file, rewritten).map_err(|e| format!("{}: {e}", file.display()))?;
    let _ = Command::new("kbuildsycoca6").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status();
    Ok(true)
}

/// An entry's text with `wrapper` in front of its own command (None: its own command back).
/// Only the main [Desktop Entry] group; an entry handing the game elsewhere is left alone.
fn entry_text(text: &str, wrapper: Option<&[String]>) -> String {
    let mut main = false;
    let original = text.lines().skip_while(|l| l.trim() != "[Desktop Entry]").skip(1).take_while(|l| !l.starts_with('[')).find_map(|l| l.strip_prefix(&format!("{ORIGINAL}="))).map(String::from);
    let mut out = String::new();
    for line in text.lines() {
        if line.starts_with('[') {
            main = line.trim() == "[Desktop Entry]";
        }
        if main && line.starts_with(&format!("{ORIGINAL}=")) {
            continue;
        }
        match line.strip_prefix("Exec=").filter(|_| main) {
            Some(exec) => {
                let own = original.clone().unwrap_or_else(|| exec.to_string());
                let words = crate::launch::exec_words(&own);
                match wrapper.filter(|_| crate::launch::hands_over(&words).is_none()) {
                    Some(w) => {
                        let quoted: Vec<String> = w.iter().map(|word| gamefix::setup::exec_arg(word)).collect();
                        out.push_str(&format!("Exec={} {own}\n{ORIGINAL}={own}\n", quoted.join(" ")));
                    }
                    None => out.push_str(&format!("Exec={own}\n")),
                }
            }
            None => {
                out.push_str(line);
                out.push('\n');
            }
        }
    }
    out
}

/// Every game's shortcut as its protection says (after a scan: games set up since, ids).
pub fn sync_shortcuts(games: &[Game]) {
    for g in games.iter().filter(|g| g.kind == Kind::Game && !g.gone) {
        match sync_shortcut(g) {
            Ok(true) => crate::log::line(&format!("protection: {}'s app-menu entry now {}", g.name, if level(g) == Level::Off { "starts it unprotected" } else { "starts it in its sandbox" })),
            Ok(false) => {}
            Err(e) => crate::log::line(&format!("protection: {}'s app-menu entry: {e}", g.name)),
        }
    }
}

/// A command ready to run in the game's sandbox; the D-Bus proxy lives as long as it.
pub struct Sandboxed {
    pub command: Command,
    proxy: Option<(Child, PathBuf)>,
}

impl Drop for Sandboxed {
    fn drop(&mut self) {
        if let Some((mut child, socket)) = self.proxy.take() {
            let _ = child.kill();
            let _ = child.wait();
            let _ = fs::remove_file(socket);
        }
    }
}

/// The game's command (`env` words and all) in its sandbox. `env` is what the caller adds to
/// the command's environment (the paths in both are what the game gets to use).
pub fn command(game: &Game, env: &[(String, String)], words: &[String], cwd: Option<&Path>) -> Result<Sandboxed, String> {
    // umu-run by its path: Lobby's own (in its folder in the home) is then there, read-only,
    // in an isolated home too.
    let words: Vec<String> = words.iter().map(|w| if w == "umu-run" { crate::launch::program_on_path(w).map_or(w.clone(), |p| real(&p).display().to_string()) } else { w.clone() }).collect();
    let words = &words[..];
    let (program, args) = words.split_first().ok_or("empty command")?;
    let level = level(game);
    if (level != Level::Off || words.iter().any(|w| w.ends_with("umu-run"))) && crate::apparmor::blocked_here() {
        return Err(format!("{} Allow Lobby first: press Play in Lobby's window, or run `lobby allow-sandbox`.", crate::apparmor::WHY));
    }
    if level == Level::Off {
        let mut c = Command::new(program);
        c.args(args).envs(env.iter().cloned());
        if let Some(dir) = cwd {
            c.current_dir(dir);
        }
        return Ok(Sandboxed { command: c, proxy: None });
    }
    let home = real(&PathBuf::from(std::env::var_os("HOME").ok_or("HOME isn't set")?));
    if Path::new(program).file_name().is_some_and(|n| n == "flatpak") && args.first().is_some_and(|a| a == "run") {
        let mut c = Command::new(program);
        c.arg("run").args(flatpak_options(game, env, words, cwd, &home)).args(args[1..].iter().filter(|a| !grants_home(a))).envs(env.iter().cloned());
        if let Some(dir) = cwd {
            c.current_dir(dir);
        }
        return Ok(Sandboxed { command: c, proxy: None });
    }
    let (bwrap, overlays) = bwrap().ok_or("Protecting the game needs bubblewrap (bwrap), which isn't installed: install your system's bubblewrap package.")?;
    let runtime = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).filter(|r| r.is_dir());
    let proxy = runtime.as_deref().and_then(start_proxy);
    let private = (level == Level::Isolated).then(|| private_home(&game.id)).transpose()?;
    let spec = Spec {
        level,
        internet: internet(game),
        game: game_folders(game, env, words, cwd, &home),
        reads: named_paths(env, words).into_iter().filter(|p| hidden_area(p, &home)).collect(),
        devices: devices(),
        sockets: runtime.as_deref().map(sockets).unwrap_or_default(),
        x11: x11_socket(),
        xauth: xauth(),
        bus: proxy.as_ref().map(|(_, s)| s.clone()),
        steam: steam_roots(&home),
        present: present(&home),
        home,
        runtime,
        private,
        overlays,
    };
    let mut c = Command::new(bwrap);
    c.args(args_for(&spec)).args(wineserver_args()).arg("--").arg(program).args(args).envs(env.iter().cloned());
    if let Some(dir) = cwd {
        c.current_dir(dir);
    }
    Ok(Sandboxed { command: c, proxy })
}

/// The bubblewrap to run, and whether it has throwaway layers: the system's when it does
/// (0.10, 2024, and later), else pressure-vessel's newer one in umu's or Steam's runtime (it runs
/// on the host; Ubuntu 24.04 has 0.9), else the system's anyway, with Proton's folders writable.
/// Where AppArmor restricts sandboxes, Lobby's copy of the system's: Ubuntu's profile for
/// /usr/bin/bwrap keeps what runs in it from making sandboxes of its own (Proton's), Lobby's
/// profile (`apparmor`) doesn't, and covers what Lobby runs from anywhere but /usr/bin/bwrap.
fn bwrap() -> Option<(PathBuf, bool)> {
    let system = crate::launch::program_on_path("bwrap").map(|b| if crate::apparmor::restricted() { own_copy(&b).unwrap_or(b) } else { b });
    if let Some(b) = system.as_ref().filter(|b| has_overlays(b)) {
        return Some((b.clone(), true));
    }
    let home = real(&PathBuf::from(std::env::var_os("HOME").unwrap_or_default()));
    let tool = Path::new("pressure-vessel/libexec/steam-runtime-tools-0/srt-bwrap");
    let umu = fs::read_dir(home.join(".local/share/umu")).into_iter().flatten().flatten().map(|e| e.path().join(tool));
    let steam = steam_roots(&home).into_iter().flat_map(|r| fs::read_dir(home.join(r).join("steamapps/common")).into_iter().flatten().flatten()).filter(|e| e.file_name().to_string_lossy().starts_with("SteamLinuxRuntime")).map(|e| e.path().join(tool));
    if let Some(b) = umu.chain(steam).find(|b| b.is_file() && has_overlays(b)) {
        return Some((b, true));
    }
    system.map(|b| (b, false))
}

/// A copy of the system's bubblewrap in Lobby's folder, made again when that one changes.
fn own_copy(bwrap: &Path) -> Option<PathBuf> {
    let copy = crate::store::dir().join("tools/bwrap");
    let same = |a: &fs::Metadata, b: &fs::Metadata| a.len() == b.len() && a.modified().ok() == b.modified().ok();
    let source = fs::metadata(bwrap).ok()?;
    if !fs::metadata(&copy).is_ok_and(|c| same(&c, &source)) {
        let part = copy.with_extension("part");
        fs::create_dir_all(copy.parent()?).ok()?;
        fs::copy(bwrap, &part).ok()?;
        fs::File::options().write(true).open(&part).ok()?.set_modified(source.modified().ok()?).ok()?;
        fs::rename(&part, &copy).ok()?;
    }
    Some(copy)
}

/// Whether a bubblewrap is 0.10 or later ("bubblewrap 0.13.0").
fn has_overlays(bwrap: &Path) -> bool {
    let out = Command::new(bwrap).arg("--version").stdin(Stdio::null()).stderr(Stdio::null()).output().map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default();
    let v: Vec<u32> = out.trim().rsplit(' ').next().unwrap_or_default().split('.').filter_map(|n| n.parse().ok()).collect();
    v.len() >= 2 && (v[0], v[1]) >= (0, 10)
}

/// A game a Flatpak app runs (Ruffle, DOSBox ...): Flatpak's sandbox, told to give it the game's
/// folder and nothing else of the home or the drives (such apps often ask for the whole
/// home), and no network when the game has no internet. Both levels alike: the app keeps its
/// own data folder (~/.var/app/<app>), where saves go.
fn flatpak_options(game: &Game, env: &[(String, String)], words: &[String], cwd: Option<&Path>, home: &Path) -> Vec<String> {
    let mut o: Vec<String> = ["--nofilesystem=host", "--nofilesystem=home"].map(String::from).to_vec();
    let folders = game_folders(game, env, words, cwd, home);
    for f in &folders {
        o.push(format!("--filesystem={}", f.display()));
    }
    for r in named_paths(env, words).into_iter().filter(|r| hidden_area(r, home) && !folders.iter().any(|f| r.starts_with(f))) {
        o.push(format!("--filesystem={}:ro", r.display()));
    }
    if !internet(game) {
        o.push("--unshare=network".into());
    }
    o
}

/// A `flatpak run` option giving the app the home or the whole computer back.
fn grants_home(word: &str) -> bool {
    word.strip_prefix("--filesystem=").is_some_and(|f| matches!(f.split(':').next().unwrap_or_default(), "home" | "host" | "~" | "/"))
}

/// Wine's server folder (/tmp/.wine-<uid>) shared with the sandbox: games in one prefix (a
/// launcher's shared one) must find one wineserver, or each starts its own on the same prefix.
fn wineserver_args() -> Vec<String> {
    let Ok(uid) = fs::metadata("/proc/self").map(|m| std::os::unix::fs::MetadataExt::uid(&m)) else { return Vec::new() };
    let dir = PathBuf::from(format!("/tmp/.wine-{uid}"));
    if !dir.is_dir() && fs::create_dir(&dir).is_ok() {
        let _ = fs::set_permissions(&dir, std::os::unix::fs::PermissionsExt::from_mode(0o700));
    }
    if dir.is_dir() {
        vec!["--bind".into(), dir.display().to_string(), dir.display().to_string()]
    } else {
        Vec::new()
    }
}

/// Everything about the computer the sandbox's arguments depend on, gathered first so that
/// building them stays a plain function.
struct Spec {
    level: Level,
    internet: bool,
    home: PathBuf,
    runtime: Option<PathBuf>,
    /// The isolated game's own home.
    private: Option<PathBuf>,
    /// Whether this bubblewrap has throwaway layers (`--tmp-overlay`, 0.10 and later).
    overlays: bool,
    /// Writable: the game's folders, prefix, the folders shared with it.
    game: Vec<PathBuf>,
    /// Read-only: other paths the command names (Java, a config file gamefix wrote ...).
    reads: Vec<PathBuf>,
    devices: Vec<PathBuf>,
    /// Display, sound and Discord sockets under the runtime folder: (the socket, where it goes).
    sockets: Vec<(PathBuf, PathBuf)>,
    x11: Option<PathBuf>,
    xauth: Option<PathBuf>,
    bus: Option<PathBuf>,
    /// Steam's folders in the home (relative to it), `steam_roots`.
    steam: Vec<PathBuf>,
    /// Which of the home's files and folders this exists (relative to the home).
    present: Vec<(PathBuf, bool)>,
}

/// Paths a stealer is after, relative to the home: hidden in Standard. Names holding
/// "token", "credential", "secret", "password" or "wallet" are hidden too (`secret_name`).
const HIDDEN: &[&str] = &[
    ".ssh", ".gnupg", ".pki", ".password-store", ".local/share/keyrings", ".local/share/kwalletd", ".config/kdeconnect",
    // Browsers and mail.
    ".mozilla", ".config/mozilla", ".thunderbird", ".librewolf", ".waterfox", ".zen", ".floorp",
    ".config/google-chrome", ".config/google-chrome-beta", ".config/google-chrome-unstable", ".config/chromium", ".config/BraveSoftware", ".config/vivaldi", ".config/opera", ".config/microsoft-edge",
    ".var/app/org.mozilla.firefox", ".var/app/org.mozilla.Thunderbird", ".var/app/net.waterfox.waterfox", ".var/app/io.gitlab.librewolf-community", ".var/app/app.zen_browser.zen", ".var/app/one.ablaze.floorp",
    ".var/app/com.google.Chrome", ".var/app/com.google.ChromeDev", ".var/app/org.chromium.Chromium", ".var/app/io.github.ungoogled_software.ungoogled_chromium", ".var/app/com.brave.Browser", ".var/app/com.microsoft.Edge", ".var/app/com.vivaldi.Vivaldi", ".var/app/com.opera.Opera",
    // Chat.
    ".config/discord", ".config/discordcanary", ".config/discordptb", ".config/vesktop", ".config/Signal", ".config/Element", ".config/Slack", ".config/stoat-desktop", ".config/revolt-desktop", ".local/share/TelegramDesktop",
    ".var/app/com.discordapp.Discord", ".var/app/com.discordapp.DiscordCanary", ".var/app/dev.vencord.Vesktop", ".var/app/org.signal.Signal", ".var/app/org.telegram.desktop", ".var/app/im.riot.Riot", ".var/app/com.slack.Slack", ".var/app/chat.revolt.RevoltDesktop",
    // Password managers.
    ".config/Bitwarden", ".config/KeePassXC", ".config/1Password", ".var/app/com.bitwarden.desktop", ".var/app/org.keepassxc.KeePassXC", ".var/app/com.onepassword.OnePassword",
    // Game stores and launchers' sign-ins (Steam's whole folder is hidden apart).
    ".config/lobby", ".config/heroic", ".config/heroic_gogdl", ".config/legendary", ".var/app/com.heroicgameslauncher.hgl", ".cache/lutris", ".config/unityhub", ".local/share/UnityAuthBroker", ".var/app/com.valvesoftware.Steam",
    // Developer and cloud sign-ins.
    ".claude", ".claude.json", ".config/Claude", ".local/share/claude", ".config/gh", ".git-credentials", ".config/git/credentials", ".netrc", ".npmrc", ".pypirc", ".cargo/credentials", ".cargo/credentials.toml",
    ".docker", ".kube", ".aws", ".azure", ".config/gcloud", ".config/rclone", ".config/.wrangler", ".config/configstore", ".config/turborepo", ".local/share/ferret-signing",
    // Wallets.
    ".electrum", ".bitcoin", ".ethereum", ".config/Exodus", ".config/Ledger Live",
];

/// What runs code at login, or when another program starts, relative to the home: read-only in
/// Standard. Lobby's and gamefix's own folders too (a game must not turn its protection off,
/// or change the fixes other games get), and the user's programs.
const LOCKED: &[&str] = &[
    ".bashrc", ".bash_profile", ".bash_login", ".bash_logout", ".profile", ".zshrc", ".zprofile", ".zshenv", ".zlogin", ".config/fish", ".config/nushell",
    ".xprofile", ".xinitrc", ".xsession", ".xsessionrc", ".pam_environment", ".config/environment.d",
    ".config/autostart", ".config/autostart-scripts", ".config/plasma-workspace", ".config/systemd", ".local/share/systemd", ".local/share/dbus-1",
    ".config/kglobalshortcutsrc", ".config/khotkeysrc", ".config/kwinrc", ".local/share/kwin", ".local/share/plasma", ".local/share/kservices5", ".local/share/kservices6",
    // GNOME's (Shell extensions; dconf holds custom shortcuts' commands and which extensions
    // run), Cinnamon's and Xfce's (xfconf: its shortcuts).
    ".local/share/gnome-shell", ".config/dconf", ".local/share/cinnamon", ".config/cinnamon", ".config/xfce4/xfconf",
    ".config/mimeapps.list", ".local/share/applications", ".local/share/mime", ".local/share/flatpak", ".var/app", ".local/bin", ".local/lib", ".cargo/bin",
    ".local/share/lobby", ".local/share/gamefix", ".config/gamefix", ".wine",
];

/// Folders under the home the umu/Proton tools live in: a throwaway writable layer over them
/// (and over each Steam folder's compatibilitytools.d and Proton builds).
const TOOLS: &[&str] = &[".local/share/umu"];

/// Read-only in an isolated home, for looks (fonts, cursors) and MangoHud.
const LOOKS: &[&str] = &[".local/share/fonts", ".fonts", ".config/fontconfig", ".icons", ".local/share/icons", ".config/MangoHud"];

fn secret_name(name: &str) -> bool {
    let n = name.to_lowercase();
    ["token", "credential", "secret", "password", "wallet"].iter().any(|w| n.contains(w))
}

/// Steam's folders in the home, relative to it, each once, the one ~/.steam/root names (where
/// Steam last ran) first: its own (~/.local/share/Steam, also where umu puts GE-Proton), the
/// Debian and Ubuntu package's (~/.steam/debian-installation, which ~/.steam/steam links to),
/// Flatpak Steam's.
fn steam_roots(home: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    for rel in [".steam/root", ".local/share/Steam", ".steam/steam", ".var/app/com.valvesoftware.Steam/.local/share/Steam"] {
        let path = real(&home.join(rel));
        let Ok(rel) = path.strip_prefix(home) else { continue };
        // compatibilitytools.d alone: umu's GE-Proton where Steam's folder is elsewhere (Ubuntu's).
        if ["steamapps", "linux64", "compatibilitytools.d"].iter().any(|d| path.join(d).is_dir()) && !out.iter().any(|o| o == rel) {
            out.push(rel.to_path_buf());
        }
    }
    out
}

/// What of HIDDEN, LOCKED, TOOLS, LOOKS, Steam and the secret-named files exists in the home.
fn present(home: &Path) -> Vec<(PathBuf, bool)> {
    let mut names: Vec<PathBuf> = HIDDEN.iter().chain(LOCKED).chain(TOOLS).chain(LOOKS).map(PathBuf::from).collect();
    let steam = steam_roots(home);
    for root in &steam {
        names.extend(["", "linux32", "linux64", "compatibilitytools.d"].map(|d| root.join(d)));
    }
    for dir in ["", ".config", ".local/share"] {
        for entry in fs::read_dir(home.join(dir)).into_iter().flatten().flatten() {
            if secret_name(&entry.file_name().to_string_lossy()) {
                names.push(Path::new(dir).join(entry.file_name()));
            }
        }
    }
    // Proton builds in Steam's libraries in the home.
    for common in steam.iter().map(|r| r.join("steamapps/common")) {
        for entry in fs::read_dir(home.join(&common)).into_iter().flatten().flatten() {
            if entry.file_name().to_string_lossy().starts_with("Proton") {
                names.push(common.join(entry.file_name()));
            }
        }
    }
    names.into_iter().filter_map(|n| fs::symlink_metadata(home.join(&n)).ok().filter(|m| !m.is_symlink()).map(|m| (n, m.is_dir()))).collect()
}

fn args_for(s: &Spec) -> Vec<String> {
    let mut a: Vec<String> = Vec::new();
    let mut push = |words: &[&str]| a.extend(words.iter().map(|w| w.to_string()));
    let p = |path: &Path| path.display().to_string();
    let at_home = |rel: &Path| s.home.join(rel);
    let has = |rel: &str| s.present.iter().find(|(n, _)| n == Path::new(rel)).map(|(_, dir)| *dir);
    match s.level {
        Level::Isolated => push(&["--ro-bind", "/", "/"]),
        _ => push(&["--bind", "/", "/"]),
    }
    push(&["--dev", "/dev", "--proc", "/proc", "--unshare-pid", "--unshare-cgroup-try", "--die-with-parent", "--new-session"]);
    for d in &s.devices {
        push(&["--dev-bind", &p(d), &p(d)]);
    }
    if !s.internet {
        push(&["--unshare-net"]);
    }
    // Shared places of their own: /tmp, the system bus's folder, the runtime folder.
    push(&["--tmpfs", "/tmp", "--tmpfs", "/var/tmp"]);
    if Path::new("/run/dbus").is_dir() {
        push(&["--tmpfs", "/run/dbus"]);
    }
    if let Some(x) = &s.x11 {
        push(&["--ro-bind", &p(x), &p(x)]);
    }
    if let Some(r) = &s.runtime {
        push(&["--tmpfs", &p(r)]);
        for (sock, to) in &s.sockets {
            push(&["--ro-bind", &p(sock), &p(to)]);
        }
        match &s.bus {
            Some(bus) => push(&["--ro-bind", &p(bus), &p(&r.join("bus")), "--setenv", "DBUS_SESSION_BUS_ADDRESS", &format!("unix:path={}", r.join("bus").display())]),
            None => push(&["--unsetenv", "DBUS_SESSION_BUS_ADDRESS"]),
        }
    }
    push(&["--unsetenv", "SSH_AUTH_SOCK", "--unsetenv", "GPG_AGENT_INFO", "--unsetenv", "GNOME_KEYRING_CONTROL", "--unsetenv", "AT_SPI_BUS_ADDRESS", "--unsetenv", "SESSION_MANAGER"]);
    match s.level {
        Level::Isolated => {
            // No home, no drives: a home of its own.
            for mask in masks(&s.home) {
                push(&["--tmpfs", &p(&mask)]);
            }
            if let Some(private) = &s.private {
                push(&["--bind", &p(private), &p(&s.home)]);
            }
            for look in LOOKS.iter().filter(|l| has(l).is_some()) {
                push(&["--ro-bind", &p(&at_home(Path::new(look))), &p(&at_home(Path::new(look)))]);
            }
        }
        _ => {
            // Read-only first: hidden folders inside one (.var/app) are hidden after.
            for rel in LOCKED.iter().filter(|l| has(l).is_some()) {
                push(&["--ro-bind", &p(&at_home(Path::new(rel))), &p(&at_home(Path::new(rel)))]);
            }
            for (rel, dir) in s.present.iter().filter(|(n, _)| HIDDEN.iter().any(|h| Path::new(h) == n) || n.file_name().is_some_and(|f| secret_name(&f.to_string_lossy()))) {
                match dir {
                    true => push(&["--tmpfs", &p(&at_home(rel))]),
                    false => push(&["--ro-bind", "/dev/null", &p(&at_home(rel))]),
                }
            }
            for root in s.steam.iter().filter(|r| has(&r.to_string_lossy()) == Some(true)) {
                push(&["--tmpfs", &p(&at_home(root))]);
            }
        }
    }
    // Steam's client libraries, where Proton's steam.exe looks (~/.steam/sdk*: links into
    // Steam's folder, which an isolated home doesn't have).
    // Standard binds each Steam's at its own path, Isolated the first Steam's at ~/.steam/sdk*.
    for bits in ["32", "64"] {
        let libs = s.steam.iter().map(|r| r.join(format!("linux{bits}"))).filter(|l| has(&l.to_string_lossy()) == Some(true));
        for lib in libs.take(if s.level == Level::Isolated { 1 } else { usize::MAX }) {
            let to = if s.level == Level::Isolated { at_home(Path::new(&format!(".steam/sdk{bits}"))) } else { at_home(&lib) };
            push(&["--ro-bind", &p(&at_home(&lib)), &p(&to)]);
        }
    }
    // umu's runtime and Proton: usable, unchanged.
    let proton = |n: &Path| s.steam.iter().any(|r| n == r.join("compatibilitytools.d") || n.starts_with(r.join("steamapps/common")));
    for (rel, _) in s.present.iter().filter(|(n, dir)| *dir && (TOOLS.iter().any(|t| Path::new(t) == n) || proton(n))) {
        let path = p(&at_home(rel));
        match s.overlays {
            true => push(&["--overlay-src", &path, "--tmp-overlay", &path]),
            false => push(&["--bind", &path, &path]),
        }
    }
    if let Some(auth) = &s.xauth {
        push(&["--ro-bind", &p(auth), &p(auth)]);
    }
    for r in s.reads.iter().filter(|r| s.level == Level::Isolated && !s.game.iter().any(|g| r.starts_with(g))) {
        push(&["--ro-bind", &p(r), &p(r)]);
    }
    for g in &s.game {
        push(&["--bind", &p(g), &p(g)]);
    }
    a
}

/// Where an isolated game sees nothing: the folders holding homes, and the drives.
fn masks(home: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = home.parent().map(Path::to_path_buf).into_iter().collect();
    for top in ["/home", "/run/media", "/media", "/mnt", "/sysroot", "/var/mnt"] {
        let path = real(Path::new(top));
        if path.is_dir() && !out.contains(&path) && path != Path::new("/") {
            out.push(path);
        }
    }
    out
}

/// The isolated game's home, made the first time.
fn private_home(id: &str) -> Result<PathBuf, String> {
    let data = std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/share"));
    let dir = data.join("lobby/sandboxes").join(id);
    fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    Ok(real(&dir))
}

fn real(p: &Path) -> PathBuf {
    fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

/// The game's writable folders: its own (not one too wide to be a game's), its Wine prefix,
/// the folder it runs in when that's the game's, and the folders the user shared with it.
fn game_folders(game: &Game, env: &[(String, String)], words: &[String], cwd: Option<&Path>, home: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    let add = |p: PathBuf, out: &mut Vec<PathBuf>| {
        let p = real(&p);
        if p.is_dir() && !out.iter().any(|o| p.starts_with(o)) {
            out.retain(|o| !o.starts_with(&p));
            out.push(p);
        }
    };
    for e in game.entries.iter().filter(|e| e.kind != Kind::Download) {
        let folder = e.folder.clone().or_else(|| e.program.as_ref().and_then(|p| p.parent().map(Path::to_path_buf)));
        if let Some(f) = folder.filter(|f| !too_wide(f, home)) {
            add(f, &mut out);
        }
        if let Some(prefix) = &e.prefix {
            add(prefix.clone(), &mut out);
        }
    }
    let word_env = words.iter().skip_while(|w| *w == "env").map_while(|w| w.split_once('=')).map(|(k, v)| (k.to_string(), v.to_string()));
    for (k, v) in env.iter().cloned().chain(word_env) {
        if matches!(k.as_str(), "WINEPREFIX" | "STEAM_COMPAT_DATA_PATH") {
            add(PathBuf::from(v), &mut out);
        }
    }
    if let Some(dir) = cwd.filter(|d| !too_wide(d, home) && game.entries.iter().any(|e| e.folder.as_ref().is_some_and(|f| real(d).starts_with(real(f))))) {
        add(dir.to_path_buf(), &mut out);
    }
    for f in &game.protection.folders {
        add(f.clone(), &mut out);
    }
    out
}

/// A folder holding more than the game: the home or above it, a drive's top, a folder Lobby
/// looks in, or one named like a folder of downloads or games.
fn too_wide(folder: &Path, home: &Path) -> bool {
    let dir = real(folder);
    let dev = |p: &Path| fs::metadata(p).map(|m| std::os::unix::fs::MetadataExt::dev(&m)).ok();
    let name = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    home.starts_with(&dir)
        || dir.parent().is_none_or(|up| dev(up) != dev(&dir))
        || crate::places::games_or_downloads(&name)
        || crate::sources::Home::find().is_some_and(|h| crate::sources::folders::roots(&h).iter().any(|r| real(r) == dir))
}

/// Absolute paths the command names (its words, `NAME=value` words and variables, values split
/// at ':'), as they are: a program, a jar, PATH's folders, gamefix's sound settings.
fn named_paths(env: &[(String, String)], words: &[String]) -> Vec<PathBuf> {
    let values = env.iter().map(|(_, v)| v.clone()).chain(words.iter().map(|w| w.split_once('=').map_or(w.clone(), |(_, v)| v.to_string())));
    let mut out = Vec::new();
    for v in values {
        for part in v.split(':').filter(|p| p.starts_with('/')) {
            let path = real(Path::new(part));
            if path.exists() && !out.contains(&path) {
                out.push(path);
            }
        }
    }
    // A Java runtime is its whole folder (bin/java needs lib/).
    out.into_iter().map(|p| p.ancestors().find(|a| a.parent().is_some_and(|up| up.ends_with("gamefix/java"))).map_or(p.clone(), Path::to_path_buf)).collect()
}

/// Whether the sandbox hides the place a path is in (so it's bound back): the home, drives.
fn hidden_area(path: &Path, home: &Path) -> bool {
    let tops = masks(home);
    (path.starts_with(home) || tops.iter().any(|t| path.starts_with(t))) && !steam_roots(home).iter().any(|r| path.starts_with(home.join(r))) && !path.starts_with(home.join(".local/share/umu"))
}

/// The GPU (and NVIDIA's), controllers, Wine's ntsync.
fn devices() -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = ["/dev/dri", "/dev/input", "/dev/ntsync", "/dev/kfd", "/dev/udmabuf", "/dev/nvidia-caps"].iter().map(PathBuf::from).filter(|p| p.exists()).collect();
    for entry in fs::read_dir("/dev").into_iter().flatten().flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if (name.starts_with("nvidia") && name != "nvidia-caps") || name.starts_with("hidraw") {
            out.push(entry.path());
        }
    }
    out
}

/// The display's, sound's and Discord's sockets (Rich Presence) in the runtime folder, each
/// with where it goes: a link (Discord's flatpak puts its socket in app/...) is replaced by the
/// socket it points at.
fn sockets(runtime: &Path) -> Vec<(PathBuf, PathBuf)> {
    let mut names: Vec<String> = ["WAYLAND_DISPLAY", "GAMESCOPE_WAYLAND_DISPLAY"].iter().filter_map(|v| std::env::var(v).ok()).filter(|n| !n.contains('/')).collect();
    names.extend(["pipewire-0", "pulse/native"].map(String::from));
    names.extend((0..10).map(|i| format!("discord-ipc-{i}")));
    names.iter().map(|n| runtime.join(n)).filter(|p| p.exists()).map(|p| (real(&p), p)).collect()
}

/// The X server's key ($XAUTHORITY), wherever the login screen put it: the runtime folder
/// (GNOME's and KDE's Xwayland), /tmp, or the home (LightDM: Mint, Xubuntu), which an isolated
/// game doesn't have.
fn xauth() -> Option<PathBuf> {
    std::env::var_os("XAUTHORITY").map(PathBuf::from).filter(|a| a.is_absolute() && a.is_file())
}

/// The X server's socket for $DISPLAY.
fn x11_socket() -> Option<PathBuf> {
    let display = std::env::var("DISPLAY").ok()?;
    let n = display.strip_prefix(':')?.split('.').next()?;
    Some(PathBuf::from(format!("/tmp/.X11-unix/X{n}"))).filter(|p| p.exists())
}

/// xdg-dbus-proxy in front of the session bus, letting through only what games use.
fn start_proxy(runtime: &Path) -> Option<(Child, PathBuf)> {
    let address = std::env::var("DBUS_SESSION_BUS_ADDRESS").ok()?;
    let proxy = crate::launch::program_on_path("xdg-dbus-proxy").or_else(|| Some(PathBuf::from("/usr/libexec/xdg-dbus-proxy")).filter(|p| p.is_file()))?;
    let dir = runtime.join("lobby");
    fs::create_dir_all(&dir).ok()?;
    // Sockets left by a `lobby sandbox` that was killed (its proxy went with it).
    for entry in fs::read_dir(&dir).into_iter().flatten().flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.strip_prefix("bus-").is_some_and(|pid| !Path::new("/proc").join(pid).exists()) {
            let _ = fs::remove_file(entry.path());
        }
    }
    let socket = dir.join(format!("bus-{}", std::process::id()));
    let desktop = "org.freedesktop.portal.Desktop";
    let rules = [
        "--filter".to_string(),
        "--talk=org.freedesktop.Notifications".into(),
        "--talk=com.feralinteractive.GameMode".into(),
        "--call=org.freedesktop.ScreenSaver=org.freedesktop.ScreenSaver.*@/org/freedesktop/ScreenSaver".into(),
        "--call=org.freedesktop.ScreenSaver=org.freedesktop.ScreenSaver.*@/ScreenSaver".into(),
        format!("--call={desktop}=org.freedesktop.portal.Settings.*@/org/freedesktop/portal/desktop"),
        format!("--call={desktop}=org.freedesktop.portal.Inhibit.*@/org/freedesktop/portal/desktop"),
        format!("--call={desktop}=org.freedesktop.DBus.Properties.*@/org/freedesktop/portal/desktop"),
        format!("--broadcast={desktop}=org.freedesktop.portal.Settings.SettingChanged@/org/freedesktop/portal/desktop"),
    ];
    let child = Command::new(proxy).arg(address).arg(&socket).args(rules).stdin(Stdio::null()).stdout(Stdio::null()).spawn().ok()?;
    for _ in 0..100 {
        if socket.exists() {
            return Some((child, socket));
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let mut child = child;
    let _ = child.kill();
    None
}

/// For the game's page and `lobby how`: what protection it gets, in a sentence.
pub fn summary(game: &Game) -> String {
    let level = level(game);
    let net = if internet(game) { "" } else { ", no internet" };
    match level {
        Level::Off => "Off: the game can do anything you can.".into(),
        _ => format!("{}{net}. {}", level.name(), level.describe()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn knows_lobby_in_every_form() {
        for w in ["/home/p/.local/bin/lobby", "/home/p/Applications/Lobby-x86_64.AppImage", "/home/p/Downloads/lobby-0.3.appimage", "/tmp/.mount_Lobby-AbC123/AppRun.wrapped"] {
            assert!(is_lobby(w), "{w}");
        }
        for w in ["/usr/bin/umu-run", "/home/p/Applications/Other.AppImage", "/tmp/.mount_Other-x/AppRun.wrapped", "/home/p/lobby-tools/x"] {
            assert!(!is_lobby(w), "{w}");
        }
    }

    fn spec(level: Level) -> Spec {
        Spec {
            level,
            internet: level != Level::Isolated,
            home: "/home/p".into(),
            runtime: Some("/run/user/1000".into()),
            private: Some("/home/p/.local/share/lobby/sandboxes/g".into()),
            game: vec!["/run/media/p/Games/G".into(), "/home/p/.local/share/gamefix/prefixes/g".into()],
            reads: vec!["/home/p/.local/share/gamefix/java/25".into(), "/run/media/p/Games/G/G.exe".into()],
            devices: vec!["/dev/dri".into()],
            sockets: vec![("/run/user/1000/wayland-0".into(), "/run/user/1000/wayland-0".into())],
            x11: None,
            xauth: Some("/home/p/.Xauthority".into()),
            bus: Some("/run/user/1000/lobby/bus-1".into()),
            overlays: true,
            steam: vec![".local/share/Steam".into()],
            present: vec![(".ssh".into(), true), (".bashrc".into(), false), (".local/share/umu".into(), true), (".local/share/Steam".into(), true), (".local/share/Steam/linux64".into(), true), ("invent-kde-token".into(), false), (".local/share/gamefix".into(), true)],
        }
    }

    #[test]
    fn standard_hides_secrets_and_locks_startup_files() {
        let a = args_for(&spec(Level::Standard));
        assert!(a.windows(2).any(|w| w == ["--tmpfs", "/home/p/.ssh"]));
        assert!(a.windows(3).any(|w| w == ["--ro-bind", "/dev/null", "/home/p/invent-kde-token"]));
        assert!(a.windows(3).any(|w| w == ["--ro-bind", "/home/p/.bashrc", "/home/p/.bashrc"]));
        assert!(a.windows(2).any(|w| w == ["--tmpfs", "/home/p/.local/share/Steam"]));
        assert!(a.windows(3).any(|w| w == ["--ro-bind", "/home/p/.local/share/Steam/linux64", "/home/p/.local/share/Steam/linux64"]));
        assert!(a.windows(4).any(|w| w == ["--overlay-src", "/home/p/.local/share/umu", "--tmp-overlay", "/home/p/.local/share/umu"]));
        assert!(!a.contains(&"--unshare-net".to_string()));
        assert!(a.contains(&"--unshare-pid".to_string()));
        // The prefix under gamefix's (read-only) folder is writable: bound after it.
        let locked = a.windows(3).position(|w| w == ["--ro-bind", "/home/p/.local/share/gamefix", "/home/p/.local/share/gamefix"]).unwrap();
        let prefix = a.windows(3).position(|w| w == ["--bind", "/home/p/.local/share/gamefix/prefixes/g", "/home/p/.local/share/gamefix/prefixes/g"]).unwrap();
        assert!(locked < prefix);
        assert!(a.windows(3).any(|w| w == ["--setenv", "DBUS_SESSION_BUS_ADDRESS", "unix:path=/run/user/1000/bus"]));
    }

    #[test]
    fn isolated_sees_only_the_game() {
        let a = args_for(&spec(Level::Isolated));
        assert_eq!(&a[..3], ["--ro-bind", "/", "/"]);
        assert!(a.contains(&"--unshare-net".to_string()));
        assert!(a.windows(3).any(|w| w == ["--bind", "/home/p/.local/share/lobby/sandboxes/g", "/home/p"]));
        assert!(a.windows(3).any(|w| w == ["--bind", "/run/media/p/Games/G", "/run/media/p/Games/G"]));
        // Java is read-only; the game's own program is in its (writable) folder.
        assert!(a.windows(3).any(|w| w == ["--ro-bind", "/home/p/.local/share/gamefix/java/25", "/home/p/.local/share/gamefix/java/25"]));
        assert!(!a.windows(3).any(|w| w == ["--ro-bind", "/run/media/p/Games/G/G.exe", "/run/media/p/Games/G/G.exe"]));
        assert!(!a.iter().any(|w| w == "/home/p/.ssh"));
        // The home's own home comes before the game's folders bound into it.
        let home = a.windows(3).position(|w| w[2] == "/home/p" && w[0] == "--bind").unwrap();
        let prefix = a.windows(3).position(|w| w[1] == "/home/p/.local/share/gamefix/prefixes/g").unwrap();
        assert!(home < prefix);
        // The X server's key in the real home (LightDM's) is there in its own.
        let xauth = a.windows(3).position(|w| w == ["--ro-bind", "/home/p/.Xauthority", "/home/p/.Xauthority"]).unwrap();
        assert!(home < xauth);
    }

    #[test]
    fn finds_debian_and_flatpak_steam() {
        let home = std::env::temp_dir().join(format!("lobby-steam-roots-{}", std::process::id()));
        let deb = home.join(".steam/debian-installation");
        let flat = home.join(".var/app/com.valvesoftware.Steam/.local/share/Steam");
        for d in [deb.join("steamapps"), deb.join("linux64"), flat.join("steamapps/common/Proton 9.0"), flat.join("compatibilitytools.d")] {
            fs::create_dir_all(d).unwrap();
        }
        std::os::unix::fs::symlink(&deb, home.join(".steam/steam")).unwrap();
        std::os::unix::fs::symlink(&deb, home.join(".steam/root")).unwrap();
        let roots = steam_roots(&home);
        assert_eq!(roots, [PathBuf::from(".steam/debian-installation"), PathBuf::from(".var/app/com.valvesoftware.Steam/.local/share/Steam")]);
        let present = present(&home);
        let mut s = spec(Level::Standard);
        (s.home, s.steam, s.present) = (home.clone(), roots, present);
        let a = args_for(&s);
        let at = |rel: &str| home.join(rel).display().to_string();
        assert!(a.windows(2).any(|w| w == ["--tmpfs".to_string(), at(".steam/debian-installation")]));
        assert!(a.windows(3).any(|w| w == ["--ro-bind".to_string(), at(".steam/debian-installation/linux64"), at(".steam/debian-installation/linux64")]));
        let proton = at(".var/app/com.valvesoftware.Steam/.local/share/Steam/steamapps/common/Proton 9.0");
        assert!(a.windows(4).any(|w| w == ["--overlay-src".to_string(), proton.clone(), "--tmp-overlay".into(), proton.clone()]));
        assert!(a.iter().any(|w| *w == at(".var/app/com.valvesoftware.Steam/.local/share/Steam/compatibilitytools.d")));
        assert!(!hidden_area(&flat.join("compatibilitytools.d/GE-Proton10-1/proton"), &home));
        fs::remove_dir_all(&home).unwrap();
    }

    #[test]
    fn entries_get_and_lose_the_wrapper() {
        let text = "[Desktop Entry]\nName=Toilet Sim\nExec=/g/Toilet Sim.x86_64\nPath=/g\n[Desktop Action x]\nExec=other\n";
        let w = ["/home/p/.local/bin/lobby".to_string(), "sandbox".into(), "toilet-sim".into(), "--".into()];
        let wrapped = entry_text(text, Some(&w));
        assert!(wrapped.contains("Exec=/home/p/.local/bin/lobby sandbox toilet-sim -- /g/Toilet Sim.x86_64\nX-Lobby-Original-Exec=/g/Toilet Sim.x86_64\n"));
        assert!(wrapped.contains("[Desktop Action x]\nExec=other\n"));
        // Again: no second wrapper; and off: the entry as it was.
        assert_eq!(entry_text(&wrapped, Some(&w)), wrapped);
        assert_eq!(entry_text(&wrapped, None), text);
        let distrobox = "[Desktop Entry]\nExec=distrobox enter box -- /g/play.sh\n";
        assert_eq!(entry_text(distrobox, Some(&w)), distrobox);
    }

    #[test]
    fn wrapping() {
        let words = |w: &[&str]| w.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let wrapped = words(&["/home/p/.local/bin/lobby", "sandbox", "g", "--", "env", "A=1", "umu-run", "x.exe"]);
        assert_eq!(unwrapped(&wrapped).unwrap(), words(&["env", "A=1", "umu-run", "x.exe"]).as_slice());
        assert!(unwrapped(&words(&["env", "A=1"])).is_none());
    }
}
