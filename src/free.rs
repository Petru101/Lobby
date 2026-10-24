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

/// The free-now games not owned that weren't announced before (remembered in
/// ~/.cache/lobby/free-announced.json), for a notification.
pub fn not_announced(games: &[FreeGame], epic: &crate::stores::epic::OwnedItems) -> Vec<FreeGame> {
    let file = stores::cache_dir().join("free-announced.json");
    let mut seen: Vec<String> = fs::read_to_string(&file).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    let key = |g: &FreeGame| format!("{}:{}:{}", g.store, g.title, g.until.unwrap_or(0));
    let new: Vec<FreeGame> = games.iter().filter(|g| g.from.is_none() && !owned(g, epic) && !seen.contains(&key(g))).cloned().collect();
    seen.extend(new.iter().map(key));
    let _ = fs::write(&file, serde_json::to_string(&seen).unwrap());
    new
}

/// The checkout page for Epic's offer.
pub fn epic_checkout(namespace: &str, offer: &str) -> String {
    format!("https://www.epicgames.com/store/purchase?highlightColor=0078f2&offers=1-{namespace}-{offer}&orderId&purchaseToken&showNavigation=true")
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
