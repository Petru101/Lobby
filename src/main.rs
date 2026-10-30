//! lobby: one library of every game on the computer, whichever store or launcher (or none)
//! put it there, and a way to start each one.

mod artwork;
mod backups;
mod battlenet;
mod clients;
mod drives;
mod free;
mod import;
mod install;
mod anticheat;
mod apparmor;
mod launch;
mod leftovers;
mod library;
mod log;
mod moving;
mod places;
mod programs;
mod run;
mod running;
mod playtime;
mod sandbox;
mod save_folders;
mod sizes;
mod sources;
mod steam_move;
mod search;
mod starts;
mod store;
mod taken;
mod uninstall;
mod windows_saves;
mod stores;
mod ui;

use library::{Game, Kind};
use std::process::ExitCode;
use store::Library;

const USAGE: &str = "usage: lobby                   open the window
       lobby scan [--json]     look for games again and list them
       lobby list [--json]     list the games found before (scans the first time)
       lobby play <game>       start a game, by its id or name
       lobby how <game> [--json]  say how lobby would start it, without starting it (--json:
                               the exact command and folder)
       lobby install <game>    install a download, or a game you own on Epic or GOG
       lobby accounts          the stores lobby is signed in to (sign in from the window)
       lobby update [<game>]   which of lobby's store installs have updates; update one
       lobby dlc <game> [<dlc>]  the DLC you own for a game lobby installed; install one
       lobby saves <game> [down|up|keep-cloud|keep-local]  sync a store game's cloud saves now
       lobby uninstall <game> [--keep-prefix]  delete a game lobby installed (and its Wine prefix)
       lobby free              games free to keep now and next (claim them from the window)
       lobby free --notify     a notification for new ones, whose Claim button opens the checkout
       lobby free --daily=on|off  the daily look for new ones (on: a systemd user timer)
       lobby claim             the window, at the checkout for the free games not claimed
       lobby windows-saves [<game>] [--copy]  saves found in Windows user folders on the drives;
                               --copy brings a game's into its Wine prefix
       lobby launch-options <game id> [<options>]  show or set what Play adds to its command
                               (like Steam's: -dx11, or VAR=1 %command% -dx11; '' clears them)
       lobby hide|unhide|favourite|unfavourite <game>  out of sight or back; starred or not
       lobby frame-rate <game> [on|off]  MangoHud's overlay (frame rate ...) when Play starts it
       lobby anticheat         games whose online play may not work here (anti-cheat, online only)
       lobby leftovers [--delete <game>]  downloads whose game is installed (with sizes and where to
                               get them again); --delete removes that game's (to the Trash when
                               its drive has one)
       lobby running           the games running now; lobby stop <game> ends one
       lobby move <game> <folder>  move a game there (another drive, say); its records follow
       lobby protection <game> [standard|isolated|off|default] [--internet=on|off] [--share=<folder>] [--unshare=<folder>]
                               how lobby protects a game it starts; change it
       lobby check             what Lobby needs that this computer lacks, and how to install it
       lobby allow-sandbox     on Ubuntu: let lobby make the sandboxes Proton and protection need
                               (an AppArmor profile; sudo asks for your password)
       lobby add <file>        add a game by the program that starts it (or its installer or archive)
       lobby open-drives       open the drives holding games that aren't open (after a restart)
       lobby search            search every drive for games outside the folders lobby looks in
       lobby take-over-heroic  make the Epic and GOG games Heroic installed lobby's (each scan does)
       lobby import [--dry]    move imported games' Wine setups into Lobby's folders (a link left
                               behind) and copy launchers' Proton builds (each scan does)
       lobby own-wine-setup <game>  give a game in a shared Wine setup a copy of its own
       lobby proton <game> [<name> | default] [--anyway]  the Proton it starts with; pick another
                               of Steam's (default: the one it was set up with)
       lobby backups <game> [now | restore [<n>]]  its save backups (newest is 1); back up now;
                               put one back (the saves there now are backed up first)";

