//! Running as an AppImage (build/appimage.sh): Lobby's insides are mounted somewhere new each
//! start ($APPDIR), and the AppImage file itself ($APPIMAGE) is what others must start
//! (`crate::me`).

use std::fs;
use std::path::PathBuf;

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
