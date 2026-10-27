//! Controllers: Lobby's window moved around and used with a gamepad. GTK has no gamepad input,
//! so the devices are read here: every /dev/input/event* with a gamepad's south button, in the
//! kernel's gamepad layout (xpad, hid-playstation, hid-nintendo and Steam's virtual controller
//! follow it), by a thread of their own that hands actions to the window's thread. The window
//! acts on them only while it's the active window, so a game started from it has the
//! controller to itself. New controllers are noticed through inotify on /dev/input.
//!
//! Moving is spatial (`spatial`): the nearest focusable widget in the direction pressed, among
//! the cards, buttons and rows of what's on top (a popover, the open dialog, else the window).

use adw::prelude::*;
use std::collections::HashMap;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dir {
    Up,
    Down,
    Left,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Action {
    Move(Dir),
    /// A: press what's focused.
    Accept,
    /// B: close what's on top.
    Back,
    /// X or Y (which is which differs between Xbox and PlayStation drivers): a card's menu.
    Menu,
    /// Start: play the focused game.
    Play,
    /// LB / RB: the pages.
    Page(i32),
    /// LT / RT: the sources.
    Source(i32),
    /// The right stick up or down: scroll, -1 to 1.
    Scroll(f64),
}

/// A held direction repeats after this, then every `REPEAT`.
const FIRST_REPEAT: Duration = Duration::from_millis(400);
const REPEAT: Duration = Duration::from_millis(120);
const SCROLL_TICK: Duration = Duration::from_millis(16);

const EV_KEY: u16 = 0x01;
const EV_ABS: u16 = 0x03;
const BTN_SOUTH: u16 = 0x130;
const BTN_EAST: u16 = 0x131;
const BTN_NORTH: u16 = 0x133;
const BTN_WEST: u16 = 0x134;
const BTN_TL: u16 = 0x136;
const BTN_TR: u16 = 0x137;
const BTN_TL2: u16 = 0x138;
const BTN_TR2: u16 = 0x139;
const BTN_START: u16 = 0x13b;
/// The pad as buttons (up, down, left, right), on drivers without a hat.
const BTN_DPAD_UP: u16 = 0x220;
const BTN_DPAD_RIGHT: u16 = 0x223;
const ABS_X: u16 = 0x00;
const ABS_Y: u16 = 0x01;
const ABS_Z: u16 = 0x02;
const ABS_RX: u16 = 0x03;
const ABS_RY: u16 = 0x04;
const ABS_RZ: u16 = 0x05;
const ABS_HAT0X: u16 = 0x10;
const ABS_HAT0Y: u16 = 0x11;

/// _IOC(_IOC_READ, 'E', nr, size): the evdev ioctls that read.
fn eviocg(nr: u64, size: usize) -> u64 {
    (2 << 30) | ((size as u64) << 16) | ((b'E' as u64) << 8) | nr
}

fn bits(fd: i32, ev: u16) -> Vec<u8> {
    let mut buf = vec![0u8; 96];
    let r = unsafe { libc::ioctl(fd, eviocg(0x20 + ev as u64, buf.len()) as _, buf.as_mut_ptr()) };
    if r < 0 {
        buf.clear();
    }
    buf
}

fn has(bits: &[u8], code: u16) -> bool {
    bits.get(code as usize / 8).is_some_and(|b| b & (1 << (code % 8)) != 0)
}

#[repr(C)]
#[derive(Default)]
struct AbsInfo {
    value: i32,
    minimum: i32,
    maximum: i32,
    fuzz: i32,
    flat: i32,
    resolution: i32,
}

fn abs_range(fd: i32, code: u16) -> Option<(i32, i32)> {
    let mut info = AbsInfo::default();
    let r = unsafe { libc::ioctl(fd, eviocg(0x40 + code as u64, std::mem::size_of::<AbsInfo>()) as _, &mut info) };
    (r >= 0 && info.maximum > info.minimum).then_some((info.minimum, info.maximum))
}

/// One controller and where its sticks, triggers and pad are now.
struct Pad {
    fd: OwnedFd,
    path: PathBuf,
    ranges: HashMap<u16, (i32, i32)>,
    /// A right stick on RX/RY: then Z/RZ are the triggers (on some generic pads Z/RZ are the
    /// right stick).
    triggers_on_z: bool,
    stick: (f64, f64),
    stick_dir: Option<Dir>,
    hat: (i32, i32),
    dpad: [bool; 4],
    right_y: f64,
    triggers: [bool; 2],
}

/// The controllers Steam says to leave alone (`0x28de/0x11ff,…`, vendor/product): running a
/// program, Steam passes the controller on as a virtual one, and the real one would act twice.
fn ignored_by_steam(list: &str) -> Vec<(u16, u16)> {
    list.split(',')
        .filter_map(|d| {
            let (v, p) = d.trim().split_once('/')?;
            let hex = |s: &str| u16::from_str_radix(s.trim().trim_start_matches("0x").trim_start_matches("0X"), 16).ok();
            Some((hex(v)?, hex(p)?))
        })
        .collect()
}

/// The device's vendor and product (struct input_id: bus, vendor, product, version).
fn ids(fd: i32) -> Option<(u16, u16)> {
    let mut id = [0u16; 4];
    let r = unsafe { libc::ioctl(fd, eviocg(0x02, 8) as _, id.as_mut_ptr()) };
    (r >= 0).then_some((id[1], id[2]))
}

impl Pad {
    fn open(path: &Path) -> Option<Pad> {
        let name = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).ok()?;
        let fd = unsafe { libc::open(name.as_ptr(), libc::O_RDONLY | libc::O_NONBLOCK | libc::O_CLOEXEC) };
        if fd < 0 {
            return None;
        }
        let fd = unsafe { OwnedFd::from_raw_fd(fd) };
        if !has(&bits(fd.as_raw_fd(), EV_KEY), BTN_SOUTH) {
            return None;
        }
        let ignored = std::env::var("SDL_GAMECONTROLLER_IGNORE_DEVICES").map(|l| ignored_by_steam(&l)).unwrap_or_default();
        if ids(fd.as_raw_fd()).is_some_and(|id| ignored.contains(&id)) {
            return None;
        }
        let abs = bits(fd.as_raw_fd(), EV_ABS);
        let ranges = [ABS_X, ABS_Y, ABS_Z, ABS_RX, ABS_RY, ABS_RZ].into_iter().filter(|&c| has(&abs, c)).filter_map(|c| Some((c, abs_range(fd.as_raw_fd(), c)?))).collect();
        Some(Pad { fd, path: path.to_path_buf(), ranges, triggers_on_z: has(&abs, ABS_RX), stick: (0.0, 0.0), stick_dir: None, hat: (0, 0), dpad: [false; 4], right_y: 0.0, triggers: [false; 2] })
    }

    /// An axis's value from -1 to 1 (a trigger's from 0 to 1 when its range starts at 0).
    fn norm(&self, code: u16, value: i32) -> f64 {
        let Some(&(min, max)) = self.ranges.get(&code) else { return 0.0 };
        if min >= 0 && matches!(code, ABS_Z | ABS_RZ) && self.triggers_on_z {
            return (value - min) as f64 / (max - min) as f64;
        }
        let mid = (min + max) as f64 / 2.0;
        ((value as f64 - mid) / ((max - min) as f64 / 2.0)).clamp(-1.0, 1.0)
    }

    /// The direction held: the pad's first, else the stick's.
    fn held(&self) -> Option<Dir> {
        let pad = match (self.hat, self.dpad) {
            ((_, -1), _) | (_, [true, _, _, _]) => Some(Dir::Up),
            ((_, 1), _) | (_, [_, true, _, _]) => Some(Dir::Down),
            ((-1, _), _) | (_, [_, _, true, _]) => Some(Dir::Left),
            ((1, _), _) | (_, [_, _, _, true]) => Some(Dir::Right),
            _ => None,
        };
        pad.or(self.stick_dir)
    }

    /// Takes one event in; returns the button action it is, if any.
    fn event(&mut self, kind: u16, code: u16, value: i32) -> Option<Action> {
        match (kind, code) {
            (EV_KEY, _) if value == 1 => match code {
                BTN_SOUTH => Some(Action::Accept),
                BTN_EAST => Some(Action::Back),
                BTN_NORTH | BTN_WEST => Some(Action::Menu),
                BTN_START => Some(Action::Play),
                BTN_TL => Some(Action::Page(-1)),
                BTN_TR => Some(Action::Page(1)),
                BTN_TL2 => Some(Action::Source(-1)),
                BTN_TR2 => Some(Action::Source(1)),
                BTN_DPAD_UP..=BTN_DPAD_RIGHT => {
                    self.dpad[(code - BTN_DPAD_UP) as usize] = true;
                    None
                }
                _ => None,
            },
            (EV_KEY, BTN_DPAD_UP..=BTN_DPAD_RIGHT) if value == 0 => {
                self.dpad[(code - BTN_DPAD_UP) as usize] = false;
                None
            }
            (EV_ABS, ABS_HAT0X) => {
                self.hat.0 = value.signum();
                None
            }
            (EV_ABS, ABS_HAT0Y) => {
                self.hat.1 = value.signum();
                None
            }
            (EV_ABS, ABS_X | ABS_Y) => {
                let v = self.norm(code, value);
                if code == ABS_X { self.stick.0 = v } else { self.stick.1 = v }
                let (x, y) = self.stick;
                // Past halfway it points somewhere; back under a third it's let go.
                let far = x.abs().max(y.abs());
                if far > 0.55 {
                    self.stick_dir = Some(if x.abs() > y.abs() { if x > 0.0 { Dir::Right } else { Dir::Left } } else if y > 0.0 { Dir::Down } else { Dir::Up });
                } else if far < 0.35 {
                    self.stick_dir = None;
                }
                None
            }
            (EV_ABS, ABS_RY) => {
                self.right_y = self.norm(code, value);
                None
            }
            (EV_ABS, ABS_Z | ABS_RZ) if self.triggers_on_z => {
                let i = (code == ABS_RZ) as usize;
                let v = self.norm(code, value);
                let was = self.triggers[i];
                self.triggers[i] = if was { v > 0.3 } else { v > 0.6 };
                (!was && self.triggers[i]).then_some(Action::Source(if i == 0 { -1 } else { 1 }))
            }
            _ => None,
        }
    }
}

