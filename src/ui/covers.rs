//! Cover art for the cards: read and shrunk off the main thread, only for what's on screen or
//! near it, and let go again far from it (960 covers in memory were 700 MB). Web covers are
//! still all downloaded once into Lobby's covers folder (`crate::covers`).

use gtk::prelude::*;
use gtk::{gdk, gio, glib};
use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// Cards are 180x270; twice that for sharp covers on scaled screens.
const WIDTH: u32 = 360;
const HEIGHT: u32 = 540;

/// How many window heights away from the window a cover is read, and let go.
const READ_WITHIN: f64 = 1.0;
const KEEP_WITHIN: f64 = 3.0;

/// The image shrunk to the card's size, as RGBA bytes.
fn decode(path: &Path) -> Option<(u32, u32, Vec<u8>)> {
    let img = image::ImageReader::open(path).ok()?.with_guessed_format().ok()?.decode().ok()?;
    let img = if img.width() > WIDTH || img.height() > HEIGHT { img.thumbnail(WIDTH, HEIGHT) } else { img };
    let rgba = img.to_rgba8();
    Some((rgba.width(), rgba.height(), rgba.into_raw()))
}

/// Big blocks of memory go back to the system as soon as they're freed. Otherwise the C library
/// learns to keep them for reuse after the first few covers, and memory stays as high as it has
/// been (700 MB after scrolling through 800 games, 420 MB with this).
pub fn give_memory_back() {
    extern "C" {
        fn mallopt(param: i32, value: i32) -> i32;
    }
    const M_MMAP_THRESHOLD: i32 = -3;
    unsafe {
        mallopt(M_MMAP_THRESHOLD, 64 * 1024);
    }
}

/// One cover on a card. `wanted` is there from when it starts being read until it's let go:
/// clearing it stops a read that hasn't begun (a fast scroll passes many) and throws away one
/// that has.
struct Slot {
    place: glib::WeakRef<gtk::Overlay>,
    backdrop: gtk::Picture,
    picture: gtk::Picture,
    cover: String,
    /// The card's width / height.
    ratio: f64,
    wanted: RefCell<Option<Arc<AtomicBool>>>,
    unreadable: RefCell<Option<Box<dyn FnOnce()>>>,
}

thread_local! {
    static SLOTS: RefCell<Vec<Rc<Slot>>> = const { RefCell::new(Vec::new()) };
    static WATCHED: RefCell<Vec<glib::WeakRef<gtk::Adjustment>>> = const { RefCell::new(Vec::new()) };
    static FETCHED: RefCell<HashSet<String>> = RefCell::new(HashSet::new());
    static WAITING: Cell<bool> = const { Cell::new(false) };
}

impl Slot {
    fn read(self: &Rc<Self>) {
        if self.wanted.borrow().is_some() {
            return;
        }
        let wanted = Arc::new(AtomicBool::new(true));
        self.wanted.replace(Some(wanted.clone()));
        let slot = self.clone();
        glib::spawn_future_local(async move {
            let (still, cover) = (wanted.clone(), slot.cover.clone());
            let decoded = gio::spawn_blocking(move || {
                if !still.load(Ordering::Relaxed) {
                    return None;
                }
                let path = if cover.starts_with("https://") { crate::covers::web_copy(&cover)? } else { PathBuf::from(&cover) };
                decode(&path)
            })
            .await
            .ok()
            .flatten();
            if !wanted.load(Ordering::Relaxed) {
                return;
            }
            let Some((w, h, bytes)) = decoded else {
                if let Some(unreadable) = slot.unreadable.take() {
                    unreadable();
                }
                return;
            };
            let texture = gdk::MemoryTexture::new(w as i32, h as i32, gdk::MemoryFormat::R8g8b8a8, &glib::Bytes::from_owned(bytes), w as usize * 4);
            let wide = w as f64 / h as f64 > slot.ratio * 1.15;
            if wide {
                slot.backdrop.set_paintable(Some(&texture));
                slot.backdrop.set_visible(true);
            }
            slot.picture.set_content_fit(if wide { gtk::ContentFit::Contain } else { gtk::ContentFit::Cover });
            slot.picture.set_paintable(Some(&texture));
            slot.picture.set_visible(true);
        });
    }

