//! Store accounts: signing in to Epic Games and GOG with lobby's own copies of their tools
//! (legendary, gogdl), the games the account owns, and installing them into the games folder.
//! Lobby keeps its own sign-ins and records (~/.config/lobby/<tool>, ~/.local/share/lobby):
//! nothing from another launcher is used. Steam does all of this itself.

pub mod epic;
pub mod gog;
pub mod saves;
pub mod tools;

use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Store {
    Epic,
    Gog,
}

impl Store {
    pub const ALL: [Store; 2] = [Store::Epic, Store::Gog];

    /// As written in `Key::Store`.
    pub fn id(self) -> &'static str {
        match self {
            Store::Epic => "epic",
            Store::Gog => "gog",
        }
    }

    pub fn from_id(id: &str) -> Option<Store> {
        Store::ALL.into_iter().find(|s| s.id() == id)
    }

    pub fn name(self) -> &'static str {
        match self {
            Store::Epic => "Epic Games",
            Store::Gog => "GOG",
        }
    }

    pub fn source(self) -> crate::library::Source {
        match self {
            Store::Epic => crate::library::Source::Epic,
            Store::Gog => crate::library::Source::Gog,
        }
    }

    /// The store's sign-in page, shown in lobby's sign-in window.
    pub fn sign_in_page(self) -> &'static str {
        match self {
            Store::Epic => epic::SIGN_IN,
            Store::Gog => gog::SIGN_IN,
        }
    }

    /// The account's name when signed in (or "" when the store doesn't say), else None.
    pub fn account(self) -> Option<String> {
        match self {
            Store::Epic => epic::account(),
            Store::Gog => gog::account(),
        }
    }

    /// Finishes signing in with the code the sign-in page gave.
    pub fn sign_in(self, code: &str) -> Result<(), String> {
        match self {
            Store::Epic => epic::sign_in(code),
            Store::Gog => gog::sign_in(code),
        }
    }

    pub fn sign_out(self) -> Result<(), String> {
        let _ = fs::remove_file(owned_file(self));
        match self {
            Store::Epic => epic::sign_out(),
            Store::Gog => gog::sign_out(),
        }
    }

    /// The games the account owns, from the store (and saved for the next scans).
    pub fn refresh_owned(self) -> Result<Vec<Owned>, String> {
        let old = owned(self);
        let games = match self {
            Store::Epic => epic::owned()?,
            Store::Gog => gog::owned(&old)?,
        };
        let file = owned_file(self);
        fs::create_dir_all(file.parent().unwrap()).map_err(|e| e.to_string())?;
        let saved = OwnedList { when: crate::store::now(), games: games.clone() };
        fs::write(&file, serde_json::to_string(&saved).unwrap()).map_err(|e| e.to_string())?;
        Ok(games)
    }

    /// Which of lobby's installs from this store have an update, from the store (and saved
    /// for the next scans).
    pub fn check_updates(self) -> Result<Vec<Update>, String> {
        let found = match self {
            Store::Epic => epic::check_updates()?,
            Store::Gog => gog::check_updates()?,
        };
        save_updates(self, &found)?;
        Ok(found)
    }

    /// Updates an installed game (and its installed DLC) to the store's latest version.
    pub fn update(self, id: &str, progress: &dyn Fn(f64)) -> Result<(), String> {
        let folder = self.installed().into_iter().find(|g| g.id == id).map(|g| g.folder).ok_or("Lobby didn't install this game, so it can't update it.")?;
        match self {
            Store::Epic => epic::update(id, progress)?,
            Store::Gog => gog::update(id, &folder, progress)?,
        }
        let mut left = updates(self);
        left.retain(|u| u.id != id);
        save_updates(self, &left)
    }

    /// The DLC the account owns for a game lobby installed, and which are installed. GOG's
    /// list comes from the store each time.
    pub fn dlc(self, id: &str) -> Result<Vec<Dlc>, String> {
        let folder = self.installed().into_iter().find(|g| g.id == id).map(|g| g.folder).ok_or("Lobby didn't install this game.")?;
        match self {
            Store::Epic => Ok(epic::dlc(id)),
            Store::Gog => gog::dlc(id, &folder),
        }
    }

    /// Downloads DLC into its installed game.
    pub fn install_dlc(self, id: &str, dlc: &[String], progress: &dyn Fn(f64)) -> Result<(), String> {
        match self {
            Store::Epic => epic::install_dlc(dlc, progress),
            Store::Gog => gog::install_dlc(id, dlc, progress),
        }
    }

    /// Lobby's installs from this store.
    pub fn installed(self) -> Vec<Installed> {
        match self {
            Store::Epic => epic::installed(),
            Store::Gog => gog::installed(),
        }
    }

    /// Downloads and installs an owned game into the games folder; `progress` gets how far it
    /// is (0 to 100). Returns the game's folder.
    pub fn install(self, id: &str, progress: &dyn Fn(f64)) -> Result<PathBuf, String> {
        let games = gamefix::setup::games_folder();
        fs::create_dir_all(&games).map_err(|e| format!("The games folder {} isn't there: {e}", games.display()))?;
        match self {
            Store::Epic => epic::install(id, &games, progress),
            Store::Gog => gog::install(id, &games, progress),
        }
    }
}

