//! What the Steam account signed in to the Steam app here owns, installed or not (Kind::Owned,
//! keyed by app id, so an installed one merges with Steam's own entry). No sign-in of Lobby's
//! own: the Steam app keeps it all, and only it can install Steam's games anyway.
//!
//! - The account's licenses: userdata/<account id>/config/licensecache, a CMsgClientLicenseList
//!   (protobuf) XOR-ed with Valve's random stream seeded with the account id.
//! - Each license's apps: appcache/packageinfo.vdf (binary key-values, one block per package).
//! - Each app's name and type: appcache/appinfo.vdf (binary key-values, key names in a table at
//!   the end since format 29). Only type "game": not DLC, tools, demos, soundtracks.

use super::steam::{library_art, roots};
use super::Home;
use crate::library::{Entry, Key, Kind, Source};
use gamefix::vdf;
use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

/// Steam ids of individual accounts are this plus the account id.
const STEAM_ID_BASE: u64 = 76561197960265728;

pub fn games(home: &Home) -> Vec<Entry> {
    let mut entries: Vec<Entry> = Vec::new();
    for root in roots(home) {
        let packages: BTreeSet<u32> = accounts(&root).iter().flat_map(|a| licenses(&root, *a)).collect();
        if packages.is_empty() {
            continue;
        }
        let apps: BTreeSet<u32> = package_apps(&fs::read(root.join("packageinfo.vdf")).unwrap_or_default(), &packages).into_iter().collect();
        for (appid, name) in app_games(&fs::read(root.join("appinfo.vdf")).unwrap_or_default(), &apps) {
            if entries.iter().any(|e| e.keys.contains(&Key::SteamApp(appid))) {
                continue;
            }
            let mut e = Entry::new(Source::Steam, name);
            e.kind = Kind::Owned;
            e.keys.push(Key::SteamApp(appid));
            e.cover = Some(library_art(&root, appid));
            entries.push(e);
        }
    }
    entries
}

/// The Steam app's account whose games these are (its profile name), and how many games it
/// owns, for Accounts. None when no account's licenses are on this computer.
pub fn account(home: &Home) -> Option<(String, usize)> {
    let root = roots(home).into_iter().find(|r| !accounts(r).is_empty())?;
    let account = *accounts(&root).first()?;
    let text = fs::read_to_string(root.join("config/loginusers.vdf")).unwrap_or_default();
    let name = vdf::parse(&text)
        .ok()
        .and_then(|doc| match vdf::get(&doc, &["users", &(STEAM_ID_BASE + u64::from(account)).to_string(), "PersonaName"]) {
            Some(vdf::Node::Str(n)) => Some(n.clone()),
            _ => None,
        })
        .unwrap_or_default();
    Some((name, games(home).len()))
}

/// The account signed in last (loginusers.vdf: MostRecent, else the newest timestamp), when it
/// has a license cache; else every account that has one.
fn accounts(root: &Path) -> Vec<u32> {
    let with_cache: Vec<u32> = fs::read_dir(root.join("userdata"))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.file_name().to_str()?.parse::<u32>().ok())
        .filter(|a| root.join(format!("userdata/{a}/config/licensecache")).is_file())
        .collect();
    let text = fs::read_to_string(root.join("config/loginusers.vdf")).unwrap_or_default();
    let last = vdf::parse(&text).ok().and_then(|doc| {
        let Some(vdf::Node::Map(users)) = vdf::get(&doc, &["users"]) else { return None };
        let field = |m: &vdf::Doc, k: &str| m.iter().find(|(key, _)| key.eq_ignore_ascii_case(k)).and_then(|(_, v)| if let vdf::Node::Str(s) = v { Some(s.clone()) } else { None });
        users
            .iter()
            .filter_map(|(id, user)| if let vdf::Node::Map(m) = user { Some((id.parse::<u64>().ok()?.checked_sub(STEAM_ID_BASE)? as u32, m)) } else { None })
            .max_by_key(|(_, m)| (field(m, "MostRecent").as_deref() == Some("1"), field(m, "timestamp").and_then(|t| t.parse::<u64>().ok()).unwrap_or(0)))
            .map(|(a, _)| a)
    });
    match last {
        Some(a) if with_cache.contains(&a) => vec![a],
        _ => with_cache,
    }
}

/// The packages an account holds a working license for (not expired, cancelled or pending).
fn licenses(root: &Path, account: u32) -> Vec<u32> {
    let Ok(data) = fs::read(root.join(format!("userdata/{account}/config/licensecache"))) else { return Vec::new() };
    decode_licenses(&unscramble(&data, account))
}

fn unscramble(data: &[u8], account: u32) -> Vec<u8> {
    let mut random = UniformRandomStream::new(account as i32);
    data.iter().map(|b| b ^ random.int(32, 126) as u8).collect()
}

