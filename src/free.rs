//! Games free to keep right now (and Epic's next ones): Epic's own list of its weekly free
//! games, which lobby can open straight at checkout, and GamerPower's list of giveaways
//! (gamerpower.com/api), for Epic's other free games and Steam's and GOG's. Fetched at most
//! hourly into ~/.cache/lobby/free.json. Claiming is the user's: lobby opens the checkout or
//! the store page, they place the order.

use crate::stores::{self, Store};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum Claim {
    /// Epic's checkout for the offer.
    EpicCheckout { namespace: String, offer: String },
    /// A page on Epic's store, where the game is got.
    EpicPage(String),
    /// A page for the browser (Steam's, GOG's, a giveaway site's).
    Browser(String),
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct FreeGame {
    pub title: String,
    /// "Epic Games", "GOG", "Steam".
    pub store: String,
    /// When it stops being free (Unix time), when known.
    pub until: Option<u64>,
    /// When it starts being free, for one that isn't yet.
    pub from: Option<u64>,
    pub cover: Option<String>,
    pub claim: Claim,
    /// Epic's catalog items in the offer.
    #[serde(default)]
    pub items: Vec<String>,
    /// Epic's kind of offer: BASE_GAME, DLC, ADD_ON, BUNDLE ...
    #[serde(default)]
    pub offer_type: String,
}

#[derive(Serialize, Deserialize, Default)]
struct Cached {
    when: u64,
    games: Vec<FreeGame>,
}

fn cache_file() -> PathBuf {
    stores::cache_dir().join("free.json")
}

/// The free games, from the hour-old list or fetched again.
pub fn list() -> Result<Vec<FreeGame>, String> {
    let cached: Cached = fs::read_to_string(cache_file()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    if crate::store::now().saturating_sub(cached.when) < 3600 && !cached.games.is_empty() {
        return Ok(cached.games);
    }
    let mut games = crate::log::timed("free games: Epic's list", epic)?;
    // GamerPower's are extra: its failing leaves Epic's.
    match crate::log::timed("free games: GamerPower", gamerpower) {
        Ok(more) => {
            let new: Vec<FreeGame> = more.into_iter().filter(|g| !games.iter().any(|e| same_title(&e.title, &g.title))).collect();
            games.extend(new);
        }
        Err(e) => crate::log::line(&format!("free games: GamerPower: {e}")),
    }
    let _ = fs::create_dir_all(stores::cache_dir());
    let _ = fs::write(cache_file(), serde_json::to_string(&Cached { when: crate::store::now(), games: games.clone() }).unwrap());
    Ok(games)
}

/// The same game's names: equal, or one is the other with a subtitle ("Home" and "Home - A
/// Unique Horror Adventure").
fn same_title(a: &str, b: &str) -> bool {
    let words = |s: &str| s.to_lowercase().split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).map(String::from).collect::<Vec<_>>();
    let (a, b) = (words(a), words(b));
    !a.is_empty() && !b.is_empty() && (a.starts_with(&b) || b.starts_with(&a))
}

/// The country Epic prices for: the locale's (LANG=en_GB.UTF-8: GB), else US.
fn country() -> String {
    std::env::var("LANG").ok().and_then(|l| l.split(['.', '@']).next()?.split_once('_').map(|(_, c)| c.to_uppercase())).filter(|c| c.len() == 2).unwrap_or_else(|| "US".into())
}

fn epic() -> Result<Vec<FreeGame>, String> {
    let v = stores::get_json(&format!("https://store-site-backend-static.ak.epicgames.com/freeGamesPromotions?locale=en-US&country={}", country()), None)?;
    Ok(parse_epic(&v))
}

/// The offers at 100% off now, and the next ones (a "Mystery Game" left out: it has no name).
fn parse_epic(v: &serde_json::Value) -> Vec<FreeGame> {
    let elements = v.pointer("/data/Catalog/searchStore/elements").and_then(|e| e.as_array()).cloned().unwrap_or_default();
    let mut out = Vec::new();
    for e in elements {
        let s = |k: &str| e.get(k).and_then(|x| x.as_str()).unwrap_or_default().to_string();
        let free = |kind: &str| {
            e.get("promotions").and_then(|p| p.get(kind)).and_then(|p| p.as_array()).into_iter().flatten().flat_map(|p| p.get("promotionalOffers").and_then(|o| o.as_array()).cloned().unwrap_or_default()).find(|o| o.pointer("/discountSetting/discountPercentage").and_then(|d| d.as_u64()) == Some(0))
        };
        let (now, next) = (free("promotionalOffers"), free("upcomingPromotionalOffers"));
        let Some(offer) = now.as_ref().or(next.as_ref()) else { continue };
        let title = s("title");
        if title.is_empty() || title.to_lowercase().contains("mystery game") {
            continue;
        }
        let time = |k: &str| offer.get(k).and_then(|t| t.as_str()).and_then(iso_time);
        let images = e.get("keyImages").and_then(|k| k.as_array()).cloned().unwrap_or_default();
        let image = |kind: &str| images.iter().find(|i| i.get("type").and_then(|t| t.as_str()) == Some(kind)).and_then(|i| i.get("url")?.as_str().map(String::from));
        let items = e.get("items").and_then(|i| i.as_array()).into_iter().flatten().filter_map(|i| i.get("id")?.as_str().map(String::from)).collect();
        out.push(FreeGame {
            title,
            store: Store::Epic.name().into(),
            until: if now.is_some() { time("endDate") } else { None },
            from: if now.is_none() { time("startDate") } else { None },
            cover: image("OfferImageTall").or_else(|| image("DieselStoreFrontTall")).or_else(|| image("Thumbnail")),
            claim: Claim::EpicCheckout { namespace: s("namespace"), offer: s("id") },
            items,
            offer_type: s("offerType"),
        });
    }
    out
}

/// GamerPower's free-to-keep games on Epic, Steam and GOG (key giveaways from other sites
/// left out: they're not the stores' own).
fn gamerpower() -> Result<Vec<FreeGame>, String> {
    let v = stores::get_json("https://www.gamerpower.com/api/filter?platform=epic-games-store.steam.gog&type=game", None)?;
    Ok(parse_gamerpower(&v))
}

fn parse_gamerpower(v: &serde_json::Value) -> Vec<FreeGame> {
    v.as_array()
        .into_iter()
        .flatten()
        .filter_map(|g| {
            let s = |k: &str| g.get(k).and_then(|x| x.as_str()).unwrap_or_default();
            let title = s("title");
            if title.contains("Key Giveaway") {
                return None;
            }
            let platforms = s("platforms");
            let store = ["Epic Games Store", "GOG", "Steam"].into_iter().find(|p| platforms.contains(p))?;
            // "Home (Epic Games) Giveaway": the game's name.
            let name = title.split(" (").next().unwrap_or(title).trim_end_matches(" Giveaway").trim().to_string();
            let url = s("open_giveaway_url").to_string();
            let store_name = if store == "Epic Games Store" { Store::Epic.name() } else { store };
            Some(FreeGame {
                title: name,
                store: store_name.into(),
                until: plain_time(s("end_date")),
                from: None,
                cover: Some(s("image").to_string()).filter(|i| !i.is_empty()),
                claim: if store == "Epic Games Store" { Claim::EpicPage(url) } else { Claim::Browser(url) },
                items: Vec::new(),
                offer_type: String::new(),
            })
        })
        .collect()
}

/// Whether the account already has it. An Epic offer: its catalog items are all owned, or,
/// for a game, the account owns a game in its namespace (the free offer can name another item
/// of the same game than the account's copy). Else an owned game of the same name there.
pub fn owned(g: &FreeGame, epic: &crate::stores::epic::OwnedItems) -> bool {
    if let Claim::EpicCheckout { namespace, .. } = &g.claim {
        let game = matches!(g.offer_type.as_str(), "BASE_GAME" | "OTHERS" | "");
        return (!g.items.is_empty() && g.items.iter().all(|i| epic.items.contains(i))) || (game && epic.game_namespaces.contains(namespace));
    }
    let store = if g.store == Store::Epic.name() { Store::Epic } else if g.store == Store::Gog.name() { Store::Gog } else { return false };
    stores::owned(store).iter().any(|o| same_title(&o.title, &g.title))
}

/// "2026-10-29T15:00:00.000Z" as Unix time.
fn iso_time(t: &str) -> Option<u64> {
    plain_time(&t.replace('T', " ").replace('Z', ""))
}

/// "2026-10-29 23:59:59" (UTC) as Unix time.
fn plain_time(t: &str) -> Option<u64> {
    let (date, time) = t.split_once(' ')?;
    let d: Vec<i64> = date.split('-').map(|p| p.parse().ok()).collect::<Option<_>>()?;
    let h: Vec<i64> = time.split(':').map(|p| p.split('.').next()?.parse().ok()).collect::<Option<_>>()?;
    let (y, m, day) = (*d.first()?, *d.get(1)?, *d.get(2)?);
    // Days from 1970-01-01 (Howard Hinnant's days_from_civil).
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    u64::try_from(days * 86_400 + h.first()? * 3600 + h.get(1).copied().unwrap_or(0) * 60 + h.get(2).copied().unwrap_or(0)).ok()
}

fn announced_file() -> PathBuf {
    stores::cache_dir().join("free-announced.json")
}

fn announced_key(g: &FreeGame) -> String {
    format!("{}:{}:{}", g.store, g.title, g.until.unwrap_or(0))
}

/// Whether a free-now game the account doesn't have hasn't been announced yet (nothing marked).
pub fn any_not_announced(games: &[FreeGame], epic: &crate::stores::epic::OwnedItems) -> bool {
    let seen: Vec<String> = fs::read_to_string(announced_file()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    games.iter().any(|g| g.from.is_none() && !owned(g, epic) && !seen.contains(&announced_key(g)))
}

/// The daily look for new free games: a systemd user timer running `lobby free --notify` at
/// 17:30 (Epic's weekly games change on Thursdays at 15:00 UTC) and 15 minutes after logging
/// in, catching up on a day missed. Written (again when lobby's path changed) and enabled
/// unless turned off (~/.config/lobby/free-daily says "off").
const UNIT: &str = "lobby-free-games";

fn units_dir() -> PathBuf {
    stores::config_home().join("systemd/user")
}

fn daily_off() -> bool {
    fs::read_to_string(stores::config_home().join("lobby/free-daily")).is_ok_and(|t| t.trim() == "off")
}

fn systemctl(args: &[&str]) -> Result<(), String> {
    let out = std::process::Command::new("systemctl").arg("--user").args(args).stdin(std::process::Stdio::null()).output().map_err(|e| format!("systemctl: {e}"))?;
    if out.status.success() { Ok(()) } else { Err(format!("systemctl --user {}: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim())) }
}

pub fn keep_daily_check() -> Result<(), String> {
    if daily_off() {
        return Ok(());
    }
    let lobby = std::env::current_exe().map_err(|e| e.to_string())?;
    let service = format!("[Unit]\nDescription=Lobby: look for games free to keep\n\n[Service]\nExecStart={} free --notify\n", lobby.display());
    let timer = "[Unit]\nDescription=Lobby: look for games free to keep, daily\n\n[Timer]\nOnCalendar=*-*-* 17:30\nOnStartupSec=15min\nPersistent=true\n\n[Install]\nWantedBy=timers.target\n";
    let dir = units_dir();
    let (s, t) = (dir.join(format!("{UNIT}.service")), dir.join(format!("{UNIT}.timer")));
    if fs::read_to_string(&s).ok().as_deref() == Some(service.as_str()) && fs::read_to_string(&t).ok().as_deref() == Some(timer) {
        return Ok(());
    }
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    fs::write(&s, &service).map_err(|e| e.to_string())?;
    fs::write(&t, timer).map_err(|e| e.to_string())?;
    systemctl(&["daemon-reload"])?;
    systemctl(&["enable", "--now", &format!("{UNIT}.timer")])
}

/// Turns the daily look on or off.
pub fn set_daily(on: bool) -> Result<(), String> {
    let setting = stores::config_home().join("lobby/free-daily");
    fs::create_dir_all(setting.parent().unwrap()).map_err(|e| e.to_string())?;
    fs::write(&setting, if on { "on\n" } else { "off\n" }).map_err(|e| e.to_string())?;
    if on {
        return keep_daily_check();
    }
    let _ = systemctl(&["disable", "--now", &format!("{UNIT}.timer")]);
    for ext in ["service", "timer"] {
        let _ = fs::remove_file(units_dir().join(format!("{UNIT}.{ext}")));
    }
    systemctl(&["daemon-reload"])
}

pub fn daily_on() -> bool {
    !daily_off()
}

/// The free-now games not owned that weren't announced before (remembered in
/// ~/.cache/lobby/free-announced.json), for a notification.
pub fn not_announced(games: &[FreeGame], epic: &crate::stores::epic::OwnedItems) -> Vec<FreeGame> {
    let file = announced_file();
    let mut seen: Vec<String> = fs::read_to_string(&file).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    let key = announced_key;
    let new: Vec<FreeGame> = games.iter().filter(|g| g.from.is_none() && !owned(g, epic) && !seen.contains(&key(g))).cloned().collect();
    seen.extend(new.iter().map(key));
    let _ = fs::write(&file, serde_json::to_string(&seen).unwrap());
    new
}

/// Epic's checkout for several offers at once: one order for all of them.
pub fn epic_checkout(offers: &[(String, String)]) -> String {
    let offers: String = offers.iter().map(|(namespace, offer)| format!("&offers=1-{namespace}-{offer}")).collect();
    format!("https://www.epicgames.com/store/purchase?highlightColor=0078f2{offers}&orderId&purchaseToken&showNavigation=true")
}

/// JavaScript run in a signed-in page of Epic's store (it passes Cloudflare; lobby's own
/// requests don't): the store's search (the persisted searchStoreQuery epicgames-freegames-node
/// uses) for the game's name, each result's offer and current price, as JSON.
pub fn epic_search_js(title: &str) -> String {
    let vars = serde_json::json!({
        "allowCountries": country(), "category": "games/edition/base|bundles/games|editors|software/edition/base",
        "count": 10, "country": country(), "keywords": title, "locale": "en-US",
        "sortBy": "relevancy", "sortDir": "DESC", "start": 0, "withPrice": true,
    });
    let ext = serde_json::json!({"persistedQuery": {"version": 1, "sha256Hash": "7d58e12d9dd8cb14c84a3ff18d360bf9f0caa96bf218f2c5fda68ba88d68a437"}});
    format!(
        "const r = await fetch('/graphql?operationName=searchStoreQuery&variables=' + encodeURIComponent({}) + '&extensions=' + encodeURIComponent({}), {{credentials: 'include'}});
         const j = await r.json();
         const els = (((j.data || {{}}).Catalog || {{}}).searchStore || {{}}).elements || [];
         return JSON.stringify(els.map(e => ({{title: e.title, namespace: e.namespace, id: e.id, price: e.price ? e.price.totalPrice.discountPrice : null}})));",
        serde_json::to_string(&vars.to_string()).unwrap(),
        serde_json::to_string(&ext.to_string()).unwrap()
    )
}

/// Whether a checkout page's text (lowercased) shows Epic's order button.
pub fn order_button(text: &str) -> bool {
    ["add to library", "place order"].iter().any(|t| text.contains(t))
}

/// Whether a checkout page's text (lowercased) is Epic's thanks for a placed order.
pub fn order_done(text: &str) -> bool {
    ["thanks for your order", "thank you for your order", "thanks for buying", "thank you for buying", "order complete", "your order is complete"].iter().any(|t| text.contains(t))
}

/// The free offer for the game among the search's results (same name, price 0).
pub fn free_offer(title: &str, results: &str) -> Option<(String, String)> {
    let v: serde_json::Value = serde_json::from_str(results).ok()?;
    let free = v.as_array()?.iter().filter(|e| e.get("price").and_then(|p| p.as_u64()) == Some(0));
    let offer = |e: &serde_json::Value| Some((e.get("namespace")?.as_str()?.to_string(), e.get("id")?.as_str()?.to_string()));
    let name = |e: &serde_json::Value| e.get("title").and_then(|t| t.as_str()).unwrap_or_default().to_string();
    // The same name first ("Terraspace", not "Terraspace Soundtrack"), else one with a subtitle.
    let free: Vec<&serde_json::Value> = free.collect();
    free.iter().find(|e| crate::library::plain(&name(e)) == crate::library::plain(title)).or_else(|| free.iter().find(|e| same_title(&name(e), title))).and_then(|e| offer(e))
}

/// "free for 5 more days", "free until tomorrow", "free from Thursday" ...
pub fn when(g: &FreeGame) -> String {
    let now = crate::store::now();
    let days = |t: u64| (t.saturating_sub(now) + 43_200) / 86_400;
    match (g.until, g.from) {
        (_, Some(from)) => match days(from) {
            0 => "free from today".into(),
            1 => "free from tomorrow".into(),
            n => format!("free in {n} days"),
        },
        (Some(until), None) => match days(until) {
            0 => "free until today".into(),
            1 => "free until tomorrow".into(),
            n => format!("free for {n} more days"),
        },
        (None, None) => "free for now".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times() {
        assert_eq!(iso_time("2026-10-29T15:00:00.000Z"), Some(1_793_286_000));
        assert_eq!(plain_time("1970-01-02 00:00:01"), Some(86_401));
        assert_eq!(plain_time("N/A"), None);
    }

    #[test]
    fn epic_list() {
        let v = serde_json::json!({"data": {"Catalog": {"searchStore": {"elements": [
            {"title": "Home", "namespace": "ns1", "id": "off1", "items": [{"id": "item1"}],
             "keyImages": [{"type": "OfferImageTall", "url": "https://x/tall.jpg"}],
             "promotions": {"promotionalOffers": [{"promotionalOffers": [{"startDate": "2026-10-22T15:00:00.000Z", "endDate": "2026-10-29T15:00:00.000Z", "discountSetting": {"discountPercentage": 0}}]}], "upcomingPromotionalOffers": []}},
            {"title": "Sorry We're Closed", "namespace": "ns2", "id": "off2", "items": [],
             "promotions": {"promotionalOffers": [], "upcomingPromotionalOffers": [{"promotionalOffers": [{"startDate": "2026-10-29T15:00:00.000Z", "endDate": "2026-11-05T15:00:00.000Z", "discountSetting": {"discountPercentage": 0}}]}]}},
            {"title": "Half Off", "namespace": "ns3", "id": "off3",
             "promotions": {"promotionalOffers": [{"promotionalOffers": [{"discountSetting": {"discountPercentage": 50}}]}]}},
            {"title": "Mystery Game", "namespace": "ns4", "id": "off4",
             "promotions": {"upcomingPromotionalOffers": [{"promotionalOffers": [{"startDate": "2026-11-05T15:00:00.000Z", "discountSetting": {"discountPercentage": 0}}]}]}}
        ]}}}});
        let games = parse_epic(&v);
        assert_eq!(games.len(), 2);
        assert_eq!(games[0].claim, Claim::EpicCheckout { namespace: "ns1".into(), offer: "off1".into() });
        assert_eq!((games[0].until, games[0].from), (Some(1_793_286_000), None));
        assert_eq!(games[0].items, ["item1"]);
        assert_eq!((games[1].until, games[1].from), (None, Some(1_793_286_000)));
        let items = |i: &[&str], n: &[&str]| crate::stores::epic::OwnedItems { items: i.iter().map(|s| s.to_string()).collect(), game_namespaces: n.iter().map(|s| s.to_string()).collect() };
        assert!(owned(&games[0], &items(&["item1", "x"], &[])));
        assert!(owned(&games[0], &items(&["other"], &["ns1"])));
        assert!(!owned(&games[0], &items(&["x"], &["ns9"])));
        assert!(same_title("Home - A Unique Horror Adventure", "Home"));
        assert!(!same_title("Homeworld", "Home"));
    }

    #[test]
    fn offers() {
        let results = r#"[{"title":"Terraspace Soundtrack","namespace":"a","id":"1","price":0},{"title":"Terraspace","namespace":"n","id":"o","price":0},{"title":"Terraspace","namespace":"n","id":"p","price":599}]"#;
        assert_eq!(free_offer("Terraspace", results), Some(("n".into(), "o".into())));
        assert_eq!(free_offer("Other", results), None);
        assert_eq!(epic_checkout(&[("n".into(), "o".into()), ("m".into(), "q".into())]), "https://www.epicgames.com/store/purchase?highlightColor=0078f2&offers=1-n-o&offers=1-m-q&orderId&purchaseToken&showNavigation=true");
    }

    #[test]
    fn gamerpower_list() {
        let v = serde_json::json!([
            {"title": "Home (Epic Games) Giveaway", "platforms": "PC, Epic Games Store", "end_date": "2026-10-29 23:59:59", "open_giveaway_url": "https://www.gamerpower.com/open/home", "image": "https://x/home.jpg"},
            {"title": "Dwarven Realms (Steam) Key Giveaway", "platforms": "PC, Steam", "end_date": "N/A", "open_giveaway_url": "https://g/d"},
            {"title": "Some Game (GOG) Giveaway", "platforms": "PC, GOG", "end_date": "N/A", "open_giveaway_url": "https://g/s", "image": ""}
        ]);
        let games = parse_gamerpower(&v);
        assert_eq!(games.len(), 2);
        assert_eq!((games[0].title.as_str(), games[0].store.as_str()), ("Home", "Epic Games"));
        assert_eq!(games[0].claim, Claim::EpicPage("https://www.gamerpower.com/open/home".into()));
        assert_eq!((games[1].title.as_str(), games[1].store.as_str(), games[1].until), ("Some Game", "GOG", None));
    }
}
