//! Starting a game: through whichever launcher knows it best, else set up once by gamefix (an
//! app-menu entry with its fixes, which then starts it from anywhere, lobby or not).

use crate::library::{Entry, Game, Key, Kind, Source};
use std::fs;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[derive(Debug, PartialEq)]
pub enum Plan {
    /// A command that starts it, and what it goes through.
    Run { how: String, command: Vec<String>, cwd: Option<PathBuf> },
    /// No launcher knows it: gamefix sets it up first.
    SetUp(PathBuf),
    /// A Blizzard game: started through lobby's Battle.net (installed the first time), with
    /// what Battle.net's --exec calls it.
    BattleNet { folder: PathBuf, launch: String },
    Cannot(String),
}

fn is_gamefix_entry(e: &Entry) -> bool {
    e.made_by_gamefix()
}

/// How a game starts, from the entries it was found by, best first.
pub fn plan(game: &Game) -> Plan {
    if let Some(note) = game.entries.iter().find_map(|e| e.note.as_ref()).filter(|_| game.kind == Kind::WrongSystem) {
        return Plan::Cannot(format!("{note}. The game's page may have a version for Windows or Linux."));
    }
    if game.kind == Kind::Download {
        return Plan::Cannot("It's downloaded, not installed yet. Install unpacks it into your games folder (or runs its installer) and sets it up.".into());
    }
    if game.kind == Kind::Owned {
        let store = game.entries.iter().find(|e| e.kind == Kind::Owned).map_or("a store", |e| e.source.label());
        return Plan::Cannot(format!("You own it on {store}; it isn't installed. Install downloads it into your games folder and sets it up."));
    }
    let keys = || game.entries.iter().flat_map(|e| e.keys.iter());
    // Steam's own install (a copy in a library Steam doesn't list isn't one Steam can start).
    let steam_app = game.entries.iter().filter(|e| e.source != Source::SteamElsewhere).flat_map(|e| e.keys.iter()).find_map(|k| if let Key::SteamApp(id) = k { Some(*id) } else { None });
    if let Some(id) = steam_app {
        return run("Steam", vec!["steam".into(), format!("steam://rungameid/{id}")], None);
    }
    if let Some(e) = game.entries.iter().find(|e| e.source == Source::SteamElsewhere) {
        let at = e.folder.as_deref().map(|f| f.display().to_string()).unwrap_or_default();
        return Plan::Cannot(format!("It's in a Steam library Steam doesn't use ({at}). \"Move into Steam\" on its page copies it into one of Steam's libraries; Steam starts it from there."));
    }
    if let Some(p) = store_plan(game) {
        return p;
    }
    // Blizzard's games only start through Battle.net (a launcher that has its own Battle.net
    // for it starts it there).
    let own_battlenet = keys().any(|k| matches!(k, Key::Heroic(..) | Key::Lutris(_) | Key::Faugus(_) | Key::SteamShortcut(_)));
    if let Some((folder, product)) = crate::battlenet::game_folder(game).filter(|_| !own_battlenet) {
        return Plan::BattleNet { folder, launch: product.launch };
    }
    if let Some(p) = game.entries.iter().filter(|e| is_gamefix_entry(e)).find_map(desktop_plan) {
        return p;
    }
    if let Some((runner, app)) = keys().find_map(|k| if let Key::Heroic(r, a) = k { Some((r, a)) } else { None }) {
        return run("Heroic", vec!["xdg-open".into(), format!("heroic://launch?appName={app}&runner={runner}")], None);
    }
    if let Some(id) = keys().find_map(|k| if let Key::Lutris(id) = k { Some(*id) } else { None }) {
        return run("Lutris", vec!["lutris".into(), format!("lutris:rungameid/{id}")], None);
    }
    // Unreal's launcher (Starship Troopers') only checks for Microsoft's VC++ runtime, which a
    // Proton update can put back to Wine's older one, then starts the game's program: started
    // directly, as the launcher would, it runs on Wine's. Only when Faugus adds nothing of its own.
    let faugus_unreal = game.entries.iter().filter(|e| e.source == Source::Faugus && e.keys.iter().any(|k| matches!(k, Key::Faugus(id) if crate::sources::faugus_plain_settings(id)))).find_map(|e| {
        let (target, args) = gamefix::binary::unreal_bootstrap(e.program.as_ref()?)?;
        Some(umu_plan(&target, e.prefix.as_ref()?, &args, "the game's own program in Faugus's Wine prefix, past Unreal's launcher"))
    });
    if let Some(p) = faugus_unreal {
        return p;
    }
    if let Some(id) = keys().find_map(|k| if let Key::Faugus(id) = k { Some(id.clone()) } else { None }) {
        let native = program_on_path("faugus-launcher");
        let command = match native {
            Some(p) => vec![p.display().to_string(), "--game".into(), id],
            None => vec!["flatpak".into(), "run".into(), "io.github.Faugus.faugus-launcher".into(), "--game".into(), id],
        };
        return run("Faugus", command, None);
    }
    if let Some(id) = keys().find_map(|k| if let Key::SteamShortcut(id) = k { Some(*id) } else { None }) {
        // Steam's run id for a non-Steam game: the shortcut's id in the top half.
        let run_id = ((id as u64) << 32) | 0x0200_0000;
        return run("Steam (non-Steam game)", vec!["steam".into(), format!("steam://rungameid/{run_id}")], None);
    }
    if let Some(p) = game.entries.iter().filter(|e| e.source == Source::AppMenu).find_map(desktop_plan) {
        return p;
    }
    // Installed into a Wine prefix by its own installer: run there, where its registry is.
    if let Some(e) = game.entries.iter().find(|e| e.prefix.is_some() && e.program.as_ref().is_some_and(|p| windows_program(p))) {
        let program = e.program.as_ref().unwrap();
        let (program, args) = gamefix::binary::unreal_bootstrap(program).unwrap_or((program.clone(), String::new()));
        return umu_plan(&program, e.prefix.as_ref().unwrap(), &args, "umu-run, in the Wine prefix it was installed into");
    }
    // A folder, or a game in one program (a loose .exe in Downloads).
    // The installed one when the game's download was found too.
    let folders = || game.entries.iter().filter(|e| e.source == Source::Folder);
    if let Some(folder) = folders().find(|e| e.kind != Kind::Download).or_else(|| folders().next()).and_then(|e| e.folder.clone().or_else(|| e.program.clone())) {
        return Plan::SetUp(folder);
    }
    Plan::Cannot("Nothing that knows how to start it was found (no launcher lists it, and its files aren't in a game folder).".into())
}

