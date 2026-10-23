//! GOG through gogdl, with its tokens in ~/.config/lobby/gogdl/auth.json: lobby's sign-in.
//! gogdl downloads; the owned list comes from GOG's own web API, and lobby keeps its own
//! record of what it installed (~/.local/share/lobby/gog-installed.json). gogdl's own files
//! (each install's manifest, which updates patch from) go in ~/.config/lobby/heroic_gogdl
//! (GOGDL_CONFIG_PATH), not the ~/.config/heroic_gogdl it uses by default.

use super::{config_dir, config_home, get_json, output, read_json, run_with_progress, tools, Dlc, Installed, Owned, Update};
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

pub(crate) fn gogdl() -> Result<Command, String> {
    let mut command = Command::new(tools::GOGDL.path()?);
    command.env("GOGDL_CONFIG_PATH", config_home().join("lobby")).arg("--auth-config-path").arg(auth_file());
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

/// The account's games, page by page, each with its tall cover from GOG's games database (else
/// GOG's wide image) and whether it has a Windows version to download: goodie packs are
/// "games" too, with nothing to install. Both are looked up once (`old` keeps them).
pub fn owned(old: &[Owned]) -> Result<Vec<Owned>, String> {
    let token = token()?;
    let mut games: Vec<(Owned, Option<String>)> = Vec::new();
    let mut page = 1;
    loop {
        let v = get_json(&format!("https://embed.gog.com/account/getFilteredProducts?mediaType=1&sortBy=title&page={page}"), Some(&token))?;
        for p in v.get("products").and_then(|p| p.as_array()).into_iter().flatten() {
            let (Some(id), Some(title)) = (p.get("id").and_then(|i| i.as_u64()), p.get("title").and_then(|t| t.as_str())) else { continue };
            let id = id.to_string();
            let before = old.iter().find(|o| o.id == id);
            let cover = before.and_then(|o| o.cover.clone());
            let wide = p.get("image").and_then(|i| i.as_str()).filter(|i| !i.is_empty()).map(|i| format!("https:{i}_784.jpg"));
            games.push((Owned { id, title: title.to_string(), cover, installable: before.and_then(|o| o.installable), dlc: Vec::new() }, wide));
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
            for (g, wide) in chunk.iter_mut().filter(|(g, _)| g.cover.is_none() || g.installable.is_none()) {
                s.spawn(move || {
                    if g.cover.is_none() {
                        g.cover = vertical_cover(&g.id).or(wide.take());
                    }
                    if g.installable.is_none() {
                        g.installable = windows_builds(&g.id).map(|n| n > 0);
                    }
                });
            }
        });
    }
    Ok(games.into_iter().map(|(g, _)| g).collect())
}

/// How many Windows builds GOG has for a product (None: the request failed).
fn windows_builds(id: &str) -> Option<u64> {
    get_json(&format!("https://content-system.gog.com/products/{id}/os/windows/builds?generation=2"), None).ok()?.get("total_count")?.as_u64()
}

fn vertical_cover(id: &str) -> Option<String> {
    let v = get_json(&format!("https://gamesdb.gog.com/platforms/gog/external_releases/{id}"), None).ok()?;
    let format = v.get("game")?.get("vertical_cover")?.get("url_format")?.as_str()?;
    Some(format.replace("{formatter}", "").replace("{ext}", "jpg"))
}

pub fn installed() -> Vec<Installed> {
    read_json(&installed_file())
}

/// What GOG says about a game's latest Windows build: folder_name, buildId, versionName,
/// languages, dlcs (owned, with an id and title each), builds, dependencies.
fn info(id: &str) -> Result<serde_json::Value, String> {
    info_for(id, "windows")
}

fn info_for(id: &str, platform: &str) -> Result<serde_json::Value, String> {
    let mut command = gogdl()?;
    command.args(["info", id, "--os", platform]);
    serde_json::from_str(output(command)?.lines().last().unwrap_or_default()).map_err(|e| format!("gogdl info: {e}"))
}

/// An installed GOG game's own record of itself (goggame-<id>.info in its folder): the build
/// installed and its languages. A DLC's installed has one too.
fn game_info(folder: &Path, id: &str) -> Option<serde_json::Value> {
    serde_json::from_str(&fs::read_to_string(folder.join(format!("goggame-{id}.info"))).ok()?).ok()
}

/// gogdl's record of an install (the manifest it saved, which its updates patch from): the
/// build installed (buildId) and the DLC installed with it (HGLdlcs).
pub(crate) fn manifests_dir() -> PathBuf {
    config_home().join("lobby/heroic_gogdl/manifests")
}

fn manifest(id: &str) -> Option<serde_json::Value> {
    serde_json::from_str(&fs::read_to_string(manifests_dir().join(id)).ok()?).ok()
}

/// The game's Galaxy client id (what its cloud save settings are filed under).
pub fn client_id(id: &str) -> Option<String> {
    let folder = installed().into_iter().find(|g| g.id == id)?.folder;
    manifest(id).or_else(|| game_info(&folder, id)).and_then(|m| m.get("clientId")?.as_str().map(String::from))
}

/// The build installed: lobby's record, else gogdl's, else the game's own .info.
fn installed_build(g: &Installed) -> Option<String> {
    g.build.clone().or_else(|| manifest(&g.id).or_else(|| game_info(&g.folder, &g.id)).and_then(|i| i.get("buildId")?.as_str().map(String::from)))
}

