//! Games that only start through an app of their own from Flathub, offered on the Library page
//! before the app is installed: Roblox through Sober (Roblox's anti-cheat turns Wine away;
//! Sober runs Roblox's Android version). Install gets the app; its app-menu entry is the game.

use crate::library::{Entry, Key, Kind, Source};
use std::path::PathBuf;

pub struct App {
    /// The game's name, which the app's entry takes too.
    pub game: &'static str,
    pub flatpak: &'static str,
    /// The app's own name, for messages.
    pub app: &'static str,
    /// The app's icon on Flathub, for the card before it's installed (its entry has its own).
    pub icon: &'static str,
    /// What to know before installing it, on the game's page.
    pub warning: &'static str,
    /// The app's own controller switch: its settings file (in ~/.var/app/<id>) and the key in
    /// it. On, the app also needs Flatpak's access to every input device.
    pub controller: Option<(&'static str, &'static str)>,
}

pub const APPS: &[App] = &[App {
    game: "Roblox",
    flatpak: "org.vinegarhq.Sober",
    app: "Sober",
    icon: "https://dl.flathub.org/repo/appstream/x86_64/icons/128x128/org.vinegarhq.Sober.png",
    warning: "Roblox doesn't support Sober and may temporarily ban accounts that use it.",
    // Sober's documented setting (off by default; its first-run screens can turn it on). On
    // without the access, Sober only shows "Permission Update Required".
    controller: Some(("config/sober/config.json", "allow_gamepad_permission")),
}];

pub fn known(flatpak: &str) -> Option<&'static App> {
    APPS.iter().find(|a| a.flatpak == flatpak)
}

/// The app a game needs and hasn't got yet (its offer's key).
pub fn wanted(game: &crate::library::Game) -> Option<&'static App> {
    if game.kind != Kind::Owned {
        return None;
    }
    game.entries.iter().filter(|e| e.kind == Kind::Owned).flat_map(|e| e.keys.iter()).find_map(|k| if let Key::Flatpak(id) = k { known(id) } else { None })
}

/// The app a game is played through, installed or not.
pub fn of_game(game: &crate::library::Game) -> Option<&'static App> {
    game.entries.iter().flat_map(|e| e.keys.iter()).find_map(|k| if let Key::Flatpak(id) = k { known(id) } else { None })
}