impl Plan {
    /// In a sentence, for people.
    pub fn describe(&self) -> String {
        match self {
            Plan::Run { how, .. } => format!("Starts with {how}."),
            Plan::SetUp(_) => "No launcher knows it: the first time you play, gamefix sets it up (an app-menu entry with any fixes it needs).".into(),
            Plan::BattleNet { .. } if !crate::battlenet::installed() => "A Blizzard game: it starts through Blizzard's Battle.net app, which signs you in. The first time you play, Lobby installs Battle.net for it.".into(),
            Plan::BattleNet { .. } => "Starts with Battle.net, which signs you in.".into(),
            Plan::Cannot(why) => why.clone(),
        }
    }
}

fn run(how: &str, command: Vec<String>, cwd: Option<PathBuf>) -> Plan {
    Plan::Run { how: how.into(), command, cwd }
}

fn windows_program(p: &Path) -> bool {
    p.extension().is_some_and(|e| e.eq_ignore_ascii_case("exe"))
}

pub fn program_on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).map(|d| d.join(name)).find(|p| p.is_file())
}

/// umu-run with the prefix the game is in and Steam's default Proton (else umu's GE-Proton).
fn umu_plan(program: &Path, prefix: &Path, args: &str, how: &str) -> Plan {
    // A Proton-style prefix (`<folder>/pfx` holding drive_c) is given to umu as its folder.
    let wineprefix = match prefix.file_name() {
        Some(n) if n == "pfx" && !prefix.is_symlink() && fs::symlink_metadata(prefix).is_ok_and(|m| m.is_dir()) => prefix.parent().unwrap_or(prefix),
        _ => prefix,
    };
    // The Proton that made the prefix: another would upgrade it, which can break the game for
    // the launcher that set it up.
    let steam = gamefix::steam::Steam::find();
    let proton = proton_of(prefix)
        .or_else(|| steam.as_ref().and_then(|s| s.tool_dir(&s.proton()?)))
        .map_or("GE-Proton".to_string(), |d| d.display().to_string());
    let command = vec![
        "env".into(),
        format!("WINEPREFIX={}", wineprefix.display()),
        format!("PROTONPATH={proton}"),
        "GAMEID=umu-0".into(),
        "umu-run".into(),
        program.display().to_string(),
    ];
    let command = command.into_iter().chain(args.split_whitespace().map(String::from)).collect();
    run(how, command, program.parent().map(Path::to_path_buf))
}

