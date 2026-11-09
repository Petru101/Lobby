//! Running as an AppImage (build/appimage.sh): Lobby's insides are mounted somewhere new each
//! start ($APPDIR), and the AppImage file itself ($APPIMAGE) is what others must start
//! (`crate::me`). It updates itself from GitHub's releases: the new file downloaded beside it,
//! checked against the fingerprint GitHub gives, and put in its place under the same name
//! (what shortcuts, the app menu and Ubuntu's AppArmor profile name); the running Lobby keeps
//! its old file open, and the next start is the new one.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Where the bundled GTK finds its parts, set by the AppImage's start-up script
/// (run/appimage.sh), which keeps the system's own values as LOBBY_HOST_<name>.
const GTK_VARS: &[&str] = &["XDG_DATA_DIRS", "GSETTINGS_SCHEMA_DIR", "GTK_PATH", "GTK_EXE_PREFIX", "GTK_DATA_PREFIX", "GDK_PIXBUF_MODULE_FILE", "GI_TYPELIB_PATH"];

/// The system's own values back, for what Lobby starts (games, Steam, store tools): the
/// bundled GTK's point into the AppImage, whose mount goes when Lobby quits. The window calls
/// it once GTK has read them.
pub fn host_env() {
    if std::env::var_os("APPDIR").is_none() {
        return;
    }
    for v in GTK_VARS {
        let host = format!("LOBBY_HOST_{v}");
        match std::env::var_os(&host) {
            Some(value) => std::env::set_var(v, value),
            None => std::env::remove_var(v),
        }
        std::env::remove_var(host);
    }
}

/// For the window: into the AppImage's folder, where WebKit finds its helper programs
/// (appimage.sh points WebKit's library at ./wk, relative, as the mount moves).
pub fn enter() {
    if let Some(dir) = std::env::var_os("APPDIR") {
        let _ = std::env::set_current_dir(dir);
    }
}

/// WebKit runs its pages in a sandbox of its own, which sees only the system's folders: the
/// AppImage's too, read-only, or its page program (in there) can't start. Before any page.
pub fn let_webkit_in() {
    if let (Some(dir), Some(context)) = (std::env::var_os("APPDIR"), webkit6::WebContext::default()) {
        context.add_path_to_sandbox(dir, true);
    }
}

/// Lobby in the app menu, started from the AppImage file, with its App Menu Shortcuts picker:
/// each written when there's no entry, or one starting a program that's gone or another
/// AppImage (an older download). An entry for another Lobby install (~/.local/bin/lobby) is
/// left alone.
pub fn menu_entry() {
    let Some(file) = std::env::var_os("APPIMAGE").map(PathBuf::from).filter(|p| p.is_file()) else { return };
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else { return };
    let apps = home.join(".local/share/applications");
    let lobby = file.display().to_string();
    for (name, text) in [("io.github.Petru101.Lobby.desktop", entry_text(&lobby)), ("io.github.Petru101.Lobby.Shortcuts.desktop", shortcuts_entry_text(&lobby))] {
        let entry = apps.join(name);
        let exec = fs::read_to_string(&entry).ok().and_then(|t| t.lines().find_map(|l| l.strip_prefix("Exec=").map(|e| e.trim_start_matches('"').split('"').next().unwrap_or_default().split(" shortcuts").next().unwrap_or_default().to_string())));
        let keep = exec.as_deref().is_some_and(|e| e == lobby || (PathBuf::from(e).is_file() && !e.to_lowercase().ends_with(".appimage")));
        if !keep {
            let _ = fs::create_dir_all(&apps);
            let _ = fs::write(&entry, text);
        }
    }
}

fn entry_text(lobby: &str) -> String {
    format!("[Desktop Entry]\nType=Application\nName=Lobby\nGenericName=Game library\nComment=Every game on this computer in one place\nExec=\"{lobby}\"\nIcon=applications-games\nCategories=Game;\nKeywords=games;launcher;steam;heroic;lutris;\nStartupWMClass=io.github.Petru101.Lobby\n")
}

