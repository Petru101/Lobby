# Runs inside run/without-launchers.sh's view while a game starts: SCREEN=gamescope
# LOBBY_KEYS_TEST=1 LOBBY_CONTROLLER_LAYOUT="arrows rb=g" DURING="python3 -I <this>" ... play <id>.
# Steps for Fjogel Rampage's title screen (RB, then B); change `steps` for another game.
# A virtual controller (uinput, 1209:0001) presses buttons; Lobby's "controller as keys" turns them into keys on its
# own keyboard device. That device is held exclusively here (EVIOCGRAB), so the real desktop
# never gets a key, and its keys are passed to the test screen's X server by XTEST: the part a
# desktop (KWin, gamescope) does for real. Phase 1 passes nothing on (does the game react to the
# controller itself?), phase 2 passes the keys on. Pictures after each step in $OUT.
import evdev, glob, os, subprocess, sys, threading, time
from evdev import AbsInfo, InputDevice, UInput, ecodes as e
from Xlib import X, display
from Xlib.ext import xtest

OUT = os.environ["OUT"]
t0 = time.monotonic()
log = open(f"{OUT}/padkeys.log", "w")


def say(*a):
    print(f"{time.monotonic() - t0:6.1f}", *a, file=log, flush=True)


stick = AbsInfo(0, -32768, 32767, 16, 128, 0)
trig = AbsInfo(0, 0, 255, 0, 0, 0)
hat = AbsInfo(0, -1, 1, 0, 0, 0)
pad = UInput(
    {
        e.EV_KEY: [e.BTN_SOUTH, e.BTN_EAST, e.BTN_NORTH, e.BTN_WEST, e.BTN_TL, e.BTN_TR, e.BTN_SELECT, e.BTN_START, e.BTN_THUMBL, e.BTN_THUMBR],
        e.EV_ABS: [(e.ABS_X, stick), (e.ABS_Y, stick), (e.ABS_RX, stick), (e.ABS_RY, stick), (e.ABS_Z, trig), (e.ABS_RZ, trig), (e.ABS_HAT0X, hat), (e.ABS_HAT0Y, hat)],
    },
    name="Lobby test pad",
    vendor=0x1209,
    product=0x0001,
)
say("pad made")

node = None
for _ in range(240):
    for n in glob.glob("/sys/class/input/event*/device/name"):
        if open(n).read().strip() == "Lobby controller keys":
            node = "/dev/input/" + n.split("/")[4]
    if node:
        break
    time.sleep(0.5)
if not node:
    say("Lobby's keyboard device never showed")
    sys.exit(1)
dev = None
for _ in range(50):  # udev gives the user the device a moment after it's made
    try:
        dev = InputDevice(node)
        break
    except PermissionError:
        time.sleep(0.1)
dev.grab()
say("held", node, dev.info)

d = display.Display()
passing = False
counts = {"keys": 0, "moves": 0, "dropped": 0}
BUTTONS = {0x110: 1, 0x111: 3, 0x112: 2}


def forward():
    dx = dy = 0
    for ev in dev.read_loop():
        if ev.type == e.EV_KEY:
            if not passing:
                counts["dropped"] += 1
                say("not passed on: key", ev.code, ev.value)
                continue
            if ev.code in BUTTONS:
                xtest.fake_input(d, X.ButtonPress if ev.value else X.ButtonRelease, BUTTONS[ev.code])
            else:
                xtest.fake_input(d, X.KeyPress if ev.value else X.KeyRelease, ev.code + 8)
            d.sync()
            counts["keys"] += 1
            say("key to the game's screen:", e.KEY.get(ev.code, e.BTN.get(ev.code, ev.code)), "down" if ev.value else "up")
        elif ev.type == e.EV_REL:
            if ev.code == e.REL_X:
                dx += ev.value
            elif ev.code == e.REL_Y:
                dy += ev.value
        elif ev.type == e.EV_SYN and (dx or dy):
            if passing:
                xtest.fake_input(d, X.MotionNotify, detail=True, x=dx, y=dy)
                d.sync()
                counts["moves"] += 1
            dx = dy = 0


threading.Thread(target=forward, daemon=True).start()


def windows():
    t = subprocess.run(["xwininfo", "-root", "-tree"], capture_output=True, text=True).stdout
    return [l for l in t.splitlines() if l.strip().startswith("0x")]


def game_window():
    import re
    best = (0, None, None)
    for l in windows():
        m = re.search(r" (\d{2,5})x(\d{2,5})\+", l)
        if not m or "steam_app" not in l:
            continue
        w = l.split()[0]
        info = subprocess.run(["xwininfo", "-id", w], capture_output=True, text=True).stdout
        if "IsViewable" in info and int(m[1]) * int(m[2]) > best[0]:
            best = (int(m[1]) * int(m[2]), w, l.strip()[:120])
    return best[1], best[2]


def shot(name):
    w, desc = game_window()
    if not w:
        apps = [l.strip()[:90] for l in windows() if "steam_app" in l and "Default IME" not in l]
        say("no viewable game window for", name, "; its windows:", apps[:5])
        w, desc = "root", "the whole screen"
    subprocess.run(["import", "-silent", "-window", w, f"{OUT}/padkeys-{name}.png"], capture_output=True)
    say("picture", name, desc)


def key(code, value):
    pad.write(e.EV_KEY, code, value)
    pad.syn()


def press(code, hold=0.15):
    key(code, 1)
    time.sleep(hold)
    key(code, 0)


def axis(code, value, hold):
    pad.write(e.EV_ABS, code, value)
    pad.syn()
    time.sleep(hold)
    pad.write(e.EV_ABS, code, 0)
    pad.syn()


for _ in range(60):
    if game_window()[0]:
        break
    time.sleep(1)
say("window", game_window()[1])
for _ in range(180):  # GameMaker 8's loading window first
    w = game_window()[1] or ""
    if "LoadForm" not in w:
        break
    time.sleep(1)
say("loaded", game_window()[1])
time.sleep(float(os.environ.get("SETTLE", "15")))


MAIN = [None]


def state(name):
    w = MAIN[0]
    if not w:
        return
    info = subprocess.run(["xwininfo", "-id", w], capture_output=True, text=True).stdout
    ms = [l.strip() for l in info.splitlines() if "Map State" in l or "geometry" in l]
    props = subprocess.run(["xprop", "-id", w, "WM_STATE", "_NET_WM_STATE"], capture_output=True, text=True).stdout.replace("\n", " | ")
    focus = subprocess.run(["xdotool", "getwindowfocus"], capture_output=True, text=True).stdout.strip()
    say("state", name, ms, props[:200], "focus", focus)


def steps(tag):
    MAIN[0] = MAIN[0] or game_window()[0]
    shot(f"{tag}-0")
    press(e.BTN_TR)  # rb: g in this run's layout
    time.sleep(2.5)
    shot(f"{tag}-1-rb")
    press(e.BTN_EAST)  # b: escape
    time.sleep(2.5)
    shot(f"{tag}-2-b")


say("phase 1: the controller alone, nothing passed on")
steps("p1")
passing = True
say("phase 2: Lobby's keys passed on")
steps("p2")
axis(e.ABS_RX, 30000, 1.0)  # right stick: the mouse
time.sleep(0.5)
say("counts", counts)
say("windows now", [w[:100] for w in windows()][:6])
