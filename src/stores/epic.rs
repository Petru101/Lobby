//! Epic Games through legendary, with its config in ~/.config/lobby/legendary
//! (LEGENDARY_CONFIG_PATH): lobby's sign-in, lobby's installs.

use super::{config_dir, output, read_json, run_with_progress, tools, Installed, Owned};
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

/// The account's games (not DLC, not ones only another store's app installs), with the tall
/// box art Epic's own launcher shows.
pub fn owned() -> Result<Vec<Owned>, String> {
    let mut command = legendary()?;
    command.args(["list", "--json"]);
    let list: serde_json::Value = serde_json::from_str(&output(command)?).map_err(|e| format!("legendary list: {e}"))?;
    Ok(parse_owned(&list))
}

fn parse_owned(list: &serde_json::Value) -> Vec<Owned> {
    let mut games: Vec<Owned> = list
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|g| {
            let id = g.get("app_name")?.as_str()?.to_string();
            let title = g.get("app_title")?.as_str()?.to_string();
            let images = g.get("metadata").and_then(|m| m.get("keyImages")).and_then(|k| k.as_array());
            let image = |kind: &str| images.into_iter().flatten().find(|i| i.get("type").and_then(|t| t.as_str()) == Some(kind)).and_then(|i| i.get("url")?.as_str().map(String::from));
            Some(Owned { id, title, cover: image("DieselGameBoxTall").or_else(|| image("Thumbnail")) })
        })
        .collect();
    games.sort_by(|a, b| a.title.to_lowercase().cmp(&b.title.to_lowercase()));
    games
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
            Some(Installed { title: g.get("title").and_then(|t| t.as_str()).unwrap_or(&id).to_string(), id, folder, program })
        })
        .collect()
}

/// Installs the Windows version with every owned DLC into the games folder.
pub fn install(app: &str, games: &Path, progress: &dyn Fn(f64)) -> Result<PathBuf, String> {
    let mut command = legendary()?;
    command.args(["-y", "install", app, "--platform", "Windows", "--with-dlcs", "--skip-sdl", "--base-path"]).arg(games);
    run_with_progress(command, progress).map_err(|e| format!("Epic's download stopped: {e}"))?;
    installed().into_iter().find(|g| g.id == app).map(|g| g.folder).ok_or_else(|| "legendary finished, but the game isn't in its list of installed games.".into())
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
            {"app_name": "Fang", "app_title": "Fall Guys", "metadata": {"keyImages": [
                {"type": "DieselGameBox", "url": "https://x/wide.jpg"},
                {"type": "DieselGameBoxTall", "url": "https://x/tall.jpg"}]}},
            {"app_name": "abc", "app_title": "another game", "metadata": {}}
        ]);
        let games = parse_owned(&list);
        assert_eq!(games[0], Owned { id: "abc".into(), title: "another game".into(), cover: None });
        assert_eq!(games[1].cover.as_deref(), Some("https://x/tall.jpg"));
    }
}
