//! GOG through gogdl, with its tokens in ~/.config/lobby/gogdl/auth.json: lobby's sign-in.
//! gogdl downloads; the owned list comes from GOG's own web API, and lobby keeps its own
//! record of what it installed (~/.local/share/lobby/gog-installed.json).

use super::{config_dir, get_json, output, read_json, run_with_progress, tools, Installed, Owned};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// GOG Galaxy's sign-in, which ends on an address carrying the code.
pub const SIGN_IN: &str = "https://auth.gog.com/auth?client_id=46899977096215655&redirect_uri=https%3A%2F%2Fembed.gog.com%2Fon_login_success%3Forigin%3Dclient&response_type=code&layout=galaxy";

fn auth_file() -> PathBuf {
    config_dir("gogdl").join("auth.json")
}

/// The account name, saved at sign-in (GOG's tokens don't carry it).
fn account_file() -> PathBuf {
    config_dir("gogdl").join("account.json")
}

fn installed_file() -> PathBuf {
    crate::store::dir().join("gog-installed.json")
}

fn gogdl() -> Result<Command, String> {
    let mut command = Command::new(tools::GOGDL.path()?);
    command.arg("--auth-config-path").arg(auth_file());
    Ok(command)
}

pub fn account() -> Option<String> {
    if !auth_file().is_file() {
        return None;
    }
    let account: serde_json::Value = read_json(&account_file());
    Some(account.get("username").and_then(|n| n.as_str()).unwrap_or_default().to_string())
}

/// A fresh access token (gogdl refreshes it when it has run out).
fn token() -> Result<String, String> {
    let mut command = gogdl()?;
    command.arg("auth");
    let creds: serde_json::Value = serde_json::from_str(output(command)?.trim()).map_err(|_| "GOG's sign-in has run out: sign in again.".to_string())?;
    creds.get("access_token").and_then(|t| t.as_str()).map(String::from).ok_or_else(|| "GOG's sign-in has run out: sign in again.".to_string())
}

pub fn sign_in(code: &str) -> Result<(), String> {
    fs::create_dir_all(config_dir("gogdl")).map_err(|e| e.to_string())?;
    let mut command = gogdl()?;
    command.args(["auth", "--code", code]);
    output(command).map_err(|e| format!("GOG didn't accept the sign-in: {e}"))?;
    let user = get_json("https://embed.gog.com/userData.json", Some(&token()?))?;
    let name = user.get("username").and_then(|n| n.as_str()).unwrap_or_default();
    fs::write(account_file(), serde_json::json!({ "username": name }).to_string()).map_err(|e| e.to_string())
}

pub fn sign_out() -> Result<(), String> {
    let _ = fs::remove_file(account_file());
    match fs::remove_file(auth_file()) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.to_string()),
        _ => Ok(()),
    }
}

/// The account's games, page by page, each with its tall cover from GOG's games database
/// (fetched once: covers already in `old` are kept).
pub fn owned(old: &[Owned]) -> Result<Vec<Owned>, String> {
    let token = token()?;
    let mut games: Vec<Owned> = Vec::new();
    let mut page = 1;
    loop {
        let v = get_json(&format!("https://embed.gog.com/account/getFilteredProducts?mediaType=1&sortBy=title&page={page}"), Some(&token))?;
        for p in v.get("products").and_then(|p| p.as_array()).into_iter().flatten() {
            let (Some(id), Some(title)) = (p.get("id").and_then(|i| i.as_u64()), p.get("title").and_then(|t| t.as_str())) else { continue };
            let id = id.to_string();
            let cover = old.iter().find(|o| o.id == id).and_then(|o| o.cover.clone());
            games.push(Owned { id, title: title.to_string(), cover });
        }
        let pages = v.get("totalPages").and_then(|t| t.as_u64()).unwrap_or(1);
        if page >= pages {
            break;
        }
        page += 1;
    }
    // Four at a time.
    for chunk in games.chunks_mut(4) {
        std::thread::scope(|s| {
            for g in chunk.iter_mut().filter(|g| g.cover.is_none()) {
                s.spawn(move || g.cover = vertical_cover(&g.id));
            }
        });
    }
    Ok(games)
}

fn vertical_cover(id: &str) -> Option<String> {
    let v = get_json(&format!("https://gamesdb.gog.com/platforms/gog/external_releases/{id}"), None).ok()?;
    let format = v.get("game")?.get("vertical_cover")?.get("url_format")?.as_str()?;
    Some(format.replace("{formatter}", "").replace("{ext}", "jpg"))
}

pub fn installed() -> Vec<Installed> {
    read_json(&installed_file())
}

/// Installs the Windows version with every owned DLC, in English when the game has it.
pub fn install(id: &str, games: &Path, progress: &dyn Fn(f64)) -> Result<PathBuf, String> {
    let mut command = gogdl()?;
    command.args(["info", id, "--os", "windows"]);
    let info: serde_json::Value = serde_json::from_str(output(command)?.lines().last().unwrap_or_default()).map_err(|e| format!("gogdl info: {e}"))?;
    let folder_name = info.get("folder_name").and_then(|f| f.as_str()).ok_or("GOG didn't say what the game's folder is called.")?;
    let languages: Vec<&str> = info.get("languages").and_then(|l| l.as_array()).into_iter().flatten().filter_map(|l| l.as_str()).collect();
    let lang = ["en-US", "en"].into_iter().find(|l| languages.contains(l)).or(languages.first().copied()).unwrap_or("en-US");
    let folder = games.join(folder_name);
    let mut command = gogdl()?;
    command.args(["download", id, "--platform", "windows", "--with-dlcs", "--lang", lang, "--path"]).arg(games);
    run_with_progress(command, progress).map_err(|e| format!("GOG's download stopped: {e}"))?;
    if !folder.is_dir() {
        return Err(format!("gogdl finished, but {} isn't there.", folder.display()));
    }
    let title = super::owned(super::Store::Gog).into_iter().find(|o| o.id == id).map(|o| o.title).unwrap_or_else(|| folder_name.to_string());
    let program = gamefix::gog::read(&folder).map(|i| i.program);
    let mut list = installed();
    list.retain(|g| g.id != id);
    list.push(Installed { id: id.to_string(), title, folder: folder.clone(), program });
    fs::create_dir_all(installed_file().parent().unwrap()).map_err(|e| e.to_string())?;
    fs::write(installed_file(), serde_json::to_string_pretty(&list).unwrap()).map_err(|e| e.to_string())?;
    Ok(folder)
}
