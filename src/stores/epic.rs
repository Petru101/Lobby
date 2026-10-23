//! Epic Games through legendary, with its config in ~/.config/lobby/legendary
//! (LEGENDARY_CONFIG_PATH): lobby's sign-in, lobby's installs.

use super::{config_dir, csv_fields, output, read_json, run_with_progress, tools, Dlc, Installed, Owned, Update};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Epic's sign-in, ending on a page with the authorization code legendary signs in with
/// (legendary's own client id).
pub const SIGN_IN: &str = "https://www.epicgames.com/id/login?redirectUrl=https%3A%2F%2Fwww.epicgames.com%2Fid%2Fapi%2Fredirect%3FclientId%3D34a02cf8f4414e29b15921876da36f9a%26responseType%3Dcode";

pub fn config() -> PathBuf {
    config_dir("legendary")
}

fn legendary() -> Result<Command, String> {
    let mut command = Command::new(tools::LEGENDARY.path()?);
    command.env("LEGENDARY_CONFIG_PATH", config());
    Ok(command)
}

pub fn account() -> Option<String> {
    let user: serde_json::Value = read_json(&config().join("user.json"));
    user.get("account_id")?;
    Some(user.get("displayName").and_then(|n| n.as_str()).unwrap_or_default().to_string())
}

pub fn sign_in(code: &str) -> Result<(), String> {
    let mut command = legendary()?;
    command.args(["auth", "--code", code]);
    output(command).map(|_| ()).map_err(|e| format!("Epic didn't accept the sign-in: {e}"))
}

pub fn sign_out() -> Result<(), String> {
    let mut command = legendary()?;
    command.args(["auth", "--delete"]);
    output(command).map(|_| ())
}

/// The account's games, with the tall box art Epic's own launcher shows. Tried twice: the
/// first list after signing in fetches every item's details, and Epic's service sometimes
/// takes longer than legendary waits (10 s).
pub fn owned() -> Result<Vec<Owned>, String> {
    let list = || {
        let mut command = legendary()?;
        command.args(["list", "--json"]);
        output(command)
    };
    let text = list().or_else(|_| list()).map_err(|e| format!("Epic didn't send the list of your games: {e}"))?;
    let list: serde_json::Value = serde_json::from_str(&text).map_err(|e| format!("legendary list: {e}"))?;
    Ok(parse_owned(&list, &saved_items()))
}

/// Every item legendary has details of (metadata/<app>.json, one per owned item, DLC too:
/// its list leaves DLC out).
fn saved_items() -> Vec<serde_json::Value> {
    std::fs::read_dir(config().join("metadata")).into_iter().flatten().flatten().filter_map(|e| serde_json::from_str(&std::fs::read_to_string(e.path()).ok()?).ok()).collect()
}

/// Games, with the DLC owned for each: add-ons naming their game
/// (metadata.mainGameItem.releaseInfo[].appId), among all the items owned.
fn parse_owned(list: &serde_json::Value, items: &[serde_json::Value]) -> Vec<Owned> {
    let mut games: Vec<Owned> = list
        .as_array()
        .into_iter()
        .flatten()
        .filter(|g| is_game(g))
        .filter_map(|g| {
            let id = g.get("app_name")?.as_str()?.to_string();
            let title = g.get("app_title")?.as_str()?.to_string();
            let images = g.get("metadata").and_then(|m| m.get("keyImages")).and_then(|k| k.as_array());
            let image = |kind: &str| images.into_iter().flatten().find(|i| i.get("type").and_then(|t| t.as_str()) == Some(kind)).and_then(|i| i.get("url")?.as_str().map(String::from));
            let dlc = items.iter().filter(|d| main_game(d).as_deref() == Some(id.as_str())).filter_map(|d| Some(Dlc { id: d.get("app_name")?.as_str()?.to_string(), title: d.get("app_title")?.as_str()?.to_string(), installed: false })).collect();
            Some(Owned { id, title, cover: image("DieselGameBoxTall").or_else(|| image("Thumbnail")), installable: Some(true), dlc })
        })
        .collect();
    games.sort_by(|a, b| a.title.to_lowercase().cmp(&b.title.to_lowercase()));
    games
}