fn main() -> ExitCode {
    stores::tools::provide();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let json = args.iter().any(|a| a == "--json");
    let rest: Vec<&str> = args.iter().skip(1).filter(|a| !a.starts_with("--")).map(String::as_str).collect();
    if args.is_empty() {
        return match ui::run(false) {
            c if c == gtk::glib::ExitCode::SUCCESS => ExitCode::SUCCESS,
            _ => ExitCode::FAILURE,
        };
    }
    let result = match (args.first().map(String::as_str), rest.as_slice()) {
        (Some("scan"), []) => scan().map(|lib| show(&lib, json)),
        (Some("list"), []) => Library::load().map_or_else(scan, Ok).map(|lib| show(&lib, json)),
        (Some("play"), words) if !words.is_empty() => play(&words.join(" ")),
        (Some("how"), words) if !words.is_empty() && json => how_json(&words.join(" ")),
        (Some("how"), words) if !words.is_empty() => how(&words.join(" ")),
        (Some("install"), words) if !words.is_empty() => install(&words.join(" ")),
        (Some("accounts"), []) => accounts(),
        (Some("take-over-heroic"), []) => take_over_heroic(),
        (Some("import"), []) => import_now(args.iter().any(|a| a == "--dry")),
        (Some("own-wine-setup"), words) if !words.is_empty() => own_wine_setup(&words.join(" ")),
        (Some("search"), []) => search_drives(),
        (Some("running"), []) => running_games(),
        (Some("move"), [game, into]) => move_game(game, into),
        (Some("stop"), words) if !words.is_empty() => stop_game(&words.join(" ")),
        (Some(what @ ("hide" | "unhide" | "favourite" | "unfavourite")), words) if !words.is_empty() => mark(what, &words.join(" ")),
        (Some("proton"), [game, pick @ ..]) if pick.len() <= 1 => proton(game, pick.first().copied(), args.iter().any(|a| a == "--anyway")),
        (Some("frame-rate"), [game, on @ ..]) if on.len() <= 1 => frame_rate(game, on.first().copied()),
        (Some("anticheat"), []) => list_online_warnings(),
        (Some("leftovers"), []) => list_leftovers(),
        (Some("leftovers"), ["--delete", game @ ..]) if !game.is_empty() => delete_leftovers(&game.join(" ")),
        // The options as typed, -- words too (`rest` leaves those out).
        (Some("launch-options"), [_, ..]) => launch_options(&args[1], &args[2..].join(" "), args.len() > 2),
        (Some("protection"), [game, level @ ..]) if level.len() <= 1 => protection(game, level.first().copied(), &args),
        (Some("check"), []) => check_system(),
        (Some("open-drives"), []) => open_drives(),
        (Some("add"), [file]) => add_game(std::path::Path::new(file)),
        (Some("allow-sandbox"), []) => apparmor::allow(true).map(|()| println!("Lobby may start games now: its AppArmor profile is in /etc/apparmor.d/lobby.")),
        // How Play runs a game protected: `lobby sandbox <id> -- <command>`.
        (Some("sandbox"), _) if args.get(2).is_some_and(|a| a == "--") && args.len() > 3 => return in_sandbox(&args[1], &args[3..]),
        (Some("windows-saves"), words) => windows_saves(&words.join(" "), args.iter().any(|a| a == "--copy")),
        (Some("free"), []) if args.iter().any(|a| a == "--notify") => free_notify(),
        (Some("free"), []) if args.iter().any(|a| a == "--daily=off") => free::set_daily(false),
        (Some("free"), []) if args.iter().any(|a| a == "--daily=on") => free::set_daily(true),
        (Some("free"), []) => free_games(),
        (Some("claim"), []) => {
            return match ui::run(true) {
                c if c == gtk::glib::ExitCode::SUCCESS => ExitCode::SUCCESS,
                _ => ExitCode::FAILURE,
            }
        }
        (Some("uninstall"), words) if !words.is_empty() => uninstall_game(&words.join(" "), args.iter().any(|a| a == "--keep-prefix")),
        (Some("backups"), [game, what @ ..]) if what.len() <= 2 => save_backups(game, what),
        (Some("saves"), [game, way @ ..]) if way.len() <= 1 => sync_saves(game, way.first().copied().unwrap_or("down")),
        // The question `lobby run` asks when saves changed both here and in the cloud (to see it).
        (Some("ask-saves"), words) => {
            println!("{:?}", ui::ask_which_saves(&words.join(" "), "GOG"));
            Ok(())
        }
        // How Play starts a game lobby installed from a store (cloud saves around it).
        (Some("run"), words) if !words.is_empty() => Library::load().ok_or("No library yet: run lobby scan.".to_string()).and_then(|lib| lib.find(&words.join(" ")).cloned()).and_then(|g| run::run(&g)),
        (Some("update"), []) => check_updates(),
        (Some("update"), words) => update(&words.join(" ")),
        (Some("dlc"), [game, name @ ..]) => dlc(game, &name.join(" ")),
        _ => Err(USAGE.to_string()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}

/// Shows a game's launch options, or sets them (empty clears).
/// Moves a game into another folder (on another drive, say), its records following.
fn move_game(query: &str, into: &str) -> Result<(), String> {
    let lib = Library::load().map_or_else(scan, Ok)?;
    let game = lib.find(query)?.clone();
    let shown = std::cell::Cell::new(-1);
    let done = moving::move_game(&game, std::path::Path::new(into), &|p| {
        let pct = (p * 100.0) as i32 / 5 * 5;
        if pct != shown.get() {
            shown.set(pct);
            println!("  {pct}%");
        }
    })?;
    for line in done {
        println!("  {line}");
    }
    scan()?;
    println!("{} is in {} now.", game.name, into);
    Ok(())
}

/// The games running now and what holds each.
fn running_games() -> Result<(), String> {
    let lib = Library::load().map_or_else(scan, Ok)?;
    for (id, pids) in running::find(&lib.games) {
        let name = lib.games.iter().find(|g| g.id == id).map_or(id.as_str(), |g| g.name.as_str());
        println!("{name} ({id}): {}", pids.iter().map(|p| p.to_string()).collect::<Vec<_>>().join(", "));
    }
    Ok(())
}

/// Ends a running game (a hanging one too: what doesn't quit within 5 s is killed).
fn stop_game(query: &str) -> Result<(), String> {
    let lib = Library::load().map_or_else(scan, Ok)?;
    let game = lib.find(query)?;
    let stopped = running::stop(game, &lib.games, std::time::Duration::from_secs(5))?;
    if !stopped.gone {
        return Err(format!("{} didn't stop.", game.name));
    }
    match stopped.held_by {
        Some(holder) => println!("{} stopped; {holder} still holds one of its ended processes (it goes when {holder} lets go).", game.name),
        None => println!("{} stopped.", game.name),
    }
    Ok(())
}

/// Hides a game or brings it back, makes it a favourite or not (the window's star and Hide).
fn mark(what: &str, query: &str) -> Result<(), String> {
    let mut lib = Library::load().map_or_else(scan, Ok)?;
    let id = lib.find(query)?.id.clone();
    let g = lib.get_mut(&id).ok_or("no such game")?;
    match what {
        "hide" => g.hidden = true,
        "unhide" => g.hidden = false,
        "favourite" => g.favourite = true,
        _ => g.favourite = false,
    }
    println!("{}: {}", g.name, if g.hidden { "hidden" } else if g.favourite { "a favourite" } else { "shown" });
    lib.save()
}

/// Every game with something that may keep its online play from working here.
fn list_online_warnings() -> Result<(), String> {
    let lib = Library::load().map_or_else(scan, Ok)?;
    let mut found: Vec<(String, Vec<String>)> = anticheat::warnings(&lib.games).into_iter().filter_map(|(id, w)| Some((lib.games.iter().find(|g| g.id == id)?.name.clone(), w))).collect();
    found.sort();
    for (name, lines) in &found {
        println!("{name}:");
        for l in lines {
            println!("  {l}");
        }
    }
    println!("{} games", found.len());
    Ok(())
}

/// The downloads whose game is installed: what deleting them would free.
fn list_leftovers() -> Result<(), String> {
    let lib = Library::load().map_or_else(scan, Ok)?;
    let found = leftovers::find(&lib.games);
    for l in &found {
        println!("{}: {} ({}; {}{})", l.name, l.path.display(), windows_saves::size_text(l.bytes), l.again.describe(), if l.trash { "" } else { "; no Trash on its drive: deleted for good" });
    }
    println!("{} downloads, {}", found.len(), windows_saves::size_text(found.iter().map(|l| l.bytes).sum()));
    Ok(())
}

fn delete_leftovers(query: &str) -> Result<(), String> {
    let lib = Library::load().map_or_else(scan, Ok)?;
    let game = lib.find(query)?;
    let mine: Vec<_> = leftovers::find(&lib.games).into_iter().filter(|l| l.id == game.id).collect();
    if mine.is_empty() {
        return Err(format!("{} has no download left to delete.", game.name));
    }
    for l in mine {
        let trashed = leftovers::delete(&l.path, &lib.games)?;
        println!("{}: {} {}", game.name, l.path.display(), if trashed { "went to the Trash" } else { "deleted" });
    }
    Ok(())
}

/// Shows whether Play shows a game's frame rate (MangoHud), or turns it on or off.
fn frame_rate(query: &str, on: Option<&str>) -> Result<(), String> {
    let mut lib = Library::load().map_or_else(scan, Ok)?;
    let id = lib.find(query)?.id.clone();
    let g = lib.get_mut(&id).ok_or("no such game")?;
    match on {
        Some("on") => g.frame_rate = true,
        Some("off") => g.frame_rate = false,
        Some(other) => return Err(format!("on or off, not {other}")),
        None => {}
    }
    let why = launch::frame_rate_unsupported(&launch::plan(g));
    println!("{}: frame rate {}{}", g.name, if g.frame_rate { "shown" } else { "not shown" }, why.map(|w| format!(" (Lobby can't show it: {w})")).unwrap_or_default());
    if on.is_some() { lib.save() } else { Ok(()) }
}

/// Shows the game's Proton and the choices, or picks one ("default": the one it was set up
/// with). A pick that may break its Wine setup needs --anyway.
fn proton(query: &str, pick: Option<&str>, anyway: bool) -> Result<(), String> {
    let mut lib = Library::load().map_or_else(scan, Ok)?;
    let game = lib.find(query)?.clone();
    let plan = launch::plan(&game);
    let (own, prefix) = launch::proton_and_prefix(&game, &plan);
    let choices = launch::protons();
    let Some(pick) = pick else {
        if let Some(why) = launch::proton_locked(&game, &plan, &lib.games) {
            println!("{}: {why}", game.name);
            return Ok(());
        }
        let own = own.as_deref().map_or_else(|| "the one gamefix picks when it sets it up".to_string(), launch::proton_name);
        println!("{}: {}", game.name, game.proton.as_deref().map_or_else(|| format!("{own} (as it's set up)"), |p| format!("{} (picked; set up with {own})", launch::proton_name(p))));
        for (name, _) in &choices {
            println!("  {name}");
        }
        return Ok(());
    };
    if let Some(why) = launch::proton_locked(&game, &plan, &lib.games) {
        return Err(format!("{}: {why}", game.name));
    }
    let chosen = match pick {
        "default" => None,
        _ => {
            let low = pick.to_lowercase();
            let exact = choices.iter().find(|(n, _)| n.to_lowercase() == low);
            let like: Vec<_> = choices.iter().filter(|(n, _)| n.to_lowercase().contains(&low)).collect();
            Some(exact.or(match like.as_slice() { [one] => Some(*one), _ => None }).ok_or(format!("No single Proton named like {pick}: lobby proton {query} lists them."))?.1.clone())
        }
    };
    if let Some(risk) = chosen.as_deref().and_then(|to| launch::proton_change_risk(prefix.as_deref(), to)).filter(|_| !anyway) {
        return Err(format!("{risk} Add --anyway to pick it all the same."));
    }
    match backups::take(&game, "before changing its Proton") {
        Ok(Some(b)) => println!("Backed up its saves ({} files) first.", b.files),
        Ok(None) => {}
        Err(e) => println!("Its saves weren't backed up: {e}."),
    }
    if let Some(g) = lib.get_mut(&game.id) {
        g.proton = chosen.clone();
    }
    lib.save()?;
    println!("{}: {}", game.name, chosen.as_deref().map_or_else(|| "back to the Proton it was set up with".to_string(), |p| format!("starts with {}", launch::proton_name(p))));
    Ok(())
}

fn launch_options(query: &str, text: &str, set: bool) -> Result<(), String> {
    let mut lib = Library::load().map_or_else(scan, Ok)?;
    let game = lib.find(query)?.clone();
    if !set {
        println!("{}: {}", game.name, game.launch_options.as_deref().unwrap_or("(none)"));
        if let Some(why) = launch::options_unsupported(&launch::plan(&game)) {
            println!("  ({why})");
        }
        return Ok(());
    }
    let text = text.trim();
    launch::parse_options(text)?;
    if let Some(g) = lib.get_mut(&game.id) {
        g.launch_options = (!text.is_empty()).then(|| text.to_string());
    }
    lib.save()?;
    println!("{}: {}", game.name, if text.is_empty() { "launch options cleared" } else { text });
    Ok(())
}

/// Shows how a game is protected, or changes it.
fn protection(query: &str, level: Option<&str>, args: &[String]) -> Result<(), String> {
    let mut lib = Library::load().map_or_else(scan, Ok)?;
    let id = lib.find(query)?.id.clone();
    let flag = |name: &str| args.iter().find_map(|a| a.strip_prefix(&format!("--{name}=")).map(str::to_string));
    let changing = level.is_some() || flag("internet").is_some() || flag("share").is_some() || flag("unshare").is_some();
    if changing {
        let g = lib.get_mut(&id).unwrap();
        match level {
            Some("default") => g.protection = sandbox::Protection { folders: std::mem::take(&mut g.protection.folders), ..Default::default() },
            Some(l) => g.protection.level = Some(sandbox::Level::parse(l).ok_or(format!("\"{l}\": standard, isolated, off or default."))?),
            None => {}
        }
        match flag("internet").as_deref() {
            Some("on") => g.protection.internet = Some(true),
            Some("off") => g.protection.internet = Some(false),
            Some(other) => return Err(format!("--internet={other}: on or off.")),
            None => {}
        }
        if let Some(f) = flag("share") {
            let f = std::fs::canonicalize(&f).map_err(|e| format!("{f}: {e}"))?;
            if !g.protection.folders.contains(&f) {
                g.protection.folders.push(f);
            }
        }
        if let Some(f) = flag("unshare") {
            let f = std::fs::canonicalize(&f).unwrap_or_else(|_| f.into());
            g.protection.folders.retain(|o| *o != f);
        }
        lib.save()?;
    }
    let game = lib.find(&id)?;
    println!("{}: {}", game.name, sandbox::summary(game));
    if changing && sandbox::sync_shortcut(game)? {
        println!("  its app-menu entry starts it the same way now");
    }
    for f in &game.protection.folders {
        println!("  shared with it: {}", f.display());
    }
    if let Some(why) = launch::protection_unsupported(&launch::plan(game)) {
        println!("  ({why})");
    }
    Ok(())
}

/// Runs a game's command in its sandbox and waits for it; exits as it did.
fn in_sandbox(id: &str, command: &[String]) -> ExitCode {
    // A shortcut naming a game the library lost (deleted, say) still starts it, at Standard.
    let game = Library::load().unwrap_or_default().games.into_iter().find(|g| g.id == id).unwrap_or_else(|| {
        eprintln!("lobby: no game \"{id}\" in the library: starting it with Standard protection.");
        Game { id: id.to_string(), name: id.to_string(), kind: Kind::Game, entries: Vec::new(), missing: false, gone: false, added: 0, played: None, chosen_cover: None, launch_options: None, protection: Default::default(), hidden: false, favourite: false, frame_rate: false, proton: None }
    });
    // The launch options and frame rate, for Play and the shortcuts alike (Play leaves them to
    // this). Not Battle.net's: its options are set in Battle.net. A shortcut still starts when
    // they can't apply.
    let battle_net = command.iter().any(|w| w.contains("Battle.net.exe"));
    // Proton's anti-cheat runtimes, the first time a Proton game starts without them (the
    // window fetches them in the background too).
    if command.iter().any(|w| w.ends_with("umu-run")) {
        if let Err(e) = anticheat::fetch_runtimes() {
            eprintln!("lobby: {e}");
        }
    }
    // An EA game in Lobby's EA app the app doesn't know is installed: registered first.
    if let Some((client, _)) = clients::needed(&game).filter(|(c, _)| c.installed()) {
        match clients::register(&game, client) {
            Ok(true) => println!("lobby: told the {} where {} is installed.", client.name, game.name),
            Ok(false) => {}
            Err(e) => eprintln!("lobby: {e}"),
        }
    }
    let command = match launch::add_extras(&game, command.to_vec()) {
        Ok(c) if !battle_net => c,
        Ok(_) => command.to_vec(),
        Err(e) => {
            eprintln!("lobby: started without its launch options or frame rate: {e}");
            command.to_vec()
        }
    };
    let command = &command[..];
    let start = store::now();
    // Its output goes to its log, unless it already does (Play's): from a shortcut it would
    // go to the journal, and the log is where a failed start's reason is read.
    let log = store::log_file(id);
    let to_log = !same_file_as_stdout(&log);
    let status = sandbox::command(&game, &[], command, None).and_then(|mut s| {
        s.command.stdin(std::process::Stdio::null());
        if to_log {
            let _ = std::fs::create_dir_all(log.parent().unwrap_or(std::path::Path::new("/")));
            if let Ok(out) = std::fs::File::create(&log) {
                if let Ok(err) = out.try_clone() {
                    s.command.stdout(out).stderr(err);
                }
            }
        }
        let mut child = s.command.spawn().map_err(|e| format!("bwrap: {e}"))?;
        // Whether anything of it shows (a GPU driver or display connection loaded), looked at
        // every second until something does.
        let pid = child.id() as i32;
        let shown = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (watching, seen) = (std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true)), shown.clone());
        let w = watching.clone();
        std::thread::spawn(move || {
            while w.load(std::sync::atomic::Ordering::SeqCst) && !seen.load(std::sync::atomic::Ordering::SeqCst) {
                if running::shows_window(pid) {
                    seen.store(true, std::sync::atomic::Ordering::SeqCst);
                }
                std::thread::sleep(std::time::Duration::from_secs(1));
            }
        });
        let status = child.wait().map_err(|e| format!("bwrap: {e}"));
        watching.store(false, std::sync::atomic::Ordering::SeqCst);
        status.map(|st| (st, shown.load(std::sync::atomic::Ordering::SeqCst)))
    });
    let ran = store::now().saturating_sub(start);
    // Timed while it runs: Battle.net's own lifetime isn't a game's.
    if status.is_ok() && !battle_net {
        playtime::session(id, start, store::now());
        backups::after_playing(id);
        // Ended before anything of it showed: a failed start, with the likely reason.
        let failed = matches!(status, Ok((_, false))) && ran < starts::WITHIN;
        let why = failed.then(|| starts::why(command, &std::fs::read_to_string(&log).unwrap_or_default())).flatten();
        starts::record(id, failed.then(|| starts::Failed { when: store::now(), seconds: ran, why: why.clone() }));
        if failed && !window_open() {
            let body = format!("It closed {} without showing anything. {}", starts::after(ran), why.unwrap_or_else(|| "Its page in Lobby has its log.".into()));
            let _ = std::process::Command::new("notify-send").args(["-a", "Lobby", "-i", "dialog-warning", &format!("{} didn't start", game.name), &body]).stdin(std::process::Stdio::null()).status();
        }
    }
    let status = status.map(|(st, _)| st);
    match status {
        Ok(status) => ExitCode::from(status.code().unwrap_or(1).clamp(0, 255) as u8),
        Err(e) => {
            eprintln!("lobby: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Whether this program's output already goes to `file` (Play sends a game's there).
fn same_file_as_stdout(file: &std::path::Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    let (Ok(out), Ok(f)) = (std::fs::metadata("/proc/self/fd/1"), std::fs::metadata(file)) else { return false };
    out.dev() == f.dev() && out.ino() == f.ino()
}

/// Lobby's window is open (a `lobby` with no command): it says a failed start itself.
fn window_open() -> bool {
    std::fs::read_dir("/proc").into_iter().flatten().flatten().any(|e| {
        let args = std::fs::read(e.path().join("cmdline")).unwrap_or_default();
        let words: Vec<&[u8]> = args.split(|&b| b == 0).filter(|w| !w.is_empty()).collect();
        words.len() == 1 && words[0].ends_with(b"/lobby")
    })
}

/// Saves found in Windows user folders: every game's (no game named), or one game's, brought
/// into its prefix with --copy.
fn windows_saves(query: &str, copy: bool) -> Result<(), String> {
    use windows_saves::{size_text, Saves};
    let lib = Library::load().map_or_else(scan, Ok)?;
    let games: Vec<&Game> = if query.is_empty() { lib.games.iter().filter(|g| g.kind == Kind::Game && !g.gone).collect() } else { vec![lib.find(query)?] };
    if query.is_empty() {
        println!("Windows user folders: {}", windows_saves::user_folders().iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(", "));
    }
    for game in games {
        let line = |f: &windows_saves::Found| format!("{} file{}, {} in {}; {} to bring over", f.files, if f.files == 1 { "" } else { "s" }, size_text(f.bytes), f.windows.display(), f.copies.len());
        match windows_saves::find(game) {
            Saves::None if query.is_empty() => {}
            Saves::None => println!("{}: no saves of it found in a Windows user folder.", game.name),
            Saves::Cloud(cloud, f) => println!("{}: {}; {cloud} keeps them, so they come back by themselves.", game.name, line(&f)),
            Saves::NoPrefix(f) => println!("{}: {}; its Wine prefix doesn't exist yet (play it once first).", game.name, line(&f)),
            Saves::Found { found, prefix } => {
                println!("{}: {} ({})", game.name, line(&found), prefix.display());
                if copy && !query.is_empty() {
                    let (copied, replaced) = windows_saves::bring_over(&game.id, &found)?;
                    println!("  copied {copied}, {replaced} of them replacing older ones (kept in {})", store::dir().join("save-backups").join(&game.id).display());
                }
            }
        }
    }
    Ok(())
}

pub fn sha256_hex(data: &[u8]) -> String {
    use sha2::Digest;
    sha2::Sha256::digest(data).iter().map(|b| format!("{b:02x}")).collect()
}

/// Every game the sources find, merged (no saved library involved: safe off the main thread).
/// Store lists are as last saved: `stores::refresh_stale` brings them up to date (the window
/// does that after showing the scan, as it takes a while).
pub fn find_games() -> Result<Vec<Game>, String> {
    let home = sources::Home::find().ok_or("lobby: no home folder")?;
    let found = sources::scan(&home);
    let mut games = log::timed("scan: merging", || library::merge(found.entries, &found.roots));
    log::timed("scan: covers", || artwork::fill(&home, &mut games));
    Ok(games)
}

fn scan() -> Result<Library, String> {
    log::timed("scan: store lists and updates", stores::refresh_stale);
    let games = find_games()?;
    let mut lib = Library::load().unwrap_or_default();
    log::timed("scan: matching the saved library", || lib.update(games));
    lib.save()?;
    playtime::take_in(&lib.games);
    // Lutris's, Faugus's and Steam shortcuts' games become Lobby's own: looked at again so they
    // show as such.
    if !log::timed("scan: taking games over", || taken::take_over(&lib.games)).is_empty() {
        return scan();
    }
    // What imported games need becomes Lobby's (their Wine prefixes, launchers' Proton builds).
    if log::timed("scan: importing", || import::run(&lib.games, false)).iter().any(|l| l.starts_with('✓')) {
        return scan();
    }
    log::timed("scan: app-menu entries' protection", || sandbox::sync_shortcuts(&lib.games));
    Ok(lib)
}

/// What Lobby needs that this computer lacks (Steam, programs, Ubuntu's permission), with how to
/// install it.
fn check_system() -> Result<(), String> {
    let missing = programs::missing();
    let steam = programs::steam_missing();
    if let Some(how) = &steam {
        println!("✗ Steam isn't installed. Lobby needs it: Steam's games, and the Protons Windows games run with.\n  Install it: {how}");
    }
    if apparmor::blocked() {
        println!("✗ {}\n  Allow Lobby: lobby allow-sandbox", apparmor::WHY);
    }
    for p in &missing {
        println!("{} {} isn't installed: it {}.", if p.essential { "✗" } else { "-" }, p.name, p.what);
    }
    if !missing.is_empty() {
        println!("  Install {}: {}", if missing.len() == 1 { "it" } else { "them" }, programs::install_command(&missing));
    }
    if missing.is_empty() && steam.is_none() && !apparmor::blocked() {
        println!("✓ Everything Lobby uses is here.");
    }
    Ok(())
}

fn play(query: &str) -> Result<(), String> {
    if apparmor::blocked() {
        return Err(format!("{} Allow Lobby first: run `lobby allow-sandbox` (sudo asks for your password), or press Play in Lobby's window.", apparmor::WHY));
    }
    let mut lib = Library::load().map_or_else(scan, Ok)?;
    let game = lib.find(query)?.clone();
    let plan = match launch::plan(&game) {
        launch::Plan::Run { how, command, cwd, protect } => (how, command, cwd, protect),
        launch::Plan::Cannot(why) => return Err(format!("{}: {why}", game.name)),
        launch::Plan::NeedsClient { client, .. } => {
            let client = clients::by_id(client).ok_or("unknown store app")?;
            println!("{} only starts with the {}. Installing it (its installer opens: follow it, then sign in)…", game.name, client.name);
            client.install()?;
            return play(query);
        }
        launch::Plan::BattleNet { folder, launch } => {
            if !battlenet::installed() {
                println!("{} starts through Blizzard's Battle.net app. Installing it (its installer opens: follow it, then sign in to Battle.net)…", game.name);
                battlenet::install()?;
            }
            let log = store::log_file(&game.id);
            if let Some(g) = lib.get_mut(&game.id) {
                if sandbox::settle(g) {
                    lib.save()?;
                }
            }
            battlenet::start(lib.find(&game.id)?, &folder, &launch, &log)?;
            println!("Starting {} with Battle.net. Its output goes to {}", game.name, log.display());
            if let Some(g) = lib.get_mut(&game.id) {
                g.played = Some(store::now());
            }
            return lib.save();
        }
        launch::Plan::SetUp(folder) => {
            println!("gamefix sets {} up first (once):", game.name);
            ask_need(&game.name, &folder)?;
            let (lines, entry) = launch::set_up(&folder, Some(&game.name))?;
            for l in &lines {
                println!("  {l}");
            }
            let entry = entry.ok_or(format!("{} couldn't be set up, so it can't be started.", game.name))?;
            match windows_saves::from_launcher(&game, &entry) {
                Ok(0) => {}
                Ok(n) => println!("Brought {n} save files over from where it ran before."),
                Err(e) => eprintln!("lobby: its saves couldn't be brought over: {e}"),
            }
            let (command, cwd) = launch::desktop_exec(&std::fs::read_to_string(&entry).map_err(|e| e.to_string())?).ok_or("gamefix's entry has no command")?;
            // Started through that entry from now on.
            if let Some(g) = lib.get_mut(&game.id) {
                let mut e = library::Entry::new(library::Source::AppMenu, game.name.clone());
                e.keys.push(library::Key::Desktop(entry.clone()));
                g.entries.push(e);
            }
            ("its new gamefix app-menu entry".to_string(), command, cwd, true)
        }
    };
    let (how, command, cwd, protect) = plan;
    // A store app's game in Lobby's prefix of the app for the first time: its saves come too.
    if let Some((client, _)) = clients::needed(&game).filter(|(c, _)| c.installed()) {
        match clients::bring_saves(&game, client) {
            Ok(0) => {}
            Ok(n) => println!("Brought {n} save files of {} over from where it ran before.", game.name),
            Err(e) => eprintln!("lobby: its saves couldn't be brought over: {e}"),
        }
    }
    // The protection it starts with is kept (a default can change once it's been played).
    if let Some(g) = lib.get_mut(&game.id) {
        if sandbox::settle(g) {
            lib.save()?;
        }
    }
    // Its app-menu shortcut too (one gamefix just made, or one from before protection).
    if let Err(e) = sandbox::sync_shortcut(lib.find(&game.id)?) {
        eprintln!("lobby: its app-menu entry couldn't be protected: {e}");
    }
    let log = store::log_file(&game.id);
    let command = launch::final_command(&game, command, protect)?;
    launch::start(&command, cwd.as_deref(), &log)?;
    println!("Starting {} with {how}. Its output goes to {}", game.name, log.display());
    if let Some(g) = lib.get_mut(&game.id) {
        g.played = Some(store::now());
    }
    lib.save()
}

fn install(query: &str) -> Result<(), String> {
    let lib = Library::load().map_or_else(scan, Ok)?;
    let game = lib.find(query)?.clone();
    if !matches!(game.kind, Kind::Download | Kind::Owned) {
        return Err(format!("{} isn't a download or an owned game waiting to be installed.", game.name));
    }
    if let Some(appid) = install::steam_owned(&game) {
        install::install_with_steam(&game, appid)?;
        println!("Steam's install window for {} is opening; once Steam has it, `lobby scan` finds it.", game.name);
        return Ok(());
    }
    println!("Installing {}…", game.name);
    let placed = install::place(&game, &progress_printer())?;
    ask_need(&game.name, &placed.target)?;
    let done = install::set_up(placed);
    for l in &done.lines {
        println!("  {l}");
    }
    // Found as installed from now on.
    scan()?;
    match done.entry {
        Some(_) => println!("{} is installed: lobby play \"{}\" starts it.", game.name, game.name),
        None => println!("{} couldn't be set up to start (see above).", game.name),
    }
    Ok(())
}

/// The exact command Play runs (launch options and sandbox included) and its folder, as JSON.
fn how_json(query: &str) -> Result<(), String> {
    let lib = Library::load().map_or_else(scan, Ok)?;
    let game = lib.find(query)?;
    let (command, cwd) = match launch::plan(game) {
        launch::Plan::Run { command, cwd, protect, .. } => (launch::final_command(game, command, protect)?, cwd),
        launch::Plan::BattleNet { launch, .. } => (sandbox::wrap(game, battlenet::command(&launch)), None),
        launch::Plan::SetUp(folder) => return Err(format!("{}: gamefix sets {} up first.", game.name, folder.display())),
        launch::Plan::NeedsClient { client, .. } => return Err(format!("{}: the {} isn't installed yet (Play installs it).", game.name, clients::by_id(client).map_or("store app", |c| c.name))),
        launch::Plan::Cannot(why) => return Err(why),
    };
    println!("{}", serde_json::json!({ "command": command, "cwd": cwd }));
    Ok(())
}

fn how(query: &str) -> Result<(), String> {
    let lib = Library::load().map_or_else(scan, Ok)?;
    let game = lib.find(query)?;
    match launch::plan(game) {
        launch::Plan::Run { how, command, cwd, protect } => {
            println!("{} starts with {how}:\n  {}", game.name, command.join(" "));
            if let Some(p) = &game.proton {
                println!("  with {} instead of its own Proton (picked on its page)", launch::proton_name(p));
            }
            match protect {
                true => println!("  protected: {}", sandbox::summary(game)),
                false => println!("  not protected: {}", launch::protection_unsupported(&launch::plan(game)).unwrap_or_default()),
            }
            if let Some(dir) = cwd {
                println!("  in {}", dir.display());
            }
        }
        launch::Plan::SetUp(folder) => println!("{}: no launcher knows it; gamefix sets up {} first (an app-menu entry), then that starts it.\n  protected: {}", game.name, folder.display(), sandbox::summary(game)),
        launch::Plan::BattleNet { folder, launch } => {
            if !battlenet::installed() {
                println!("{}: a Blizzard game; Battle.net isn't installed yet: lobby installs it into {} first.", game.name, battlenet::prefix().display());
            } else if !battlenet::knows(&folder) {
                println!("{}: Battle.net doesn't know it's in {} yet: lobby adds it (closing Battle.net first if it's open).", game.name, folder.display());
            }
            println!("{} starts with Battle.net:\n  {}\n  protected: {}", game.name, battlenet::command(&launch).join(" "), sandbox::summary(game));
        }
        launch::Plan::NeedsClient { client, program, .. } => {
            let c = clients::by_id(client);
            println!("{}: only starts with the {}; Lobby installs it into {} first, then runs {} there.", game.name, c.map_or("store app", |c| c.name), c.map(|c| c.prefix().display().to_string()).unwrap_or_default(), program.display());
        }
        launch::Plan::Cannot(why) => println!("{}: {why}", game.name),
    }
    if let Some(o) = &game.launch_options {
        println!("  launch options: {o}");
    }
    if game.frame_rate {
        println!("  frame rate: {}", launch::frame_rate_unsupported(&launch::plan(game)).map_or("shown (MangoHud)".to_string(), |why| format!("can't be shown: {why}")));
    }
    for warning in drives::warnings(game, &drives::mounts()) {
        println!("\nWarning: {warning}");
    }
    for warning in anticheat::warnings(std::slice::from_ref(game)).remove(&game.id).unwrap_or_default() {
        println!("\nOnline play: {warning}");
    }
    if let Some(f) = starts::failed(&game.id) {
        println!("\nLast start failed: it ended {} without showing anything. {}\n  log: {}", starts::after(f.seconds), f.why.unwrap_or_default(), store::log_file(&game.id).display());
    }
    if game.kind != Kind::Game {
        return Ok(());
    }
    match moving::plan(game) {
        Ok(p) => println!("Move: {} {} can go to another folder or drive", if p.prefix { "its Windows setup (Wine prefix) holding it," } else { "its folder," }, p.from.display()),
        Err(why) => println!("Move: {why}"),
    }
    for f in save_folders::find(game) {
        println!("{}: {}", f.what, f.path.display());
    }
    println!("Uninstall: {}", match uninstall::how(game) {
        uninstall::How::Store(store, _) => format!("{}'s files go, through lobby's {} tool", game.name, store.name()),
        uninstall::How::Steam(id) => format!("Steam uninstalls it (app {id})"),
        uninstall::How::SteamCopy(f, _) => format!("deletes the copy in {}", f.display()),
        uninstall::How::Folder(f) => format!("deletes {} (lobby or gamefix put it there)", f.display()),
        uninstall::How::Found(f) => format!("deletes {} (found on your drive)", f.display()),
        uninstall::How::Not(why) => why,
    });
    Ok(())
}

/// Prints a store job's progress every 5%.
fn progress_printer() -> impl Fn(f64) {
    let shown = std::cell::Cell::new(-1);
    move |p| {
        if p as i32 / 5 != shown.get() {
            shown.set(p as i32 / 5);
            println!("  {p:.0}%");
        }
    }
}

fn check_updates() -> Result<(), String> {
    for store in stores::Store::ALL.into_iter().filter(|s| s.account().is_some()) {
        let names = store.installed();
        let found = match log::timed(&format!("update check: {}", store.name()), || store.check_updates()) {
            Ok(found) => found,
            Err(e) => {
                println!("{}: {e}", store.name());
                continue;
            }
        };
        for u in &found {
            let title = names.iter().find(|g| g.id == u.id).map_or(u.id.as_str(), |g| g.title.as_str());
            println!("{}: {title} {} → {}", store.name(), u.from, u.to);
        }
        if found.is_empty() {
            println!("{}: everything lobby installed is up to date", store.name());
        }
    }
    scan().map(|_| ())
}

fn update(query: &str) -> Result<(), String> {
    let lib = Library::load().map_or_else(scan, Ok)?;
    let game = lib.find(query)?.clone();
    let (store, id) = stores::installed_in(&game).ok_or_else(|| format!("{} wasn't installed by lobby from a store.", game.name))?;
    println!("Updating {}…", game.name);
    store.update(&id, &progress_printer())?;
    println!("{} is up to date.", game.name);
    scan().map(|_| ())
}

fn dlc(query: &str, name: &str) -> Result<(), String> {
    let lib = Library::load().map_or_else(scan, Ok)?;
    let game = lib.find(query)?.clone();
    let (store, id) = stores::installed_in(&game).ok_or_else(|| format!("{} wasn't installed by lobby from a store.", game.name))?;
    let list = store.dlc(&id)?;
    if name.is_empty() {
        for d in &list {
            println!("  {}{}", d.title, if d.installed { "  (installed)" } else { "" });
        }
        if list.is_empty() {
            println!("You own no DLC for {}.", game.name);
        }
        return Ok(());
    }
    let wanted = list.iter().find(|d| d.title.to_lowercase().contains(&name.to_lowercase())).ok_or_else(|| format!("No DLC you own for {} is called {name}.", game.name))?;
    println!("Installing {}…", wanted.title);
    store.install_dlc(&id, std::slice::from_ref(&wanted.id), &progress_printer())?;
    println!("{} is installed.", wanted.title);
    Ok(())
}

fn sync_saves(query: &str, way: &str) -> Result<(), String> {
    use stores::saves::{self, Direction, Synced};
    let lib = Library::load().map_or_else(scan, Ok)?;
    let game = lib.find(query)?.clone();
    let (store, id) = stores::installed_in(&game).ok_or_else(|| format!("{} wasn't installed by lobby from a store.", game.name))?;
    if !saves::supported(store, &id) {
        println!("{} doesn't keep {}'s saves in {}'s cloud.", store.name(), game.name, store.name());
        return Ok(());
    }
    let wine = launch::wine_command(&game).ok_or("It isn't set up to start yet: play it once first.")?;
    let umu = wine.umu.as_deref().ok_or("It's a Linux version: lobby syncs only Windows versions' saves.")?;
    saves::make_prefix(&wine.env, umu)?;
    let direction = match way {
        "down" => Direction::Down,
        "up" => Direction::Up,
        "keep-cloud" => Direction::KeepCloud,
        "keep-local" => Direction::KeepLocal,
        _ => return Err(USAGE.into()),
    };
    match saves::sync(store, &id, &wine.env, direction)? {
        Synced::Moved(what) => println!("{what}"),
        Synced::Nothing => println!("Nothing to move: the saves are the same here and in the cloud{}.", if direction == Direction::Up { ", or newer in the cloud" } else { ", or newer here" }),
        Synced::Conflict => println!("The saves changed here and in the cloud since the last sync: lobby saves {query:?} keep-cloud (or keep-local), or play it from lobby's window to choose."),
    }
    Ok(())
}

fn uninstall_game(query: &str, keep_prefix: bool) -> Result<(), String> {
    let lib = Library::load().map_or_else(scan, Ok)?;
    let game = lib.find(query)?.clone();
    println!("Uninstalling {}…", game.name);
    for l in uninstall::uninstall(&game, &lib.games, !keep_prefix)? {
        println!("  {l}");
    }
    scan().map(|_| ())
}

/// A notification for free games not announced before; its Claim button opens lobby at the
/// checkout (`lobby claim`). Waits for the notification to be answered or closed.
fn free_notify() -> Result<(), String> {
    let games = free::list()?;
    let epic = stores::epic::owned_items();
    let new = free::not_announced(&games, &epic);
    if new.is_empty() {
        return Ok(());
    }
    let body: Vec<String> = new.iter().map(|g| format!("{} ({}, {})", g.title, g.store, free::when(g))).collect();
    let epic_ones = new.iter().any(|g| g.store == stores::Store::Epic.name());
    let action = if epic_ones { "claim=Claim" } else { "claim=Show" };
    let out = std::process::Command::new("notify-send").args(["-a", "Lobby", "-i", "applications-games", "-A", action, "-w", "Free to keep", &body.join("\n")]).stdin(std::process::Stdio::null()).output().map_err(|e| e.to_string())?;
    if String::from_utf8_lossy(&out.stdout).trim() == "claim" {
        let lobby = std::env::current_exe().map_err(|e| e.to_string())?;
        std::os::unix::process::CommandExt::process_group(&mut std::process::Command::new(lobby), 0).arg("claim").stdin(std::process::Stdio::null()).spawn().map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn free_games() -> Result<(), String> {
    let games = free::list()?;
    let epic = stores::epic::owned_items();
    for g in &games {
        let state = if g.from.is_some() { "" } else if free::owned(g, &epic) { "  (in your library)" } else { "  (not claimed)" };
        println!("{}: {}, {}{state}", g.store, g.title, free::when(g));
    }
    Ok(())
}

/// Prints what the search of all drives finds (nothing is added: the window's search adds).
fn search_drives() -> Result<(), String> {
    let home = sources::Home::find().ok_or("no home folder")?;
    let lib = Library::load().unwrap_or_default();
    let places = places::Places::load();
    let mut skip: Vec<std::path::PathBuf> = places.games.iter().chain(&places.dismissed).cloned().collect();
    skip.extend(lib.games.iter().flat_map(|g| g.entries.iter().filter_map(|e| e.folder.clone())));
    skip.extend(sources::folders::roots(&home));
    let seen = std::sync::atomic::AtomicUsize::new(0);
    let found = log::timed("search of all drives", || search::search(&search::starts(&home), &skip, &seen, &std::sync::atomic::AtomicBool::new(false)));
    println!("Looked through {} folders.", seen.into_inner());
    for f in &found.games {
        println!("  {} ({})", f.folder.display(), f.why);
    }
    for l in &found.libraries {
        println!("  Steam library: {}", l.display());
    }
    Ok(())
}

/// What a game needs to run before gamefix sets it up (Java, a runner from Flathub): asked on
/// the terminal, installed on yes.
fn ask_need(name: &str, target: &std::path::Path) -> Result<(), String> {
    let Some(need) = launch::setup_need(target) else { return Ok(()) };
    let (what, size) = need.describe();
    let size = size.map(|s| format!(" ({s} to download)")).unwrap_or_default();
    print!("{name} needs {what}, which isn't installed. Install it{size} for you only? [y/N] ");
    let _ = std::io::Write::flush(&mut std::io::stdout());
    let mut answer = String::new();
    let yes = std::io::IsTerminal::is_terminal(&std::io::stdin()) && std::io::stdin().read_line(&mut answer).is_ok() && matches!(answer.trim().to_lowercase().as_str(), "y" | "yes");
    if yes {
        println!("Installing {what}…");
        need.install()?;
    } else {
        println!();
    }
    Ok(())
}

fn save_backups(query: &str, what: &[&str]) -> Result<(), String> {
    let lib = Library::load().map_or_else(scan, Ok)?;
    let game = lib.find(query)?;
    let list = backups::list(&game.id);
    match what {
        [] => {
            if list.is_empty() {
                println!("{}: no save backups yet.", game.name);
            }
            for (i, b) in list.iter().enumerate() {
                println!("{:>3}  {}  {} file{}, {}  ({})", i + 1, backups::time_text(b.when), b.files, if b.files == 1 { "" } else { "s" }, windows_saves::size_text(b.bytes), b.why);
                for f in &b.folders {
                    println!("       {}", f.from.display());
                }
            }
            Ok(())
        }
        ["now"] => match backups::take(game, "backed up by hand")? {
            Some(b) => {
                println!("Backed up {} files of {}'s saves in {}.", b.files, game.name, b.dir.display());
                Ok(())
            }
            None if list.is_empty() => Err(format!("{}: no saves found here to back up.", game.name)),
            None => {
                println!("{}'s saves haven't changed since the newest backup.", game.name);
                Ok(())
            }
        },
        ["restore", n @ ..] => {
            let n: usize = n.first().map_or(Ok(1), |n| n.parse()).map_err(|_| "restore takes a backup's number (1 is the newest)")?;
            let b = list.get(n.wrapping_sub(1)).ok_or(format!("{} has no backup {n}.", game.name))?;
            let count = backups::restore(game, b)?;
            println!("Put back {count} files of {}'s saves from backup {n}.", game.name);
            Ok(())
        }
        _ => Err(USAGE.to_string()),
    }
}

/// Makes what imported games need Lobby's own now (--dry: says what it would do).
fn import_now(dry: bool) -> Result<(), String> {
    let lib = Library::load().map_or_else(scan, Ok)?;
    let done = import::run(&lib.games, dry);
    if done.is_empty() {
        println!("Everything imported games need is Lobby's already.");
    }
    for l in &done {
        println!("  {l}");
    }
    if !dry && done.iter().any(|l| l.starts_with('✓')) {
        scan()?;
    }
    Ok(())
}

fn own_wine_setup(query: &str) -> Result<(), String> {
    let lib = Library::load().map_or_else(scan, Ok)?;
    let game = lib.find(query)?;
    let to = import::own_prefix(game, &lib.games)?;
    println!("{} has a Wine setup of its own now: {} (a copy of the shared one; its Proton can be picked).", game.name, to.display());
    Ok(())
}

fn take_over_heroic() -> Result<(), String> {
    let home = sources::Home::find().ok_or("no home folder")?;
    let done = stores::takeover::take_over(&home)?;
    if done.is_empty() {
        println!("Heroic has no Epic or GOG games lobby hasn't taken over.");
        return Ok(());
    }
    println!("Lobby took over {} games from Heroic:", done.len());
    for l in &done {
        println!("  ✓ {l}");
    }
    scan().map(|_| ())
}

fn accounts() -> Result<(), String> {
    for store in stores::Store::ALL {
        match store.account() {
            Some(name) => println!("{}: signed in{}; {} games owned", store.name(), if name.is_empty() { String::new() } else { format!(" as {name}") }, stores::owned(store).len()),
            None => println!("{}: not signed in (sign in from lobby's window: Accounts)", store.name()),
        }
    }
    Ok(())
}

fn where_found(g: &Game) -> String {
    g.found_in()
}

fn show(lib: &Library, json: bool) {
    if json {
        println!("{}", serde_json::to_string_pretty(&lib.games).unwrap());
        return;
    }
    let sections: [(&str, Box<dyn Fn(&Game) -> bool>); 8] = [
        ("Games", Box::new(|g: &Game| g.kind == Kind::Game && !g.missing && !g.gone && !g.unknown_to_launchers())),
        ("Games no launcher knows about", Box::new(|g: &Game| g.kind == Kind::Game && !g.missing && !g.gone && g.unknown_to_launchers())),
        ("Downloaded, not installed yet", Box::new(|g: &Game| g.kind == Kind::Download && !g.missing && !g.gone)),
        ("Downloads for another system", Box::new(|g: &Game| g.kind == Kind::WrongSystem && !g.missing && !g.gone)),
        ("Owned in a store, not installed", Box::new(|g: &Game| g.kind == Kind::Owned && !g.gone)),
        ("Store apps", Box::new(|g: &Game| g.kind == Kind::StoreApp && !g.missing && !g.gone)),
        ("Listed somewhere, but not on disk", Box::new(|g: &Game| g.missing && !g.gone)),
        ("Not found any more (uninstalled, or on a drive that isn't plugged in)", Box::new(|g: &Game| g.gone)),
    ];
    for (title, wanted) in sections {
        let list: Vec<&Game> = lib.games.iter().filter(|g| wanted(g)).collect();
        if list.is_empty() {
            continue;
        }
        println!("\n{title} ({})", list.len());
        let width = list.iter().map(|g| g.name.chars().count()).max().unwrap_or(0).min(40);
        for g in list {
            let name: String = g.name.chars().take(40).collect();
            println!("  {name:width$}  {}  [{}]", where_found(g), g.id);
            println!("  {:width$}  {}", "", g.program().or(g.folder()).map(|p| p.display().to_string()).unwrap_or_default());
        }
    }
    let closed = drives::closed_with_games(&lib.games);
    if !closed.is_empty() {
        println!("\n{}. Open {}: lobby open-drives", drives::closed_text(&closed), if closed.len() == 1 { "it" } else { "them" });
    }
}

/// Adds a game by its program, then looks again and says what it became.
fn add_game(file: &std::path::Path) -> Result<(), String> {
    let file = std::fs::canonicalize(file).map_err(|e| format!("{}: {e}", file.display()))?;
    if !file.is_file() {
        return Err(format!("{} isn't a file: pick the program that starts the game", file.display()));
    }
    let mut places = places::Places::load();
    places.add_program(&file);
    places.save()?;
    let lib = scan()?;
    match lib.games.iter().find(|g| g.entries.iter().any(|e| e.program.as_deref() == Some(file.as_path()))) {
        Some(g) if g.kind == Kind::Download => println!("Added {} [{}] to Downloads: lobby install {} installs it.", g.name, g.id, g.id),
        Some(g) => println!("Added {} [{}]: lobby play {} starts it.", g.name, g.id, g.id),
        None => println!("Saved, but no game came of it."),
    }
    Ok(())
}

/// Opens the drives holding saved games that aren't open, then looks again.
fn open_drives() -> Result<(), String> {
    let lib = Library::load().map_or_else(scan, Ok)?;
    let closed = drives::closed_with_games(&lib.games);
    if closed.is_empty() {
        println!("No drive with games on it is closed.");
        return Ok(());
    }
    for (d, _) in &closed {
        drives::open(d)?;
        println!("Opened the {} drive.", d.name);
    }
    show(&scan()?, false);
    Ok(())
}
