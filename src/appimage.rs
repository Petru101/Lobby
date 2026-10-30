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

/// Lobby in the app menu, started from the AppImage file: written when there's no entry, or
/// one starting a program that's gone or another AppImage (an older download). An entry for
/// another Lobby install (~/.local/bin/lobby) is left alone.
pub fn menu_entry() {
    let Some(file) = std::env::var_os("APPIMAGE").map(PathBuf::from).filter(|p| p.is_file()) else { return };
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else { return };
    let entry = home.join(".local/share/applications/io.github.Petru101.Lobby.desktop");
    let exec = fs::read_to_string(&entry).ok().and_then(|t| t.lines().find_map(|l| l.strip_prefix("Exec=").map(|e| e.trim_matches('"').to_string())));
    let keep = exec.as_deref().is_some_and(|e| e == file.to_string_lossy() || (PathBuf::from(e).is_file() && !e.to_lowercase().ends_with(".appimage")));
    if keep {
        return;
    }
    let text = format!(
        "[Desktop Entry]\nType=Application\nName=Lobby\nGenericName=Game library\nComment=Every game on this computer in one place\nExec=\"{}\"\nIcon=applications-games\nCategories=Game;\nKeywords=games;launcher;steam;heroic;lutris;\nStartupWMClass=io.github.Petru101.Lobby\n",
        file.display()
    );
    let _ = fs::create_dir_all(entry.parent().unwrap());
    let _ = fs::write(&entry, text);
}

/// Where the latest release is described (LOBBY_UPDATE_URL instead, to try updating against a
/// release of one's own).
const LATEST: &str = "https://api.github.com/repos/Petru101/Lobby/releases/latest";
const ASSET: &str = "Lobby-x86_64.AppImage";

/// A newer Lobby on GitHub.
#[derive(Debug, Clone, PartialEq)]
pub struct Update {
    pub version: String,
    url: String,
    sha256: String,
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
    Some(Update {
        version,
        url: asset.get("browser_download_url")?.as_str()?.to_string(),
        sha256: asset.get("digest")?.as_str()?.strip_prefix("sha256:")?.to_string(),
        page: release.get("html_url").and_then(|u| u.as_str()).unwrap_or("https://github.com/Petru101/Lobby/releases").to_string(),
    })
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

/// Downloads the update beside this AppImage, checks its fingerprint, and puts it in this one's
/// place. From the next start (or a restart) it's the new version.
pub fn install(update: &Update) -> Result<(), String> {
    let file = file().ok_or("Lobby isn't running as an AppImage")?;
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
    fn reads_a_release() {
        let json = r#"{"tag_name":"v0.2.0","html_url":"https://github.com/Petru101/Lobby/releases/tag/v0.2.0","assets":[{"name":"Lobby-x86_64.AppImage.zsync","browser_download_url":"https://x/z","digest":"sha256:00"},{"name":"Lobby-x86_64.AppImage","browser_download_url":"https://x/a","digest":"sha256:abc"}]}"#;
        let u = update_in(json, "0.1.0").unwrap();
        assert_eq!((u.version.as_str(), u.url.as_str(), u.sha256.as_str()), ("0.2.0", "https://x/a", "abc"));
        assert_eq!(update_in(json, "0.2.0"), None);
        assert!(newer("0.10.0", "0.9.9") && !newer("0.1.0", "0.1.0") && newer("1.0.0", "0.99.1"));
    }
}