/// Saves lobby's record of its GOG installs, with `g` added or replacing its earlier self.
pub(crate) fn record(g: Installed) -> Result<(), String> {
    let mut list = installed();
    list.retain(|o| o.id != g.id);
    list.push(g);
    fs::create_dir_all(installed_file().parent().unwrap()).map_err(|e| e.to_string())?;
    fs::write(installed_file(), serde_json::to_string_pretty(&list).unwrap()).map_err(|e| e.to_string())
}

fn language(folder: &Path, id: &str) -> String {
    game_info(folder, id).and_then(|i| i.get("languages")?.as_array()?.first()?.as_str().map(String::from)).unwrap_or_else(|| "en-US".into())
}

/// Lobby's installs whose build isn't GOG's latest.
pub fn check_updates() -> Result<Vec<Update>, String> {
    let mut found = Vec::new();
    for g in installed() {
        let Some(have) = installed_build(&g) else { continue };
        let latest = info_for(&g.id, g.platform.as_deref().unwrap_or("windows")).map_err(|e| format!("GOG didn't say whether {} has an update: {e}", g.title))?;
        let Some(newest) = latest.get("buildId").and_then(|b| b.as_str()) else { continue };
        if newest != have {
            let builds = latest.get("builds").and_then(|b| b.get("items")).and_then(|i| i.as_array()).cloned().unwrap_or_default();
            let from = builds.iter().find(|b| b.get("build_id").and_then(|i| i.as_str()) == Some(&have)).and_then(|b| b.get("version_name")?.as_str()).unwrap_or_default();
            let to = latest.get("versionName").and_then(|v| v.as_str()).unwrap_or_default();
            found.push(Update { id: g.id, from: from.to_string(), to: to.to_string() });
        }
    }
    Ok(found)
}

/// gogdl's update patches the folder from its saved manifest to the latest build, with the
/// installed DLC (and `more` DLC, to add some).
fn update_with(id: &str, folder: &Path, more: &[String], progress: &dyn Fn(f64)) -> Result<(), String> {
    let platform = installed().into_iter().find(|g| g.id == id).and_then(|g| g.platform).unwrap_or_else(|| "windows".into());
    let mut dlc: Vec<String> = dlc(id, folder).unwrap_or_default().into_iter().filter(|d| d.installed).map(|d| d.id).collect();
    dlc.extend(more.iter().cloned());
    let mut command = gogdl()?;
    command.args(["update", id, "--platform", &platform, "--lang", &language(folder, id), "--path"]).arg(folder);
    if !dlc.is_empty() {
        command.args(["--with-dlcs", "--dlcs", &dlc.join(",")]);
    }
    run_with_progress(command, progress).map_err(|e| format!("GOG's download stopped: {e}"))
}

/// Updates to the latest build, and records it as installed (gogdl leaves its own record as
/// it was when the new build changed no files).
pub fn update(id: &str, folder: &Path, progress: &dyn Fn(f64)) -> Result<(), String> {
    let platform = installed().into_iter().find(|g| g.id == id).and_then(|g| g.platform).unwrap_or_else(|| "windows".into());
    let latest = info_for(id, &platform)?.get("buildId").and_then(|b| b.as_str()).map(String::from);
    update_with(id, folder, &[], progress)?;
    match installed().into_iter().find(|g| g.id == id) {
        Some(g) => record(Installed { build: latest, ..g }),
        None => Ok(()),
    }
}

/// The DLC the account owns for the game (from GOG), installed when gogdl's record has it or
/// its .info is in the folder.
pub fn dlc(id: &str, folder: &Path) -> Result<Vec<Dlc>, String> {
    let info = info(id)?;
    let recorded: Vec<String> = manifest(id).and_then(|m| m.get("HGLdlcs").cloned()).and_then(|d| d.as_array().cloned()).unwrap_or_default().iter().filter_map(|d| d.get("id").map(|i| i.as_str().map(String::from).unwrap_or_else(|| i.to_string()))).collect();
    Ok(info
        .get("dlcs")
        .and_then(|d| d.as_array())
        .into_iter()
        .flatten()
        .filter_map(|d| {
            let id = d.get("id").map(|i| i.as_str().map(String::from).unwrap_or_else(|| i.to_string()))?;
            let title = d.get("title")?.as_str()?.to_string();
            Some(Dlc { installed: recorded.contains(&id) || folder.join(format!("goggame-{id}.info")).is_file(), id, title })
        })
        .collect())
}

pub fn install_dlc(id: &str, more: &[String], progress: &dyn Fn(f64)) -> Result<(), String> {
    let folder = installed().into_iter().find(|g| g.id == id).map(|g| g.folder).ok_or("Lobby didn't install this game.")?;
    update_with(id, &folder, more, progress)
}

/// Installs the Windows version with every owned DLC, in English when the game has it.
pub fn install(id: &str, games: &Path, progress: &dyn Fn(f64)) -> Result<PathBuf, String> {
    let info = info(id)?;
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
    let build = info.get("buildId").and_then(|b| b.as_str()).map(String::from);
    record(Installed { id: id.to_string(), title, folder: folder.clone(), program, build, platform: None })?;
    Ok(folder)
}