/// The Proton a prefix was made or last upgraded with: Proton's `config_info` (beside drive_c,
/// or beside `pfx`) names its version, then its own font folder.
pub fn proton_of(prefix: &Path) -> Option<PathBuf> {
    [Some(prefix), prefix.parent()].into_iter().flatten().find_map(|dir| {
        let text = fs::read_to_string(dir.join("config_info")).ok()?;
        let fonts = text.lines().nth(1)?.trim_end_matches('/');
        let tool = ["/files/share/fonts", "/dist/share/fonts"].iter().find_map(|s| fonts.strip_suffix(s))?;
        let tool = PathBuf::from(tool);
        tool.join("proton").is_file().then_some(tool)
    })
}

/// A game lobby installed from a store starts through `lobby run`, which syncs its cloud saves
/// around playing and (Epic) passes Epic's sign-in; the game itself runs as its gamefix entry
/// runs it (prefix, Proton, fixes).
fn store_plan(game: &Game) -> Option<Plan> {
    let (store, id) = crate::stores::installed_in(game)?;
    let folder = game.entries.iter().find(|e| e.source == store.source()).and_then(|e| e.folder.clone())?;
    if wine_command(game).is_none() {
        return Some(Plan::SetUp(folder));
    }
    let lobby = std::env::current_exe().map_or_else(|_| "lobby".to_string(), |p| p.display().to_string());
    let setup = match crate::stores::wine_setup(store, &id) {
        Some(s) => format!("in the Wine prefix it had in Heroic, {}", s.prefix.display()),
        None => "as its gamefix setup runs it".to_string(),
    };
    let how = match store {
        crate::stores::Store::Epic => format!("Lobby (Epic's sign-in and cloud saves), {setup}"),
        crate::stores::Store::Gog => format!("Lobby (GOG cloud saves, when the game has them), {setup}"),
    };
    Some(run(&how, vec![lobby, "run".into(), game.id.clone()], Some(folder)))
}

