//! A controller as keyboard and mouse, for a game that has no controller support of its own:
//! while `lobby sandbox` waits for the game, a thread reads every controller and presses keys,
//! moves and clicks the mouse through a keyboard-and-mouse device of Lobby's own (uinput). The
//! desktop (KWin, gamescope ...) takes it as one more keyboard, so the keys reach the game's
//! window like typed ones, whatever starts the game and whether it's protected or not.
//!
//! What Steam Input does for what Steam starts, without Steam: a game's layout is a word
//! (`arrows`, `wasd`) and changes (`a=z b=x`), kept in `Game.controller` (`lobby controller`).

use crate::evdev::{self, *};
use std::collections::{BTreeSet, HashMap};
use std::os::fd::{AsRawFd, OwnedFd};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// A layout for one start, over the game's own (for trying one out).
pub const ENV: &str = "LOBBY_CONTROLLER_LAYOUT";
/// Checking runs: the device is made readable (Valve's vendor id, which Steam's udev rules give
/// the user) and writes only while another program holds it for itself (EVIOCGRAB), so no key
/// can reach the desktop's windows; the checking program passes them to a test display.
pub const TEST_ENV: &str = "LOBBY_KEYS_TEST";
pub const DEVICE_NAME: &str = "Lobby controller keys";

/// What a controller has to press: buttons, the pad, the left stick's directions, the right
/// stick's (when it isn't the mouse).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Input {
    A,
    B,
    X,
    Y,
    Lb,
    Rb,
    Lt,
    Rt,
    Back,
    Start,
    Ls,
    Rs,
    Up,
    Down,
    Left,
    Right,
    LUp,
    LDown,
    LLeft,
    LRight,
    RUp,
    RDown,
    RLeft,
    RRight,
}

const INPUTS: [(&str, Input); 24] = [
    ("a", Input::A),
    ("b", Input::B),
    ("x", Input::X),
    ("y", Input::Y),
    ("lb", Input::Lb),
    ("rb", Input::Rb),
    ("lt", Input::Lt),
    ("rt", Input::Rt),
    ("back", Input::Back),
    ("start", Input::Start),
    ("ls", Input::Ls),
    ("rs", Input::Rs),
    ("up", Input::Up),
    ("down", Input::Down),
    ("left", Input::Left),
    ("right", Input::Right),
    ("lup", Input::LUp),
    ("ldown", Input::LDown),
    ("lleft", Input::LLeft),
    ("lright", Input::LRight),
    ("rup", Input::RUp),
    ("rdown", Input::RDown),
    ("rleft", Input::RLeft),
    ("rright", Input::RRight),
];

/// Key names and their kernel codes (input-event-codes.h); mouse buttons are 0x110-0x112.
const KEYS: &[(&str, u16)] = &[
    ("escape", 1), ("esc", 1), ("1", 2), ("2", 3), ("3", 4), ("4", 5), ("5", 6), ("6", 7), ("7", 8), ("8", 9), ("9", 10), ("0", 11),
    ("minus", 12), ("equal", 13), ("backspace", 14), ("tab", 15),
    ("q", 16), ("w", 17), ("e", 18), ("r", 19), ("t", 20), ("y", 21), ("u", 22), ("i", 23), ("o", 24), ("p", 25),
    ("enter", 28), ("ctrl", 29), ("lctrl", 29),
    ("a", 30), ("s", 31), ("d", 32), ("f", 33), ("g", 34), ("h", 35), ("j", 36), ("k", 37), ("l", 38),
    ("shift", 42), ("lshift", 42),
    ("z", 44), ("x", 45), ("c", 46), ("v", 47), ("b", 48), ("n", 49), ("m", 50),
    ("rshift", 54), ("alt", 56), ("lalt", 56), ("space", 57),
    ("f1", 59), ("f2", 60), ("f3", 61), ("f4", 62), ("f5", 63), ("f6", 64), ("f7", 65), ("f8", 66), ("f9", 67), ("f10", 68),
    ("f11", 87), ("f12", 88), ("rctrl", 97), ("ralt", 100),
    ("home", 102), ("up", 103), ("pageup", 104), ("left", 105), ("right", 106), ("end", 107), ("down", 108), ("pagedown", 109),
    ("mouse1", 0x110), ("mouse2", 0x111),
];

