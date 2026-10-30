//! The programs Lobby and gamefix run that a system may lack (Bazzite has them all), what each
//! is for, and the command that installs them on this system: `lobby check`, and the window's
//! banner for the ones most games need.

use std::path::Path;

pub struct Program {
    /// What to call it.
    pub name: &'static str,
    /// The commands any of which will do.
    commands: &'static [&'static str],
    pub what: &'static str,
    /// Most games need it (else only some do, or one feature).
    pub essential: bool,
    /// Its package: apt, dnf, pacman, zypper.
    packages: [&'static str; 4],
}

const PROGRAMS: &[Program] = &[
    Program { name: "curl", commands: &["curl"], what: "downloads covers, store games, Proton and umu", essential: true, packages: ["curl"; 4] },
    Program { name: "Python 3", commands: &["python3"], what: "runs umu, which starts Windows games (this system has no umu-launcher of its own)", essential: true, packages: ["python3", "python3", "python", "python3"] },
    Program { name: "7-Zip", commands: &["7z"], what: "unpacks downloads, installers and disc images", essential: true, packages: ["7zip"; 4] },
    Program { name: "bubblewrap", commands: &["bwrap"], what: "protects your files from the games (their sandbox)", essential: true, packages: ["bubblewrap"; 4] },
    Program { name: "xdg-dbus-proxy", commands: &["xdg-dbus-proxy"], what: "lets store sign-in pages open (their sandbox needs it), and protected games show notifications and keep the screen on", essential: true, packages: ["xdg-dbus-proxy"; 4] },
    Program { name: "unrar", commands: &["unrar"], what: "unpacks .rar downloads", essential: false, packages: ["unrar"; 4] },
    Program { name: "Flatpak", commands: &["flatpak"], what: "installs the programs some games run in (DOSBox, Ruffle, LÖVE ...) from Flathub", essential: false, packages: ["flatpak"; 4] },
    Program { name: "MangoHud", commands: &["mangohud"], what: "shows a game's frame rate (Show frame rate)", essential: false, packages: ["mangohud"; 4] },
];

/// The program by its name, for saying how to install it.
pub fn named(name: &str) -> Option<&'static Program> {
    PROGRAMS.iter().find(|p| p.name == name)
}

/// The programs this system lacks.
pub fn missing() -> Vec<&'static Program> {
    PROGRAMS.iter().filter(|p| p.commands.iter().all(|c| crate::launch::program_on_path(c).is_none()) && (p.name != "Python 3" || own_umu())).collect()
}

/// Whether Lobby runs its own umu (a Python program): the system has none.
fn own_umu() -> bool {
    crate::launch::program_on_path("umu-run").is_none_or(|p| p.starts_with(crate::stores::tools::bin()))
}

/// How this system installs packages, from /etc/os-release (and ostree's mark).
#[derive(Clone, Copy, PartialEq, Debug)]
enum Manager {
    Apt,
    Dnf,
    Pacman,
    Zypper,
    RpmOstree,
    Unknown,
}

fn manager(os_release: &str, ostree: bool) -> Manager {
    let field = |k: &str| os_release.lines().find_map(|l| l.strip_prefix(k)?.strip_prefix('=')).unwrap_or_default().trim_matches('"').to_lowercase();
    let ids = format!("{} {}", field("ID"), field("ID_LIKE"));
    let is = |id: &str| ids.split_whitespace().any(|w| w == id);
    match () {
        _ if ostree => Manager::RpmOstree,
        _ if is("debian") || is("ubuntu") => Manager::Apt,
        _ if is("fedora") || is("rhel") => Manager::Dnf,
        _ if is("arch") => Manager::Pacman,
        _ if ids.contains("suse") => Manager::Zypper,
        _ => Manager::Unknown,
    }
}

fn this_manager() -> Manager {
    manager(&std::fs::read_to_string("/etc/os-release").unwrap_or_default(), Path::new("/run/ostree-booted").exists())
}

/// The command installing these programs here.
pub fn install_command(programs: &[&Program]) -> String {
    install_command_for(this_manager(), programs)
}

fn install_command_for(manager: Manager, programs: &[&Program]) -> String {
    let i = match manager {
        Manager::Apt => 0,
        Manager::Dnf | Manager::RpmOstree | Manager::Unknown => 1,
        Manager::Pacman => 2,
        Manager::Zypper => 3,
    };
    let mut packages: Vec<&str> = programs.iter().map(|p| p.packages[i]).collect();
    packages.dedup();
    let packages = packages.join(" ");
    match manager {
        Manager::Apt => format!("sudo apt install {packages}"),
        Manager::Dnf => format!("sudo dnf install {packages}"),
        Manager::Pacman => format!("sudo pacman -S {packages}"),
        Manager::Zypper => format!("sudo zypper install {packages}"),
        Manager::RpmOstree => format!("rpm-ostree install {packages}, then restart the computer"),
        Manager::Unknown => format!("your system's packages: {packages}"),
    }
}

/// Steam, which Lobby needs (its games, its Protons): how to get it here, or None when it's
/// there (or Flatpak Steam is).
pub fn steam_missing() -> Option<String> {
    if crate::launch::program_on_path("steam").is_some() {
        return None;
    }
    let package = match this_manager() {
        Manager::Apt => "sudo apt install steam-installer",
        Manager::Dnf => "sudo dnf install steam (from RPM Fusion's nonfree repository)",
        Manager::Pacman => "sudo pacman -S steam (with the multilib repository on)",
        Manager::Zypper => "sudo zypper install steam",
        Manager::RpmOstree | Manager::Unknown => "your system's steam package",
    };
    Some(format!("{package}, or Steam from Flathub: flatpak install flathub com.valvesoftware.Steam"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn knows_the_package_managers() {
        assert_eq!(manager("NAME=\"Ubuntu\"\nID=ubuntu\nID_LIKE=debian\n", false), Manager::Apt);
        assert_eq!(manager("ID=linuxmint\nID_LIKE=\"ubuntu debian\"\n", false), Manager::Apt);
        assert_eq!(manager("ID=fedora\n", false), Manager::Dnf);
        assert_eq!(manager("ID=bazzite\nID_LIKE=\"fedora\"\n", true), Manager::RpmOstree);
        assert_eq!(manager("ID=cachyos\nID_LIKE=arch\n", false), Manager::Pacman);
        assert_eq!(manager("ID=\"opensuse-tumbleweed\"\nID_LIKE=\"opensuse suse\"\n", false), Manager::Zypper);
        assert_eq!(manager("ID=nixos\n", false), Manager::Unknown);
        let seven = PROGRAMS.iter().find(|p| p.name == "7-Zip").unwrap();
        let python = PROGRAMS.iter().find(|p| p.name == "Python 3").unwrap();
        assert_eq!(install_command_for(Manager::Pacman, &[seven, python]), "sudo pacman -S 7zip python");
        assert_eq!(install_command_for(Manager::Apt, &[seven, python]), "sudo apt install 7zip python3");
    }
}