/// The App Menu Shortcuts picker's own entry (run/install.sh writes the same).
fn shortcuts_entry_text(lobby: &str) -> String {
    format!("[Desktop Entry]\nType=Application\nName=App Menu Shortcuts\nGenericName=Pick what the app menu shows\nComment=Games and Windows programs (Battle.net ...) in the app menu or out\nExec=\"{lobby}\" shortcuts\nIcon=applications-games\nCategories=Game;\nKeywords=wine;proton;windows;start menu;shortcut;battle.net;games;menu;\nStartupWMClass=io.github.Petru101.Lobby.Shortcuts\n")
}

/// Where the latest release is described (LOBBY_UPDATE_URL instead, to try updating against a
/// release of one's own).
const LATEST: &str = "https://api.github.com/repos/Petru101/Lobby/releases/latest";
const ASSET: &str = "Lobby-x86_64.AppImage";
const SIGNATURE: &str = "Lobby-x86_64.AppImage.minisig";

/// The public half of the key releases are signed with (minisign). The private half is on its
/// maker's computer and never on GitHub, so a broken-into GitHub account can't make an update
/// Lobby accepts. Changing this locks out every copy already out there.
const RELEASE_KEY: &str = "RWQAl3HEvrPvVnn2KWxBGZkieuWRHgR7ypStvwaBp/xUKvBkKU4cxaaY";

/// LOBBY_TEST_UPDATE_KEY, at build time only: a throwaway key for trying an update, so the real
/// one never signs a test file.
fn release_key() -> &'static str {
    option_env!("LOBBY_TEST_UPDATE_KEY").unwrap_or(RELEASE_KEY)
}

/// A newer Lobby on GitHub.
/// What Lobby does about new versions (~/.config/lobby/updates; automatic when it says nothing).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Updates {
    Automatic,
    Ask,
    Off,
}

impl Updates {
    pub const ALL: [Updates; 3] = [Updates::Automatic, Updates::Ask, Updates::Off];

    fn file() -> PathBuf {
        crate::stores::config_home().join("lobby/updates")
    }