/// The store and id of a game lobby installed from a store.
pub fn installed_in(game: &crate::library::Game) -> Option<(Store, String)> {
    game.entries.iter().flat_map(|e| e.keys.iter()).find_map(|k| match k {
        crate::library::Key::Store(s, id) => {
            let store = Store::from_id(s)?;
            store.installed().iter().any(|g| &g.id == id).then(|| (store, id.clone()))
        }
        _ => None,
    })
}

/// A game the account owns.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Owned {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub cover: Option<String>,
    /// Whether there's a Windows version to download (GOG lists goodie packs as games): known
    /// once checked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub installable: Option<bool>,
    /// The DLC the account owns for it, when the owned list says (Epic's does).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dlc: Vec<Dlc>,
}

#[derive(Serialize, Deserialize, Default)]
struct OwnedList {
    when: u64,
    games: Vec<Owned>,
}

/// A DLC the account owns for a game.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Dlc {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub installed: bool,
}

/// An update the store has for a game lobby installed: the version installed and the new one
/// (as the store names them; either can be unknown).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Update {
    pub id: String,
    pub from: String,
    pub to: String,
}

fn updates_file(store: Store) -> PathBuf {
    cache_dir().join(format!("updates-{}.json", store.id()))
}

/// The updates found by the last check.
pub fn updates(store: Store) -> Vec<Update> {
    read_json(&updates_file(store))
}

fn save_updates(store: Store, list: &[Update]) -> Result<(), String> {
    let file = updates_file(store);
    fs::create_dir_all(file.parent().unwrap()).map_err(|e| e.to_string())?;
    fs::write(&file, serde_json::to_string(list).unwrap()).map_err(|e| e.to_string())
}

/// A game lobby installed from a store.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Installed {
    pub id: String,
    pub title: String,
    pub folder: PathBuf,
    #[serde(default)]
    pub program: Option<PathBuf>,
    /// The store's build installed, when lobby records it (GOG: gogdl doesn't record an update
    /// that changed no files).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build: Option<String>,
}

pub(crate) fn config_home() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config"))
}

/// Where lobby keeps a store tool's sign-in and settings.
fn config_dir(tool: &str) -> PathBuf {
    config_home().join("lobby").join(tool)
}

pub(crate) fn cache_dir() -> PathBuf {
    std::env::var_os("XDG_CACHE_HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".cache")).join("lobby")
}

fn owned_file(store: Store) -> PathBuf {
    cache_dir().join(format!("owned-{}.json", store.id()))
}

/// The owned games saved by the last refresh.
pub fn owned(store: Store) -> Vec<Owned> {
    fs::read_to_string(owned_file(store)).ok().and_then(|t| serde_json::from_str::<OwnedList>(&t).ok()).map(|l| l.games).unwrap_or_default()
}

/// When the owned games were last fetched (0: never).
fn owned_when(store: Store) -> u64 {
    fs::read_to_string(owned_file(store)).ok().and_then(|t| serde_json::from_str::<OwnedList>(&t).ok()).map_or(0, |l| l.when)
}

/// Brings each signed-in store's owned list up to date when it's more than a few hours old (a
/// scan does this; a failure keeps the saved list).
pub fn refresh_stale() {
    for store in Store::ALL {
        if store.account().is_some() && crate::store::now().saturating_sub(owned_when(store)) > 6 * 3600 {
            if let Err(e) = store.refresh_owned() {
                eprintln!("{}: {e}", store.name());
            }
            if let Err(e) = store.check_updates() {
                eprintln!("{}: {e}", store.name());
            }
        }
    }
}

/// The fields of a CSV line (quoted fields may hold commas and doubled quotes).
fn csv_fields(line: &str) -> Vec<String> {
    let (mut fields, mut cur, mut quoted) = (Vec::new(), String::new(), false);
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' if quoted && chars.peek() == Some(&'"') => {
                cur.push('"');
                chars.next();
            }
            '"' => quoted = !quoted,
            ',' if !quoted => fields.push(std::mem::take(&mut cur)),
            c => cur.push(c),
        }
    }
    fields.push(cur);
    fields
}

/// The number in "Progress: 12.34%" (legendary) or "Progress: 12.34 5/40" (gogdl).
fn progress_in(line: &str) -> Option<f64> {
    let rest = &line[line.find("Progress: ")? + 10..];
    let number: String = rest.chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
    number.parse().ok()
}

