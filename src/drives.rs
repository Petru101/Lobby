//! Drives a game is on that keep it from working: a Windows-format drive (NTFS, exFAT, FAT)
//! holding a Wine prefix or a Steam library's Proton prefixes (Wine needs links, a "c:" link
//! name and Unix permissions, which those drives don't hold), and a Windows drive mounted
//! read-only (Windows hibernated or shut down with Fast Startup). Games themselves run fine
//! from Windows drives, so that alone isn't warned about.

use crate::library::{Game, Key, Kind, Source};
use std::path::{Path, PathBuf};

/// One line of /proc/self/mountinfo.
#[derive(Debug, Clone)]
pub struct Mount {
    pub point: PathBuf,
    pub fs: String,
    pub device: String,
    pub read_only: bool,
}

impl Mount {
    pub fn windows_format(&self) -> bool {
        ["fuseblk", "ntfs", "ntfs3", "exfat", "vfat", "msdos"].contains(&self.fs.as_str())
    }

    /// "the Storage drive" for a drive under /run/media or /media, "the drive your Downloads
    /// folder is on" for one mounted in the home folder, else where it's mounted.
    fn name(&self) -> String {
        let removable = ["/run/media", "/media", "/mnt"].iter().any(|r| self.point.starts_with(r));
        let home = std::env::var_os("HOME").and_then(|h| std::fs::canonicalize(h).ok());
        let in_home = home.is_some_and(|h| self.point.starts_with(&h));
        match self.point.file_name() {
            Some(n) if removable => format!("the {} drive", n.to_string_lossy()),
            Some(n) if in_home => format!("the drive your {} folder is on", n.to_string_lossy()),
            _ => format!("the drive at {}", self.point.display()),
        }
    }

    /// NTFS, exFAT or FAT. ntfs-3g and exfat-fuse both show as "fuseblk": the drive's own
    /// format then comes from lsblk.
    fn format(&self) -> String {
        let named = |fs: &str| match fs {
            "ntfs" | "ntfs3" => "NTFS".to_string(),
            "exfat" => "exFAT".into(),
            "vfat" | "msdos" => "FAT32".into(),
            other => other.to_string(),
        };
        if self.fs != "fuseblk" {
            return named(&self.fs);
        }
        let out = std::process::Command::new("lsblk").args(["-dno", "FSTYPE"]).arg(&self.device).stdin(std::process::Stdio::null()).output();
        match out.map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()) {
            Ok(fs) if !fs.is_empty() => named(&fs),
            _ => "NTFS or exFAT".into(),
        }
    }
}

/// mountinfo writes spaces and a few other characters as \ and three octal digits.
fn unescape(field: &str) -> String {
    let bytes = field.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let octal = bytes.get(i + 1..i + 4).and_then(|d| u8::from_str_radix(std::str::from_utf8(d).ok()?, 8).ok());
        match (bytes[i], octal) {
            (b'\\', Some(b)) => {
                out.push(b);
                i += 4;
            }
            (b, _) => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub fn parse(mountinfo: &str) -> Vec<Mount> {
    mountinfo
        .lines()
        .filter_map(|line| {
            let (mine, theirs) = line.split_once(" - ")?;
            let mine: Vec<&str> = mine.split(' ').collect();
            let mut theirs = theirs.split(' ');
            let (fs, device) = (theirs.next()?, theirs.next()?);
            Some(Mount { point: PathBuf::from(unescape(mine.get(4)?)), fs: fs.into(), device: unescape(device), read_only: mine.get(5)?.split(',').any(|o| o == "ro") })
        })
        .collect()
}

pub fn mounts() -> Vec<Mount> {
    std::fs::read_to_string("/proc/self/mountinfo").map(|t| parse(&t)).unwrap_or_default()
}

/// The mount a path is on: the one mounted at the path or the nearest folder above it, the
/// last mounted when several are (links resolved, so /home is /var/home).
pub fn mount_of<'a>(path: &Path, mounts: &'a [Mount]) -> Option<&'a Mount> {
    let real = path.ancestors().find_map(|p| std::fs::canonicalize(p).ok())?;
    mounts.iter().filter(|m| real.starts_with(&m.point)).max_by_key(|m| m.point.components().count())
}