/// Reads every controller, now and plugged in later, for as long as Lobby runs, calling `send`
/// with each action (from this thread: `send` passes it on to the window's).
pub fn watch(send: impl Fn(Action) + Send + 'static) {
    let _ = std::thread::Builder::new().name("lobby-pads".into()).spawn(move || read(send));
}

fn read(send: impl Fn(Action)) {
    let dir = Path::new("/dev/input");
    let mut pads: Vec<Pad> = Vec::new();
    let add = |pads: &mut Vec<Pad>, path: PathBuf| {
        if !pads.iter().any(|p| p.path == path) && path.file_name().is_some_and(|n| n.to_string_lossy().starts_with("event")) {
            if let Some(p) = Pad::open(&path) {
                crate::log::line(&format!("controller: {}", path.display()));
                pads.push(p);
            }
        }
    };
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        add(&mut pads, e.path());
    }
    // New devices: created, then given to the user (udev sets the ACL just after).
    let watch = unsafe { libc::inotify_init1(libc::IN_NONBLOCK | libc::IN_CLOEXEC) };
    let watch = (watch >= 0).then(|| unsafe { OwnedFd::from_raw_fd(watch) });
    if let Some(w) = &watch {
        let name = std::ffi::CString::new("/dev/input").unwrap();
        unsafe { libc::inotify_add_watch(w.as_raw_fd(), name.as_ptr(), libc::IN_CREATE | libc::IN_ATTRIB) };
    }
    let mut held: Option<Dir> = None;
    let mut next_repeat = Instant::now();
    let mut next_scroll = Instant::now();
    let mut last_press: Option<(Action, PathBuf, Instant)> = None;
    loop {
        let scrolling = pads.iter().map(|p| p.right_y).find(|v| v.abs() > 0.25);
        let now = Instant::now();
        let wait = match (held, scrolling) {
            (Some(_), Some(_)) => SCROLL_TICK.min(next_repeat.saturating_duration_since(now)),
            (None, Some(_)) => SCROLL_TICK,
            (Some(_), None) => next_repeat.saturating_duration_since(now),
            (None, None) => Duration::from_secs(3600),
        };
        let mut fds: Vec<libc::pollfd> = pads.iter().map(|p| libc::pollfd { fd: p.fd.as_raw_fd(), events: libc::POLLIN, revents: 0 }).collect();
        if let Some(w) = &watch {
            fds.push(libc::pollfd { fd: w.as_raw_fd(), events: libc::POLLIN, revents: 0 });
        }
        unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as _, wait.as_millis().min(i32::MAX as u128) as i32) };
        // Events.
        let mut gone = Vec::new();
        for (i, p) in pads.iter_mut().enumerate() {
            if fds[i].revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0 {
                gone.push(i);
                continue;
            }
            if fds[i].revents & libc::POLLIN == 0 {
                continue;
            }
            let mut buf = [0u8; 24 * 64];
            loop {
                let n = unsafe { libc::read(p.fd.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len()) };
                if n <= 0 {
                    if n == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::ENODEV) {
                        gone.push(i);
                    }
                    break;
                }
                // struct input_event: a timeval (16 bytes), type, code, value.
                for e in buf[..n as usize].chunks_exact(24) {
                    let kind = u16::from_ne_bytes([e[16], e[17]]);
                    let code = u16::from_ne_bytes([e[18], e[19]]);
                    let value = i32::from_ne_bytes([e[20], e[21], e[22], e[23]]);
                    if let Some(a) = p.event(kind, code, value) {
                        // The same press from two devices (one controller seen twice): once.
                        let twice = last_press.as_ref().is_some_and(|(la, lp, at): &(Action, PathBuf, Instant)| *la == a && *lp != p.path && at.elapsed() < Duration::from_millis(40));
                        if !twice {
                            send(a);
                        }
                        last_press = Some((a, p.path.clone(), Instant::now()));
                    }
                }
            }
        }
        for i in gone.into_iter().rev() {
            crate::log::line(&format!("controller gone: {}", pads[i].path.display()));
            pads.remove(i);
        }
        // Devices plugged in.
        if let Some(w) = &watch {
            if fds.last().is_some_and(|f| f.revents & libc::POLLIN != 0) {
                let mut buf = [0u8; 4096];
                let n = unsafe { libc::read(w.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len()) };
                let mut at = 0usize;
                while n > 0 && at + 16 <= n as usize {
                    // struct inotify_event: wd, mask, cookie, len, then the name.
                    let len = u32::from_ne_bytes(buf[at + 12..at + 16].try_into().unwrap()) as usize;
                    let name = &buf[at + 16..(at + 16 + len).min(n as usize)];
                    let name = String::from_utf8_lossy(name.split(|&b| b == 0).next().unwrap_or_default()).into_owned();
                    add(&mut pads, dir.join(name));
                    at += 16 + len;
                }
            }
        }
        // A direction pressed, or still held.
        let now_held = pads.iter().find_map(Pad::held);
        let now = Instant::now();
        if now_held != held {
            held = now_held;
            if let Some(d) = held {
                send(Action::Move(d));
                next_repeat = now + FIRST_REPEAT;
            }
        } else if let Some(d) = held.filter(|_| now >= next_repeat) {
            send(Action::Move(d));
            next_repeat = now + REPEAT;
        }
        // Scrolling: a step per tick, not per event (a stick reports many a second).
        if let Some(v) = pads.iter().map(|p| p.right_y).find(|v| v.abs() > 0.25) {
            if now >= next_scroll {
                send(Action::Scroll(v));
                next_scroll = now + SCROLL_TICK;
            }
        }
    }
}

