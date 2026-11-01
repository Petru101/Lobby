//! Cover art for the cards: read and shrunk off the main thread, web covers downloaded once
//! into Lobby's covers folder (`crate::covers`).

use gtk::{gdk, gio, glib};
use std::path::{Path, PathBuf};

/// Cards are 180x270; twice that for sharp covers on scaled screens.
const WIDTH: u32 = 360;
const HEIGHT: u32 = 540;

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
            let path = if cover.starts_with("https://") { crate::covers::web_copy(&cover)? } else { PathBuf::from(&cover) };
            decode(&path)
        })
        .await
        .ok()
        .flatten();
        ready(decoded.map(|(w, h, bytes)| gdk::MemoryTexture::new(w as i32, h as i32, gdk::MemoryFormat::R8g8b8a8, &glib::Bytes::from_owned(bytes), w as usize * 4)));
    });
}
