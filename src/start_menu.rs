//! Windows programs' own Start Menu shortcuts, in the app menu when the player picks them.
//! Proton writes the menu entries Wine would make into the prefix (`C:\proton_shortcuts`)
//! instead of the app menu, so a Windows program installed through it (Battle.net, a GOG
//! installer's game) never shows there. Lobby lists the shortcuts of every prefix it knows and
//! writes an entry for the ones picked, which starts the program through Lobby.

use crate::library::{self, Kind};
use crate::store::Library;
use std::fs;
use std::path::{Path, PathBuf};

/// A shortcut an installer made: what it's called, the program, the prefix it runs in, whose
/// prefix that is, and Lobby's app-menu entry for it when there is one.
#[derive(Debug, Clone, PartialEq)]
pub struct Shortcut {
    pub name: String,
    pub program: PathBuf,
    pub prefix: PathBuf,
    pub place: String,
    pub entry: Option<PathBuf>,
}

impl Shortcut {
    pub fn gone(&self) -> bool {
        !self.program.is_file()
    }
}

const PREFIX: &str = "lobby-shortcut-";

fn applications() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/share/applications")
}

fn icons() -> PathBuf {
    crate::store::dir().join("shortcut-icons")
}

/// Every prefix Lobby knows: its own (games', Battle.net's, the store apps'), the ones the
/// library's games run in, and where prefixes usually are.
fn prefixes(lib: &Library) -> Vec<PathBuf> {
    let own = crate::store::dir();
    let mut dirs: Vec<PathBuf> = ["prefixes", "clients"].iter().flat_map(|d| fs::read_dir(own.join(d)).into_iter().flatten().flatten().map(|e| e.path())).collect();
    dirs.push(crate::battlenet::prefix());
    dirs.extend(lib.games.iter().flat_map(|g| g.entries.iter().filter_map(|e| e.prefix.clone())));
    if let Some(home) = crate::sources::Home::find() {
        dirs.extend(crate::sources::usual_prefixes(&home));
    }
    let mut seen = Vec::new();
    dirs.into_iter().filter_map(|d| library::prefix_in(&d)).filter(|p| {
        let n = library::norm(p);
        !seen.contains(&n) && {
            seen.push(n);
            true
        }
    }).collect()
}

/// Whose prefix it is, as the player knows it: Battle.net, a store app, the one game in it,
/// else its folder's name.
fn place(pfx: &Path, lib: &Library) -> String {
    let n = library::norm(pfx);
    if library::prefix_in(&crate::battlenet::prefix()).is_some_and(|p| library::norm(&p) == n) {
        return "Battle.net".into();
    }
    if let Some(c) = crate::clients::CLIENTS.iter().find(|c| library::norm(&c.prefix()) == n) {
        return c.name.into();
    }
    // The store app or game installed into it (a launcher's or Lobby's record names the
    // prefix), else the only one found there: Empire Earth Gold's prefix also has NeoEE's
    // shortcuts, the EA app's Mass Effect's.
    let in_it = |others: bool| {
        let mut found: Vec<&library::Game> = lib.games.iter().filter(|g| matches!(g.kind, Kind::Game | Kind::StoreApp) && g.entries.iter().any(|e| (others || e.source != library::Source::Prefix) && e.prefix.as_deref().is_some_and(|p| library::norm(p) == n))).collect();
        if found.iter().any(|g| g.kind == Kind::StoreApp) {
            found.retain(|g| g.kind == Kind::StoreApp);
        }
        match found.as_slice() {
            [one] => Some(one.name.clone()),
            _ => None,
        }
    };
    if let Some(name) = in_it(false).or_else(|| in_it(true)) {
        return name;
    }
    let dir = if pfx.file_name().is_some_and(|f| f == "pfx") { pfx.parent().unwrap_or(pfx) } else { pfx };
    dir.file_name().map_or_else(|| dir.display().to_string(), |f| f.to_string_lossy().into_owned())
}

/// The shortcuts in every prefix Lobby knows, and the entries made for ones since gone, by name.
pub fn all(lib: &Library) -> Vec<Shortcut> {
    let added = added();
    let mut out = Vec::new();
    for pfx in prefixes(lib) {
        let place = place(&pfx, lib);
        let mut found = gamefix::disc::shortcuts(&pfx);
        // Several shortcuts to one program (Start Menu and desktop).
        found.sort_by(|a, b| a.0.cmp(&b.0));
        found.dedup_by(|a, b| a.0 == b.0);
        for (program, name) in found {
            let entry = added.iter().find(|s| same(&s.program, &program) && same(&s.prefix, &pfx)).and_then(|s| s.entry.clone());
            out.push(Shortcut { name, program, prefix: pfx.clone(), place: place.clone(), entry });
        }
    }
    let gone: Vec<Shortcut> = added.into_iter().filter(|a| !out.iter().any(|s| s.entry == a.entry)).collect();
    out.extend(gone);
    out.sort_by_key(|s| (s.name.to_lowercase(), s.place.to_lowercase()));
    out
}

fn same(a: &Path, b: &Path) -> bool {
    library::norm(a) == library::norm(b)
}

/// The entries Lobby made, read back from the app-menu folder.
fn added() -> Vec<Shortcut> {
    let mut out: Vec<Shortcut> = fs::read_dir(applications()).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| is_entry(p)).filter_map(|p| read(&p)).collect();
    out.sort_by(|a, b| a.entry.cmp(&b.entry));
    out
}

/// Whether an app-menu entry is one Lobby made for a shortcut.
pub fn is_entry(file: &Path) -> bool {
    file.file_name().is_some_and(|n| {
        let n = n.to_string_lossy();
        n.starts_with(PREFIX) && n.ends_with(".desktop")
    })
}

