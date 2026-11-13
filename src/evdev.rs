//! The kernel's input devices, read directly (/dev/input/event*): what the window's controller
//! support (`ui::pad`) and the controller-as-keyboard of a running game (`pad_keys`) share.

use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::path::Path;

pub const EV_SYN: u16 = 0x00;
pub const EV_KEY: u16 = 0x01;
pub const EV_REL: u16 = 0x02;
pub const EV_ABS: u16 = 0x03;
pub const BTN_SOUTH: u16 = 0x130;
pub const BTN_EAST: u16 = 0x131;
pub const BTN_NORTH: u16 = 0x133;
pub const BTN_WEST: u16 = 0x134;
pub const BTN_TL: u16 = 0x136;
pub const BTN_TR: u16 = 0x137;
pub const BTN_TL2: u16 = 0x138;
pub const BTN_TR2: u16 = 0x139;
pub const BTN_SELECT: u16 = 0x13a;
pub const BTN_START: u16 = 0x13b;
pub const BTN_THUMBL: u16 = 0x13d;
pub const BTN_THUMBR: u16 = 0x13e;
/// The pad as buttons (up, down, left, right), on drivers without a hat.
pub const BTN_DPAD_UP: u16 = 0x220;
pub const BTN_DPAD_DOWN: u16 = 0x221;
pub const BTN_DPAD_LEFT: u16 = 0x222;
pub const BTN_DPAD_RIGHT: u16 = 0x223;
pub const ABS_X: u16 = 0x00;
pub const ABS_Y: u16 = 0x01;
pub const ABS_Z: u16 = 0x02;
pub const ABS_RX: u16 = 0x03;
pub const ABS_RY: u16 = 0x04;
pub const ABS_RZ: u16 = 0x05;
pub const ABS_HAT0X: u16 = 0x10;
pub const ABS_HAT0Y: u16 = 0x11;

/// _IOC(_IOC_READ, 'E', nr, size): the evdev ioctls that read.
pub fn eviocg(nr: u64, size: usize) -> u64 {
    (2 << 30) | ((size as u64) << 16) | ((b'E' as u64) << 8) | nr
}

pub fn bits(fd: i32, ev: u16) -> Vec<u8> {
    let mut buf = vec![0u8; 96];
    let r = unsafe { libc::ioctl(fd, eviocg(0x20 + ev as u64, buf.len()) as _, buf.as_mut_ptr()) };
    if r < 0 {
        buf.clear();
    }
    buf
}

pub fn has(bits: &[u8], code: u16) -> bool {
    bits.get(code as usize / 8).is_some_and(|b| b & (1 << (code % 8)) != 0)
}

#[repr(C)]
#[derive(Default)]
pub struct AbsInfo {
    pub value: i32,
    pub minimum: i32,
    pub maximum: i32,
    pub fuzz: i32,
    pub flat: i32,
    pub resolution: i32,
}

pub fn abs_range(fd: i32, code: u16) -> Option<(i32, i32)> {
    let mut info = AbsInfo::default();
    let r = unsafe { libc::ioctl(fd, eviocg(0x40 + code as u64, std::mem::size_of::<AbsInfo>()) as _, &mut info) };
    (r >= 0 && info.maximum > info.minimum).then_some((info.minimum, info.maximum))
}

/// The device's vendor and product (struct input_id: bus, vendor, product, version).
pub fn ids(fd: i32) -> Option<(u16, u16)> {
    let mut id = [0u16; 4];
    let r = unsafe { libc::ioctl(fd, eviocg(0x02, 8) as _, id.as_mut_ptr()) };
    (r >= 0).then_some((id[1], id[2]))
}

/// Opens a device to read, without blocking. By the system call itself: Steam's overlay, loaded
/// into what Steam starts, refuses the C library's open of a controller it hides.
pub fn open(path: &Path, flags: i32) -> Option<OwnedFd> {
    let name = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).ok()?;
    let fd = unsafe { libc::syscall(libc::SYS_openat, libc::AT_FDCWD, name.as_ptr(), flags | libc::O_NONBLOCK | libc::O_CLOEXEC, 0) } as i32;
    (fd >= 0).then(|| unsafe { OwnedFd::from_raw_fd(fd) })
}

