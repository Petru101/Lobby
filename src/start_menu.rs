//! Windows programs' own Start Menu shortcuts, in the app menu when the player picks them.
//! Proton writes the menu entries Wine would make into the prefix (`C:\proton_shortcuts`)
//! instead of the app menu, so a Windows program installed through it (Battle.net, a GOG
//! installer's game) never shows there. Lobby lists the shortcuts of every prefix it knows and
//! writes an entry for the ones picked, which starts the program through Lobby. Games that
//! already have an entry in the home's app menu (gamefix's, made when Lobby set them up) are
//! listed with it, once: switching one off hides that entry (Play still starts the game
//! through it), on shows it again.

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
        "[Desktop Entry]\nType=Application\nName={}\nComment=Windows program in {}'s Wine prefix, started through Lobby\nExec=\"{lobby}\" shortcut {slug}\nIcon={}\nCategories=Game;\nStartupWMClass={class}\nX-Lobby-Program={}\nX-Lobby-Prefix={}\nX-Lobby-Place={}\n",
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

/// A row of the picker: a game with its own app-menu entries in the home and Windows shortcuts,
/// or a Windows program that's no game in the library (`game` None, one shortcut).
#[derive(Debug, Clone)]
pub struct Item {
    pub name: String,
    /// The program's file, or for a game none.
    pub detail: String,
    /// "Games", whose prefix the program is in, or that it's gone.
    pub group: String,
    /// A picture file: an entry's icon, else the program (its .exe icon).
    pub icon: Option<PathBuf>,
    pub game: Option<String>,
    /// The game's entries not made here (gamefix's ...), shown or hidden.
    pub own: Vec<PathBuf>,
    /// The entry made here for its shortcut.
    pub made: Option<PathBuf>,
    pub shortcut: Option<Shortcut>,
}

pub const GAMES: &str = "Games";
pub const GONE: &str = "No longer on this computer";

impl Item {
    pub fn shown(&self) -> bool {
        self.made.is_some() || self.own.iter().any(|e| !hidden(e))
    }

    /// Into the app menu: its own entry shown again, else one made for its shortcut.
    pub fn show(&mut self) -> Result<(), String> {
        if let Some(entry) = self.own.first() {
            set_hidden(entry, false)?;
        } else if self.made.is_none() {
            let s = self.shortcut.as_ref().ok_or("Nothing to put in the app menu.")?;
            self.made = Some(add(s)?);
        }
        Ok(())
    }

    /// Out of the app menu: the entry made here goes, its own entries are hidden (kept: Play
    /// starts gamefix's games through theirs).
    pub fn hide(&mut self) -> Result<(), String> {
        if let Some(made) = self.made.take() {
            remove(&made)?;
        }
        for entry in self.own.iter().filter(|e| !hidden(e)) {
            set_hidden(entry, true)?;
        }
        Ok(())
    }
}

