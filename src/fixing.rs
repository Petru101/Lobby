//! gamefix on a game's page: what's wrong with it and the fix, as gamefix's right-click menu has
//! them. The fix goes where the game's start will use it: a Steam game's own settings in Steam,
//! or the app-menu entry Lobby starts a game from. Other starts (a store's, another launcher's
//! settings) don't use gamefix's fixes, so those games only get told what's wrong.

use crate::launch::{self, Plan};
use crate::library::{Entry, Game, Kind, Source};
use gamefix::setup::{self, ProtonChoice, Targets};
use gamefix::steam::Steam;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq)]
pub enum How {
    /// Steam's own install: its launch options and Proton, in Steam.
    Steam(PathBuf),
    /// An app-menu entry made by gamefix, which Play then starts. `name` keeps an entry's old
    /// record, whose name also names its Wine prefix: a new name would start a new prefix,
    /// without the game's saves.
    Menu { target: PathBuf, name: String },
    /// Never started: its first Play sets it up, with any fixes it needs.
    FirstPlay(PathBuf),
    /// Lobby doesn't start it through gamefix: why, for the page.
    OnlyCheck { target: PathBuf, why: String },
}

/// What gamefix found.
pub struct Look {
    pub how: How,
    /// What its report says, for the page.
    pub text: String,
    /// Something's wrong, not counting what every Windows game has (no Linux version).
    pub problems: bool,
    /// Fix has something to do.
    pub fixable: bool,
    /// gamefix has set it up: what the report names is already seen to.
    pub set_up: bool,
    /// Its last start failed.
    pub failed: bool,
    /// gamefix changes nothing about how this game starts (an anti-cheat, Steam's containers).
    pub left_alone: bool,
    /// All gamefix has changed for it is which Proton Steam uses.
    pub only_proton: bool,
    /// Steam's settings hold a fix of gamefix's for it, which can be taken back.
    pub can_undo: bool,
    /// The Protons it can try, after a failed start of a Steam game's Windows build.
    pub protons: Vec<ProtonChoice>,
}

impl Look {
    /// Whether a failed start should offer the fix: not when it's in place and the game failed
    /// with it, nor when there's nothing to do.
    pub fn offers_fix(&self) -> bool {
        self.how.can_fix() && self.fixable && !self.set_up
    }

    /// The button, if any. Setting a game up again is only for one that failed with its fixes
    /// in: a game that works has nothing to fix.
    pub fn button(&self) -> Option<&'static str> {
        if self.offers_fix() {
            Some("Fix")
        } else if self.how.can_fix() && self.set_up && self.failed && !self.left_alone {
            Some("Set up again")
        } else {
            None
        }
    }

    pub fn summary(&self) -> &'static str {
        if self.only_proton {
            "gamefix changed which Proton it uses"
        } else if self.set_up {
            "gamefix's fixes are in place"
        } else if self.problems {
            "gamefix found problems"
        } else {
            "gamefix found nothing wrong in its files"
        }
    }
}

/// What gamefix reads for a game no plan names.
fn files_of(game: &Game) -> Option<PathBuf> {
    let of = |e: &Entry| {
        // A prefix game's folder may be the whole prefix.
        let folder = e.folder.clone().filter(|f| f.is_dir() && e.prefix.is_none());
        folder.or_else(|| e.program.clone().filter(|p| p.exists()))
    };
    game.entries.iter().filter(|e| !matches!(e.kind, Kind::Download | Kind::Owned)).find_map(of)
}

/// Steam's own install of it (not a copy in a library Steam doesn't list).
fn steam_folder(game: &Game) -> Option<PathBuf> {
    game.entries.iter().find(|e| e.source == Source::Steam && e.kind != Kind::Owned).and_then(|e| e.folder.clone()).filter(|f| f.is_dir())
}

/// How gamefix can help with it; None for what isn't an installed game.
pub fn how(game: &Game, plan: &Plan) -> Option<How> {
    if game.kind != Kind::Game || game.gone || game.missing {
        return None;
    }
    if let Some(folder) = steam_folder(game) {
        return Some(How::Steam(folder));
    }
    if let Plan::SetUp(target) = plan {
        return Some(How::FirstPlay(target.clone()));
    }
    if matches!(plan, Plan::Cannot(_)) {
        return None;
    }
    let target = files_of(game)?;
    let record = launch::starts_from_gamefix(game).then(|| crate::uninstall::gamefix_record(game)).flatten();
    match record {
        // Installed by its own installer, into a prefix setting it up again wouldn't use.
        Some(r) if fs::read_to_string(&r).is_ok_and(|t| t.lines().any(|l| l.starts_with("installed-game\t"))) => Some(How::OnlyCheck { target, why: "It was installed into its Windows setup by its own installer, so setting it up again could lose that. This only says what's wrong.".into() }),
        Some(r) => {
            let stem = r.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            let name = if setup::slug(&game.name) == stem { game.name.clone() } else { stem };
            Some(How::Menu { target, name })
        }
        None => {
            let by = match plan {
                // Taken over from another launcher: started as that one set it up.
                Plan::Run { how, .. } if how == "Lobby" => "what Lobby took over from its launcher".to_string(),
                Plan::Run { how, .. } => how.clone(),
                _ => "its store's app".to_string(),
            };
            Some(How::OnlyCheck { target, why: format!("It starts with {by}, which doesn't use gamefix's fixes. This only says what's wrong.") })
        }
    }
}