    fn let_go(&self) {
        let Some(wanted) = self.wanted.take() else { return };
        wanted.store(false, Ordering::Relaxed);
        for picture in [&self.backdrop, &self.picture] {
            picture.set_paintable(gdk::Paintable::NONE);
            picture.set_visible(false);
        }
    }
}

/// Shows `cover` in these two pictures of `place` while `place` is on screen or near it.
/// `unreadable` runs if the file can't be read.
pub fn show(place: &gtk::Overlay, backdrop: gtk::Picture, picture: gtk::Picture, cover: &str, ratio: f64, unreadable: Option<Box<dyn FnOnce()>>) {
    // Downloaded whether it's looked at or not: it may not be there to fetch later.
    if cover.starts_with("https://") && FETCHED.with(|f| f.borrow_mut().insert(cover.to_string())) {
        let url = cover.to_string();
        drop(gio::spawn_blocking(move || {
            crate::covers::web_copy(&url);
        }));
    }
    let slot = Rc::new(Slot { place: place.downgrade(), backdrop, picture, cover: cover.to_string(), ratio, wanted: RefCell::new(None), unreadable: RefCell::new(unreadable) });
    SLOTS.with(|s| s.borrow_mut().push(slot));
    place.connect_map(|place| {
        watch_scrolling(place);
        soon();
    });
    place.connect_unmap(|_| soon());
}

/// Cards moved without anything scrolling (sorted, filtered). Looked at again once they're laid
/// out, which can take a moment with every card in the library.
pub fn look_again() {
    soon();
    glib::timeout_add_local_once(Duration::from_millis(250), soon);
}

fn watch_scrolling(place: &gtk::Overlay) {
    let mut at = place.ancestor(gtk::ScrolledWindow::static_type());
    while let Some(scrolled) = at.and_downcast::<gtk::ScrolledWindow>() {
        let adjustment = scrolled.vadjustment();
        WATCHED.with(|w| {
            let mut watched = w.borrow_mut();
            watched.retain(|a| a.upgrade().is_some());
            if !watched.iter().any(|a| a.upgrade().as_ref() == Some(&adjustment)) {
                adjustment.connect_value_changed(|_| soon());
                adjustment.connect_changed(|_| soon());
                watched.push(adjustment.downgrade());
            }
        });
        at = scrolled.parent().and_then(|p| p.ancestor(gtk::ScrolledWindow::static_type()));
    }
}

/// Everything that moves a card ends here, many times a frame while scrolling.
fn soon() {
    if !WAITING.replace(true) {
        glib::timeout_add_local_once(Duration::from_millis(40), || {
            WAITING.set(false);
            look();
        });
    }
}

fn look() {
    let mut not_laid_out = false;
    let slots = SLOTS.take();
    let slots: Vec<Rc<Slot>> = slots
        .into_iter()
        .filter(|slot| {
            let Some(place) = slot.place.upgrade() else {
                slot.let_go();
                return false;
            };
            let Some(root) = place.root().filter(|_| place.is_mapped()) else {
                slot.let_go();
                return true;
            };
            let bounds = place.compute_bounds(&root);
            let Some(bounds) = bounds.filter(|_| place.width() > 0) else {
                not_laid_out = true;
                return true;
            };
            let (top, bottom, height) = (bounds.y() as f64, (bounds.y() + bounds.height()) as f64, root.height() as f64);
            let within = |screens: f64| bottom > -screens * height && top < (1.0 + screens) * height;
            if within(READ_WITHIN) {
                slot.read();
            } else if !within(KEEP_WITHIN) {
                slot.let_go();
            }
            true
        })
        .collect();
    if std::env::var_os("LOBBY_DEBUG_COVERS").is_some() {
        let memory = std::fs::read_to_string("/proc/self/statm").ok().and_then(|t| t.split(' ').nth(1)?.parse::<u64>().ok()).unwrap_or(0) * 4 / 1024;
        eprintln!("covers: {} held of {}, {memory} MB in memory", slots.iter().filter(|s| s.wanted.borrow().is_some()).count(), slots.len());
    }
    // Cards added while looking (reading a cover adds none, but be safe about what's in the list).
    SLOTS.with(|s| {
        let mut list = s.borrow_mut();
        let new = std::mem::take(&mut *list);
        *list = slots;
        list.extend(new);
    });
    if not_laid_out {
        glib::timeout_add_local_once(Duration::from_millis(100), soon);
    }
}