/// Epic's list holds whatever the account got from Epic: games, but also Fab/Unreal assets,
/// plugins and sample projects, add-ons and software. A game is in Epic's "games" category and
/// isn't an add-on (LEGO Fortnite's experiences are launchable add-ons).
fn is_game(item: &serde_json::Value) -> bool {
    let categories: Vec<&str> = item.get("metadata").and_then(|m| m.get("categories")).and_then(|c| c.as_array()).into_iter().flatten().filter_map(|c| c.get("path")?.as_str()).collect();
    categories.contains(&"games") && !categories.contains(&"addons")
}

/// The game an add-on belongs to (its app name), for add-ons with something to install.
fn main_game(item: &serde_json::Value) -> Option<String> {
    let categories: Vec<&str> = item.get("metadata")?.get("categories")?.as_array()?.iter().filter_map(|c| c.get("path")?.as_str()).collect();
    if !categories.contains(&"addons") || item.get("asset_infos").and_then(|a| a.get("Windows")).is_none() {
        return None;
    }
    item.get("metadata")?.get("mainGameItem")?.get("releaseInfo")?.as_array()?.iter().find_map(|r| r.get("appId")?.as_str().map(String::from))
}

/// legendary's installed.json: app name to what it installed.
fn installed_json() -> BTreeMap<String, serde_json::Value> {
    read_json(&config().join("installed.json"))
}

pub fn installed() -> Vec<Installed> {
    installed_json()
        .into_iter()
        .filter(|(_, g)| g.get("is_dlc").and_then(|d| d.as_bool()) != Some(true))
        .filter_map(|(id, g)| {
            let folder = PathBuf::from(g.get("install_path")?.as_str()?);
            let program = g.get("executable").and_then(|x| x.as_str()).filter(|x| !x.is_empty()).map(|x| folder.join(x.replace('\\', "/").trim_start_matches('/')));
            Some(Installed { title: g.get("title").and_then(|t| t.as_str()).unwrap_or(&id).to_string(), id, folder, program, build: None })
        })
        .collect()
}

/// Installs the Windows version with every owned DLC into the games folder, in a folder named
/// after the game (Epic's own names can be "MinutesTillDawnxgosd").
pub fn install(app: &str, games: &Path, progress: &dyn Fn(f64)) -> Result<PathBuf, String> {
    let title = super::owned(super::Store::Epic).into_iter().find(|o| o.id == app).map(|o| o.title);
    let mut command = legendary()?;
    command.args(["-y", "install", app, "--platform", "Windows", "--with-dlcs", "--skip-sdl", "--base-path"]).arg(games);
    if let Some(title) = title {
        command.arg("--game-folder").arg(crate::install::free_folder(games, &title));
    }
    run_with_progress(command, progress).map_err(|e| format!("Epic's download stopped: {e}"))?;
    installed().into_iter().find(|g| g.id == app).map(|g| g.folder).ok_or_else(|| "legendary finished, but the game isn't in its list of installed games.".into())
}

/// Lobby's installed games with a newer version on Epic (legendary compares each install's
/// version with Epic's latest build).
pub fn check_updates() -> Result<Vec<Update>, String> {
    let mut command = legendary()?;
    command.args(["list-installed", "--check-updates", "--csv"]);
    let text = output(command).map_err(|e| format!("Epic didn't say which games have updates: {e}"))?;
    let games: Vec<String> = installed().into_iter().map(|g| g.id).collect();
    Ok(parse_updates(&text, &games))
}

/// legendary's CSV: app name, title, installed version, available version, update available...
fn parse_updates(csv: &str, games: &[String]) -> Vec<Update> {
    csv.lines()
        .skip(1)
        .map(csv_fields)
        .filter(|f| f.len() >= 5 && f[4] == "True" && games.contains(&f[0]))
        .map(|f| Update { id: f[0].clone(), from: f[2].clone(), to: f[3].clone() })
        .collect()
}

