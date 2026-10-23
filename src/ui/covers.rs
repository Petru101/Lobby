//! Cover art for the cards: read and shrunk off the main thread, web covers downloaded once
//! into ~/.cache/lobby/covers.

use gtk::{gdk, gio, glib};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Cards are 180x270; twice that for sharp covers on scaled screens.
const WIDTH: u32 = 360;
const HEIGHT: u32 = 540;

fn cache_dir() -> PathBuf {
    let cache = std::env::var_os("XDG_CACHE_HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".cache"));
    cache.join("lobby/covers")
}

/// A web cover's local copy, downloaded the first time (curl, which every desktop has).
fn fetch(url: &str) -> Option<PathBuf> {
    let file = cache_dir().join(format!("{}.img", crate::sha256_hex(url.as_bytes())));
    if file.is_file() {
        return Some(file);
    }
    std::fs::create_dir_all(file.parent()?).ok()?;
    let tmp = file.with_extension("part");
    let ok = Command::new("curl").args(["-sfL", "--max-time", "30", "-o"]).arg(&tmp).arg(url).stdin(Stdio::null()).status().is_ok_and(|s| s.success());
    if !ok {
        let _ = std::fs::remove_file(&tmp);
        return None;
    }
    std::fs::rename(&tmp, &file).ok()?;
    Some(file)
}

/// The image shrunk to the card's size, as RGBA bytes.
fn decode(path: &Path) -> Option<(u32, u32, Vec<u8>)> {
    let img = image::ImageReader::open(path).ok()?.with_guessed_format().ok()?.decode().ok()?;
    let img = if img.width() > WIDTH || img.height() > HEIGHT { img.thumbnail(WIDTH, HEIGHT) } else { img };
    let rgba = img.to_rgba8();
    Some((rgba.width(), rgba.height(), rgba.into_raw()))
}

/// Calls `ready` with the cover once it's read (None: there's none to show).
pub fn load(cover: String, ready: impl FnOnce(Option<gdk::MemoryTexture>) + 'static) {
    glib::spawn_future_local(async move {
        let decoded = gio::spawn_blocking(move || {
            let path = if cover.starts_with("https://") { fetch(&cover)? } else { PathBuf::from(&cover) };
            decode(&path)
        })
        .await
        .ok()
        .flatten();
        ready(decoded.map(|(w, h, bytes)| gdk::MemoryTexture::new(w as i32, h as i32, gdk::MemoryFormat::R8g8b8a8, &glib::Bytes::from_owned(bytes), w as usize * 4)));
    });
}