/// How a store game lobby installed runs, split: the environment (NAME=value words), umu-run
/// (none for a Linux version), then the program and its arguments; and the folder it runs in.
/// A game taken over from Heroic runs in the prefix and Proton it had there; any other, as
/// its gamefix entry runs it.
pub fn wine_command(game: &Game) -> Option<WineCommand> {
    if let Some((store, id)) = crate::stores::installed_in(game) {
        let ours = store.installed().into_iter().find(|g| g.id == id)?;
        if ours.platform.as_deref() == Some("linux") {
            return Some(WineCommand { env: Vec::new(), umu: None, program: vec![ours.program?.display().to_string()], cwd: Some(ours.folder) });
        }
        if let Some(setup) = crate::stores::wine_setup(store, &id) {
            let gog = gamefix::gog::read(&ours.folder);
            let program = gog.as_ref().map(|g| g.program.clone()).or(ours.program)?;
            let mut words = vec![program.display().to_string()];
            words.extend(gog.as_ref().map(|g| g.args.clone()).unwrap_or_default());
            let cwd = gog.and_then(|g| g.workdir).or_else(|| program.parent().map(Path::to_path_buf));
            let proton = setup.proton.map_or("GE-Proton".to_string(), |p| p.display().to_string());
            let env = vec![format!("WINEPREFIX={}", setup.prefix.display()), format!("PROTONPATH={proton}"), "GAMEID=umu-0".into()];
            let umu = program_on_path("umu-run").map_or("umu-run".to_string(), |p| p.display().to_string());
            return Some(WineCommand { env, umu: Some(umu), program: words, cwd });
        }
    }
    let (words, cwd) = game.entries.iter().filter(|e| is_gamefix_entry(e)).find_map(|e| {
        let file = e.keys.iter().find_map(|k| if let Key::Desktop(p) = k { Some(p) } else { None })?;
        desktop_exec(&fs::read_to_string(file).ok()?)
    })?;
    let umu = words.iter().position(|w| w.ends_with("umu-run"))?;
    let env = if words[0] == "env" { words[1..umu].to_vec() } else { Vec::new() };
    Some(WineCommand { env, umu: Some(words[umu].clone()), program: words[umu + 1..].to_vec(), cwd })
}

pub struct WineCommand {
    pub env: Vec<String>,
    pub umu: Option<String>,
    pub program: Vec<String>,
    pub cwd: Option<PathBuf>,
}

fn desktop_plan(e: &Entry) -> Option<Plan> {
    let file = e.keys.iter().find_map(|k| if let Key::Desktop(p) = k { Some(p) } else { None })?;
    let (exec, path) = desktop_exec(&fs::read_to_string(file).ok()?)?;
    let how = if is_gamefix_entry(e) { "its gamefix app-menu entry" } else { "its app-menu entry" };
    Some(run(how, exec, path))
}

/// A .desktop file's command (field codes dropped) and working folder.
pub fn desktop_exec(text: &str) -> Option<(Vec<String>, Option<PathBuf>)> {
    let mut main = false;
    let (mut exec, mut path) = (None, None);
    for line in text.lines() {
        if line.starts_with('[') {
            main = line.trim() == "[Desktop Entry]";
        } else if main {
            if let Some(v) = line.strip_prefix("Exec=") {
                exec = Some(unescape_value(v.trim()));
            } else if let Some(v) = line.strip_prefix("Path=") {
                path = Some(PathBuf::from(unescape_value(v.trim()))).filter(|p| !p.as_os_str().is_empty());
            }
        }
    }
    let words: Vec<String> = exec_words(&exec?).into_iter().filter(|w| !(w.len() == 2 && w.starts_with('%') && w != "%%")).map(|w| w.replace("%%", "%")).collect();
    (!words.is_empty()).then_some((words, path))
}

/// The desktop entry spec's string escapes (`\\`, `\s`, `\n`, `\t`, `\r`).
fn unescape_value(v: &str) -> String {
    let mut out = String::new();
    let mut chars = v.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('s') => out.push(' '),
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some(other) => out.push(other),
            None => out.push('\\'),
        }
    }
    out
}

/// An Exec value's words: spaces separate them, "double quotes" hold one, inside which a
/// backslash keeps the next character as it is.
pub fn exec_words(exec: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut started = false;
    let mut quoted = false;
    let mut chars = exec.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                quoted = !quoted;
                started = true;
            }
            '\\' if quoted => cur.extend(chars.next()),
            c if c.is_whitespace() && !quoted => {
                if started {
                    out.push(std::mem::take(&mut cur));
                    started = false;
                }
            }
            c => {
                cur.push(c);
                started = true;
            }
        }
    }
    if started {
        out.push(cur);
    }
    out
}

/// Starts the command on its own (it outlives lobby), its output going to the game's log.
pub fn start(command: &[String], cwd: Option<&Path>, log: &Path) -> Result<(), String> {
    let (program, args) = command.split_first().ok_or("empty command")?;
    fs::create_dir_all(log.parent().unwrap()).map_err(|e| e.to_string())?;
    let out = fs::File::create(log).map_err(|e| format!("{}: {e}", log.display()))?;
    let err = out.try_clone().map_err(|e| e.to_string())?;
    let mut cmd = Command::new(program);
    cmd.args(args).stdin(Stdio::null()).stdout(out).stderr(err).process_group(0);
    if let Some(dir) = cwd.filter(|d| d.is_dir()) {
        cmd.current_dir(dir);
    }
    cmd.spawn().map(|_| ()).map_err(|e| format!("{program}: {e}"))
}