/// The widgets a controller can move to under `root`: cards, buttons, switches, entries and
/// activatable rows that show and can be used now. Not what's inside a card (its Play button:
/// Start plays) nor popovers (their own root while open).
pub fn focusables(root: &gtk::Widget) -> Vec<gtk::Widget> {
    let mut out = Vec::new();
    walk(root, root, &mut out);
    out
}

fn walk(w: &gtk::Widget, root: &gtk::Widget, out: &mut Vec<gtk::Widget>) {
    if !w.is_visible() || !w.is_mapped() || !w.is_sensitive() || !w.can_focus() || (w != root && w.is::<gtk::Popover>()) {
        return;
    }
    let usable = w.is::<gtk::FlowBoxChild>()
        || w.is::<gtk::Button>()
        || w.is::<gtk::CheckButton>()
        || w.is::<gtk::Switch>()
        || w.is::<gtk::Text>()
        || w.downcast_ref::<gtk::ListBoxRow>().is_some_and(|r| r.is_activatable())
        || w.type_().name() == "GtkListItemWidget";
    if usable && w.is_focusable() && w.width() > 0 && w.height() > 0 {
        out.push(w.clone());
    }
    if w.is::<gtk::FlowBoxChild>() {
        return;
    }
    let mut child = w.first_child();
    while let Some(c) = child {
        walk(&c, root, out);
        child = c.next_sibling();
    }
}

