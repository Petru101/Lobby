//! The store tools lobby runs, each its official release at a pinned version, downloaded on
//! first use into ~/.local/share/lobby/tools and checked against the hash pinned here. Lobby's
//! own copies: nothing another launcher installed is used.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};

pub struct Tool {
    pub name: &'static str,
    version: &'static str,
    url: &'static str,
    sha256: &'static str,
}

/// Epic Games: github.com/derrod/legendary.
pub const LEGENDARY: Tool = Tool {
    name: "legendary",
    version: "0.21.1",
    url: "https://github.com/derrod/legendary/releases/download/0.21.1/legendary_linux_x64",
    sha256: "dbed33bbe96031e65858233e4badf0a1d401bb6cb0cc995d1237cf3a0c826a45",
};

/// GOG: github.com/Heroic-Games-Launcher/heroic-gogdl (a standalone tool, used without Heroic).
pub const GOGDL: Tool = Tool {
    name: "gogdl",
    version: "1.3.1",
    url: "https://github.com/Heroic-Games-Launcher/heroic-gogdl/releases/download/v1.3.1/gogdl_linux_x86_64",
    sha256: "7a6f22da4943bdb4c00baf6250fcee7085c1104ba2ac401479e70ec96e722b59",
};

/// Programs of Lobby's own that games and gamefix find by name (umu-run, steam), at the end of
/// Lobby's PATH so that the system's own come first.
pub fn bin() -> PathBuf {
    crate::store::dir().join("tools/bin")
}

/// Fills `bin` with what this system lacks and adds it to Lobby's PATH (at the start, before
/// any thread): umu-run from umu's release, and `steam` starting Flatpak Steam when that's the
/// only Steam.
pub fn provide() {
    let bin = bin();
    let lacks = |name: &str| crate::launch::program_on_path(name).is_none_or(|p| p.starts_with(&bin));
    if lacks("umu-run") {
        if let Err(e) = own_umu(&bin) {
            crate::log::line(&format!("umu-run: {e}"));
        }
    }
    let shim = bin.join("steam");
    let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    let flatpak_steam = [home.join(".local/share/flatpak/app/com.valvesoftware.Steam/current"), PathBuf::from("/var/lib/flatpak/app/com.valvesoftware.Steam/current")].iter().any(|p| p.exists());
    if lacks("steam") && flatpak_steam {
        if !shim.is_file() && fs::create_dir_all(&bin).is_ok() && fs::write(&shim, "#!/bin/sh\nexec flatpak run com.valvesoftware.Steam \"$@\"\n").is_ok() {
            let _ = fs::set_permissions(&shim, fs::Permissions::from_mode(0o755));
        }
    } else if shim.is_file() {
        let _ = fs::remove_file(&shim);
    }
    let mut path: Vec<PathBuf> = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
    if !path.contains(&bin) {
        path.push(bin);
        if let Ok(joined) = std::env::join_paths(path) {
            std::env::set_var("PATH", joined);
        }
    }
}

/// umu-run as `bin`/umu-run: a link to gamefix's copy, which gamefix downloads when there's
/// none (umu's pinned release, checked by hash, in ~/.local/share/gamefix/tools: one copy for
/// both). A copy Lobby downloaded itself before (tools/umu-run-<version>) stays in use.
fn own_umu(bin: &std::path::Path) -> Result<(), String> {
    let link = bin.join("umu-run");
    if link.is_file() {
        return Ok(());
    }
    let program = gamefix::proton::umu_run(&mut Vec::new())?;
    fs::create_dir_all(bin).map_err(|e| e.to_string())?;
    let _ = fs::remove_file(&link);
    std::os::unix::fs::symlink(&program, &link).map_err(|e| e.to_string())
}

impl Tool {
    /// Where the tool is kept (downloaded by `path`).
    pub fn file(&self) -> PathBuf {
        crate::store::dir().join("tools").join(format!("{}-{}", self.name, self.version))
    }

    /// The tool's program, downloaded first if it isn't there yet.
    pub fn path(&self) -> Result<PathBuf, String> {
        let file = self.file();
        if file.is_file() {
            return Ok(file);
        }
        fs::create_dir_all(file.parent().unwrap()).map_err(|e| e.to_string())?;
        let part = file.with_extension("part");
        let failed = |why: String| format!("Couldn't download {} (the store tool lobby uses): {why}", self.name);
        let out = Command::new("curl").args(["-sSLf", "--max-time", "300", "-o"]).arg(&part).arg(self.url).stdin(Stdio::null()).output().map_err(|e| failed(e.to_string()))?;
        if !out.status.success() {
            let _ = fs::remove_file(&part);
            return Err(failed(String::from_utf8_lossy(&out.stderr).trim().to_string()));
        }
        let data = fs::read(&part).map_err(|e| failed(e.to_string()))?;
        if crate::sha256_hex(&data) != self.sha256 {
            let _ = fs::remove_file(&part);
            return Err(failed("the download isn't the release lobby expects (its checksum differs)".into()));
        }
        fs::set_permissions(&part, fs::Permissions::from_mode(0o755)).map_err(|e| failed(e.to_string()))?;
        fs::rename(&part, &file).map_err(|e| failed(e.to_string()))?;
        Ok(file)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Downloads both tools (network): `cargo test -- --ignored`, with XDG_DATA_HOME set to a
    /// scratch folder.
    #[test]
    #[ignore]
    fn downloads_and_checks() {
        for tool in [LEGENDARY, GOGDL] {
            let path = tool.path().unwrap();
            assert!(Command::new(&path).arg("--version").output().unwrap().status.success());
        }
    }
}