fn key_code(name: &str) -> Option<u16> {
    match name {
        "click" | "lmb" => Some(0x110),
        "rmb" => Some(0x111),
        "mouse3" | "mmb" => Some(0x112),
        "insert" => Some(110),
        "delete" => Some(111),
        _ => KEYS.iter().find(|(n, _)| *n == name).map(|&(_, c)| c),
    }
}

fn key_name(code: u16) -> String {
    match code {
        0x110 => "click".into(),
        0x111 => "rmb".into(),
        0x112 => "mmb".into(),
        _ => KEYS.iter().find(|&&(_, c)| c == code).map_or(format!("key{code}"), |(n, _)| n.to_string()),
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Layout {
    pub keys: HashMap<Input, u16>,
    /// The right stick moves the mouse (unless its directions are keys).
    pub mouse: bool,
}

/// The layouts by name: `arrows` for menus and old 2D games, `wasd` for games played with
/// keyboard and mouse.
const LAYOUTS: [(&str, &str); 2] = [
    ("arrows", "up=up down=down left=left right=right lup=up ldown=down lleft=left lright=right a=enter b=escape x=space y=ctrl lb=shift rb=alt lt=rmb rt=click back=tab start=escape"),
    ("wasd", "lup=w ldown=s lleft=a lright=d up=1 right=2 down=3 left=4 a=space b=ctrl x=r y=e lb=q rb=f lt=rmb rt=click ls=shift rs=mmb back=tab start=escape"),
];

impl Layout {
    /// A layout's text: a layout's name (none: `arrows`), then changes, `a=z`, `b=none`,
    /// `rstick=keys` (the right stick's directions as keys, `rup=` ...), `rstick=mouse`.
    pub fn parse(text: &str) -> Result<Layout, String> {
        let mut words: Vec<&str> = text.split_whitespace().collect();
        let base = match words.first().and_then(|w| LAYOUTS.iter().find(|(n, _)| n == w)) {
            Some(&(_, base)) => {
                words.remove(0);
                base
            }
            None => LAYOUTS[0].1,
        };
        let mut layout = Layout { keys: HashMap::new(), mouse: true };
        for word in base.split_whitespace().chain(words.iter().copied()) {
            let lower = word.to_lowercase();
            let (input, key) = lower.split_once('=').ok_or_else(|| format!("\"{word}\": a layout is arrows or wasd, then changes like a=z"))?;
            if input == "rstick" {
                layout.mouse = match key {
                    "mouse" => true,
                    "keys" => false,
                    _ => return Err(format!("\"{word}\": rstick=mouse or rstick=keys")),
                };
                continue;
            }
            let input = INPUTS.iter().find(|(n, _)| *n == input).map(|&(_, i)| i).ok_or_else(|| format!("\"{input}\" isn't a controller button: {}", INPUTS.iter().map(|(n, _)| *n).collect::<Vec<_>>().join(" ")))?;
            if key == "none" || key.is_empty() {
                layout.keys.remove(&input);
            } else {
                layout.keys.insert(input, key_code(key).ok_or_else(|| format!("\"{key}\" isn't a key Lobby knows (letters, digits, f1-f12, enter, escape, space, tab, shift, ctrl, alt, arrows, click, rmb, mmb ...)"))?);
            }
        }
        if layout.keys.keys().any(|i| matches!(i, Input::RUp | Input::RDown | Input::RLeft | Input::RRight)) && !text.to_lowercase().contains("rstick=mouse") {
            layout.mouse = false;
        }
        Ok(layout)
    }

    /// For people: "A enter, B escape, …, right stick: mouse".
    pub fn describe(&self) -> String {
        let mut parts: Vec<String> = INPUTS.iter().filter_map(|(n, i)| self.keys.get(i).map(|&k| format!("{} {}", n.to_uppercase(), key_name(k)))).collect();
        parts.push(format!("right stick: {}", if self.mouse { "mouse" } else { "keys" }));
        parts.join(", ")
    }
}

/// Starts the thread for as long as this program runs; says in its log what it does.
pub fn start(text: &str) -> Result<(), String> {
    let layout = Layout::parse(text)?;
    let test = std::env::var_os(TEST_ENV).is_some();
    // Valve's vendor id only in checking runs (see TEST_ENV); pid.codes' otherwise.
    let (vendor, product) = if test { (0x28de, 0x4c42) } else { (0x1209, 0x4c42) };
    let out = evdev::Virtual::create(DEVICE_NAME, vendor, product)?;
    eprintln!("lobby: controller as keys ({}), {}", out.node.as_ref().map_or("?".into(), |n| n.display().to_string()), layout.describe());
    std::thread::Builder::new().name("lobby-pad-keys".into()).spawn(move || run(layout, out, test)).map_err(|e| e.to_string())?;
    Ok(())
}

/// One controller and what it holds.
struct Pad {
    fd: OwnedFd,
    path: PathBuf,
    ranges: HashMap<u16, (i32, i32)>,
    /// A right stick on RX/RY: then Z/RZ are the triggers.
    triggers_on_z: bool,
    /// PlayStation's driver puts the top button where Xbox's has X (BTN_NORTH/BTN_WEST).
    playstation: bool,
    held: BTreeSet<Input>,
    left: (f64, f64),
    right: (f64, f64),
}

const STICK_ON: f64 = 0.5;
const STICK_OFF: f64 = 0.35;
const TRIGGER_ON: f64 = 0.5;
const MOUSE_DEAD: f64 = 0.15;
/// Pixels a tick at full tilt, and the tick.
const MOUSE_SPEED: f64 = 18.0;
const MOUSE_TICK: Duration = Duration::from_millis(8);

impl Pad {
    fn open(path: &Path) -> Option<Pad> {
        let fd = evdev::open(path, libc::O_RDONLY)?;
        let raw = fd.as_raw_fd();
        if !has(&bits(raw, EV_KEY), BTN_SOUTH) {
            return None;
        }
        let abs = bits(raw, EV_ABS);
        let ranges = [ABS_X, ABS_Y, ABS_Z, ABS_RX, ABS_RY, ABS_RZ].into_iter().filter(|&c| has(&abs, c)).filter_map(|c| Some((c, abs_range(raw, c)?))).collect();
        let playstation = ids(raw).is_some_and(|(v, _)| v == 0x054c);
        Some(Pad { fd, path: path.to_path_buf(), ranges, triggers_on_z: has(&abs, ABS_RX), playstation, held: BTreeSet::new(), left: (0.0, 0.0), right: (0.0, 0.0) })
    }

    fn norm(&self, code: u16, value: i32) -> f64 {
        let Some(&(min, max)) = self.ranges.get(&code) else { return 0.0 };
        if min >= 0 && matches!(code, ABS_Z | ABS_RZ) {
            return (value - min) as f64 / (max - min) as f64;
        }
        let mid = (min + max) as f64 / 2.0;
        ((value as f64 - mid) / ((max - min) as f64 / 2.0)).clamp(-1.0, 1.0)
    }

    fn set(&mut self, input: Input, on: bool) {
        if on {
            self.held.insert(input);
        } else {
            self.held.remove(&input);
        }
    }

    /// A stick's two directions on one axis, with some give so it doesn't flicker.
    fn stick(&mut self, v: f64, neg: Input, pos: Input) {
        for (input, side) in [(neg, -v), (pos, v)] {
            let was = self.held.contains(&input);
            self.set(input, if was { side > STICK_OFF } else { side > STICK_ON });
        }
    }

    fn event(&mut self, kind: u16, code: u16, value: i32) {
        match (kind, code) {
            (EV_KEY, _) => {
                let (x, y) = if self.playstation { (BTN_WEST, BTN_NORTH) } else { (BTN_NORTH, BTN_WEST) };
                let input = match code {
                    BTN_SOUTH => Input::A,
                    BTN_EAST => Input::B,
                    c if c == x => Input::X,
                    c if c == y => Input::Y,
                    BTN_TL => Input::Lb,
                    BTN_TR => Input::Rb,
                    BTN_TL2 => Input::Lt,
                    BTN_TR2 => Input::Rt,
                    BTN_SELECT => Input::Back,
                    BTN_START => Input::Start,
                    BTN_THUMBL => Input::Ls,
                    BTN_THUMBR => Input::Rs,
                    BTN_DPAD_UP => Input::Up,
                    BTN_DPAD_DOWN => Input::Down,
                    BTN_DPAD_LEFT => Input::Left,
                    BTN_DPAD_RIGHT => Input::Right,
                    _ => return,
                };
                self.set(input, value != 0);
            }
            (EV_ABS, ABS_HAT0X) => {
                self.set(Input::Left, value < 0);
                self.set(Input::Right, value > 0);
            }
            (EV_ABS, ABS_HAT0Y) => {
                self.set(Input::Up, value < 0);
                self.set(Input::Down, value > 0);
            }
            (EV_ABS, ABS_X) => {
                self.left.0 = self.norm(code, value);
                self.stick(self.left.0, Input::LLeft, Input::LRight);
            }
            (EV_ABS, ABS_Y) => {
                self.left.1 = self.norm(code, value);
                self.stick(self.left.1, Input::LUp, Input::LDown);
            }
            (EV_ABS, ABS_RX) => {
                self.right.0 = self.norm(code, value);
                self.stick(self.right.0, Input::RLeft, Input::RRight);
            }
            (EV_ABS, ABS_RY) => {
                self.right.1 = self.norm(code, value);
                self.stick(self.right.1, Input::RUp, Input::RDown);
            }
            (EV_ABS, ABS_Z | ABS_RZ) if self.triggers_on_z => {
                let v = self.norm(code, value);
                self.set(if code == ABS_Z { Input::Lt } else { Input::Rt }, v > TRIGGER_ON);
            }
            _ => {}
        }
    }
}

/// The checking run's guard: whether another program holds the device for itself. Trying to
/// take it fails with EBUSY then; taken, it's let go at once.
fn held_elsewhere(own: &OwnedFd) -> bool {
    const EVIOCGRAB: u64 = (1 << 30) | (4 << 16) | ((b'E' as u64) << 8) | 0x90;
    if unsafe { libc::ioctl(own.as_raw_fd(), EVIOCGRAB as _, 1 as libc::c_int) } == 0 {
        unsafe { libc::ioctl(own.as_raw_fd(), EVIOCGRAB as _, 0 as libc::c_int) };
        return false;
    }
    std::io::Error::last_os_error().raw_os_error() == Some(libc::EBUSY)
}

fn run(layout: Layout, out: evdev::Virtual, test: bool) {
    // udev gives the user the device a moment after it's made.
    let own = test.then(|| out.node.as_deref().and_then(|n| (0..50).find_map(|_| evdev::open(n, libc::O_RDONLY).or_else(|| { std::thread::sleep(Duration::from_millis(100)); None })))).flatten();
    if test && own.is_none() {
        eprintln!("lobby: controller as keys: can't check who holds the device; nothing will be sent");
    }
    let may_send = || !test || own.as_ref().is_some_and(held_elsewhere);
    let debug = std::env::var_os("LOBBY_DEBUG_PAD").is_some();
    let mut pads: Vec<Pad> = Vec::new();
    let mut pressed: BTreeSet<u16> = BTreeSet::new();
    let mut next_look = Instant::now();
    loop {
        // Controllers plugged in: looked for every two seconds (a game runs for minutes).
        if Instant::now() >= next_look {
            for e in std::fs::read_dir("/dev/input").into_iter().flatten().flatten() {
                let path = e.path();
                if path.file_name().is_some_and(|n| n.to_string_lossy().starts_with("event")) && Some(&path) != out.node.as_ref() && !pads.iter().any(|p| p.path == path) {
                    if let Some(p) = Pad::open(&path) {
                        eprintln!("lobby: controller as keys: {}", path.display());
                        pads.push(p);
                    }
                }
            }
            next_look = Instant::now() + Duration::from_secs(2);
        }
        let moving = layout.mouse && pads.iter().any(|p| p.right.0.hypot(p.right.1) > MOUSE_DEAD);
        let wait = if moving { MOUSE_TICK } else { next_look.saturating_duration_since(Instant::now()) };
        let mut fds: Vec<libc::pollfd> = pads.iter().map(|p| libc::pollfd { fd: p.fd.as_raw_fd(), events: libc::POLLIN, revents: 0 }).collect();
        unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as _, wait.as_millis().max(1) as i32) };
        let mut gone = Vec::new();
        for (i, p) in pads.iter_mut().enumerate() {
            if fds[i].revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0 {
                gone.push(i);
                continue;
            }
            if fds[i].revents & libc::POLLIN == 0 {
                continue;
            }
            match evdev::read(&p.fd) {
                Ok(events) => events.into_iter().for_each(|(k, c, v)| p.event(k, c, v)),
                Err(()) => gone.push(i),
            }
        }
        for i in gone.into_iter().rev() {
            pads.remove(i);
        }
        // Keys: what any controller holds (one controller seen twice, Steam's virtual one and
        // the real one, presses once).
        let want: BTreeSet<u16> = pads.iter().flat_map(|p| p.held.iter()).filter(|i| !(layout.mouse && matches!(i, Input::RUp | Input::RDown | Input::RLeft | Input::RRight))).filter_map(|i| layout.keys.get(i).copied()).collect();
        let changes: Vec<(u16, u16, i32)> = pressed.difference(&want).map(|&k| (EV_KEY, k, 0)).chain(want.difference(&pressed).map(|&k| (EV_KEY, k, 1))).collect();
        if !changes.is_empty() {
            if may_send() {
                if debug {
                    eprintln!("lobby: controller as keys: {}", changes.iter().map(|&(_, k, v)| format!("{}{}", key_name(k), if v == 1 { " down" } else { " up" })).collect::<Vec<_>>().join(", "));
                }
                out.send(&changes);
                pressed = want;
            } else if debug {
                eprintln!("lobby: controller as keys: not sent (nothing holds the device)");
            }
        }
        // The mouse: the stick tilted furthest, faster the further.
        if layout.mouse {
            if let Some((x, y)) = pads.iter().map(|p| p.right).max_by(|a, b| a.0.hypot(a.1).total_cmp(&b.0.hypot(b.1))) {
                let tilt = x.hypot(y);
                if tilt > MOUSE_DEAD && may_send() {
                    let speed = ((tilt - MOUSE_DEAD) / (1.0 - MOUSE_DEAD)).min(1.0).powi(2) * MOUSE_SPEED / tilt;
                    let (dx, dy) = ((x * speed).round() as i32, (y * speed).round() as i32);
                    if dx != 0 || dy != 0 {
                        out.send(&[(EV_REL, 0, dx), (EV_REL, 1, dy)]);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layouts() {
        let l = Layout::parse("").unwrap();
        assert_eq!((l.keys[&Input::A], l.keys[&Input::LDown], l.mouse), (28, 108, true));
        let l = Layout::parse("wasd a=z b=none").unwrap();
        assert_eq!((l.keys[&Input::A], l.keys.get(&Input::B), l.keys[&Input::LUp]), (44, None, 17));
        let l = Layout::parse("arrows rup=w rdown=s").unwrap();
        assert!(!l.mouse);
        assert!(Layout::parse("a=banana").is_err());
        assert!(Layout::parse("q=z").is_err());
        assert!(Layout::parse("wasd x").is_err());
    }

    #[test]
    fn sticks_have_give() {
        let fd = evdev::open(Path::new("/dev/null"), libc::O_RDONLY).unwrap();
        let mut p = Pad { fd, path: PathBuf::new(), ranges: HashMap::from([(ABS_X, (-32768, 32767))]), triggers_on_z: true, playstation: false, held: BTreeSet::new(), left: (0.0, 0.0), right: (0.0, 0.0) };
        p.event(EV_ABS, ABS_X, 20000);
        assert!(p.held.contains(&Input::LRight));
        p.event(EV_ABS, ABS_X, 13000);
        assert!(p.held.contains(&Input::LRight), "still held between the two thresholds");
        p.event(EV_ABS, ABS_X, 5000);
        assert!(p.held.is_empty());
        p.event(EV_ABS, ABS_HAT0Y, -1);
        assert!(p.held.contains(&Input::Up));
    }
}