/// An open popover under `root` (a card's menu, the Show menu, a drop-down's list).
pub fn open_popover(root: &gtk::Widget) -> Option<gtk::Popover> {
    if let Some(p) = root.downcast_ref::<gtk::Popover>() {
        return p.is_visible().then(|| p.clone());
    }
    if !root.is_visible() {
        return None;
    }
    let mut child = root.first_child();
    while let Some(c) = child {
        if let Some(p) = open_popover(&c) {
            return Some(p);
        }
        child = c.next_sibling();
    }
    None
}

/// Where a controller starts under `root`: the first game card, else the main button (Play on a
/// game's page), else the widget nearest the top left.
pub fn first(root: &gtk::Widget) -> Option<gtk::Widget> {
    let all = focusables(root);
    let place = |w: &gtk::Widget| w.compute_bounds(root).map_or((f32::MAX, f32::MAX), |b| (b.y(), b.x()));
    let top_left = |ws: Vec<&gtk::Widget>| ws.into_iter().min_by(|a, b| place(a).partial_cmp(&place(b)).unwrap_or(std::cmp::Ordering::Equal)).cloned();
    top_left(all.iter().filter(|w| w.is::<gtk::FlowBoxChild>()).collect())
        .or_else(|| top_left(all.iter().filter(|w| w.has_css_class("suggested-action")).collect()))
        .or_else(|| top_left(all.iter().collect()))
}