/// A game's launch options, as typed in Steam's style: words added after the game's program, or
/// with `%command%`, variables before it and words after (`PROTON_LOG=1 %command% -dx11`).
pub struct LaunchOptions {
    pub vars: Vec<(String, String)>,
    pub args: Vec<String>,
}

pub fn parse_options(text: &str) -> Result<LaunchOptions, String> {
    let words = exec_words(text);
    match words.iter().position(|w| w == "%command%") {
        Some(i) => {
            let vars = words[..i]
                .iter()
                .map(|w| w.split_once('=').filter(|(k, _)| !k.is_empty() && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')).map(|(k, v)| (k.to_string(), v.to_string())).ok_or_else(|| format!("Before %command% go only variables (NAME=value): \"{w}\" isn't one.")))
                .collect::<Result<_, _>>()?;
            Ok(LaunchOptions { vars, args: words[i + 1..].to_vec() })
        }
        None => Ok(LaunchOptions { vars: Vec::new(), args: words }),
    }
}

/// The command a plan runs, with the game's launch options added. An Err says why they can't be
/// (another launcher starts the game: they're set there). `lobby run` adds them itself.
pub fn with_options(command: Vec<String>, options: Option<&str>) -> Result<Vec<String>, String> {
    let Some(text) = options.filter(|o| !o.trim().is_empty()) else { return Ok(command) };
    let o = parse_options(text)?;
    let first = command.first().map(String::as_str).unwrap_or_default();
    if command.get(1).is_some_and(|w| w == "run") && first.ends_with("lobby") {
        return Ok(command);
    }
    if first == "steam" {
        let id = command.get(1).and_then(|u| u.strip_prefix("steam://rungameid/")).and_then(|i| i.parse::<u64>().ok());
        return match id {
            Some(id) if id <= u32::MAX as u64 && o.vars.is_empty() => Ok([vec!["steam".into(), "-applaunch".into(), id.to_string()], o.args].concat()),
            Some(id) if id <= u32::MAX as u64 => Err("Steam doesn't take variables from Lobby: put them in the game's launch options in Steam (right-click it → Properties).".into()),
            _ => Err("Steam starts this non-Steam game: set its launch options in Steam (right-click it → Properties).".into()),
        };
    }
    let launcher = match first {
        "xdg-open" => Some("Heroic"),
        "lutris" => Some("Lutris"),
        "flatpak" => Some("Faugus"),
        f if f.ends_with("faugus-launcher") => Some("Faugus"),
        _ => None,
    };
    if let Some(l) = launcher {
        return Err(format!("{l} starts it: set its launch options in {l}."));
    }
    let mut c = command;
    if c.first().is_none_or(|w| w != "env") {
        c.insert(0, "env".into());
    }
    for (i, (k, v)) in o.vars.iter().enumerate() {
        c.insert(1 + i, format!("{k}={v}"));
    }
    c.extend(o.args);
    Ok(c)
}

/// Why launch options can't apply to how the plan starts the game, if they can't.
pub fn options_unsupported(plan: &Plan) -> Option<String> {
    match plan {
        Plan::Run { command, .. } => with_options(command.clone(), Some("-x")).err(),
        Plan::BattleNet { .. } => Some("Battle.net starts it: set its launch options in Battle.net (the game's settings, Additional command line arguments).".into()),
        Plan::SetUp(_) => None,
        Plan::Cannot(why) => Some(why.clone()),
    }
}

/// Something gamefix needs installed before it can set a game up.
pub enum Need {
    /// A Java runtime of this version: gamefix downloads Eclipse Temurin's.
    Java(u32),
    /// A program from Flathub that runs the game (a .love file's LÖVE ...).
    Flatpak { name: &'static str, app: &'static str },
}

impl Need {
    /// For the question: what it is, and how big a download (when known).
    pub fn describe(&self) -> (String, Option<String>) {
        match self {
            Need::Java(major) => (format!("Java {major}"), gamefix::java::latest(*major).ok().map(|r| r.size_text())),
            Need::Flatpak { name, app } => (name.to_string(), gamefix::setup::flathub_size(app)),
        }
    }

    pub fn install(&self) -> Result<(), String> {
        match self {
            Need::Java(major) => gamefix::java::install(*major).map(|_| ()),
            Need::Flatpak { app, .. } => gamefix::setup::install_flatpak(app),
        }
    }
}

/// What gamefix needs installed first to set the game in the folder up, if anything.
pub fn setup_need(folder: &Path) -> Option<Need> {
    let game = gamefix::scan::scan(folder)?;
    let report = gamefix::check::check(&game, &gamefix::system::System::load());
    if let Some(major) = gamefix::setup::missing_java(&game, &report) {
        return Some(Need::Java(major));
    }
    gamefix::setup::missing_runner(&game, &report).map(|r| Need::Flatpak { name: r.name, app: r.app })
}

/// gamefix's setup for a game no launcher knows: an app-menu entry with its fixes, named
/// `name` when lobby knows the game's real name (a store's, else gamefix names it from its
/// files: "MinutesTillDawn"). Returns what it did and the entry made, which starts it from
/// then on.
pub fn set_up(folder: &Path, name: Option<&str>) -> Result<(Vec<String>, Option<PathBuf>), String> {
    let mut game = gamefix::scan::scan(folder).ok_or_else(|| format!("{}: nothing there gamefix can read", folder.display()))?;
    if let Some(name) = name {
        game.name = name.to_string();
    }
    let report = gamefix::check::check(&game, &gamefix::system::System::load());
    if let Some(runner) = gamefix::setup::missing_runner(&game, &report) {
        return Err(format!("It needs {} (from Flathub) to run. gamefix setup \"{}\" installs it and sets the game up.", runner.name, folder.display()));
    }
    let outcome = gamefix::setup::setup(&game, &report, &gamefix::setup::Targets { menu: true, steam: false })?;
    let mut lines: Vec<String> = outcome.done.iter().map(|l| format!("✓ {l}")).collect();
    lines.extend(outcome.not_fixed.iter().map(|l| format!("✗ {l}")));
    let record = fs::read_to_string(gamefix::setup::record_path(&game)).unwrap_or_default();
    let entry = record.lines().find_map(|l| l.strip_prefix("created\t")).map(PathBuf::from).filter(|p| p.extension().is_some_and(|e| e == "desktop"));
    Ok((lines, entry))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::Entry;

    fn game(entries: Vec<Entry>) -> Game {
        Game { id: "g".into(), name: "G".into(), kind: Kind::Game, entries, missing: false, gone: false, added: 0, played: None, chosen_cover: None, launch_options: None }
    }

    fn with_key(source: Source, key: Key) -> Entry {
        let mut e = Entry::new(source, "G");
        e.keys.push(key);
        e
    }

    #[test]
    fn steam_games_start_from_steam() {
        let g = game(vec![with_key(Source::AppMenu, Key::Desktop("/nowhere.desktop".into())), with_key(Source::Steam, Key::SteamApp(49520))]);
        assert_eq!(plan(&g), run("Steam", vec!["steam".into(), "steam://rungameid/49520".into()], None));
    }

    #[test]
    fn launchers_by_their_ids() {
        let heroic = game(vec![with_key(Source::Gog, Key::Heroic("gog".into(), "1207658750".into()))]);
        assert_eq!(plan(&heroic), run("Heroic", vec!["xdg-open".into(), "heroic://launch?appName=1207658750&runner=gog".into()], None));
        let lutris = game(vec![with_key(Source::SteamShortcut, Key::SteamShortcut(3847822183)), with_key(Source::Lutris, Key::Lutris(2))]);
        assert_eq!(plan(&lutris), run("Lutris", vec!["lutris".into(), "lutris:rungameid/2".into()], None));
        let shortcut = game(vec![with_key(Source::SteamShortcut, Key::SteamShortcut(3847822183))]);
        assert_eq!(plan(&shortcut), run("Steam (non-Steam game)", vec!["steam".into(), "steam://rungameid/16526270436841881600".into()], None));
    }

    #[test]
    fn unknown_folders_are_set_up_and_downloads_wait() {
        let mut e = Entry::new(Source::Folder, "G");
        e.folder = Some("/g/LAPIDARY".into());
        assert_eq!(plan(&game(vec![e.clone()])), Plan::SetUp("/g/LAPIDARY".into()));
        e.kind = Kind::Download;
        let mut g = game(vec![e.clone()]);
        g.kind = Kind::Download;
        assert!(matches!(plan(&g), Plan::Cannot(_)));
        // Installed, with its download found too: the installed folder is set up.
        let mut installed = Entry::new(Source::Folder, "G");
        installed.folder = Some("/games/G".into());
        e.folder = Some("/downloads/G".into());
        assert_eq!(plan(&game(vec![e, installed])), Plan::SetUp("/games/G".into()));
    }

    #[test]
    fn launch_options() {
        let cmd = |w: &[&str]| w.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let umu = cmd(&["env", "WINEPREFIX=/p", "umu-run", "/g/game.exe"]);
        assert_eq!(with_options(umu.clone(), Some("-dx11 -windowed")).unwrap(), cmd(&["env", "WINEPREFIX=/p", "umu-run", "/g/game.exe", "-dx11", "-windowed"]));
        assert_eq!(with_options(umu.clone(), Some("PROTON_LOG=1 %command% -dx11")).unwrap(), cmd(&["env", "PROTON_LOG=1", "WINEPREFIX=/p", "umu-run", "/g/game.exe", "-dx11"]));
        assert_eq!(with_options(cmd(&["/g/game.x86_64"]), Some("-w \"two words\"")).unwrap(), cmd(&["env", "/g/game.x86_64", "-w", "two words"]));
        assert_eq!(with_options(umu.clone(), Some("  ")).unwrap(), umu);
        assert!(with_options(umu, Some("-dx11 %command%")).is_err());
        assert_eq!(with_options(cmd(&["steam", "steam://rungameid/49520"]), Some("-novid")).unwrap(), cmd(&["steam", "-applaunch", "49520", "-novid"]));
        assert!(with_options(cmd(&["lutris", "lutris:rungameid/2"]), Some("-x")).is_err());
        assert_eq!(with_options(cmd(&["/usr/bin/lobby", "run", "g"]), Some("-x")).unwrap(), cmd(&["/usr/bin/lobby", "run", "g"]));
    }

    #[test]
    fn desktop_commands() {
        let text = "[Desktop Entry]\nName=X\nExec=env WINEPREFIX=/p \"PROTONPATH=/s/Proton - Experimental\" /usr/bin/umu-run \"/g/A \\\\\\\\$B.exe\" %U\nPath=/g\n[Desktop Action x]\nExec=other\n";
        let (exec, path) = desktop_exec(text).unwrap();
        assert_eq!(exec, ["env", "WINEPREFIX=/p", "PROTONPATH=/s/Proton - Experimental", "/usr/bin/umu-run", "/g/A \\$B.exe"]);
        assert_eq!(path, Some(PathBuf::from("/g")));
        // Both ways of quoting a prefix with a space (gamefix's, and Wine's own entries').
        for exec in ["env \"WINEPREFIX=/g/My Game 2\" umu-run x.exe", "env WINEPREFIX=\"/g/My Game 2\" wine x.exe"] {
            let (words, _) = desktop_exec(&format!("[Desktop Entry]\nExec={exec}\n")).unwrap();
            assert_eq!(words.iter().find_map(|w| w.strip_prefix("WINEPREFIX=")), Some("/g/My Game 2"), "{exec}");
        }
    }
}