fn read(file: &Path) -> Option<Shortcut> {
    let text = fs::read_to_string(file).ok()?;
    let get = |k: &str| text.lines().find_map(|l| l.strip_prefix(k)?.strip_prefix('=')).map(str::to_string);
    Some(Shortcut {
        name: get("Name")?,
        program: get("X-Lobby-Program")?.into(),
        prefix: get("X-Lobby-Prefix")?.into(),
        place: get("X-Lobby-Place").unwrap_or_default(),
        entry: Some(file.to_path_buf()),
    })
}

/// Puts the shortcut in the app menu: an entry that runs `lobby shortcut <slug>`, with the
/// program's own icon. Returns the entry's file.
pub fn add(s: &Shortcut) -> Result<PathBuf, String> {
    let dir = applications();
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let base = gamefix::setup::slug(&s.name);
    let slug = (1..).map(|n| if n == 1 { base.clone() } else { format!("{base}-{n}") }).find(|slug| !dir.join(format!("{PREFIX}{slug}.desktop")).exists()).unwrap();
    let icon = gamefix::peicon::icon_png(&s.program).and_then(|png| {
        let file = icons().join(format!("{slug}.png"));
        fs::create_dir_all(icons()).ok()?;
        fs::write(&file, png).ok()?;
        Some(file.display().to_string())
    });
    let text = entry_text(s, &slug, &crate::sandbox::installed_lobby(), icon.as_deref());
    let file = dir.join(format!("{PREFIX}{slug}.desktop"));
    fs::write(&file, text).map_err(|e| format!("{}: {e}", file.display()))?;
    Ok(file)
}

fn entry_text(s: &Shortcut, slug: &str, lobby: &str, icon: Option<&str>) -> String {
    // Wine names a program's windows after its file, which the taskbar matches to the entry.
    let class = s.program.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
    format!(
        "[Desktop Entry]\nType=Application\nName={}\nComment=Windows program, in {}'s Wine prefix\nExec=\"{lobby}\" shortcut {slug}\nIcon={}\nCategories=Game;\nStartupWMClass={class}\nX-Lobby-Program={}\nX-Lobby-Prefix={}\nX-Lobby-Place={}\n",
        one_line(&s.name),
        one_line(&s.place),
        icon.unwrap_or("applications-games"),
        s.program.display(),
        s.prefix.display(),
        one_line(&s.place)
    )
}

fn one_line(s: &str) -> String {
    s.replace(['\n', '\r'], " ")
}

/// Takes the shortcut's entry (and its icon) out of the app menu.
pub fn remove(entry: &Path) -> Result<(), String> {
    if !is_entry(entry) {
        return Err(format!("{} isn't one of Lobby's shortcut entries.", entry.display()));
    }
    if let Some(stem) = entry.file_stem().and_then(|s| s.to_string_lossy().strip_prefix(PREFIX).map(str::to_string)) {
        let _ = fs::remove_file(icons().join(format!("{stem}.png")));
    }
    fs::remove_file(entry).map_err(|e| format!("{}: {e}", entry.display()))
}

/// The shortcut an entry's `lobby shortcut <slug>` names.
pub fn by_slug(slug: &str) -> Result<Shortcut, String> {
    let file = applications().join(format!("{PREFIX}{slug}.desktop"));
    read(&file).ok_or_else(|| format!("No shortcut \"{slug}\" in the app menu ({} isn't there).", file.display()))
}

/// The game in the library the program starts, if any: then it starts as Play starts it.
pub fn game_of<'a>(s: &Shortcut, lib: &'a Library) -> Option<&'a library::Game> {
    lib.games.iter().find(|g| g.kind == Kind::Game && !g.gone && g.entries.iter().any(|e| e.program.as_deref().is_some_and(|p| same(p, &s.program))))
}

/// The command starting the program in its prefix, as its prefix's app or game runs: Battle.net
/// and the store apps with their own settings, anything else with the Proton that made the
/// prefix.
pub fn command(s: &Shortcut) -> Vec<String> {
    let n = library::norm(&s.prefix);
    if library::prefix_in(&crate::battlenet::prefix()).is_some_and(|p| library::norm(&p) == n) {
        return crate::battlenet::app_command(&s.program);
    }
    if let Some(c) = crate::clients::CLIENTS.iter().find(|c| library::norm(&c.prefix()) == n) {
        return c.command(&s.program);
    }
    crate::launch::in_prefix(&s.program, &s.prefix)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entries_say_what_to_start() {
        let s = Shortcut { name: "Battle.net".into(), program: "/p/drive_c/Program Files (x86)/Battle.net/Battle.net Launcher.exe".into(), prefix: "/p".into(), place: "Battle.net".into(), entry: None };
        let text = entry_text(&s, "battle-net", "/home/u/.local/bin/lobby", Some("/i/battle-net.png"));
        assert!(text.contains("\nExec=\"/home/u/.local/bin/lobby\" shortcut battle-net\n"));
        assert!(text.contains("\nStartupWMClass=battle.net launcher.exe\n"));
        let dir = std::env::temp_dir().join(format!("lobby-start-menu-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("lobby-shortcut-battle-net.desktop");
        fs::write(&file, &text).unwrap();
        assert!(is_entry(&file));
        assert_eq!(read(&file), Some(Shortcut { entry: Some(file.clone()), ..s }));
        assert!(!is_entry(&dir.join("gamefix-battle-net.desktop")));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn names_stay_on_one_line() {
        let s = Shortcut { name: "Two\nlines".into(), program: "/p/a.exe".into(), prefix: "/p".into(), place: "x".into(), entry: None };
        assert!(entry_text(&s, "two-lines", "lobby", None).contains("\nName=Two lines\n"));
    }
}