/// The nearest widget from `from` in direction `dir`: ahead of it, preferring ones in its line
/// (overlapping it across the direction), then the closest.
pub fn spatial(root: &gtk::Widget, from: &gtk::Widget, dir: Dir) -> Option<gtk::Widget> {
    let f = from.compute_bounds(root)?;
    let gap = |a: f32, al: f32, b: f32, bl: f32| if a + al <= b { b - (a + al) } else if b + bl <= a { a - (b + bl) } else { 0.0 };
    let mut best: Option<(f32, gtk::Widget)> = None;
    for w in focusables(root) {
        if &w == from || w.is_ancestor(from) {
            continue;
        }
        let Some(c) = w.compute_bounds(root) else { continue };
        let (fc, cc) = ((f.x() + f.width() / 2.0, f.y() + f.height() / 2.0), (c.x() + c.width() / 2.0, c.y() + c.height() / 2.0));
        let (beyond, ahead, off) = match dir {
            Dir::Right => (cc.0 > fc.0 + 1.0, c.x() - (f.x() + f.width()), gap(c.y(), c.height(), f.y(), f.height())),
            Dir::Left => (cc.0 < fc.0 - 1.0, f.x() - (c.x() + c.width()), gap(c.y(), c.height(), f.y(), f.height())),
            Dir::Down => (cc.1 > fc.1 + 1.0, c.y() - (f.y() + f.height()), gap(c.x(), c.width(), f.x(), f.width())),
            Dir::Up => (cc.1 < fc.1 - 1.0, f.y() - (c.y() + c.height()), gap(c.x(), c.width(), f.x(), f.width())),
        };
        // Overlapping it along the direction (a row inside the same group) still counts when
        // its middle is past. Sideways only within its own line: at the end of a row of cards,
        // Right stays put rather than jumping to another row.
        if !beyond || (matches!(dir, Dir::Left | Dir::Right) && off > 0.0) {
            continue;
        }
        let along = match dir {
            Dir::Left | Dir::Right => (cc.1 - fc.1).abs(),
            Dir::Up | Dir::Down => (cc.0 - fc.0).abs(),
        };
        let score = ahead.max(0.0) + off * 4.0 + along * 0.05;
        if std::env::var("LOBBY_DEBUG_PAD").is_ok_and(|v| v == "2") {
            eprintln!("  {score:.0}: {}", describe(&w));
        }
        if best.as_ref().is_none_or(|(s, _)| score < *s) {
            best = Some((score, w));
        }
    }
    best.map(|(_, w)| w)
}

