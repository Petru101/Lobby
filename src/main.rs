//! lobby: one library of every game on the computer, whichever store or launcher (or none)
//! put it there, and a way to start each one.

mod artwork;
mod drives;
mod free;
mod install;
mod launch;
mod library;
mod log;
mod run;
mod sources;
mod store;
mod uninstall;
mod stores;
mod ui;

use library::{Game, Kind};
use std::process::ExitCode;
use store::Library;

const USAGE: &str = "usage: lobby                   open the window
       lobby scan [--json]     look for games again and list them
       lobby list [--json]     list the games found before (scans the first time)
       lobby play <game>       start a game, by its id or name
       lobby how <game>        say how lobby would start it, without starting it
       lobby install <game>    install a download, or a game you own on Epic or GOG
       lobby accounts          the stores lobby is signed in to (sign in from the window)
       lobby update [<game>]   which of lobby's store installs have updates; update one
       lobby dlc <game> [<dlc>]  the DLC you own for a game lobby installed; install one
       lobby saves <game> [down|up|keep-cloud|keep-local]  sync a store game's cloud saves now
       lobby uninstall <game> [--keep-prefix]  delete a game lobby installed (and its Wine prefix)
       lobby free              games free to keep now and next (claim them from the window)
       lobby take-over-heroic  make the Epic and GOG games Heroic installed lobby's (Heroic can go)";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let json = args.iter().any(|a| a == "--json");
    let rest: Vec<&str> = args.iter().skip(1).filter(|a| !a.starts_with("--")).map(String::as_str).collect();
    if args.is_empty() {
        return match ui::run() {
            c if c == gtk::glib::ExitCode::SUCCESS => ExitCode::SUCCESS,
            _ => ExitCode::FAILURE,
        };
    }
    let result = match (args.first().map(String::as_str), rest.as_slice()) {
        (Some("scan"), []) => scan().map(|lib| show(&lib, json)),
        (Some("list"), []) => Library::load().map_or_else(scan, Ok).map(|lib| show(&lib, json)),
        (Some("play"), words) if !words.is_empty() => play(&words.join(" ")),
        (Some("how"), words) if !words.is_empty() => how(&words.join(" ")),
        (Some("install"), words) if !words.is_empty() => install(&words.join(" ")),
        (Some("accounts"), []) => accounts(),
        (Some("take-over-heroic"), []) => take_over_heroic(),
        (Some("free"), []) => free_games(),
        (Some("uninstall"), words) if !words.is_empty() => uninstall_game(&words.join(" "), args.iter().any(|a| a == "--keep-prefix")),
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
    lib.update(games);
    lib.save()?;
    Ok(lib)
}

fn play(query: &str) -> Result<(), String> {
    let mut lib = Library::load().map_or_else(scan, Ok)?;
    let game = lib.find(query)?.clone();
    let (how, command, cwd) = match launch::plan(&game) {
        launch::Plan::Run { how, command, cwd } => (how, command, cwd),
        launch::Plan::Cannot(why) => return Err(format!("{}: {why}", game.name)),
        launch::Plan::SetUp(folder) => {
            println!("No launcher knows {}, so gamefix sets it up first (once):", game.name);
            let (lines, entry) = launch::set_up(&folder, None)?;
            for l in &lines {
                println!("  {l}");
            }
            let entry = entry.ok_or(format!("{} couldn't be set up, so it can't be started.", game.name))?;
            let (command, cwd) = launch::desktop_exec(&std::fs::read_to_string(&entry).map_err(|e| e.to_string())?).ok_or("gamefix's entry has no command")?;
            // Started through that entry from now on.
            if let Some(g) = lib.get_mut(&game.id) {
                let mut e = library::Entry::new(library::Source::AppMenu, game.name.clone());
                e.keys.push(library::Key::Desktop(entry.clone()));
                g.entries.push(e);
            }
            ("its new gamefix app-menu entry".to_string(), command, cwd)
        }
    };
    let log = store::log_file(&game.id);
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
    println!("Installing {}…", game.name);
    let shown = std::cell::Cell::new(-1);
    let done = install::install(&game, &|p| {
        if p as i32 / 5 != shown.get() {
            shown.set(p as i32 / 5);
            println!("  {p:.0}%");
        }
    })?;
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

fn how(query: &str) -> Result<(), String> {
    let lib = Library::load().map_or_else(scan, Ok)?;
    let game = lib.find(query)?;
    match launch::plan(game) {
        launch::Plan::Run { how, command, cwd } => {
            println!("{} starts with {how}:\n  {}", game.name, command.join(" "));
            if let Some(dir) = cwd {
                println!("  in {}", dir.display());
            }
        }
        launch::Plan::SetUp(folder) => println!("{}: no launcher knows it; gamefix sets up {} first (an app-menu entry), then that starts it.", game.name, folder.display()),
        launch::Plan::Cannot(why) => println!("{}: {why}", game.name),
    }
    for warning in drives::warnings(game, &drives::mounts()) {
        println!("\nWarning: {warning}");
    }
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

fn free_games() -> Result<(), String> {
    let games = free::list()?;
    let epic = stores::epic::owned_items();
    for g in &games {
        let state = if g.from.is_some() { "" } else if free::owned(g, &epic) { "  (in your library)" } else { "  (not claimed)" };
        println!("{}: {}, {}{state}", g.store, g.title, free::when(g));
    }
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
    g.sources().iter().map(|s| s.label()).collect::<Vec<_>>().join(", ")
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
}