/// What the picker lists: every game with an entry of its own in the home's app menu or a
/// Windows shortcut (one row each, whatever made them), then the Windows programs that are
/// no game, by prefix.
pub fn items(lib: &Library) -> Vec<Item> {
    let apps = library::norm(&applications());
    let shortcuts = all(lib);
    let mut used = vec![false; shortcuts.len()];
    // Each game's programs, their paths resolved once (game_of for each pair took a minute).
    let mut programs: std::collections::HashMap<PathBuf, &str> = std::collections::HashMap::new();
    for g in lib.games.iter().filter(|g| g.kind == Kind::Game && !g.gone) {
        for p in g.entries.iter().filter_map(|e| e.program.as_deref()) {
            programs.entry(library::norm(p)).or_insert(&g.id);
        }
    }
    let owner: Vec<Option<&str>> = shortcuts.iter().map(|s| programs.get(&library::norm(&s.program)).copied()).collect();
    let mut out = Vec::new();
    for g in lib.games.iter().filter(|g| g.kind == Kind::Game && !g.gone) {
        let mut own: Vec<PathBuf> = g.entries.iter().flat_map(|e| &e.keys).filter_map(|k| if let library::Key::Desktop(p) = k { Some(p.clone()) } else { None }).filter(|p| own_entry(p, &apps)).collect();
        own.sort();
        own.dedup();
        let mine: Vec<usize> = (0..shortcuts.len()).filter(|&i| !used[i] && owner[i] == Some(g.id.as_str())).collect();
        if own.is_empty() && mine.is_empty() {
            continue;
        }
        mine.iter().for_each(|&i| used[i] = true);
        let made = mine.iter().find_map(|&i| shortcuts[i].entry.clone());
        // An entry's own picture (gamefix's .exe icon, a launcher's), else the program's.
        let icon = own.iter().find_map(|e| entry_icon(e)).or_else(|| mine.first().map(|&i| shortcuts[i].program.clone())).or_else(|| g.entries.iter().find_map(|e| e.program.clone()));
        let shortcut = mine.first().map(|&i| Shortcut { name: g.name.clone(), ..shortcuts[i].clone() });
        out.push(Item { name: g.name.clone(), detail: String::new(), group: GAMES.into(), icon, game: Some(g.id.clone()), own, made, shortcut });
    }
    for (s, _) in shortcuts.into_iter().zip(used).filter(|(_, used)| !used) {
        let group = if s.gone() { GONE.into() } else { s.place.clone() };
        let detail = s.program.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        out.push(Item { name: s.name.clone(), detail, group, icon: Some(s.program.clone()), game: None, own: Vec::new(), made: s.entry.clone(), shortcut: Some(s) });
    }
    out.sort_by_key(|i| (i.group != GAMES, i.group == GONE, i.group.to_lowercase(), i.name.to_lowercase()));
    out
}

/// A game's entry this picker may show and hide: a file of the home's app-menu folder, not a
/// link (a Flatpak's) and not one made here.
fn own_entry(p: &Path, apps: &Path) -> bool {
    library::norm(p).starts_with(apps) && p.is_file() && !is_entry(p) && !fs::symlink_metadata(p).is_ok_and(|m| m.file_type().is_symlink()) && !p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("io.github.Petru101.Lobby"))
}

fn main_group(text: &str) -> impl Iterator<Item = &str> {
    text.lines().skip_while(|l| l.trim() != "[Desktop Entry]").skip(1).take_while(|l| !l.starts_with('['))
}

fn entry_icon(entry: &Path) -> Option<PathBuf> {
    let text = fs::read_to_string(entry).ok()?;
    let icon = main_group(&text).find_map(|l| l.strip_prefix("Icon=")).map(PathBuf::from);
    icon.filter(|p| p.is_absolute() && p.is_file())
}

/// Whether an entry is kept out of the app menu (NoDisplay).
pub fn hidden(entry: &Path) -> bool {
    fs::read_to_string(entry).is_ok_and(|t| main_group(&t).any(|l| l.trim() == "NoDisplay=true"))
}

/// Marks Lobby's hiding of an entry (NoDisplay), so the app-menu source still reads it: the
/// game still starts through it.
pub const HIDDEN_BY_LOBBY: &str = "X-Lobby-Hidden=true";

fn set_hidden(entry: &Path, hide: bool) -> Result<(), String> {
    let text = fs::read_to_string(entry).map_err(|e| format!("{}: {e}", entry.display()))?;
    let rewritten = hidden_text(&text, hide);
    if rewritten != text {
        fs::write(entry, rewritten).map_err(|e| format!("{}: {e}", entry.display()))?;
    }
    Ok(())
}

/// An entry's text with NoDisplay (and Lobby's mark) set or taken out, in its main group only.
fn hidden_text(text: &str, hide: bool) -> String {
    let mut main = false;
    let mut out = String::new();
    for line in text.lines() {
        if line.starts_with('[') {
            main = line.trim() == "[Desktop Entry]";
        }
        if main && (line.trim().starts_with("NoDisplay=") || line.trim() == HIDDEN_BY_LOBBY) {
            continue;
        }
        out.push_str(line);
        out.push('\n');
        if hide && line.trim() == "[Desktop Entry]" {
            out.push_str("NoDisplay=true\n");
            out.push_str(HIDDEN_BY_LOBBY);
            out.push('\n');
        }
    }
    out
}

