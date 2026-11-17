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
}

pub const APPS: &[App] = &[App { game: "Roblox", flatpak: "org.vinegarhq.Sober", app: "Sober", icon: "https://dl.flathub.org/repo/appstream/x86_64/icons/128x128/org.vinegarhq.Sober.png",
    warning: "Roblox doesn't support Sober and may temporarily ban accounts that use it.",
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

/// Whether Flatpak has the app, for this user or the whole system.
fn installed(app: &App) -> bool {
    let user = std::env::var_os("FLATPAK_USER_DIR").map(PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share/flatpak")));
    user.into_iter().chain([PathBuf::from("/var/lib/flatpak")]).any(|d| d.join("app").join(app.flatpak).join("current").exists())
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