/// Flags that mean the license doesn't give the game: pending, expired, cancelled by the user,
/// by Steam, or for fraud, not activated (ELicenseFlags).
const NOT_HELD: u64 = 0x4 | 0x8 | 0x10 | 0x20 | 0x400 | 0x800;

fn decode_licenses(list: &[u8]) -> Vec<u32> {
    proto_fields(list)
        .into_iter()
        .filter_map(|(field, value)| if let (2, Proto::Bytes(license)) = (field, value) { Some(license) } else { None })
        .filter_map(|license| {
            let fields = proto_fields(license);
            let int = |n: u64| fields.iter().find_map(|(f, v)| if let (true, Proto::Int(i)) = (*f == n, v) { Some(*i) } else { None });
            let package = int(1)?;
            (int(7).unwrap_or(0) & NOT_HELD == 0).then_some(package as u32)
        })
        .collect()
}

enum Proto<'a> {
    Int(u64),
    Bytes(&'a [u8]),
}

/// A protobuf message's fields, as far as they read cleanly (the cache ends in a checksum).
fn proto_fields(b: &[u8]) -> Vec<(u64, Proto<'_>)> {
    fn varint(b: &[u8], i: &mut usize) -> Option<u64> {
        let mut v = 0u64;
        for shift in (0..64).step_by(7) {
            let byte = *b.get(*i)?;
            *i += 1;
            v |= u64::from(byte & 0x7f) << shift;
            if byte < 0x80 {
                return Some(v);
            }
        }
        None
    }
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let Some(key) = varint(b, &mut i) else { break };
        let value = match key & 7 {
            0 => match varint(b, &mut i) {
                Some(v) => Proto::Int(v),
                None => break,
            },
            1 | 5 => {
                let n = if key & 7 == 1 { 8 } else { 4 };
                let Some(bytes) = b.get(i..i + n) else { break };
                i += n;
                Proto::Int(bytes.iter().rev().fold(0, |v, &x| v << 8 | u64::from(x)))
            }
            2 => {
                let Some(n) = varint(b, &mut i).and_then(|n| usize::try_from(n).ok()) else { break };
                let Some(bytes) = b.get(i..i.saturating_add(n)) else { break };
                i += n;
                Proto::Bytes(bytes)
            }
            _ => break,
        };
        out.push((key >> 3, value));
    }
    out
}

/// Valve's CUniformRandomStream (Numerical Recipes' ran1), as the Steam client uses it.
struct UniformRandomStream {
    idum: i64,
    iy: i64,
    iv: [i64; 32],
}

impl UniformRandomStream {
    const IA: i64 = 16807;
    const IM: i64 = 2147483647;
    const IQ: i64 = 127773;
    const IR: i64 = 2836;
    const NDIV: i64 = 1 + (Self::IM - 1) / 32;

    fn new(seed: i32) -> Self {
        let seed = i64::from(seed);
        UniformRandomStream { idum: if seed < 0 { seed } else { -seed }, iy: 0, iv: [0; 32] }
    }

    fn step(&mut self) {
        let k = self.idum / Self::IQ;
        self.idum = Self::IA * (self.idum - k * Self::IQ) - Self::IR * k;
        if self.idum < 0 {
            self.idum += Self::IM;
        }
    }

    fn next(&mut self) -> i64 {
        if self.idum <= 0 || self.iy == 0 {
            self.idum = if -self.idum < 1 { 1 } else { -self.idum };
            for j in (0..32 + 8).rev() {
                self.step();
                if j < 32 {
                    self.iv[j] = self.idum;
                }
            }
            self.iy = self.iv[0];
        }
        self.step();
        let j = (self.iy / Self::NDIV) as usize;
        self.iy = self.iv[j];
        self.iv[j] = self.idum;
        self.iy
    }

    fn int(&mut self, low: i64, high: i64) -> i64 {
        let x = high - low + 1;
        let max = Self::IM - ((Self::IM + 1) % x);
        loop {
            let n = self.next();
            if n <= max {
                return low + n % x;
            }
        }
    }
}

/// Binary key-values, as Steam's appcache files keep them.
#[derive(Debug)]
enum Kv {
    Map(Vec<(String, Kv)>),
    Str(String),
    Int(i64),
}

impl Kv {
    fn get(&self, key: &str) -> Option<&Kv> {
        if let Kv::Map(m) = self { m.iter().find(|(k, _)| k.eq_ignore_ascii_case(key)).map(|(_, v)| v) } else { None }
    }

    fn path(&self, keys: &[&str]) -> Option<&Kv> {
        keys.iter().try_fold(self, |kv, k| kv.get(k))
    }