/// What a widget is, for the log (LOBBY_DEBUG_PAD).
pub fn describe(w: &gtk::Widget) -> String {
    let text = w
        .downcast_ref::<gtk::Button>()
        .and_then(|b| b.label().or_else(|| b.tooltip_text()))
        .or_else(|| w.downcast_ref::<adw::PreferencesRow>().map(|r| r.title()))
        .or_else(|| w.tooltip_text())
        .map(|t| t.to_string())
        .unwrap_or_default();
    let at = w.root().and_then(|r| w.compute_bounds(&r)).map(|b| format!("at {:.0},{:.0}", b.x(), b.y())).unwrap_or_else(|| "nowhere".into());
    format!("{} {} {} mapped={} {at}", w.type_().name(), w.widget_name(), text, w.is_mapped())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pad() -> Pad {
        let fd = unsafe { OwnedFd::from_raw_fd(libc::dup(0)) };
        let ranges = [(ABS_X, (-32768, 32767)), (ABS_Y, (-32768, 32767)), (ABS_RY, (-32768, 32767)), (ABS_Z, (0, 255)), (ABS_RZ, (0, 255))].into_iter().collect();
        Pad { fd, path: PathBuf::new(), ranges, triggers_on_z: true, stick: (0.0, 0.0), stick_dir: None, hat: (0, 0), dpad: [false; 4], right_y: 0.0, triggers: [false; 2] }
    }

    #[test]
    fn steams_ignore_list() {
        assert_eq!(ignored_by_steam("0x045e/0x028e, 0x054C/0x0CE6,bad"), vec![(0x045e, 0x028e), (0x054c, 0x0ce6)]);
        assert!(ignored_by_steam("").is_empty());
    }

    #[test]
    fn buttons_sticks_and_triggers() {
        let mut p = pad();
        assert_eq!(p.event(EV_KEY, BTN_SOUTH, 1), Some(Action::Accept));
        assert_eq!(p.event(EV_KEY, BTN_SOUTH, 0), None);
        assert_eq!(p.event(EV_KEY, BTN_TR, 1), Some(Action::Page(1)));
        // The stick: past halfway it points; a little back it still does; near the middle not.
        p.event(EV_ABS, ABS_X, 30000);
        assert_eq!(p.held(), Some(Dir::Right));
        p.event(EV_ABS, ABS_X, 14000);
        assert_eq!(p.held(), Some(Dir::Right));
        p.event(EV_ABS, ABS_X, 3000);
        assert_eq!(p.held(), None);
        p.event(EV_ABS, ABS_Y, -32768);
        assert_eq!(p.held(), Some(Dir::Up));
        // The pad wins over the stick.
        p.event(EV_ABS, ABS_HAT0Y, 1);
        assert_eq!(p.held(), Some(Dir::Down));
        p.event(EV_ABS, ABS_HAT0Y, 0);
        assert_eq!(p.held(), Some(Dir::Up));
        // A trigger counts once per press.
        assert_eq!(p.event(EV_ABS, ABS_RZ, 200), Some(Action::Source(1)));
        assert_eq!(p.event(EV_ABS, ABS_RZ, 255), None);
        assert_eq!(p.event(EV_ABS, ABS_RZ, 0), None);
        assert_eq!(p.event(EV_ABS, ABS_Z, 255), Some(Action::Source(-1)));
    }
}