/// The picture an entry had before Lobby gave it one from the game's cover.
const ORIGINAL_ICON: &str = "X-Lobby-Original-Icon";
/// The cover that picture was made from: made again when the game's cover changes.
const ICON_FROM: &str = "X-Lobby-Icon-From";
/// Icon names that say nothing about the game (gamefix's when the program had none).
const PLAIN_ICONS: &[&str] = &["", "applications-games", "application-x-executable", "wine"];

fn menu_icons() -> PathBuf {
    crate::store::dir().join("menu-icons")
}

/// Gives the games' own app-menu entries a picture made from the game's cover where theirs says
/// nothing about the game: none, or the same picture as another game's (an engine's own icon:
/// 14 of the Unreal games here had Unreal's, the developers never set one). The picture they
/// had is kept beside it. After every scan.
pub fn sync_icons(games: &[library::Game]) {
    let apps = library::norm(&applications());
    let mut entries: Vec<(&library::Game, PathBuf, String, String)> = Vec::new();
    for g in games.iter().filter(|g| g.kind == Kind::Game && !g.gone) {
        let mut own: Vec<PathBuf> = g.entries.iter().flat_map(|e| &e.keys).filter_map(|k| if let library::Key::Desktop(p) = k { Some(p.clone()) } else { None }).filter(|p| own_entry(p, &apps)).collect();
        own.sort();
        own.dedup();
        for entry in own {
            let Ok(text) = fs::read_to_string(&entry) else { continue };
            let get = |k: &str| main_group(&text).find_map(|l| l.strip_prefix(k)?.strip_prefix('=')).map(str::to_string);
            let original = get(ORIGINAL_ICON).or_else(|| get("Icon")).unwrap_or_default();
            let content = fs::read(&original).ok().filter(|_| original.starts_with('/')).map(|b| format!("{:x}", content_hash(&b))).unwrap_or_default();
            entries.push((g, entry, original, content));
        }
    }
    let mut changed = false;
    for (g, entry, original, content) in &entries {
        let shared = !content.is_empty() && entries.iter().any(|(o, _, _, c)| c == content && o.id != g.id);
        let plain = content.is_empty() && PLAIN_ICONS.contains(&original.as_str());
        if !(shared || plain) {
            continue;
        }
        let Some(cover) = g.cover().map(str::to_string) else { continue };
        match game_icon(&g.id, &cover, entry, original) {
            Ok(true) => {
                changed = true;
                crate::log::line(&format!("menu icons: {}'s entry shows its cover now ({})", g.name, if shared { "its program's icon is another game's too" } else { "it had none" }));
            }
            Ok(false) => {}
            Err(e) => crate::log::line(&format!("menu icons: {}: {e}", g.name)),
        }
    }
    if changed {
        refresh();
    }
}

/// Points the entry at a square picture cut from the cover (made when it's missing or the
/// cover changed). Whether the entry changed.
fn game_icon(id: &str, cover: &str, entry: &Path, original: &str) -> Result<bool, String> {
    let text = fs::read_to_string(entry).map_err(|e| e.to_string())?;
    let file = menu_icons().join(format!("{id}.png"));
    let from = main_group(&text).find_map(|l| l.strip_prefix(ICON_FROM)?.strip_prefix('=')).map(str::to_string);
    if from.as_deref() != Some(cover) || !file.is_file() {
        let source = if cover.starts_with("https://") { crate::covers::web_copy(cover).ok_or("its cover isn't downloaded")? } else { PathBuf::from(cover) };
        let img = image::ImageReader::open(&source).map_err(|e| e.to_string())?.with_guessed_format().map_err(|e| e.to_string())?.decode().map_err(|e| e.to_string())?;
        fs::create_dir_all(menu_icons()).map_err(|e| e.to_string())?;
        square(&img).save_with_format(&file, image::ImageFormat::Png).map_err(|e| e.to_string())?;
    }
    let rewritten = with_keys(&text, &[("Icon", &file.display().to_string()), (ORIGINAL_ICON, original), (ICON_FROM, cover)]);
    if rewritten == text {
        return Ok(false);
    }
    fs::write(entry, rewritten).map_err(|e| format!("{}: {e}", entry.display()))?;
    Ok(true)
}