/// Runs a store tool to its end, passing on its progress; on failure, the last thing it said.
fn run_with_progress(mut command: Command, progress: &dyn Fn(f64)) -> Result<(), String> {
    let mut child = command.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).env("NO_COLOR", "1").spawn().map_err(|e| e.to_string())?;
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    let readers: Vec<_> = [child.stdout.take().map(|s| Box::new(s) as Box<dyn std::io::Read + Send>), child.stderr.take().map(|s| Box::new(s) as Box<dyn std::io::Read + Send>)]
        .into_iter()
        .flatten()
        .map(|pipe| {
            let tx = tx.clone();
            std::thread::spawn(move || {
                for line in BufReader::new(pipe).lines().map_while(Result::ok) {
                    let _ = tx.send(line);
                }
            })
        })
        .collect();
    drop(tx);
    let mut last = Vec::new();
    for line in rx {
        if let Some(p) = progress_in(&line) {
            progress(p);
        } else if !line.trim().is_empty() {
            last.push(line);
            if last.len() > 8 {
                last.remove(0);
            }
        }
    }
    for r in readers {
        let _ = r.join();
    }
    let status = child.wait().map_err(|e| e.to_string())?;
    if status.success() {
        return Ok(());
    }
    let said = last.iter().rev().find(|l| l.contains("ERROR") || l.contains("Error") || l.contains("rror:")).or(last.last()).cloned().unwrap_or_default();
    Err(said.split("ERROR:").last().unwrap_or(&said).trim().to_string())
}

/// A tool's output, or the last line it wrote about what went wrong.
fn output(mut command: Command) -> Result<String, String> {
    let out = command.stdin(Stdio::null()).env("NO_COLOR", "1").output().map_err(|e| e.to_string())?;
    if out.status.success() {
        return Ok(String::from_utf8_lossy(&out.stdout).into_owned());
    }
    let err = String::from_utf8_lossy(&out.stderr);
    let last = err.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("it stopped with an error");
    Err(last.split("ERROR:").last().unwrap_or(last).trim().to_string())
}

/// A web request with the account's token, its answer as JSON. The token goes to curl on its
/// standard input, not its command line (where other programs could see it).
fn get_json(url: &str, token: Option<&str>) -> Result<serde_json::Value, String> {
    let mut command = Command::new("curl");
    command.args(["-sSf", "--max-time", "30", "-A", "lobby", url]);
    if token.is_some() {
        command.args(["-H", "@-"]);
    }
    let mut child = command.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().map_err(|e| e.to_string())?;
    let mut stdin = child.stdin.take().unwrap();
    if let Some(t) = token {
        let _ = writeln!(stdin, "Authorization: Bearer {t}");
    }
    drop(stdin);
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(format!("{url}: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    serde_json::from_slice(&out.stdout).map_err(|e| format!("{url}: {e}"))
}

/// The value of a URL's query parameter.
fn query_param(url: &str, name: &str) -> Option<String> {
    let query = url.split_once('?')?.1.split('#').next()?;
    query.split('&').find_map(|p| p.strip_prefix(name)?.strip_prefix('=')).map(String::from)
}

/// The sign-in code in what the sign-in window shows: the page's address (GOG's) or the
/// page's text (Epic's redirect page is JSON).
pub fn code_in(store: Store, url: &str, text: Option<&str>) -> Option<String> {
    match store {
        Store::Gog => url.starts_with("https://embed.gog.com/on_login_success").then(|| query_param(url, "code")).flatten(),
        Store::Epic => {
            if !url.starts_with("https://www.epicgames.com/id/api/redirect") {
                return None;
            }
            let v: serde_json::Value = serde_json::from_str(text?.trim()).ok()?;
            v.get("authorizationCode")?.as_str().filter(|c| !c.is_empty()).map(String::from)
        }
    }
}

fn read_json<T: for<'a> Deserialize<'a> + Default>(file: &Path) -> T {
    fs::read_to_string(file).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn csv() {
        assert_eq!(csv_fields(r#"a1,"Tom Clancy's ""Ghost"", Recon",1.0,1.1,True"#), ["a1", r#"Tom Clancy's "Ghost", Recon"#, "1.0", "1.1", "True"]);
    }

    #[test]
    fn progress_lines() {
        assert_eq!(progress_in("[DLManager] INFO: = Progress: 3.53% (66/1868), Running for 00:00:04, ETA: 00:01:49"), Some(3.53));
        assert_eq!(progress_in("[PROGRESS] INFO: = Progress: 12.50 5/40, Running for: 00:00:02"), Some(12.5));
        assert_eq!(progress_in("[DLManager] INFO: - Downloaded: 1.00 MiB"), None);
    }

    #[test]
    fn sign_in_codes() {
        assert_eq!(code_in(Store::Gog, "https://embed.gog.com/on_login_success?origin=client&code=abc123", None), Some("abc123".into()));
        assert_eq!(code_in(Store::Gog, "https://login.gog.com/login?x=1", None), None);
        let page = r#"{"warning":"Do not share this code","redirectUrl":"https://localhost/launcher/authorized?code=x","authorizationCode":"c0de","exchangeCode":null,"sid":null}"#;
        assert_eq!(code_in(Store::Epic, "https://www.epicgames.com/id/api/redirect?clientId=34a&responseType=code", Some(page)), Some("c0de".into()));
        assert_eq!(code_in(Store::Epic, "https://www.epicgames.com/id/api/redirect?clientId=34a", Some(r#"{"authorizationCode":null}"#)), None);
    }
}
