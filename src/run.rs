//! `lobby run <game>`: how a game lobby installed from a store is played. Its cloud saves come
//! down first (when the store keeps them), then the game runs (Epic's with the arguments Epic's
//! launcher passes) in its Wine prefix, with umu's protonfixes for it when umu knows the game,
//! lobby waits for it to quit, and its saves go up. Started detached by Play, its output going
//! to the game's log.

use crate::library::Game;
use crate::stores::saves::{self, Direction, Synced};
use crate::stores::Store;
use std::process::{Command, Stdio};

pub fn run(game: &Game) -> Result<(), String> {
    let (store, id) = crate::stores::installed_in(game).ok_or_else(|| format!("{} wasn't installed by lobby from a store.", game.name))?;
    let wine = crate::launch::wine_command(game).ok_or_else(|| format!("{} isn't set up to start yet: press Play in lobby's window to set it up.", game.name))?;
    // umu's id for the game picks its protonfixes (gamefix's entries say umu-0: none).
    let mut env: Vec<(String, String)> = wine.env.iter().filter_map(|w| w.split_once('=')).map(|(k, v)| (k.to_string(), v.to_string())).collect();
    if env.iter().any(|(k, v)| k == "GAMEID" && v == "umu-0") {
        if let Some(umu_id) = crate::stores::umu_id(store, &id) {
            env.retain(|(k, _)| k != "GAMEID" && k != "STORE");
            env.push(("GAMEID".into(), umu_id));
            env.push(("STORE".into(), if store == Store::Epic { "egs".into() } else { "gog".into() }));
        }
    }
    let env_words: Vec<String> = env.iter().map(|(k, v)| format!("{k}={v}")).collect();
    let cloud = wine.umu.is_some() && saves::supported(store, &id);
    if cloud {
        let down = saves::make_prefix(&env_words, wine.umu.as_deref().unwrap()).and_then(|()| saves::sync(store, &id, &env_words, Direction::Down));
        match down {
            Ok(Synced::Moved(what)) => println!("lobby: {what}"),
            Ok(Synced::Nothing) => println!("lobby: its cloud saves were up to date."),
            Ok(Synced::Conflict) => match crate::ui::ask_which_saves(&game.name, store.name()) {
                Some(choice) => match saves::sync(store, &id, &env_words, choice) {
                    Ok(_) => println!("lobby: kept {} saves.", if choice == Direction::KeepCloud { "the cloud's" } else { "this computer's" }),
                    Err(e) => notify(&game.name, &e),
                },
                None => println!("lobby: saves changed here and in the cloud; left both as they are."),
            },
            // Offline, or the store turned it away: the game still starts, with the saves here.
            Err(e) => notify(&game.name, &format!("Its cloud saves couldn't be checked ({e}). It starts with the saves on this computer.")),
        }
    }

    let (program, cwd, extra_env) = match store {
        Store::Epic => crate::stores::epic::game_command(&id)?,
        Store::Gog => (wine.program.clone(), wine.cwd.clone(), Vec::new()),
    };
    // The user's launch options: variables for the game, words after its arguments.
    let options = crate::launch::parse_options(game.launch_options.as_deref().unwrap_or_default())?;
    let program: Vec<String> = program.into_iter().chain(options.args).collect();
    let words: Vec<String> = wine.umu.iter().cloned().chain(program).collect();
    let env: Vec<(String, String)> = env.into_iter().chain(extra_env).chain(options.vars).collect();
    // In the game's sandbox: the store's sign-in and its saves' sync stay out here.
    let mut sandboxed = crate::sandbox::command(game, &env, &words, cwd.as_deref().filter(|d| d.is_dir()))?;
    println!("lobby: protection: {}", crate::sandbox::summary(game));
    // umu-run returns when the game has quit (Proton's waitforexitandrun).
    let status = sandboxed.command.stdin(Stdio::null()).status().map_err(|e| format!("{}: {e}", words[0]))?;
    println!("lobby: {} quit ({status}).", game.name);

    if cloud {
        match saves::sync(store, &id, &env_words, Direction::Up) {
            Ok(Synced::Moved(what)) => println!("lobby: {what}"),
            Ok(Synced::Nothing) => println!("lobby: no saves to put in the cloud."),
            Ok(Synced::Conflict) => notify(&game.name, "Its saves weren't put in the cloud: they changed there too since the last sync. Lobby asks which to keep when you next play it."),
            Err(e) => notify(&game.name, &format!("Its saves couldn't be put in the cloud: {e}. Lobby tries again after you next play it.")),
        }
    }
    Ok(())
}

/// A desktop notification (the game's window is gone; lobby's may not be open).
fn notify(title: &str, body: &str) {
    eprintln!("lobby: {body}");
    let _ = Command::new("notify-send").args(["-a", "Lobby", title, body]).stdin(Stdio::null()).status();
}