pub fn update(app: &str, progress: &dyn Fn(f64)) -> Result<(), String> {
    let mut command = legendary()?;
    command.args(["-y", "install", app, "--update-only", "--with-dlcs", "--skip-sdl"]);
    run_with_progress(command, progress).map_err(|e| format!("Epic's update stopped: {e}"))
}

/// The DLC owned for a game, from the owned list; installed when legendary has it installed.
pub fn dlc(app: &str) -> Vec<Dlc> {
    let installed = installed_json();
    let owned = super::owned(super::Store::Epic).into_iter().find(|o| o.id == app).map(|o| o.dlc).unwrap_or_default();
    owned.into_iter().map(|d| Dlc { installed: installed.contains_key(&d.id), ..d }).collect()
}

/// legendary installs a DLC into its game's folder.
pub fn install_dlc(dlc: &[String], progress: &dyn Fn(f64)) -> Result<(), String> {
    for id in dlc {
        let mut command = legendary()?;
        command.args(["-y", "install", id, "--skip-sdl"]);
        run_with_progress(command, progress).map_err(|e| format!("Epic's download stopped: {e}"))?;
    }
    Ok(())
}

/// How a game lobby installed starts: legendary signs it in to Epic (the arguments Epic's
/// launcher passes) and runs it through `wrapper` (umu-run with the game's prefix and Proton,
/// from its gamefix entry), with `env` set.
pub fn launch_command(app: &str, env: &[String], wrapper: &str) -> Vec<String> {
    let legendary = tools::LEGENDARY.file();
    let mut command = vec!["env".to_string(), format!("LEGENDARY_CONFIG_PATH={}", config().display())];
    command.extend(env.iter().cloned());
    command.extend([legendary.display().to_string(), "launch".into(), app.into(), "--no-wine".into(), "--wrapper".into(), wrapper.into()]);
    command
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owned_list() {
        let list = serde_json::json!([
            {"app_name": "Fang", "app_title": "Fall Guys", "metadata": {"categories": [{"path": "games"}, {"path": "applications"}], "keyImages": [
                {"type": "DieselGameBox", "url": "https://x/wide.jpg"},
                {"type": "DieselGameBoxTall", "url": "https://x/tall.jpg"}]}},
            {"app_name": "abc", "app_title": "another game", "metadata": {"categories": [{"path": "applications"}, {"path": "games"}, {"path": "public"}]}},
            {"app_name": "acf", "app_title": "Ascent Combat Framework", "metadata": {"categories": [{"path": "asset-format"}, {"path": "plugins"}]}},
            {"app_name": "lego", "app_title": "LEGO Fortnite: Odyssey", "metadata": {"categories": [{"path": "addons"}, {"path": "addons/launchable"}, {"path": "games"}]}},
            {"app_name": "kenney", "app_title": "Kenney Shape", "metadata": {"categories": [{"path": "applications"}, {"path": "software"}]}},
            {"app_name": "dlc1", "app_title": "Fall Guys: Pack", "asset_infos": {"Windows": {}}, "metadata": {"categories": [{"path": "addons"}], "mainGameItem": {"releaseInfo": [{"appId": "Fang"}]}}}
        ]);
        let games = parse_owned(&list, list.as_array().unwrap());
        assert_eq!(games.len(), 2);
        assert_eq!(games[0], Owned { id: "abc".into(), title: "another game".into(), cover: None, installable: Some(true), dlc: vec![] });
        assert_eq!(games[1].cover.as_deref(), Some("https://x/tall.jpg"));
        assert_eq!(games[1].dlc, [Dlc { id: "dlc1".into(), title: "Fall Guys: Pack".into(), installed: false }]);
    }

    #[test]
    fn updates() {
        let csv = "App name,App title,Installed version,Available version,Update available,Install size,Install path,Platform\n\
                   a,\"Game, The\",1.0,1.1,True,10,/g/a,Windows\n\
                   b,B,2.0,2.0,False,10,/g/b,Windows\n\
                   c,Not ours,1,2,True,1,/g/c,Windows\n";
        assert_eq!(parse_updates(csv, &["a".into(), "b".into()]), [Update { id: "a".into(), from: "1.0".into(), to: "1.1".into() }]);
    }
}