/// Why the drives a game is on may keep it from working, one paragraph each.
pub fn warnings(game: &Game, mounts: &[Mount]) -> Vec<String> {
    if game.kind != Kind::Game {
        return Vec::new();
    }
    let mut said: Vec<String> = Vec::new();
    let mut warn = |text: String| {
        if !said.contains(&text) {
            said.push(text);
        }
    };
    for e in &game.entries {
        if let Some(m) = e.prefix.as_deref().and_then(|p| mount_of(p, mounts)).filter(|m| m.windows_format()) {
            warn(format!("Its Windows setup (Wine prefix) is on {}, which is formatted for Windows ({}). Wine needs things that format can't hold (links, file permissions), so the game may not start or may lose its settings. Install it again on a drive formatted for Linux (btrfs or ext4).", m.name(), m.format()));
        }
        // A Steam game keeps its Proton prefix in its library's steamapps/compatdata.
        let app = e.keys.iter().find_map(|k| if let Key::SteamApp(id) = k { Some(*id) } else { None });
        if let (Source::Steam, Some(app), Some(folder)) = (e.source, app, e.folder.as_deref()) {
            let steamapps = folder.parent().and_then(Path::parent);
            let proton = steamapps.is_some_and(|s| s.join("compatdata").join(app.to_string()).is_dir()) || has_exe(folder);
            if let Some(m) = mount_of(folder, mounts).filter(|m| proton && m.windows_format()) {
                warn(format!("It's in a Steam library on {}, which is formatted for Windows ({}). Steam keeps a Windows game's setup (its Proton prefix) in the same library, and that format can't hold one, so the game may not start. Move it to a library on a drive formatted for Linux: in Steam, right-click it → Properties → Installed Files → Move install folder.", m.name(), m.format()));
            }
        }
        for path in [e.folder.as_deref(), e.program.as_deref(), e.prefix.as_deref()].into_iter().flatten() {
            if let Some(m) = mount_of(path, mounts).filter(|m| m.read_only && m.windows_format()) {
                warn(format!("{} can only be read right now, so the game can't save or update. That happens when Windows didn't fully shut down (Fast Startup or hibernation). Start Windows, turn off Fast Startup (Control Panel → Power Options → Choose what the power buttons do), shut it down, then restart this computer.", capitalized(&m.name())));
            }
        }
    }
    said
}

fn capitalized(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
}

/// A Windows program in the folder or one folder down.
fn has_exe(folder: &Path) -> bool {
    let is_exe = |p: &Path| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("exe"));
    std::fs::read_dir(folder).into_iter().flatten().flatten().any(|e| {
        let p = e.path();
        is_exe(&p) || p.is_dir() && std::fs::read_dir(&p).into_iter().flatten().flatten().any(|e| is_exe(&e.path()))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const MOUNTINFO: &str = "49 1 0:40 / / ro,relatime shared:1 - overlay composefs ro,seclabel
67 60 0:57 / /run/media/Petru/Games rw,noatime shared:199 - btrfs /dev/nvme0n1p1 rw,seclabel
264 60 8:2 / /run/media/Petru/My\\040Storage ro,noatime shared:223 - fuseblk /dev/sda2 ro,user_id=0
276 121 8:2 /Downloads /var/home/Petru/Downloads rw,noatime shared:223 - fuseblk /dev/sda2 rw,user_id=0";

    #[test]
    fn reads_mountinfo() {
        let m = parse(MOUNTINFO);
        assert_eq!(m.len(), 4);
        assert!(m[0].read_only && !m[0].windows_format());
        assert_eq!(m[2].point, PathBuf::from("/run/media/Petru/My Storage"));
        assert!(m[2].read_only && m[2].windows_format());
        assert_eq!(m[2].name(), "the My Storage drive");
                assert!(!m[3].read_only);
    }

    #[test]
    fn unescapes() {
        assert_eq!(unescape("a\\040b\\134c"), "a b\\c");
        assert_eq!(unescape("plain\\"), "plain\\");
    }
}
