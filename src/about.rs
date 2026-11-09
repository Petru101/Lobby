//! What the About page says about who makes Lobby and what it connects to. It has to stay true:
//! a new server Lobby or a tool it runs talks to goes in CONNECTIONS (the rule, and how the list
//! was made: notes/distros.md, under The AppImage).

pub const SOURCE: &str = "https://github.com/Petru101/Lobby";
pub const RELEASES: &str = "https://github.com/Petru101/Lobby/releases";

pub const WHO: &str = "Lobby is made by one person (Petru101 on GitHub), not a company. Nobody independent has checked it. Its code is public, so anyone can read it, and the AppImage is built on GitHub's servers from that code, not on its maker's computer. GitHub keeps a signed record of which code each file came from, and you can check yours: gh attestation verify Lobby-x86_64.AppImage -R Petru101/Lobby (GitHub's gh tool, signed in). That shows where the file came from, not that the code is safe. You can also build it from the code yourself.";

pub const CONNECTS: &str = "Nothing about you or what you play goes to Lobby's maker unless you send a report. There's no tracking or usage statistics in Lobby's code or in the tools it runs for the stores (legendary, gogdl, umu). Lobby can't speak for Proton, the stores' apps or the games themselves. Every site below sees your internet address.";

/// Each one's name, and what goes there and when.
pub const CONNECTIONS: &[(&str, &str)] = &[
    ("GitHub", "Asked for new versions of Lobby (see Updates), and downloads: legendary, gogdl and umu, anti-cheat runtimes, GE-Proton, and lists of anti-cheat games and of where games keep their saves. No personal details."),
    ("Steam's store", "When a scan finds a game with no cover: its name and its folder's name are sent as search words, to find its cover and real name. No account."),
    ("itch.io", "The same search for folders named like an itch.io game. Lobby also reads your browsers' download history on this computer to find downloads from itch.io pages, keeps only those, and asks itch.io about those pages, so it can see which ones."),
    ("Epic and GOG, not signed in", "The list of free games, with your country from your system's language. Covers, and details of each GOG game by its id."),
    ("Epic and GOG, signed in", "Your sign-in goes to them, for what you own, updates, installs and cloud saves, the saves being uploaded and downloaded around each play. Sign-ins are kept on this computer, in Lobby's own folders. Their sign-in pages open in a Lobby window and load what those pages load."),
    ("GamerPower", "The list of free games on Steam, GOG and Epic. No personal details."),
    ("The free games timer", "Checks the free game lists at 17:30 and after you log in, even with Lobby closed. Turn it off in Free games."),
    ("legendary's own server (api.legendary.gl)", "Not Epic's. The Epic tool asks it about once a day for its own version and settings. No personal details."),
    ("Valve's servers (repo.steampowered.com)", "umu asks for the newest Steam Runtime each time a Windows game starts through it, and downloads it when needed. No personal details."),
    ("umu's server (umu.openwinecomponents.org)", "The first time you play an Epic or GOG game Lobby installed: its store's name and game id, to find fixes for it."),
    ("Where covers are", "Cover pictures come from wherever the stores' and launchers' lists say, mostly Epic, GOG, Steam, itch.io and GamerPower. The address can show which game it's for."),
    ("Only when a game needs them", "Setting a game up may download what it needs: Java (Adoptium), Flathub, Microsoft's .NET, Python packages (PyPI), libraries from Debian, Ubuntu or Fedora, Maven, RPG Maker's runtime. The EA, Ubisoft, Rockstar and Blizzard apps come from their makers. Nothing personal goes along."),
    ("gamefix's server (gamefix.petru101.workers.dev)", "Only when you press Send on Report a problem, after seeing everything that goes, with a random id for this install. It belongs to Lobby's maker."),
    ("Games and store apps", "They connect wherever they like once started, as they would anywhere. A game's Protection section can cut its internet off."),
];
