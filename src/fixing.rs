//! gamefix on a game's page: what's wrong with it and the fix, as gamefix's right-click menu has
//! them. The fix goes where the game's start will use it: a Steam game's own settings in Steam,
//! or the app-menu entry Lobby starts a game from. Other starts (a store's, another launcher's
//! settings) don't use gamefix's fixes, so those games only get told what's wrong.

use crate::launch::{self, Plan};
use crate::library::{Entry, Game, Kind, Source};
use gamefix::setup::{self, Targets};
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
    /// Steam's settings hold a fix of gamefix's for it, which can be taken back.
    pub can_undo: bool,
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
        } else if self.how.can_fix() && self.set_up && self.failed {
            Some("Set up again")
        } else {
            None
        }
    }

    pub fn summary(&self) -> &'static str {
        if self.set_up {
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
    let (text, advice) = for_the_page(&report.render(&read), word);
    let others = report.problems.iter().any(|p| !matches!(p, gamefix::check::Problem::NoLinuxBuild));
    // gamefix leaves a game alone when its anti-cheat is what stops it.
    let blocked = text.contains(ANTI_CHEAT_WINS);
    let can_undo = matches!(how, How::Steam(_)) && setup::touches_steam(&read);
    Ok(Some(Look { problems: others || advice || blocked, fixable: (others || advice) && !blocked, set_up: setup::record_path(&read).exists(), failed: crate::starts::failed(&game.id).is_some(), can_undo, how, text }))
}

const NO_LINUX: &str = "✗ There is no Linux version of this game.";
const ANTI_CHEAT_WINS: &str = "othing on this side gets past that";

/// A report as the page says it, and whether what it says about a Windows game is advice to
/// change something (ProtonDB's Proton or options), not only how gamefix starts such a game.
/// Without the heading about where the game is, and without "no Linux version" with its advice
/// to run it with Proton, which every Windows game would have and every start here does already.
/// gamefix's advice names its commands; here it's `word`: what does it on this game's page.
fn for_the_page(report: &str, word: &str) -> (String, bool) {
    let body = report.split_once("\n\n").map_or(report, |(_, rest)| rest).trim();
    let mut advice = false;
    let blocks: Vec<String> = body
        .split("\n\n")
        .filter_map(|block| {
            let Some(rest) = block.strip_prefix(NO_LINUX) else { return Some(block.to_string()) };
            let kept: Vec<&str> = rest.lines().filter(|l| !l.is_empty() && !l.contains("Steam runs it with Proton. ProtonDB's reports don't name") && !l.contains("that run it better than Steam's own setup") && !l.trim_start().starts_with("Fix: run ")).collect();
            advice = kept.iter().any(|l| l.trim_start().starts_with("Fix: gamefix setup"));
            (!kept.is_empty()).then(|| kept.iter().map(|l| l.trim_start()).collect::<Vec<_>>().join("\n"))
        })
        .collect();
    let text = blocks.join("\n\n");
    let text = if text.is_empty() { "If it still doesn't start, the game has to run to show why: its files look fine.".to_string() } else { text };
    (text.replace(" (--steam: to Steam)", "").replace("gamefix setup --steam", word).replace("gamefix setup", word).replace("Fix: Fix", "Fix"), advice)
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
        Game { id: "g".into(), name: "G".into(), kind: Kind::Game, entries, missing: false, gone: false, added: 0, played: None, chosen_cover: None, launch_options: None, protection: Default::default(), hidden: false, favourite: false, frame_rate: false, proton: None, kept_cover: None, kept_icon: None }
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

    fn for_the_page_fix(report: &str) -> (String, bool) {
        for_the_page(report, "Fix")
    }

    #[test]
    fn the_report_as_the_page_says_it() {
        let head = "G  (/g)\n  Runs:  g.exe (Windows, 64-bit)\n\n";
        // Every Windows game's first problem, with nothing further to say.
        let (text, advice) = for_the_page_fix(&format!("{head}{NO_LINUX}\n    Fix: run g.exe with Proton: gamefix setup adds it to the app menu that way (--steam: to Steam).\n"));
        assert!(text.contains("its files look fine") && !advice);
        let (text, advice) = for_the_page_fix(&format!("{head}{NO_LINUX}\n    Steam runs it with Proton. ProtonDB's reports don't name a Proton or launch options\n    that run it better than Steam's own setup.\n"));
        assert!(text.contains("its files look fine") && !advice);
        // A Proton ProtonDB's reports name is worth the button.
        let (text, advice) = for_the_page_fix(&format!("{head}{NO_LINUX}\n    On ProtonDB it worked with GE for 9 players.\n    Fix: gamefix setup --steam sets it to GE.\n"));
        assert_eq!(text, "On ProtonDB it worked with GE for 9 players.\nFix sets it to GE.");
        assert!(advice);
        // Other problems stay, in the page's words.
        let (text, advice) = for_the_page_fix(&format!("{head}{NO_LINUX}\n    Fix: run g.exe with Proton: gamefix setup adds it to the app menu that way.\n\n✗ It brings its own ddraw.dll.\n    Fix (at launch): WINEDLLOVERRIDES=ddraw=n,b; gamefix setup adds it.\n"));
        assert_eq!(text, "✗ It brings its own ddraw.dll.\n    Fix (at launch): WINEDLLOVERRIDES=ddraw=n,b; Fix adds it.");
        assert!(!advice);
        // A game with no problems keeps gamefix's own sentence.
        let (text, _) = for_the_page_fix("G  (/g)\n\nNo problems found in its files (3 libraries checked).\n");
        assert_eq!(text, "No problems found in its files (3 libraries checked).");
        // How gamefix starts an Unreal game isn't a problem, and a first Play does it.
        let (text, advice) = for_the_page(&format!("{head}{NO_LINUX}\n    Fix: run g.exe with Proton: gamefix setup adds it.\n• g.exe is Unreal's launcher.\n    gamefix setup starts the game's program directly.\n"), "Play");
        assert_eq!(text, "• g.exe is Unreal's launcher.\nPlay starts the game's program directly.");
        assert!(!advice);
    }

    #[test]
    fn a_game_that_works_gets_no_button() {
        let look = |how, fixable, set_up, failed| Look { how, text: String::new(), problems: fixable, fixable, set_up, failed, can_undo: false };
        let steam = || How::Steam(PathBuf::from("/g"));
        let menu = || How::Menu { target: PathBuf::from("/g"), name: "G".into() };
        assert_eq!(look(steam(), false, false, false).button(), None);
        assert_eq!(look(steam(), true, false, false).button(), Some("Fix"));
        assert_eq!(look(menu(), false, true, false).button(), None);
        assert_eq!(look(menu(), true, true, false).button(), None);
        assert_eq!(look(menu(), false, true, true).button(), Some("Set up again"));
        assert_eq!(look(How::FirstPlay(PathBuf::from("/g")), true, false, true).button(), None);
    }

    #[test]
    fn changes_are_told_from_the_lines() {
        assert!(changed(&["✓ Set its launch options".to_string(), "✗ Not this".to_string()]));
        assert!(!changed(&["✗ ProtonDB's reports name nothing better, so nothing was changed.".to_string()]));
    }
}
