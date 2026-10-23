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
    Cannot(String),
}

fn is_gamefix_entry(e: &Entry) -> bool {
    e.keys.iter().any(|k| matches!(k, Key::Desktop(p) if p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("gamefix-"))))
}

/// How a game starts, from the entries it was found by, best first.
pub fn plan(game: &Game) -> Plan {
    if game.kind == Kind::Download {
        return Plan::Cannot("It's downloaded but not installed yet; installing from lobby comes later. gamefix setup on the download installs it.".into());
    }
    let keys = || game.entries.iter().flat_map(|e| e.keys.iter());
    if let Some(id) = keys().find_map(|k| if let Key::SteamApp(id) = k { Some(*id) } else { None }) {
        return run("Steam", vec!["steam".into(), format!("steam://rungameid/{id}")], None);
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
        return umu_plan(e.program.as_ref().unwrap(), e.prefix.as_ref().unwrap());
    }
    if let Some(folder) = game.entries.iter().find(|e| e.source == Source::Folder).and_then(|e| e.folder.clone()) {
        return Plan::SetUp(folder);
    }
    Plan::Cannot("Nothing that knows how to start it was found (no launcher lists it, and its files aren't in a game folder).".into())
}

fn run(how: &str, command: Vec<String>, cwd: Option<PathBuf>) -> Plan {
    Plan::Run { how: how.into(), command, cwd }
}

fn windows_program(p: &Path) -> bool {
    p.extension().is_some_and(|e| e.eq_ignore_ascii_case("exe"))
}

fn program_on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).map(|d| d.join(name)).find(|p| p.is_file())
}

/// umu-run with the prefix the game is in and Steam's default Proton (else umu's GE-Proton).
fn umu_plan(program: &Path, prefix: &Path) -> Plan {
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
    run("umu-run, in the Wine prefix it was installed into", command, program.parent().map(Path::to_path_buf))
}

/// The Proton a prefix was made or last upgraded with: Proton's `config_info` (beside drive_c,
/// or beside `pfx`) names its version, then its own font folder.
fn proton_of(prefix: &Path) -> Option<PathBuf> {
    [Some(prefix), prefix.parent()].into_iter().flatten().find_map(|dir| {
        let text = fs::read_to_string(dir.join("config_info")).ok()?;
        let fonts = text.lines().nth(1)?.trim_end_matches('/');
        let tool = ["/files/share/fonts", "/dist/share/fonts"].iter().find_map(|s| fonts.strip_suffix(s))?;
        let tool = PathBuf::from(tool);
        tool.join("proton").is_file().then_some(tool)
    })
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

/// gamefix's setup for a game no launcher knows: an app-menu entry with its fixes. Returns
/// what it did and the entry made, which starts it from then on.
pub fn set_up(folder: &Path) -> Result<(Vec<String>, Option<PathBuf>), String> {
    let game = gamefix::scan::scan(folder).ok_or_else(|| format!("{}: nothing there gamefix can read", folder.display()))?;
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
        Game { id: "g".into(), name: "G".into(), kind: Kind::Game, entries, missing: false, gone: false, added: 0, played: None }
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
        let mut g = game(vec![e]);
        g.kind = Kind::Download;
        assert!(matches!(plan(&g), Plan::Cannot(_)));
    }

    #[test]
    fn desktop_commands() {
        let text = "[Desktop Entry]\nName=X\nExec=env WINEPREFIX=/p \"PROTONPATH=/s/Proton - Experimental\" /usr/bin/umu-run \"/g/A \\\\\\\\$B.exe\" %U\nPath=/g\n[Desktop Action x]\nExec=other\n";
        let (exec, path) = desktop_exec(text).unwrap();
        assert_eq!(exec, ["env", "WINEPREFIX=/p", "PROTONPATH=/s/Proton - Experimental", "/usr/bin/umu-run", "/g/A \\$B.exe"]);
        assert_eq!(path, Some(PathBuf::from("/g")));
    }
}