    fn str(&self) -> Option<&str> {
        if let Kv::Str(s) = self { Some(s) } else { None }
    }
}

struct Reader<'a> {
    b: &'a [u8],
    i: usize,
}

impl Reader<'_> {
    fn bytes(&mut self, n: usize) -> Option<&[u8]> {
        let out = self.b.get(self.i..self.i.checked_add(n)?)?;
        self.i += n;
        Some(out)
    }
    fn u32(&mut self) -> Option<u32> {
        self.bytes(4).map(|b| u32::from_le_bytes(b.try_into().unwrap()))
    }
    fn u64(&mut self) -> Option<u64> {
        self.bytes(8).map(|b| u64::from_le_bytes(b.try_into().unwrap()))
    }
    fn cstr(&mut self) -> Option<String> {
        let len = self.b.get(self.i..)?.iter().position(|&c| c == 0)?;
        let s = String::from_utf8_lossy(&self.b[self.i..self.i + len]).into_owned();
        self.i += len + 1;
        Some(s)
    }
    /// A block's entries up to its end marker; key names inline, or indices into `strings`.
    fn kv(&mut self, strings: Option<&[String]>, depth: usize) -> Option<Kv> {
        let mut items = Vec::new();
        loop {
            let kind = *self.bytes(1)?.first()?;
            if kind == 8 {
                return Some(Kv::Map(items));
            }
            let key = match strings {
                Some(table) => table.get(self.u32()? as usize)?.clone(),
                None => self.cstr()?,
            };
            let value = match kind {
                0 if depth < 32 => self.kv(strings, depth + 1)?,
                1 => Kv::Str(self.cstr()?),
                2 => Kv::Int(i64::from(self.u32()? as i32)),
                3 | 4 | 6 => {
                    self.bytes(4)?;
                    Kv::Int(0)
                }
                7 | 10 => Kv::Int(self.u64()? as i64),
                _ => return None,
            };
            items.push((key, value));
        }
    }
}

/// The apps of the given packages, from packageinfo.vdf (format 39, or 40 with an access
/// token per package).
fn package_apps(b: &[u8], wanted: &BTreeSet<u32>) -> Vec<u32> {
    let mut r = Reader { b, i: 0 };
    let Some(magic) = r.u32() else { return Vec::new() };
    if magic & 0xFFFF_FF00 != 0x0656_5500 || r.u32().is_none() {
        return Vec::new();
    }
    let mut apps = Vec::new();
    while let Some(package) = r.u32().filter(|&p| p != u32::MAX) {
        // SHA-1, change number, then (40) the access token.
        if r.bytes(24).is_none() || (magic >= 0x0656_5528 && r.bytes(8).is_none()) {
            break;
        }
        let Some(block) = r.kv(None, 0) else { break };
        if !wanted.contains(&package) {
            continue;
        }
        // One block named after the package.
        let info = match &block {
            Kv::Map(m) if m.len() == 1 => &m[0].1,
            other => other,
        };
        if let Some(Kv::Map(ids)) = info.get("appids") {
            apps.extend(ids.iter().filter_map(|(_, v)| if let Kv::Int(i) = v { u32::try_from(*i).ok() } else { None }));
        }
    }
    apps
}