impl How {
    fn target(&self) -> &Path {
        match self {
            How::Steam(t) | How::FirstPlay(t) | How::Menu { target: t, .. } | How::OnlyCheck { target: t, .. } => t,
        }
    }

    pub fn can_fix(&self) -> bool {
        matches!(self, How::Steam(_) | How::Menu { .. })
    }

    fn name<'a>(&'a self, game: &'a Game) -> &'a str {
        match self {
            How::Menu { name, .. } => name,
            _ => &game.name,
        }
    }
}

/// gamefix's reading of the game: `game`'s files, under the name it's known by.
fn read(game: &Game, how: &How) -> Result<gamefix::scan::Game, String> {
    let mut read = gamefix::scan::scan(how.target()).ok_or_else(|| format!("gamefix can't read {}.", how.target().display()))?;
    read.name = how.name(game).to_string();
    Ok(read)
}

/// Checks the game (reads its files: seconds on a big one, so not on the window's thread).
pub fn look(game: &Game) -> Result<Option<Look>, String> {
    let Some(how) = how(game, &launch::plan(game)) else { return Ok(None) };
    let read = read(game, &how)?;
    let report = gamefix::check::check(&read, &gamefix::system::System::load());
    let word = match how {
        How::Steam(_) => "Fix",
        How::FirstPlay(_) => "Play",
        How::Menu { .. } | How::OnlyCheck { .. } => "gamefix",
    };
    let said = report.for_app(&read, word);
    let steam = matches!(how, How::Steam(_));
    let can_undo = steam && setup::touches_steam(&read);
    let failed = crate::starts::failed(&game.id).is_some();
    // Only after a failed start: it may be nothing to do with Proton, and the person picks.
    let protons = if steam && failed { setup::proton_choices(&read, &report).unwrap_or_default() } else { Vec::new() };
    let record = fs::read_to_string(setup::record_path(&read)).ok();
    let only_proton = record.as_deref().is_some_and(|r| r.lines().next().is_some() && r.lines().all(|l| l.starts_with("steam-compat\t")));
    Ok(Some(Look { problems: said.problems, fixable: said.fixable, left_alone: report.hands_off.is_some(), set_up: record.is_some(), failed, only_proton, can_undo, protons, how, text: said.text }))
}

/// A report to gamefix's developer about the game, with gamefix's check of it. The caller adds
/// what the person wrote, shows them `preview`, then calls `send`.
pub fn problem_report(game: &Game) -> Result<gamefix::feedback::Report, String> {
    let how = how(game, &launch::plan(game)).ok_or_else(|| format!("{} isn't an installed game gamefix can check.", game.name))?;
    Ok(gamefix::feedback::Report::about(&read(game, &how)?))
}

/// Whether Steam has to close before gamefix can change this game's settings.
pub fn steam_in_the_way(game: &Game) -> bool {
    steam_folder(game).is_some() && Steam::find().is_some_and(|s| s.running())
}

/// Asks Steam to quit, as gamefix's right-click menu does: false if it's still open after a minute.
pub fn close_steam() -> bool {
    Steam::find().is_none_or(|s| !s.running() || s.shut_down())
}

/// What has to be installed before the fix (Java, a runner from Flathub), for the question.
pub fn need(game: &Game) -> Option<launch::Need> {
    let how = how(game, &launch::plan(game)).filter(How::can_fix)?;
    launch::setup_need(how.target())
}

/// Fixes the game. Lines say what happened: ✓ done, ✗ not fixed.
pub fn fix(game: &Game) -> Result<Vec<String>, String> {
    match how(game, &launch::plan(game)) {
        Some(how @ How::Steam(_)) => {
            let read = read(game, &how)?;
            let report = gamefix::check::check(&read, &gamefix::system::System::load());
            let outcome = setup::setup(&read, &report, &Targets { menu: false, steam: true })?;
            Ok(outcome.done.iter().map(|l| format!("✓ {l}")).chain(outcome.not_fixed.iter().map(|l| format!("✗ {l}"))).collect())
        }
        Some(How::Menu { target, name }) => {
            let (lines, entry) = launch::set_up(&target, Some(&name))?;
            // A launcher's game in a Wine prefix of its own now: its saves come too.
            if let Some(entry) = &entry {
                if let Err(e) = crate::windows_saves::from_launcher(game, entry) {
                    crate::log::line(&format!("{}: saves not brought over: {e}", game.name));
                }
            }
            Ok(lines)
        }
        Some(How::FirstPlay(_)) => Err(format!("{} hasn't been started yet: its first Play sets it up, with any fixes it needs.", game.name)),
        Some(How::OnlyCheck { why, .. }) => Err(why),
        None => Err(format!("{} isn't an installed game gamefix can fix.", game.name)),
    }
}

