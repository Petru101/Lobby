//! `lobby run <game>`: how a game lobby installed from a store is played. Its cloud saves come
//! down first (when the store keeps them), then the game runs as its gamefix entry runs it
//! (Epic's games with the arguments Epic's launcher passes), lobby waits for it to quit, and
//! its saves go up. Started detached by Play, its output going to the game's log.

use crate::library::Game;
use crate::stores::saves::{self, Direction, Synced};
use crate::stores::Store;
use std::process::{Command, Stdio};

pub fn run(game: &Game) -> Result<(), String> {
    let (store, id) = crate::stores::installed_in(game).ok_or_else(|| format!("{} wasn't installed by lobby from a store.", game.name))?;
    let gamefix = crate::launch::gamefix_command(game).ok_or_else(|| format!("{} has no gamefix setup yet: press Play in lobby's window to set it up.", game.name))?;
    let cloud = saves::supported(store, &id);
    if cloud {
        let down = saves::make_prefix(&gamefix.env, &gamefix.umu).and_then(|()| saves::sync(store, &id, &gamefix.env, Direction::Down));
        match down {
            Ok(Synced::Moved(what)) => println!("lobby: {what}"),
            Ok(Synced::Nothing) => println!("lobby: its cloud saves were up to date."),
            Ok(Synced::Conflict) => match crate::ui::ask_which_saves(&game.name, store.name()) {
                Some(choice) => match saves::sync(store, &id, &gamefix.env, choice) {
                    Ok(_) => println!("lobby: kept {} saves.", if choice == Direction::KeepCloud { "the cloud's" } else { "this computer's" }),
                    Err(e) => notify(&game.name, &e),
                },
                None => println!("lobby: saves changed here and in the cloud; left both as they are."),
            },
            // Offline, or the store turned it away: the game still starts, with the saves here.
            Err(e) => {
                eprintln!("lobby: {e}");
                notify(&game.name, &format!("Its cloud saves couldn't be checked ({e}). It starts with the saves on this computer."));
            }
        }
    }

    let (program, cwd, extra_env) = match store {
        Store::Epic => crate::stores::epic::game_command(&id)?,
        Store::Gog => (gamefix.program.clone(), gamefix.cwd.clone(), Vec::new()),
    };
    let mut command = Command::new(&gamefix.umu);
    command.args(&program).stdin(Stdio::null());
    for (k, v) in gamefix.env.iter().filter_map(|w| w.split_once('=')).map(|(k, v)| (k.to_string(), v.to_string())).chain(extra_env) {
        command.env(k, v);
    }
    if let Some(dir) = cwd.filter(|d| d.is_dir()) {
        command.current_dir(dir);
    }
    // umu-run returns when the game has quit (Proton's waitforexitandrun).
    let status = command.status().map_err(|e| format!("{}: {e}", gamefix.umu))?;
    println!("lobby: {} quit ({status}).", game.name);

    if cloud {
        match saves::sync(store, &id, &gamefix.env, Direction::Up) {
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