/// The middle square of a picture (a cover's 315x250 loses its sides), at most 256 px.
fn square(img: &image::DynamicImage) -> image::DynamicImage {
    let side = img.width().min(img.height());
    let cut = img.crop_imm((img.width() - side) / 2, (img.height() - side) / 2, side, side);
    if side > 256 { cut.resize_exact(256, 256, image::imageops::FilterType::Lanczos3) } else { cut }
}

/// An entry's text with these keys set in its main group: replaced where they are, else added
/// after the group's first line.
fn with_keys(text: &str, keys: &[(&str, &str)]) -> String {
    let mut main = false;
    let mut out = String::new();
    for line in text.lines() {
        if line.starts_with('[') {
            main = line.trim() == "[Desktop Entry]";
        }
        if main && keys.iter().any(|(k, _)| line.split_once('=').is_some_and(|(key, _)| key.trim() == *k)) {
            continue;
        }
        out.push_str(line);
        out.push('\n');
        if line.trim() == "[Desktop Entry]" {
            for (k, v) in keys {
                out.push_str(&format!("{k}={}\n", one_line(v)));
            }
        }
    }
    out
}

fn content_hash(bytes: &[u8]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut h);
    h.finish()
}

/// Has the app menu read its entries again (KDE's; others watch the folder themselves).
pub fn refresh() {
    let _ = std::process::Command::new("kbuildsycoca6").stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status();
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
    fn hiding_keeps_the_entry() {
        let text = "[Desktop Entry]\nName=Mycopunk\nExec=x\n\n[Desktop Action a]\nNoDisplay=true\n";
        let hidden = hidden_text(text, true);
        assert_eq!(hidden, "[Desktop Entry]\nNoDisplay=true\nX-Lobby-Hidden=true\nName=Mycopunk\nExec=x\n\n[Desktop Action a]\nNoDisplay=true\n");
        assert_eq!(hidden_text(&hidden, true), hidden);
        assert_eq!(hidden_text(&hidden, false), text);
    }

    #[test]
    fn keys_are_set_in_the_main_group() {
        let text = "[Desktop Entry]\nName=Restore\nIcon=/g/restore.png\n[Desktop Action a]\nIcon=x\n";
        let set = with_keys(text, &[("Icon", "/l/restore.png"), (ORIGINAL_ICON, "/g/restore.png")]);
        assert_eq!(set, "[Desktop Entry]\nIcon=/l/restore.png\nX-Lobby-Original-Icon=/g/restore.png\nName=Restore\n[Desktop Action a]\nIcon=x\n");
        assert_eq!(with_keys(&set, &[("Icon", "/l/restore.png"), (ORIGINAL_ICON, "/g/restore.png")]), set);
    }

    #[test]
    fn icons_are_the_covers_middle() {
        let wide = image::DynamicImage::new_rgba8(315, 250);
        let icon = square(&wide);
        assert_eq!((icon.width(), icon.height()), (250, 250));
        assert_eq!((square(&image::DynamicImage::new_rgba8(600, 900)).width(), square(&image::DynamicImage::new_rgba8(600, 900)).height()), (256, 256));
    }

    #[test]
    fn names_stay_on_one_line() {
        let s = Shortcut { name: "Two\nlines".into(), program: "/p/a.exe".into(), prefix: "/p".into(), place: "x".into(), entry: None };
        assert!(entry_text(&s, "two-lines", "lobby", None).contains("\nName=Two lines\n"));
    }
}