/// The events waiting on a device (type, code, value); Err when it's gone.
pub fn read(fd: &OwnedFd) -> Result<Vec<(u16, u16, i32)>, ()> {
    let mut out = Vec::new();
    let mut buf = [0u8; 24 * 64];
    loop {
        let n = unsafe { libc::read(fd.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len()) };
        if n <= 0 {
            if n == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::ENODEV) {
                return Err(());
            }
            return Ok(out);
        }
        // struct input_event: a timeval (16 bytes), type, code, value.
        for e in buf[..n as usize].chunks_exact(24) {
            out.push((u16::from_ne_bytes([e[16], e[17]]), u16::from_ne_bytes([e[18], e[19]]), i32::from_ne_bytes([e[20], e[21], e[22], e[23]])));
        }
    }
}

/// A keyboard and mouse of Lobby's own making (uinput): what it writes reaches whatever window
/// has the keyboard, as from a real one.
pub struct Virtual {
    fd: OwnedFd,
    /// Its node under /dev/input, when found.
    pub node: Option<std::path::PathBuf>,
}

/// _IOC(dir, 'U', nr, size).
fn uioc(dir: u64, nr: u64, size: usize) -> u64 {
    (dir << 30) | ((size as u64) << 16) | ((b'U' as u64) << 8) | nr
}

impl Virtual {
    /// Every key of a keyboard, the mouse's buttons, its movement and wheel.
    pub fn create(name: &str, vendor: u16, product: u16) -> Result<Virtual, String> {
        let fd = open(Path::new("/dev/uinput"), libc::O_WRONLY).ok_or_else(|| format!("/dev/uinput: {}", std::io::Error::last_os_error()))?;
        let raw = fd.as_raw_fd();
        let set = |nr: u64, value: u16| unsafe { libc::ioctl(raw, uioc(1, nr, 4) as _, value as libc::c_int) };
        set(100, EV_KEY);
        set(100, EV_REL);
        set(100, EV_SYN);
        for key in (1..=248).chain(0x110..=0x114) {
            set(101, key);
        }
        for rel in [0x00, 0x01, 0x08] {
            set(102, rel);
        }
        // struct uinput_setup: input_id (bus, vendor, product, version), name[80], ff_effects_max.
        let mut setup = [0u8; 92];
        setup[0..2].copy_from_slice(&0x06u16.to_ne_bytes()); // BUS_VIRTUAL
        setup[2..4].copy_from_slice(&vendor.to_ne_bytes());
        setup[4..6].copy_from_slice(&product.to_ne_bytes());
        setup[6..8].copy_from_slice(&1u16.to_ne_bytes());
        let n = name.len().min(79);
        setup[8..8 + n].copy_from_slice(&name.as_bytes()[..n]);
        if unsafe { libc::ioctl(raw, uioc(1, 3, setup.len()) as _, setup.as_ptr()) } < 0 || unsafe { libc::ioctl(raw, uioc(0, 1, 0) as _) } < 0 {
            return Err(format!("uinput: {}", std::io::Error::last_os_error()));
        }
        // UI_GET_SYSNAME: inputN, whose eventM is the node.
        let mut sys = [0u8; 64];
        let node = (unsafe { libc::ioctl(raw, uioc(2, 44, sys.len()) as _, sys.as_mut_ptr()) } >= 0)
            .then(|| String::from_utf8_lossy(sys.split(|&b| b == 0).next().unwrap_or_default()).into_owned())
            .and_then(|sys| std::fs::read_dir(format!("/sys/devices/virtual/input/{sys}")).ok())
            .and_then(|d| d.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).find(|n| n.starts_with("event")))
            .map(|e| Path::new("/dev/input").join(e));
        Ok(Virtual { fd, node })
    }

    /// Writes events and the report that ends them.
    pub fn send(&self, events: &[(u16, u16, i32)]) {
        let mut buf = Vec::with_capacity((events.len() + 1) * 24);
        for &(kind, code, value) in events.iter().chain([(EV_SYN, 0, 0)].iter()) {
            buf.extend_from_slice(&[0u8; 16]);
            buf.extend_from_slice(&kind.to_ne_bytes());
            buf.extend_from_slice(&code.to_ne_bytes());
            buf.extend_from_slice(&value.to_ne_bytes());
        }
        unsafe { libc::write(self.fd.as_raw_fd(), buf.as_ptr().cast(), buf.len()) };
    }
}