    fn word(self) -> &'static str {
        match self {
            Updates::Automatic => "automatic",
            Updates::Ask => "ask",
            Updates::Off => "off",
        }
    }

    pub fn from_word(word: &str) -> Option<Updates> {
        Updates::ALL.into_iter().find(|u| u.word() == word.trim())
    }

    pub fn load() -> Updates {
        fs::read_to_string(Updates::file()).ok().and_then(|t| Updates::from_word(&t)).unwrap_or(Updates::Automatic)
    }

    pub fn save(self) -> Result<(), String> {
        let file = Updates::file();
        fs::create_dir_all(file.parent().unwrap()).map_err(|e| e.to_string())?;
        fs::write(&file, self.word()).map_err(|e| format!("{}: {e}", file.display()))
    }

    /// As the About page's list says it.
    pub fn label(self) -> &'static str {
        match self {
            Updates::Automatic => "Install them by itself",
            Updates::Ask => "Ask me first",
            Updates::Off => "Don't look for them",
        }
    }

    pub fn explained(self) -> &'static str {
        match self {
            Updates::Automatic => "Once a day while its window is open, Lobby asks GitHub for a newer version. If there is one, it downloads it and replaces itself, then tells you. The new version runs from the next start.",
            Updates::Ask => "Once a day while its window is open, Lobby asks GitHub for a newer version. If there is one, it tells you, and installs it only if you say so.",
            Updates::Off => "Lobby doesn't ask GitHub for new versions unless you press Check now, and stays on this version.",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Update {
    pub version: String,
    url: String,
    sha256: String,
    /// Its signature's address; a release with none is refused when installing.
    signature: Option<String>,
    /// The release's page, to download by hand.
    pub page: String,
}

/// "v0.10.2" → [0, 10, 2].
fn version_parts(v: &str) -> Vec<u64> {
    v.trim_start_matches('v').split(['.', '-', '+']).map_while(|p| p.parse().ok()).collect()
}

fn newer(latest: &str, running: &str) -> bool {
    version_parts(latest) > version_parts(running)
}

/// The update GitHub's description of a release offers this Lobby, if it's newer.
fn update_in(json: &str, running: &str) -> Option<Update> {
    let release: serde_json::Value = serde_json::from_str(json).ok()?;
    let version = release.get("tag_name")?.as_str()?.trim_start_matches('v').to_string();
    if !newer(&version, running) {
        return None;
    }
    let asset = release.get("assets")?.as_array()?.iter().find(|a| a.get("name").and_then(|n| n.as_str()) == Some(ASSET))?;
    let url_of = |name: &str| release.get("assets")?.as_array()?.iter().find(|a| a.get("name").and_then(|n| n.as_str()) == Some(name))?.get("browser_download_url")?.as_str().map(String::from);
    Some(Update {
        version,
        signature: url_of(SIGNATURE),
        url: asset.get("browser_download_url")?.as_str()?.to_string(),
        sha256: asset.get("digest")?.as_str()?.strip_prefix("sha256:")?.to_string(),
        page: release.get("html_url").and_then(|u| u.as_str()).unwrap_or("https://github.com/Petru101/Lobby/releases").to_string(),
    })
}

/// Whether `data` is what was signed with `key` for this version. The version is in the signed
/// comment, so an older release can't be passed off as a newer one to take someone back to it.
fn signature_holds(data: &[u8], signature: &str, key: &str, version: &str) -> Result<(), String> {
    let refused = "Its signature doesn't match the key Lobby's releases are signed with, so it wasn't installed.";
    let key = minisign_verify::PublicKey::from_base64(key).map_err(|_| "Lobby's own release key can't be read, so nothing was installed.")?;
    let signature = minisign_verify::Signature::decode(signature).map_err(|_| refused)?;
    key.verify(data, &signature, false).map_err(|_| refused)?;
    if signature.trusted_comment() != format!("Lobby {version}") {
        return Err(format!("Its signature is for another version ({}), so it wasn't installed.", signature.trusted_comment()));
    }
    Ok(())
}

/// The AppImage file this Lobby runs from, when it does.
pub fn file() -> Option<PathBuf> {
    std::env::var_os("APPIMAGE").map(PathBuf::from).filter(|p| p.is_file())
}

/// A newer Lobby than this one, asking GitHub. None when there's none (or this isn't an
/// AppImage: a build from source updates with its source).
pub fn check() -> Result<Option<Update>, String> {
    if file().is_none() {
        return Ok(None);
    }
    let url = std::env::var("LOBBY_UPDATE_URL").unwrap_or_else(|_| LATEST.to_string());
    let out = Command::new("curl").args(["-fsSL", "--max-time", "20", "-H", "Accept: application/vnd.github+json"]).arg(&url).stdin(Stdio::null()).output().map_err(|e| format!("curl: {e}"))?;
    if !out.status.success() {
        return Err(format!("GitHub didn't answer: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    Ok(update_in(&String::from_utf8_lossy(&out.stdout), env!("CARGO_PKG_VERSION")))
}

/// Whether Lobby may replace its own file (one in a folder of the user's, not /opt).
pub fn can_replace() -> bool {
    file().and_then(|f| f.parent().map(Path::to_path_buf)).is_some_and(|dir| {
        let probe = dir.join(".lobby-update-probe");
        let ok = fs::write(&probe, "").is_ok();
        let _ = fs::remove_file(&probe);
        ok
    })
}

/// Downloads the update beside this AppImage, checks its fingerprint and its signature, and puts
/// it in this one's place. From the next start (or a restart) it's the new version.
pub fn install(update: &Update) -> Result<(), String> {
    let file = file().ok_or("Lobby isn't running as an AppImage")?;
    let signature_url = update.signature.as_ref().ok_or("This release isn't signed, so it wasn't installed.")?;
    let signature = Command::new("curl").args(["-fsSL", "--retry", "2", "--max-time", "60"]).arg(signature_url).stdin(Stdio::null()).output().map_err(|e| format!("curl: {e}"))?;
    if !signature.status.success() {
        return Err(format!("Its signature couldn't be downloaded: {}", String::from_utf8_lossy(&signature.stderr).trim()));
    }
    let signature = String::from_utf8_lossy(&signature.stdout).into_owned();
    let part = file.with_file_name(format!(".{}.new", file.file_name().unwrap_or_default().to_string_lossy()));
    let got = Command::new("curl").args(["-fsSL", "--retry", "2", "-o"]).arg(&part).arg(&update.url).stdin(Stdio::null()).output().map_err(|e| format!("curl: {e}"))?;
    if !got.status.success() {
        let _ = fs::remove_file(&part);
        return Err(format!("The download failed: {}", String::from_utf8_lossy(&got.stderr).trim()));
    }
    let sum = Command::new("sha256sum").arg(&part).stdin(Stdio::null()).output().map_err(|e| format!("sha256sum: {e}"))?;
    let sum = String::from_utf8_lossy(&sum.stdout).split_whitespace().next().unwrap_or_default().to_string();
    if !sum.eq_ignore_ascii_case(&update.sha256) {
        let _ = fs::remove_file(&part);
        return Err("The download doesn't match the release's fingerprint, so it wasn't used.".into());
    }
    let downloaded = fs::read(&part).map_err(|e| e.to_string())?;
    if let Err(e) = signature_holds(&downloaded, &signature, release_key(), &update.version) {
        let _ = fs::remove_file(&part);
        return Err(e);
    }
    drop(downloaded);
    fs::set_permissions(&part, std::os::unix::fs::PermissionsExt::from_mode(0o755)).map_err(|e| e.to_string())?;
    // A rename: the running Lobby keeps the old file it has open.
    fs::rename(&part, &file).map_err(|e| {
        let _ = fs::remove_file(&part);
        format!("Lobby couldn't replace {}: {e}", file.display())
    })
}

/// When Lobby last looked (~/.cache/lobby/update-checked), so the window asks GitHub at most
/// once a day.
fn stamp() -> PathBuf {
    let cache = std::env::var_os("XDG_CACHE_HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".cache"));
    cache.join("lobby/update-checked")
}

/// Whether a day has passed since the last look (and marks this one).
pub fn due() -> bool {
    if file().is_none() {
        return false;
    }
    let file = stamp();
    let recent = fs::metadata(&file).and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|age| age.as_secs() < 24 * 3600);
    if recent && std::env::var_os("LOBBY_UPDATE_URL").is_none() {
        return false;
    }
    let _ = fs::create_dir_all(file.parent().unwrap());
    let _ = fs::write(&file, "");
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_update_setting_reads_its_words() {
        assert_eq!(Updates::from_word("automatic\n"), Some(Updates::Automatic));
        assert_eq!(Updates::from_word("ask"), Some(Updates::Ask));
        assert_eq!(Updates::from_word("off"), Some(Updates::Off));
        assert_eq!(Updates::from_word("sometimes"), None);
    }

    // Made with a throwaway key whose private half was deleted.
    const TEST_KEY: &str = "RWTKwRAMyby1v2jqi/8hHUHpoZUU8WUiCmaj5+xFOB4dzAi3CVGWImxP";
    const SIGNED: &[u8] = b"test key file\n";
    const SIGNATURE_FOR_0_2_0: &str = "untrusted comment: signature from minisign secret key\nRUTKwRAMyby1vwiake8lIkNxR0WY0hHeRUGvp5FX6PzdgPnFJMXbKwa9M41WT481ewEb3Ln9KTyFUjt4FxIL2m+10FbYbj+v/go=\ntrusted comment: Lobby 0.2.0\nuSeizVz1NSf67XRF53J20w/OXRfUk054JP1t4vUDlIul9i1952+PhcEOdXwsZUfKlAxTdPS72PqJxFAR5Kz+BQ==\n";
    const SIGNATURE_FOR_0_1_5: &str = "untrusted comment: signature from minisign secret key\nRUTKwRAMyby1vwiake8lIkNxR0WY0hHeRUGvp5FX6PzdgPnFJMXbKwa9M41WT481ewEb3Ln9KTyFUjt4FxIL2m+10FbYbj+v/go=\ntrusted comment: Lobby 0.1.5\nRyAxo0Q/zrGKgVkdB40BDhMMt21BHiqkfxGE9iPgIlNmaFjfhU4xcPCpATVKs2C8RQ7XOgqNczfkJcPjfschBw==\n";

    #[test]
    fn only_a_signature_by_the_release_key_for_this_version_holds() {
        assert_eq!(signature_holds(SIGNED, SIGNATURE_FOR_0_2_0, TEST_KEY, "0.2.0"), Ok(()));
        // Changed file.
        assert!(signature_holds(b"something else\n", SIGNATURE_FOR_0_2_0, TEST_KEY, "0.2.0").unwrap_err().contains("doesn't match"));
        // Signed by someone else: the real release key didn't make this one.
        assert!(signature_holds(SIGNED, SIGNATURE_FOR_0_2_0, RELEASE_KEY, "0.2.0").unwrap_err().contains("doesn't match"));
        // An old release passed off as this version.
        assert!(signature_holds(SIGNED, SIGNATURE_FOR_0_1_5, TEST_KEY, "0.2.0").unwrap_err().contains("another version"));
        // Nonsense.
        assert!(signature_holds(SIGNED, "not a signature", TEST_KEY, "0.2.0").is_err());
    }

    #[test]
    fn the_real_release_key_can_be_read() {
        assert!(minisign_verify::PublicKey::from_base64(RELEASE_KEY).is_ok());
        assert_eq!(option_env!("LOBBY_TEST_UPDATE_KEY"), None, "a test key must never reach a real build");
    }

    #[test]
    fn a_release_says_where_its_signature_is() {
        let signed = r#"{"tag_name":"v0.2.0","assets":[{"name":"Lobby-x86_64.AppImage","browser_download_url":"https://x/a","digest":"sha256:abc"},{"name":"Lobby-x86_64.AppImage.minisig","browser_download_url":"https://x/s","digest":"sha256:def"}]}"#;
        assert_eq!(update_in(signed, "0.1.0").unwrap().signature.as_deref(), Some("https://x/s"));
    }

    #[test]
    fn an_unsigned_release_isnt_installed() {
        let unsigned = r#"{"tag_name":"v0.2.0","assets":[{"name":"Lobby-x86_64.AppImage","browser_download_url":"https://x/a","digest":"sha256:abc"}]}"#;
        let update = update_in(unsigned, "0.1.0").unwrap();
        assert_eq!(update.signature, None);
    }

    #[test]
    fn reads_a_release() {
        let json = r#"{"tag_name":"v0.2.0","html_url":"https://github.com/Petru101/Lobby/releases/tag/v0.2.0","assets":[{"name":"Lobby-x86_64.AppImage.zsync","browser_download_url":"https://x/z","digest":"sha256:00"},{"name":"Lobby-x86_64.AppImage","browser_download_url":"https://x/a","digest":"sha256:abc"}]}"#;
        let u = update_in(json, "0.1.0").unwrap();
        assert_eq!((u.version.as_str(), u.url.as_str(), u.sha256.as_str()), ("0.2.0", "https://x/a", "abc"));
        assert_eq!(update_in(json, "0.2.0"), None);
        assert!(newer("0.10.0", "0.9.9") && !newer("0.1.0", "0.1.0") && newer("1.0.0", "0.99.1"));
    }
}