fn user_flatpak() -> Option<PathBuf> {
    std::env::var_os("FLATPAK_USER_DIR").map(PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share/flatpak")))
}

/// Whether Flatpak has the app, for this user or the whole system.
fn installed(app: &App) -> bool {
    user_flatpak().into_iter().chain([PathBuf::from("/var/lib/flatpak")]).any(|d| d.join("app").join(app.flatpak).join("current").exists())
}

fn settings_file(app: &App) -> Option<PathBuf> {
    let (file, _) = app.controller?;
    Some(PathBuf::from(std::env::var_os("HOME")?).join(".var/app").join(app.flatpak).join(file))
}

/// Where a key's true/false value is in the app's settings (JSON after `//` comment lines).
fn value_at(text: &str, key: &str) -> Option<(usize, bool)> {
    let at = text.find(&format!("\"{key}\""))? + key.len() + 2;
    let rest = &text[at..];
    let colon = rest.find(':')?;
    let start = at + colon + 1 + rest[colon + 1..].len() - rest[colon + 1..].trim_start().len();
    let value = &text[start..];
    if value.starts_with("true") {
        Some((start, true))
    } else if value.starts_with("false") {
        Some((start, false))
    } else {
        None
    }
}

/// The app's settings with the key set: changed in place, added after the opening brace, or a
/// file of its own when the app hasn't written one yet.
fn with_value(text: Option<&str>, key: &str, on: bool) -> String {
    let word = if on { "true" } else { "false" };
    let Some(text) = text else { return format!("{{\n    \"{key}\": {word}\n}}\n") };
    if let Some((at, old)) = value_at(text, key) {
        let len = if old { 4 } else { 5 };
        return format!("{}{word}{}", &text[..at], &text[at + len..]);
    }
    // The first line that opens the object (the comment lines above it hold no brace).
    let mut out = String::new();
    let mut added = false;
    for line in text.split_inclusive('\n') {
        out.push_str(line);
        if !added && line.trim_start().starts_with('{') {
            if line.trim() == "{}" {
                out.truncate(out.len() - line.len());
                out.push_str(&format!("{{\n    \"{key}\": {word}\n}}\n"));
            } else {
                out.push_str(&format!("    \"{key}\": {word},\n"));
            }
            added = true;
        }
    }
    out
}

/// The app's own controller setting is on.
fn setting_on(app: &App) -> bool {
    let (Some(file), Some((_, key))) = (settings_file(app), app.controller) else { return false };
    std::fs::read_to_string(file).ok().and_then(|t| value_at(&t, key)).is_some_and(|(_, on)| on)
}

/// Flatpak lets the app see every input device (`flatpak override --user --device=input`).
fn input_granted(app: &App) -> bool {
    let Some(file) = user_flatpak().map(|d| d.join("overrides").join(app.flatpak)) else { return false };
    let text = std::fs::read_to_string(file).unwrap_or_default();
    text.lines().filter_map(|l| l.trim().strip_prefix("devices=")).any(|d| d.split(';').any(|x| x == "input"))
}

/// The game gets the controller: the app's setting is on and Flatpak lets it see input devices.
pub fn controller_on(app: &App) -> bool {
    setting_on(app) && input_granted(app)
}

/// Turns the app's controller support on (its setting, and every input device for it) or off
/// (both taken back, so the app can't read the keyboard either).
pub fn set_controller(app: &App, on: bool) -> Result<(), String> {
    let (Some(file), Some((_, key))) = (settings_file(app), app.controller) else { return Err(format!("{} has no controller setting Lobby knows.", app.app)) };
    let flag = if on { "--device=input" } else { "--nodevice=input" };
    let out = std::process::Command::new("flatpak").args(["override", "--user", flag, app.flatpak]).stdin(std::process::Stdio::null()).output().map_err(|e| format!("flatpak: {e}"))?;
    if !out.status.success() {
        return Err(format!("flatpak override: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    let text = std::fs::read_to_string(&file).ok();
    if text.is_none() && !on {
        return Ok(());
    }
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    std::fs::write(&file, with_value(text.as_deref(), key, on)).map_err(|e| format!("{}: {e}", file.display()))
}

/// Before the game starts: an app whose controller setting is on without the access it needs
/// only asks for a command to be run, so the setting goes off and the game starts; the game's
/// page turns it on properly. Says what it did.
pub fn before_start(game: &crate::library::Game) -> Option<String> {
    let app = of_game(game)?;
    let (file, key) = (settings_file(app)?, app.controller?.1);
    if !setting_on(app) || input_granted(app) {
        return None;
    }
    let text = std::fs::read_to_string(&file).ok()?;
    match std::fs::write(&file, with_value(Some(&text), key, false)) {
        Ok(()) => Some(format!("{}'s controller support was on without the input access it asks for, so it was turned off for {} to start. {}'s page in Lobby turns it on.", app.app, game.name, game.name)),
        Err(e) => Some(format!("{}'s controller support couldn't be turned off: {e}", app.app)),
    }
}

/// The games whose app isn't installed, as games to install.
pub fn offers() -> Vec<Entry> {
    APPS.iter().filter(|a| !installed(a)).map(offer).collect()
}

fn offer(app: &App) -> Entry {
    let mut e = Entry::new(Source::AppMenu, app.game.to_string());
    e.kind = Kind::Owned;
    e.icon = crate::covers::web_copy(app.icon);
    e.keys.push(Key::Flatpak(app.flatpak.to_string()));
    e
}

/// Installs the app from Flathub, for this user.
pub fn install(app: &App) -> Result<(), String> {
    gamefix::setup::install_flatpak(app.flatpak).map_err(|e| format!("{} couldn't be installed from Flathub: {e}", app.app))
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_controller_setting_is_changed_in_place() {
        let sober = "// !!! STOP !!!\n// comment\n{\n    \"allow_gamepad_permission\": true,\n    \"close_on_leave\": true\n}\n";
        let off = super::with_value(Some(sober), "allow_gamepad_permission", false);
        assert_eq!(off, sober.replace("\"allow_gamepad_permission\": true", "\"allow_gamepad_permission\": false"));
        assert_eq!(super::value_at(&off, "allow_gamepad_permission").map(|(_, v)| v), Some(false));
        assert_eq!(super::with_value(Some(&off), "allow_gamepad_permission", true), sober);
        let missing = super::with_value(Some("// c\n{\n    \"a\": 1\n}\n"), "allow_gamepad_permission", true);
        assert_eq!(missing, "// c\n{\n    \"allow_gamepad_permission\": true,\n    \"a\": 1\n}\n");
        assert_eq!(super::with_value(None, "k", true), "{\n    \"k\": true\n}\n");
        assert_eq!(super::with_value(Some("{}\n"), "k", false), "{\n    \"k\": false\n}\n");
    }

    #[test]
    fn an_offer_merges_with_its_apps_entry() {
        let offer = super::offer(&super::APPS[0]);
        let mut entry = crate::library::Entry::new(crate::library::Source::AppMenu, "Roblox");
        entry.keys.push(crate::library::Key::Flatpak("org.vinegarhq.Sober".into()));
        let games = crate::library::merge(vec![offer, entry], &[]);
        assert_eq!(games.len(), 1);
        assert_eq!(games[0].kind, crate::library::Kind::Game);
        assert!(super::wanted(&games[0]).is_none());
    }
}
