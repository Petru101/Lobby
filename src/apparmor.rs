//! Ubuntu (23.10 and later, and what's built on it: Mint, Pop!_OS, Zorin) lets a program make
//! user namespaces, which bubblewrap (protection) and umu's pressure-vessel (every Windows game)
//! need, only when an AppArmor profile names it; without one both fail with "setting up uid
//! map: Permission denied". Steam's package brings a profile for Steam; Lobby writes one for
//! itself when the user allows it (their password, once): `/etc/apparmor.d/lobby`, attached to
//! Lobby's program, so everything Lobby starts (`lobby sandbox`, which every Play goes
//! through, is Lobby started again) may make them.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const PROFILE: &str = "/etc/apparmor.d/lobby";

/// Whether this system keeps programs without a profile from making user namespaces.
pub fn restricted() -> bool {
    fs::read_to_string("/proc/sys/kernel/apparmor_restrict_unprivileged_userns").is_ok_and(|v| v.trim() == "1")
}

/// The AppArmor profile this process runs under ("unconfined", or "lobby (unconfined)").
fn label() -> String {
    ["/proc/self/attr/apparmor/current", "/proc/self/attr/current"].iter().find_map(|f| fs::read_to_string(f).ok()).unwrap_or_default().trim().to_string()
}

/// The program the profile is for: this one, as installed; an AppImage's file (what it starts,
/// Lobby inside it among them, stays in the profile).
fn program() -> PathBuf {
    fs::canonicalize(crate::me()).unwrap_or_else(|_| PathBuf::from("/usr/bin/lobby"))
}

fn profile_text(program: &Path) -> String {
    format!(
        "# Written by Lobby: lets Lobby, and the games it starts, make the user namespaces that\n\
         # bubblewrap and Proton (umu's pressure-vessel) need. Remove this file to take it back.\n\
         abi <abi/4.0>,\n\
         include <tunables/global>\n\n\
         profile lobby \"{}\" flags=(unconfined) {{\n  userns,\n\n  include if exists <local/lobby>\n}}\n",
        program.display()
    )
}

/// Whether games Lobby starts from now on would be stopped: the system restricts user
/// namespaces and no profile of Lobby's names this program.
pub fn blocked() -> bool {
    restricted() && !label().starts_with("lobby") && !fs::read_to_string(PROFILE).is_ok_and(|t| t == profile_text(&program()))
}

/// Whether this very process may not make them (a game started now would fail).
pub fn blocked_here() -> bool {
    restricted() && !label().starts_with("lobby")
}

pub const WHY: &str = "Ubuntu only lets programs it knows make the kind of sandbox games run in: Proton needs one for every Windows game, and Lobby's protection for every game.";

/// Writes and loads Lobby's profile, as root: through pkexec (a password dialog) from the
/// window, sudo from a terminal.
pub fn allow(terminal: bool) -> Result<(), String> {
    let script = "printf '%s' \"$1\" > /etc/apparmor.d/lobby && apparmor_parser -r /etc/apparmor.d/lobby";
    let text = profile_text(&program());
    let elevate = if terminal { "sudo" } else { "pkexec" };
    if crate::launch::program_on_path(elevate).is_none() {
        return Err(format!("{elevate} isn't installed. As root, write this into {PROFILE} and run apparmor_parser -r {PROFILE}:\n\n{text}"));
    }
    let status = Command::new(elevate).args(["/bin/sh", "-c", script, "sh", &text]).status().map_err(|e| format!("{elevate}: {e}"))?;
    if !status.success() {
        return Err("The profile wasn't written (the password was refused, or the dialog closed).".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_names_the_program() {
        let text = profile_text(Path::new("/home/p/.local/bin/lobby"));
        assert!(text.contains("profile lobby \"/home/p/.local/bin/lobby\" flags=(unconfined) {\n  userns,\n"));
    }
}