/// The Protons a Steam game's Windows build can be switched to, best to try first.
pub fn protons(game: &Game) -> Result<Vec<ProtonChoice>, String> {
    let how = steam_how(game)?;
    let read = read(game, &how)?;
    setup::proton_choices(&read, &gamefix::check::check(&read, &gamefix::system::System::load()))
}

/// Sets a Steam game to one of `protons`. Steam has to be closed.
pub fn switch_proton(game: &Game, tool: &str) -> Result<Vec<String>, String> {
    let how = steam_how(game)?;
    let read = read(game, &how)?;
    let lines = setup::switch_proton(&read, &gamefix::check::check(&read, &gamefix::system::System::load()), tool)?;
    Ok(lines.into_iter().map(|l| format!("✓ {l}")).chain(["Press Play to try it.".to_string()]).collect())
}

fn steam_how(game: &Game) -> Result<How, String> {
    steam_folder(game).map(How::Steam).ok_or_else(|| "It isn't a Steam game: Steam's Proton setting doesn't start it.".to_string())
}

/// Takes back gamefix's changes to a Steam game's settings.
pub fn undo(game: &Game) -> Result<Vec<String>, String> {
    let how = how(game, &launch::plan(game)).filter(|h| matches!(h, How::Steam(_))).ok_or("Only a Steam game's fix can be undone here: other games start through theirs.")?;
    Ok(setup::undo(&read(game, &how)?)?.into_iter().map(|l| format!("✓ {l}")).collect())
}

/// Whether anything was changed, from a fix's lines.
pub fn changed(lines: &[String]) -> bool {
    lines.iter().any(|l| l.starts_with('✓'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::Key;

    fn game(entries: Vec<Entry>) -> Game {
        Game { id: "g".into(), name: "G".into(), kind: Kind::Game, entries, missing: false, gone: false, added: 0, played: None, chosen_cover: None, launch_options: None, protection: Default::default(), hidden: false, favourite: false, frame_rate: false, proton: None, kept_cover: None, kept_icon: None, controller: None }
    }

    fn run(how: &str) -> Plan {
        Plan::Run { how: how.into(), command: Vec::new(), cwd: None, protect: false }
    }

    #[test]
    fn only_a_start_that_uses_the_fix_gets_one() {
        let dir = std::env::temp_dir().join(format!("lobby-fixing-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let in_folder = |source| {
            let mut e = Entry::new(source, "G");
            e.folder = Some(dir.clone());
            e
        };
        // Steam's own: fixed in Steam, whatever its plan says.
        let mut steam = in_folder(Source::Steam);
        steam.keys.push(Key::SteamApp(1));
        assert_eq!(how(&game(vec![steam.clone()]), &run("Steam")), Some(How::Steam(dir.clone())));
        // A game only owned on Steam isn't installed.
        let mut owned = game(vec![Entry::new(Source::Steam, "G")]);
        owned.kind = Kind::Owned;
        assert_eq!(how(&owned, &Plan::Cannot("not installed".into())), None);
        // A game gamefix would set up on Play.
        assert_eq!(how(&game(vec![in_folder(Source::Folder)]), &Plan::SetUp(dir.clone())), Some(How::FirstPlay(dir.clone())));
        assert!(!How::FirstPlay(dir.clone()).can_fix());
        // Another launcher's settings don't carry gamefix's fixes.
        let lutris = how(&game(vec![in_folder(Source::Lutris)]), &run("Lobby, as Lutris set it up")).unwrap();
        assert!(!lutris.can_fix());
        assert!(matches!(lutris, How::OnlyCheck { why, .. } if why.contains("as Lutris set it up")));
        // Gone games have nothing to check.
        let mut gone = game(vec![in_folder(Source::Folder)]);
        gone.gone = true;
        assert_eq!(how(&gone, &Plan::SetUp(dir.clone())), None);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_game_that_works_gets_no_button() {
        let look = |how, fixable, set_up, failed| Look { how, text: String::new(), problems: fixable, fixable, set_up, failed, left_alone: false, only_proton: false, can_undo: false, protons: Vec::new() };
        let steam = || How::Steam(PathBuf::from("/g"));
        let menu = || How::Menu { target: PathBuf::from("/g"), name: "G".into() };
        assert_eq!(look(steam(), false, false, false).button(), None);
        assert_eq!(look(steam(), true, false, false).button(), Some("Fix"));
        assert_eq!(look(menu(), false, true, false).button(), None);
        assert_eq!(look(menu(), true, true, false).button(), None);
        assert_eq!(look(menu(), false, true, true).button(), Some("Set up again"));
        assert_eq!(look(How::FirstPlay(PathBuf::from("/g")), true, false, true).button(), None);
        // What gamefix leaves alone can't be set up again.
        let mut alone = look(menu(), false, true, true);
        alone.left_alone = true;
        assert_eq!(alone.button(), None);
    }

    #[test]
    fn changes_are_told_from_the_lines() {
        assert!(changed(&["✓ Set its launch options".to_string(), "✗ Not this".to_string()]));
        assert!(!changed(&["✗ ProtonDB's reports name nothing better, so nothing was changed.".to_string()]));
    }
}