/// Name of each wanted app that's a game, from appinfo.vdf (formats 27-29).
fn app_games(b: &[u8], wanted: &BTreeSet<u32>) -> Vec<(u32, String)> {
    let mut r = Reader { b, i: 0 };
    let Some(magic) = r.u32() else { return Vec::new() };
    if !(0x0756_4427..=0x0756_4429).contains(&magic) || r.u32().is_none() {
        return Vec::new();
    }
    let strings: Option<Vec<String>> = if magic >= 0x0756_4429 {
        let Some(offset) = r.u64().and_then(|o| usize::try_from(o).ok()) else { return Vec::new() };
        let mut t = Reader { b, i: offset };
        let count = t.u32().unwrap_or(0);
        Some((0..count).map_while(|_| t.cstr()).collect())
    } else {
        None
    };
    let mut games = Vec::new();
    while let Some(appid) = r.u32().filter(|&a| a != 0) {
        let Some(size) = r.u32().and_then(|s| usize::try_from(s).ok()) else { break };
        let next = r.i + size;
        if wanted.contains(&appid) {
            // Info state, last updated, token, SHA-1, change number, then (28+) the binary's SHA-1.
            let header = 4 + 4 + 8 + 20 + 4 + if magic >= 0x0756_4428 { 20 } else { 0 };
            r.i += header;
            if let Some(info) = r.kv(strings.as_deref(), 0) {
                let common = info.path(&["appinfo", "common"]);
                let field = |k: &str| common.and_then(|c| c.get(k)).and_then(Kv::str);
                if field("type").is_some_and(|t| t.eq_ignore_ascii_case("game")) {
                    if let Some(name) = field("name") {
                        games.push((appid, name.to_string()));
                    }
                }
            }
        }
        r.i = next;
    }
    games
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn licenses_unscrambled() {
        // A license list (eresult 1, one license: package 21357, flags 512, another expired),
        // scrambled the client's way for account 12345678: the first bytes match the real cache.
        let mut list = vec![0x08, 0x01];
        for (package, flags) in [(21357u32, 512u64), (99, 8)] {
            let mut license = vec![0x08];
            let mut p = u64::from(package);
            while p >= 0x80 {
                license.push((p as u8) | 0x80);
                p >>= 7;
            }
            license.push(p as u8);
            license.extend([0x38, (flags as u8 & 0x7f) | if flags >= 0x80 { 0x80 } else { 0 }]);
            if flags >= 0x80 {
                license.push((flags >> 7) as u8);
            }
            list.extend([0x12, license.len() as u8]);
            list.extend(license);
        }
        let scrambled = unscramble(&list, 12345678);
        assert_eq!(&scrambled[..3], &[0x4d, 0x39, 0x41]);
        assert_eq!(decode_licenses(&unscramble(&scrambled, 12345678)), vec![21357]);
    }

    #[test]
    fn appcache_files() {
        // packageinfo 40: package 7 holding apps 10 and 80; package 8 not wanted.
        let mut p = Vec::new();
        p.extend(0x0656_5528u32.to_le_bytes());
        p.extend(1u32.to_le_bytes());
        for (id, apps) in [(7u32, [10u32, 80]), (8, [20, 30])] {
            p.extend(id.to_le_bytes());
            p.extend([0u8; 32]);
            p.push(0);
            p.extend(format!("{id}\0").bytes());
            p.push(0);
            p.extend(b"appids\0");
            for (n, a) in apps.iter().enumerate() {
                p.push(2);
                p.extend(format!("{n}\0").bytes());
                p.extend(a.to_le_bytes());
            }
            p.extend([8, 8, 8]);
        }
        p.extend(u32::MAX.to_le_bytes());
        assert_eq!(package_apps(&p, &BTreeSet::from([7])), vec![10, 80]);

        // appinfo 29: key names in the table at the end.
        let table = ["appinfo", "common", "name", "type"];
        let mut a = Vec::new();
        a.extend(0x0756_4429u32.to_le_bytes());
        a.extend(1u32.to_le_bytes());
        let offset_at = a.len();
        a.extend(0u64.to_le_bytes());
        for (id, name, kind) in [(10u32, "Counter-Strike", "Game"), (80, "Some Tool", "Tool")] {
            let mut body = vec![0u8; 4 + 4 + 8 + 20 + 4 + 20];
            body.push(0);
            body.extend(0u32.to_le_bytes());
            body.push(0);
            body.extend(1u32.to_le_bytes());
            body.push(1);
            body.extend(2u32.to_le_bytes());
            body.extend(format!("{name}\0").bytes());
            body.push(1);
            body.extend(3u32.to_le_bytes());
            body.extend(format!("{kind}\0").bytes());
            body.extend([8, 8, 8]);
            a.extend(id.to_le_bytes());
            a.extend((body.len() as u32).to_le_bytes());
            a.extend(body);
        }
        a.extend(0u32.to_le_bytes());
        let offset = a.len() as u64;
        a[offset_at..offset_at + 8].copy_from_slice(&offset.to_le_bytes());
        a.extend((table.len() as u32).to_le_bytes());
        for t in table {
            a.extend(format!("{t}\0").bytes());
        }
        assert_eq!(app_games(&a, &BTreeSet::from([10, 80])), vec![(10, "Counter-Strike".to_string())]);

        // A Steam update that changes the files must never crash a scan: cut short anywhere,
        // any byte changed, or the license cache scrambled another way, they read as less or
        // nothing.
        let all = BTreeSet::from([7, 8, 10, 80]);
        for file in [&p, &a] {
            for cut in 0..file.len() {
                package_apps(&file[..cut], &all);
                app_games(&file[..cut], &all);
            }
            for at in 0..file.len() {
                for x in [0x01, 0x80, 0xff] {
                    let mut changed = file.clone();
                    changed[at] ^= x;
                    package_apps(&changed, &all);
                    app_games(&changed, &all);
                }
            }
        }
        let mut noise = UniformRandomStream::new(1);
        for len in [0, 1, 7, 100, 5000] {
            let bytes: Vec<u8> = (0..len).map(|_| noise.int(0, 255) as u8).collect();
            decode_licenses(&bytes);
            decode_licenses(&unscramble(&bytes, 42));
            package_apps(&bytes, &all);
            app_games(&bytes, &all);
        }
    }
}
