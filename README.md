# Lobby

**One game library for Linux, whatever put the games there.** Steam, Epic, GOG, itch.io
downloads, Lutris, Faugus, Heroic, a game in a folder on any drive, an installer you once ran
into a Wine prefix: Lobby finds them all and shows them in one window, and starts them. It's
meant for people coming from Windows, with games scattered over several drives.

Lobby is a game launcher for Linux (GTK 4 and libadwaita), written in Rust. It's early: it's
used daily on one computer (Bazzite, KDE Plasma, NVIDIA), tested on Ubuntu 24.04 and 26.04,
Fedora 44 and Arch, and isn't packaged yet.

<img width="1330" height="910" alt="image" src="https://github.com/user-attachments/assets/4305bbb6-d44b-4ae6-9443-d20a8a249725" />


## What it does

- **Finds every game.** It reads what Steam, Heroic, Lutris, Faugus and the app menu know, the
  games folders on every drive, your downloads (zips, installers, disc images, itch.io games)
  and Wine prefixes, and merges what's the same game into one card. A game found in a folder
  gets its real name from its own files and a cover from the stores.
- **Starts them,** with Proton through [umu-launcher](https://github.com/Open-Wine-Components/umu-launcher)
  for Windows games. Games no launcher knows are set up the first time you press Play by
  [gamefix](https://github.com/Petru101/gamefix), built in, which works out why a game won't
  start on Linux and fixes it.
- **Its own store accounts.** Sign in to Epic Games and GOG in Lobby to see what you own,
  install, update and get DLC, with cloud saves synced before and after playing (through
  [legendary](https://github.com/derrod/legendary) and [gogdl](https://github.com/Heroic-Games-Launcher/heroic-gogdl),
  in Lobby's own copies). Your Steam library comes from the Steam app signed in on the
  computer; Steam still installs and starts Steam games.
- **Takes games over** from Lutris, Faugus and Steam shortcuts, and Epic and GOG installs from
  Heroic, so those launchers can be removed.
- **Protects your computer.** Every game Lobby starts runs in a sandbox (bubblewrap): it can't
  read your passwords, keys, browser profiles or sign-ins, or make itself start with the
  computer. Untrusted downloads start isolated, without the internet, until you say otherwise.
- **Moves games between drives,** keeping everything that points at them working.
- **Library tools:** Installed, Library (everything you own) and Downloads pages; sort by last
  played, play time (Lobby's own and what Steam, Heroic, Lutris and Faugus counted), size or
  date added; Continue playing; favourites and hidden games; demos out of sight; each game's
  save folder; Stop for a game that hangs; Show frame rate (MangoHud's overlay) per game.
- **Says when online play won't work** (anti-cheat Linux can't run, online-only games), and
  when a game closes without showing anything, with the likely reason from its log.
- **Cleans up downloads:** the archives, installers and repacks of games already installed,
  with where to get each again (itch.io, MyAbandonware), deleted when you say so.
- **Works with a controller:** move with the D-pad or stick, A to open, B to go back, Start
  to play, the shoulder buttons for pages and the triggers for sources.
- **Saves from Windows.** It finds a game's saves in a Windows install or backup on another
  drive and copies them into the game's Wine prefix.
- **Free games.** Epic's weekly free games and others, with a reminder, claimed in Lobby.

## Installing

The AppImage runs on Ubuntu 24.04 and newer, Fedora, Arch and the like: download
[Lobby-x86_64.AppImage](https://github.com/Petru101/Lobby/releases/latest/download/Lobby-x86_64.AppImage)
(all versions: [Releases](https://github.com/Petru101/Lobby/releases)), make it executable (`chmod +x Lobby-x86_64.AppImage`, or the file's
Properties) and start it. The first start puts Lobby in the app menu, and Lobby updates
itself from here (Gear Lever and AppImageUpdate can update it too). It brings its own GTK,
libadwaita and WebKitGTK; the system needs FUSE (`fusermount3`), bubblewrap and
xdg-dbus-proxy, which desktops almost always have.

To build the AppImage, run `run/appimage.sh` on Ubuntu 24.04 (the oldest system it should
start on) with the build packages below installed; it lands in `target/appimage`.

## Building

You need Rust (from [rustup](https://rustup.rs)), and GTK 4.14, libadwaita 1.5 and WebKitGTK
6.0 or newer with their development files:

- Ubuntu 24.04 and later, Mint 22, Pop!_OS, Debian 13: `sudo apt install build-essential
  pkg-config libgtk-4-dev libadwaita-1-dev libwebkitgtk-6.0-dev`
- Fedora: `sudo dnf install gcc pkgconf-pkg-config gtk4-devel libadwaita-devel webkitgtk6.0-devel`
- Arch: `sudo pacman -S base-devel gtk4 libadwaita webkitgtk-6.0`

gamefix is fetched from GitHub by Cargo.

```sh
cargo build --release
./target/release/lobby
```

To play, it needs Steam (its own package, Ubuntu's `steam-installer`, or Steam from Flathub),
curl, 7-Zip, bubblewrap, xdg-dbus-proxy (store sign-in pages open in a sandbox that needs
it), and Python 3 for umu-launcher, which runs Windows games (Lobby downloads its own copy when
the system has none). unrar, Flatpak and MangoHud are used when there. `lobby check` lists what's missing and the command that installs it. On Ubuntu (23.10
and later) Lobby asks once for your password to add itself to the programs AppArmor lets
make sandboxes, which Proton needs for every Windows game and store pages need too
(`lobby allow-sandbox` does it from a terminal).

## Commands

`lobby` opens the window. Everything also works from a terminal: `lobby scan`, `lobby play
<game>`, `lobby how <game>` (how it would start, without starting it), `lobby install
<game>`, `lobby move <game> <folder>`, `lobby running`, `lobby stop <game>`, `lobby
protection <game> ...`, and more: `lobby --help`.

## Privacy

Lobby only reads launchers' files and changes its own (and the app-menu entries it makes).
To know which downloads came from itch.io, it reads the browsers' download history (Firefox,
LibreWolf, Zen, Floorp, Waterfox; Chrome, Chromium, Brave, Edge, Vivaldi, Opera) for downloads
whose page was an itch.io game page, and keeps only those. It asks the stores and Steam's public store
for covers and names. Sign-ins stay on your computer, in Lobby's own folders.

## License

GPL-3.0-or-later.
