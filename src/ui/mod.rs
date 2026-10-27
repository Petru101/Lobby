//! lobby's window: every game as a card with its cover, a search box and a few filters; a
//! click shows where the game is and how it starts, Play starts it.

mod covers;
mod pad;

use crate::battlenet;
use crate::launch::{self, Plan};
use crate::library::{Entry, Game, Key, Kind, Source};
use crate::store::{self, Library};
use adw::prelude::*;
use webkit6::prelude::*;
use gtk::{gdk, gio, glib};
use crate::stores::Store;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

const APP_ID: &str = "io.github.Petru101.Lobby";

thread_local! {
    /// The window's state, for app actions (claim-free) that arrive from another `lobby`.
    static UI: RefCell<Option<Rc<Ui>>> = const { RefCell::new(None) };
    /// claim-free arrived before the window was built: it claims once it is.
    static CLAIM_PENDING: Cell<bool> = const { Cell::new(false) };
}

/// The window; with `claim`, straight at the checkout for the free games not claimed yet (the
/// notification's Claim button). Lobby already open gets the request.
pub fn run(claim: bool) -> glib::ExitCode {
    // A checking run (LOBBY_SNAPSHOT) is a copy of its own: otherwise it would hand over to the
    // user's open Lobby and quit.
    let flags = if std::env::var_os("LOBBY_SNAPSHOT").is_some() { gio::ApplicationFlags::NON_UNIQUE } else { gio::ApplicationFlags::empty() };
    let app = adw::Application::builder().application_id(APP_ID).flags(flags).build();
    app.connect_startup(|_| {
        let css = gtk::CssProvider::new();
        css.load_from_string(include_str!("style.css"));
        gtk::style_context_add_provider_for_display(&gdk::Display::default().expect("a display"), &css, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
    });
    // Opened again while running: the window there is, brought forward.
    app.connect_activate(|app| match app.active_window() {
        Some(window) => window.present(),
        None => build(app),
    });
    let action = gio::SimpleAction::new("claim-free", None);
    action.connect_activate(|_, _| match UI.with(|u| u.borrow().clone()) {
        Some(ui) => ui.claim_unclaimed(),
        None => CLAIM_PENDING.with(|p| p.set(true)),
    });
    app.add_action(&action);
    if claim {
        let _ = app.register(gio::Cancellable::NONE);
        app.activate_action("claim-free", None);
    }
    app.set_accels_for_action("win.search", &["<Control>f"]);
    app.set_accels_for_action("win.rescan", &["<Control>r", "F5"]);
    app.set_accels_for_action("window.close", &["<Control>w", "<Control>q"]);
    app.run_with_args::<&str>(&[])
}

/// Asked by `lobby run` when a game's saves changed both here and in the store's cloud since
/// the last sync: a small window of its own (lobby's may not be open). None: keep both as they
/// are this time.
pub fn ask_which_saves(game: &str, store: &str) -> Option<crate::stores::saves::Direction> {
    use crate::stores::saves::Direction;
    let choice: Rc<Cell<Option<Direction>>> = Rc::new(Cell::new(None));
    let app = adw::Application::builder().application_id("io.github.Petru101.Lobby.Saves").flags(gio::ApplicationFlags::NON_UNIQUE).build();
    let (c, game, store) = (choice.clone(), game.to_string(), store.to_string());
    app.connect_activate(move |app| {
        let page = adw::StatusPage::builder()
            .icon_name("weather-overcast-symbolic")
            .title("Which saves should it use?")
            .description(format!("{game}'s saves changed on this computer and in {store}'s cloud since they were last synced. The ones you don't keep are replaced."))
            .build();
        let cloud = gtk::Button::builder().label(format!("Keep {store}'s cloud saves")).build();
        let local = gtk::Button::builder().label("Keep this computer's saves").build();
        let neither = gtk::Button::builder().label("Decide later (play with this computer's)").build();
        cloud.add_css_class("pill");
        local.add_css_class("pill");
        neither.add_css_class("pill");
        neither.add_css_class("flat");
        let buttons = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(12).halign(gtk::Align::Center).build();
        buttons.append(&cloud);
        buttons.append(&local);
        buttons.append(&neither);
        page.set_child(Some(&buttons));
        let view = adw::ToolbarView::new();
        view.add_top_bar(&adw::HeaderBar::builder().show_title(false).build());
        view.set_content(Some(&page));
        let window = adw::ApplicationWindow::builder().application(app).title(format!("Lobby: {game}")).default_width(520).default_height(600).content(&view).build();
        for (button, pick) in [(cloud, Some(Direction::KeepCloud)), (local, Some(Direction::KeepLocal)), (neither, None)] {
            let (c, w) = (c.clone(), window.clone());
            button.connect_clicked(move |_| {
                c.set(pick);
                w.close();
            });
        }
        window.present();
        if let Some(file) = std::env::var_os("LOBBY_SNAPSHOT").map(std::path::PathBuf::from) {
            let w = window.clone();
            glib::timeout_add_local_once(std::time::Duration::from_secs(2), move || {
                let paintable = gtk::WidgetPaintable::new(Some(&w));
                let snapshot = gtk::Snapshot::new();
                paintable.snapshot(&snapshot, w.width() as f64, w.height() as f64);
                let saved = snapshot.to_node().zip(w.renderer()).map(|(node, r)| r.render_texture(&node, None).save_to_png(&file));
                eprintln!("snapshot: {saved:?}");
                w.close();
            });
        }
    });
    app.run_with_args::<&str>(&[]);
    choice.get()
}

/// Which games the grid shows: one of three pages, by whether they're installed.
#[derive(Clone, Copy, PartialEq)]
enum Page {
    Installed,
    Library,
    Downloads,
}

impl Page {
    const ALL: [(Page, &'static str, &'static str, &'static str); 3] = [
        (Page::Installed, "installed", "Installed", "Games ready to play on this computer"),
        (Page::Library, "library", "Library", "Every game you have: installed, and owned on Epic or GOG but not installed (sign in with Accounts)"),
        (Page::Downloads, "downloads", "Downloads", "Downloaded but not installed yet"),
    ];

    fn wants(self, g: &Game) -> bool {
        match self {
            Page::Installed => g.kind == Kind::Game,
            Page::Library => matches!(g.kind, Kind::Game | Kind::Owned),
            Page::Downloads => matches!(g.kind, Kind::Download | Kind::WrongSystem),
        }
    }
}

/// And where they're from, on any page. Each game is under one: a store's game under its store
/// whoever installed it, else itch.io's, else Other.
#[derive(Clone, Copy, PartialEq)]
enum Origin {
    All,
    Steam,
    Epic,
    Gog,
    Amazon,
    Itch,
    Other,
}

impl Origin {
    const ALL: [(Origin, &'static str, &'static str, &'static str); 7] = [
        (Origin::All, "all", "All sources", "Games from everywhere"),
        (Origin::Steam, "steam", "Steam", "Steam's games"),
        (Origin::Epic, "epic", "Epic", "Epic Games Store games: installed by Lobby, Heroic or Lutris, found on your drives, or owned"),
        (Origin::Gog, "gog", "GOG", "GOG's games: installed by Lobby, Heroic or Lutris, found on your drives, or owned"),
        (Origin::Amazon, "amazon", "Amazon", "Amazon Games' games"),
        (Origin::Itch, "itch", "itch.io", "Games downloaded from itch.io"),
        (Origin::Other, "other", "Other", "Other launchers (Lutris, Faugus, Steam shortcuts, the app menu, gamefix) and games found on your drives"),
    ];

    fn of(g: &Game) -> Origin {
        let has = |s: &[Source]| g.entries.iter().any(|e| s.contains(&e.source));
        let store = |id: &str| g.entries.iter().any(|e| e.keys.iter().any(|k| matches!(k, Key::Store(s, _) if s == id)));
        if has(&[Source::Steam, Source::SteamElsewhere]) {
            Origin::Steam
        } else if has(&[Source::Epic]) || store("epic") {
            Origin::Epic
        } else if has(&[Source::Gog]) || store("gog") {
            Origin::Gog
        } else if has(&[Source::Amazon]) || store("amazon") {
            Origin::Amazon
        } else if has(&[Source::Itch]) {
            Origin::Itch
        } else {
            Origin::Other
        }
    }
}

/// The grid's order, kept in ~/.config/lobby/sort.
#[derive(Clone, Copy, PartialEq)]
enum Sort {
    Name,
    Played,
    Playtime,
    Size,
    Added,
}

impl Sort {
    const ALL: [(Sort, &'static str, &'static str); 5] = [(Sort::Name, "name", "Name"), (Sort::Played, "played", "Last played"), (Sort::Playtime, "playtime", "Play time"), (Sort::Size, "size", "Size"), (Sort::Added, "added", "Date added")];

    fn file() -> std::path::PathBuf {
        crate::stores::config_home().join("lobby/sort")
    }

    fn load() -> Sort {
        let saved = std::fs::read_to_string(Self::file()).unwrap_or_default();
        Self::ALL.iter().find(|(_, name, _)| *name == saved.trim()).map_or(Sort::Name, |(s, ..)| *s)
    }

    fn save(self) {
        if let Some((_, name, _)) = Self::ALL.iter().find(|(s, ..)| *s == self) {
            let _ = std::fs::create_dir_all(Self::file().parent().unwrap());
            let _ = std::fs::write(Self::file(), name);
        }
    }
}

/// What the Show menu lets through, kept in ~/.config/lobby/show.json.
#[derive(Clone, Copy, Default, serde::Serialize, serde::Deserialize)]
struct Show {
    /// Demos and prologues the account owns but hasn't installed (GOG adds every free one
    /// clicked on its store).
    #[serde(default)]
    demos: bool,
    #[serde(default)]
    favourites_only: bool,
    #[serde(default)]
    hidden: bool,
}

impl Show {
    fn file() -> std::path::PathBuf {
        crate::stores::config_home().join("lobby/show.json")
    }

    fn load() -> Show {
        std::fs::read_to_string(Self::file()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }

    fn save(self) {
        let _ = std::fs::create_dir_all(Self::file().parent().unwrap());
        let _ = serde_json::to_string(&self).map(|t| std::fs::write(Self::file(), t));
    }

    fn lets_through(self, g: &Game) -> bool {
        (self.hidden || !g.hidden) && (!self.favourites_only || g.favourite) && (self.demos || g.kind != Kind::Owned || !g.demo())
    }
}

/// What a game is sorted by, worked out once per fill.
#[derive(Clone, Copy, Default)]
struct Facts {
    /// Unix times (0: never, not known).
    last: u64,
    since: u64,
    seconds: u64,
    size: Option<u64>,
}

impl Facts {
    fn of(games: &[Game]) -> HashMap<String, Facts> {
        let played = crate::playtime::load();
        let sizes = crate::sizes::known(games);
        games
            .iter()
            .map(|g| {
                let p = crate::playtime::of(&played, g);
                (g.id.clone(), Facts { last: p.last, since: g.since(), seconds: p.seconds(), size: sizes.get(&g.id).copied() })
            })
            .collect()
    }

    /// Lower first.
    fn rank(&self, sort: Sort) -> i64 {
        let newest = |t: u64| -(t as i64);
        match sort {
            Sort::Name => 0,
            Sort::Played => newest(self.last),
            Sort::Playtime => newest(self.seconds),
            Sort::Size => self.size.map_or(1, newest),
            Sort::Added => newest(self.since),
        }
    }

    /// What the card says about it, sorted this way.
    fn text(&self, sort: Sort) -> Option<String> {
        match sort {
            Sort::Name => None,
            Sort::Played => Some(if self.last > 0 { when(self.last) } else { "Not played".into() }),
            Sort::Playtime => Some(if self.seconds > 0 { crate::playtime::text(self.seconds) } else { "Not played".into() }),
            Sort::Size => self.size.map(crate::windows_saves::size_text),
            Sort::Added => (self.since > 0).then(|| match store::now().saturating_sub(self.since) / 86_400 {
                0 => "Added today".into(),
                1 => "Added yesterday".into(),
                _ => format!("Added {}", when(self.since)),
            }),
        }
    }
}

struct Ui {
    window: adw::ApplicationWindow,
    /// How many games show, at the end of the source row.
    count: gtk::Label,
    toasts: adw::ToastOverlay,
    stack: gtk::Stack,
    flow: gtk::FlowBox,
    search: gtk::SearchEntry,
    rescan_button: gtk::Button,
    lib: RefCell<Library>,
    page: Cell<Page>,
    origin: Cell<Origin>,
    /// Downloads whose game is installed (`leftovers::find`, after each scan), and the
    /// Downloads page's banner offering to clean them up.
    leftovers: RefCell<Vec<crate::leftovers::Leftover>>,
    leftover_banner: adw::Banner,
    /// What may keep each game's online play from working here (`anticheat::warnings`, after
    /// each scan), for its page.
    online_warnings: RefCell<HashMap<String, Vec<String>>>,
    /// The newest failed start told (a toast for each newer one).
    failures_seen: Cell<u64>,
    /// A game just installed from a download: its download is offered for deleting once the
    /// scan after the install knows it.
    offer_leftover: RefCell<Option<String>>,
    /// The page buttons (Installed, Library, Downloads), for a controller's shoulder buttons.
    pages: RefCell<Vec<gtk::ToggleButton>>,
    /// A controller's way back: the card a page was opened from, focused again when it closes.
    pad_return: RefCell<glib::WeakRef<gtk::Widget>>,
    /// A page is closing (it animates): a move made meanwhile waits for it to be gone.
    pad_closing: Cell<bool>,
    pad_pending: Cell<Option<pad::Action>>,
    /// The source buttons: only sources with games on the page show.
    origins: RefCell<Vec<(Origin, gtk::ToggleButton)>>,
    sort: Cell<Sort>,
    facts: RefCell<HashMap<String, Facts>>,
    /// Each card's line saying what it's sorted by, by game (a game can have two cards: in
    /// Continue playing and in the grid).
    values: RefCell<Vec<(String, gtk::Label)>>,
    /// Continue playing: the games played last, above the grid.
    recent: gtk::FlowBox,
    recent_box: gtk::Box,
    /// The recent games shown now (the first few of the page and source picked).
    recent_shown: RefCell<Vec<String>>,
    measuring: Cell<bool>,
    show: Cell<Show>,
    /// The games running now (`running::find`, every 2 s), and the widgets that say so: each
    /// card's badge and Play button, a page's Play button.
    running: RefCell<std::collections::HashSet<String>>,
    run_widgets: RefCell<Vec<(String, Option<gtk::Widget>, glib::WeakRef<gtk::Button>, bool)>>,
    looking_for_running: Cell<bool>,
    scanning: Cell<bool>,
    /// A rescan was asked for during a scan (after an uninstall, say): another runs after it,
    /// since that scan may have read the disk before the change.
    scan_again: Cell<bool>,
    /// Store lists and update checks are being brought up to date in the background.
    refreshing_stores: Cell<bool>,
    free_button: gtk::Button,
    /// Games being installed (one install each at a time), with how far each is: 0 to 100, or
    /// below 0 when the tool doesn't say (unpacking an archive).
    installing: RefCell<Vec<(String, Arc<Mutex<f64>>, &'static str)>>,
    /// The progress bars on the cards of games being installed.
    bars: RefCell<Vec<(String, gtk::ProgressBar)>>,
    /// The game installed last, with its card's "Installed" badge: it stays on the Downloads
    /// page, first, so it doesn't vanish from where it was installed, until it's opened or
    /// another page is picked.
    just_installed: RefCell<Option<(String, Option<gtk::Widget>)>>,
}

fn build(app: &adw::Application) {
    let window = adw::ApplicationWindow::builder().application(app).title("Lobby").default_width(1280).default_height(860).build();
    // The pages, in the middle of the header.
    let pages = gtk::Box::builder().orientation(gtk::Orientation::Horizontal).spacing(6).build();
    let header = adw::HeaderBar::builder().title_widget(&pages).build();
    let rescan_button = gtk::Button::builder().icon_name("view-refresh-symbolic").tooltip_text("Look for games again (F5)").build();
    header.pack_start(&rescan_button);
    let accounts_button = gtk::Button::builder().icon_name("avatar-default-symbolic").tooltip_text("Store accounts: Epic Games and GOG").build();
    header.pack_start(&accounts_button);
    let places_button = gtk::Button::builder().icon_name("folder-symbolic").tooltip_text("Where Lobby looks for games").build();
    header.pack_start(&places_button);
    let free_button = gtk::Button::builder().label("Free games").tooltip_text("Games free to keep right now on Epic, GOG and Steam").build();
    header.pack_start(&free_button);
    let search = gtk::SearchEntry::builder().placeholder_text("Search games").width_chars(28).build();
    header.pack_end(&search);

    // Sources, as a row of toggle buttons, and how many games show.
    let sources = gtk::Box::builder().orientation(gtk::Orientation::Horizontal).spacing(6).build();
    let count = gtk::Label::builder().margin_end(12).build();
    count.add_css_class("dim-label");
    let sort_names: Vec<&str> = Sort::ALL.iter().map(|(_, _, label)| *label).collect();
    let sort = gtk::DropDown::from_strings(&sort_names);
    sort.set_tooltip_text(Some("Order of the games"));
    sort.set_margin_start(12);
    sort.set_valign(gtk::Align::Center);
    sort.add_css_class("flat");
    let show_box = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(2).margin_top(6).margin_bottom(6).margin_start(6).margin_end(6).build();
    let show_menu = gtk::MenuButton::builder().label("Show").tooltip_text("Demos, favourites and hidden games").popover(&gtk::Popover::builder().child(&show_box).build()).build();
    show_menu.add_css_class("flat");
    let start = gtk::Box::builder().spacing(6).valign(gtk::Align::Center).build();
    start.append(&sort);
    start.append(&show_menu);
    let filters = gtk::CenterBox::builder().start_widget(&start).center_widget(&sources).end_widget(&count).margin_top(6).margin_bottom(6).build();
    filters.add_css_class("filters");

    let flow = gtk::FlowBox::builder()
        .valign(gtk::Align::Start)
        .homogeneous(true)
        .min_children_per_line(2)
        .max_children_per_line(30)
        .column_spacing(18)
        .row_spacing(18)
        .selection_mode(gtk::SelectionMode::None)
        .activate_on_single_click(true)
        .margin_top(12)
        .margin_bottom(24)
        .margin_start(24)
        .margin_end(24)
        .build();
    // Continue playing: one row of cards, scrolling sideways when there are more.
    let recent = gtk::FlowBox::builder()
        .orientation(gtk::Orientation::Vertical)
        .max_children_per_line(1)
        .column_spacing(18)
        .selection_mode(gtk::SelectionMode::None)
        .activate_on_single_click(true)
        .halign(gtk::Align::Start)
        .build();
    let recent_scroll = gtk::ScrolledWindow::builder().child(&recent).vscrollbar_policy(gtk::PolicyType::Never).hscrollbar_policy(gtk::PolicyType::Automatic).propagate_natural_height(true).build();
    let recent_title = gtk::Label::builder().label("Continue playing").xalign(0.0).build();
    recent_title.add_css_class("title-4");
    let all_title = gtk::Label::builder().label("All games").xalign(0.0).margin_top(12).build();
    all_title.add_css_class("title-4");
    let recent_box = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(12).margin_top(12).margin_start(24).margin_end(24).visible(false).build();
    recent_box.append(&recent_title);
    recent_box.append(&recent_scroll);
    recent_box.append(&all_title);
    let column = gtk::Box::builder().orientation(gtk::Orientation::Vertical).build();
    column.append(&recent_box);
    column.append(&flow);
    let scrolled = gtk::ScrolledWindow::builder().hscrollbar_policy(gtk::PolicyType::Never).child(&column).vexpand(true).build();

    let stack = gtk::Stack::builder().transition_type(gtk::StackTransitionType::Crossfade).build();
    let loading = adw::StatusPage::builder().title("Looking for your games").description("Steam, Heroic, Lutris, Faugus, your app menu, and the games folders on every drive.").build();
    let spinner = adw::Spinner::builder().width_request(48).height_request(48).halign(gtk::Align::Center).build();
    loading.set_child(Some(&spinner));
    let empty = adw::StatusPage::builder().icon_name("edit-find-symbolic").title("No games here").description("Try another filter, or a different search.").build();
    stack.add_named(&loading, Some("loading"));
    stack.add_named(&scrolled, Some("games"));
    stack.add_named(&empty, Some("empty"));

    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    toolbar.add_top_bar(&filters);
    let leftover_banner = adw::Banner::builder().button_label("Clean up…").revealed(false).build();
    toolbar.add_top_bar(&leftover_banner);
    toolbar.set_content(Some(&stack));
    let toasts = adw::ToastOverlay::new();
    toasts.set_child(Some(&toolbar));
    window.set_content(Some(&toasts));

    let ui = Rc::new(Ui { window: window.clone(), count, toasts, stack, flow: flow.clone(), search: search.clone(), rescan_button: rescan_button.clone(), lib: RefCell::new(Library::load().unwrap_or_default()), page: Cell::new(Page::Installed), origin: Cell::new(Origin::All), leftovers: RefCell::default(), leftover_banner: leftover_banner.clone(), online_warnings: RefCell::default(), failures_seen: Cell::new(crate::store::now()), offer_leftover: RefCell::default(), pages: RefCell::default(), pad_return: RefCell::default(), pad_closing: Cell::new(false), pad_pending: Cell::new(None), origins: RefCell::default(), sort: Cell::new(Sort::load()), facts: RefCell::default(), values: RefCell::default(), recent: recent.clone(), recent_box: recent_box.clone(), recent_shown: RefCell::default(), measuring: Cell::new(false), show: Cell::new(Show::load()), running: RefCell::default(), run_widgets: RefCell::default(), looking_for_running: Cell::new(false), scanning: Cell::new(false), scan_again: Cell::new(false), refreshing_stores: Cell::new(false), free_button: free_button.clone(), installing: RefCell::default(), bars: RefCell::default(), just_installed: RefCell::default() });

    let mut group: Option<gtk::ToggleButton> = None;
    for (page, name, label, tip) in Page::ALL {
        let button = gtk::ToggleButton::builder().label(label).tooltip_text(tip).name(name).build();
        button.add_css_class("pill");
        if let Some(first) = &group {
            button.set_group(Some(first));
        } else {
            button.set_active(true);
            group = Some(button.clone());
        }
        let u = ui.clone();
        button.connect_toggled(move |b| {
            if b.is_active() {
                u.page.set(page);
                if page != Page::Downloads {
                    u.forget_installed();
                }
                u.refilter();
            }
        });
        pages.append(&button);
        ui.pages.borrow_mut().push(button);
    }
    let mut group: Option<gtk::ToggleButton> = None;
    for (origin, name, label, tip) in Origin::ALL {
        let button = gtk::ToggleButton::builder().label(label).tooltip_text(tip).name(name).build();
        button.add_css_class("pill");
        button.add_css_class("flat");
        if let Some(first) = &group {
            button.set_group(Some(first));
        } else {
            button.set_active(true);
            group = Some(button.clone());
        }
        let u = ui.clone();
        button.connect_toggled(move |b| {
            if b.is_active() {
                u.origin.set(origin);
                u.refilter();
            }
        });
        sources.append(&button);
        ui.origins.borrow_mut().push((origin, button));
    }

    let u = ui.clone();
    flow.set_filter_func(move |child| u.lib.borrow().games.iter().find(|g| g.id == child.widget_name()).is_some_and(|g| u.shows(g)));
    let u = ui.clone();
    flow.set_sort_func(move |a, b| {
        let lib = u.lib.borrow();
        let name = |c: &gtk::FlowBoxChild| lib.games.iter().find(|g| g.id == c.widget_name()).map(|g| g.name.to_lowercase()).unwrap_or_default();
        let first = |c: &gtk::FlowBoxChild| !u.is_just_installed(&c.widget_name());
        // By name, in Library: installed games before the ones only owned.
        let sort = u.sort.get();
        let owned = |c: &gtk::FlowBoxChild| sort == Sort::Name && lib.games.iter().any(|g| g.id == c.widget_name() && g.kind == Kind::Owned);
        let facts = u.facts.borrow();
        let rank = |c: &gtk::FlowBoxChild| facts.get(c.widget_name().as_str()).map_or(i64::MAX, |f| f.rank(sort));
        (first(a), owned(a), rank(a), name(a)).cmp(&(first(b), owned(b), rank(b), name(b))).into()
    });
    let u = ui.clone();
    flow.connect_child_activated(move |_, child| {
        u.details(&child.widget_name());
    });
    // The Show menu's switches.
    let shown = ui.show.get();
    for (label, on, set) in [
        ("Demos and prologues not installed", shown.demos, (|s: &mut Show, v| s.demos = v) as fn(&mut Show, bool)),
        ("Only favourites", shown.favourites_only, |s, v| s.favourites_only = v),
        ("Hidden games", shown.hidden, |s, v| s.hidden = v),
    ] {
        let check = gtk::CheckButton::builder().label(label).active(on).build();
        let u = ui.clone();
        check.connect_toggled(move |c| {
            let mut s = u.show.get();
            set(&mut s, c.is_active());
            s.save();
            u.show.set(s);
            u.refilter();
        });
        show_box.append(&check);
    }
    let u = ui.clone();
    recent.connect_child_activated(move |_, child| {
        u.details(&child.widget_name());
    });
    let u = ui.clone();
    recent.set_filter_func(move |child| u.recent_shown.borrow().iter().any(|id| *id == child.widget_name()));
    sort.set_selected(Sort::ALL.iter().position(|(s, ..)| *s == ui.sort.get()).unwrap_or(0) as u32);
    let u = ui.clone();
    sort.connect_selected_notify(move |d| {
        let Some((s, ..)) = Sort::ALL.get(d.selected() as usize) else { return };
        u.sort.set(*s);
        s.save();
        u.resort();
    });
    let u = ui.clone();
    search.connect_search_changed(move |_| u.refilter());
    // Typing anywhere searches. Not while rendering a snapshot: keys typed elsewhere would land
    // here if the window takes focus.
    if std::env::var_os("LOBBY_SNAPSHOT").is_none() {
        search.set_key_capture_widget(Some(&window));
    }
    let u = ui.clone();
    rescan_button.connect_clicked(move |_| u.rescan());
    let u = ui.clone();
    ui.leftover_banner.connect_button_clicked(move |_| u.clean_up());
    // Which games are running, while the window is open (reading /proc: a few ms).
    let u = Rc::downgrade(&ui);
    glib::timeout_add_seconds_local(2, move || {
        let Some(u) = u.upgrade() else { return glib::ControlFlow::Break };
        if u.window.is_visible() {
            u.look_for_running();
        }
        glib::ControlFlow::Continue
    });
    let u = ui.clone();
    accounts_button.connect_clicked(move |_| u.accounts());
    let u = ui.clone();
    free_button.connect_clicked(move |_| u.free_games());
    let u = ui.clone();
    places_button.connect_clicked(move |_| u.places());
    // A folder dropped on the window: looked in from now on.
    let drop = gtk::DropTarget::new(gdk::FileList::static_type(), gdk::DragAction::COPY);
    let u = ui.clone();
    drop.connect_drop(move |_, value, _, _| {
        let Ok(files) = value.get::<gdk::FileList>() else { return false };
        let folders: Vec<std::path::PathBuf> = files.files().iter().filter_map(|f| f.path()).filter(|p| p.is_dir()).collect();
        if folders.is_empty() {
            u.toast("Drop a folder to have Lobby look for games in it.");
            return false;
        }
        u.add_places(&folders);
        true
    });
    window.add_controller(drop);

    let action = gio::SimpleAction::new("search", None);
    let s = search.clone();
    action.connect_activate(move |_, _| {
        s.grab_focus();
    });
    window.add_action(&action);
    let action = gio::SimpleAction::new("rescan", None);
    let u = ui.clone();
    action.connect_activate(move |_, _| u.rescan());
    window.add_action(&action);

    // The saved library shows at once; a scan then brings it up to date.
    if ui.lib.borrow().games.is_empty() {
        ui.stack.set_visible_child_name("loading");
    } else {
        ui.fill();
    }
    ui.rescan();
    window.present();
    snapshot_for_checking(&ui);
    UI.with(|u| *u.borrow_mut() = Some(ui.clone()));
    // The mouse moving (a few pixels: GTK also reports motion when the window changes under a
    // still pointer) ends a controller's outline.
    let motion = gtk::EventControllerMotion::new();
    let (w, last) = (window.clone(), Cell::new(None::<(f64, f64)>));
    motion.connect_motion(move |_, x, y| {
        if let Some((lx, ly)) = last.replace(Some((x, y))) {
            if w.has_css_class("using-pad") && ((x - lx).abs() > 4.0 || (y - ly).abs() > 4.0) {
                w.remove_css_class("using-pad");
            }
        }
    });
    window.add_controller(motion);
    // Controllers, read by a thread of their own; acted on here. Scrolling sends a step every
    // few milliseconds: only one waits at a time, or a busy window falls behind and a button
    // pressed after scrolling comes seconds late.
    static SCROLL_WAITING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    pad::watch(|action| {
        if matches!(action, pad::Action::Scroll(_)) && SCROLL_WAITING.swap(true, std::sync::atomic::Ordering::SeqCst) {
            return;
        }
        glib::MainContext::default().invoke(move || {
            if matches!(action, pad::Action::Scroll(_)) {
                SCROLL_WAITING.store(false, std::sync::atomic::Ordering::SeqCst);
            }
            if let Some(ui) = UI.with(|u| u.borrow().clone()) {
                ui.pad(action);
            }
        });
    });
    if CLAIM_PENDING.with(|p| p.replace(false)) {
        ui.claim_unclaimed();
    }
    if std::env::var_os("LOBBY_SNAPSHOT").is_none() {
        // Proton's anti-cheat runtimes, once (27 KB and 9 MB).
        gio::spawn_blocking(|| {
            if let Err(e) = crate::anticheat::fetch_runtimes() {
                crate::log::line(&e);
            }
        });
        // The daily look for new free games, unless it was turned off.
        glib::spawn_future_local(async {
            if let Ok(Err(e)) = gio::spawn_blocking(crate::free::keep_daily_check).await {
                crate::log::line(&format!("daily check: {e}"));
            }
        });
    }
}

/// For checking how the window looks without a screenshot of the desktop: with
/// LOBBY_SNAPSHOT=<file.png> the window renders itself into that file once the scan is done
/// and covers have had time to load (LOBBY_SNAPSHOT_DETAILS=<game id> opens that game's page
/// first, LOBBY_SNAPSHOT_PICKER=<game id> its cover picker, LOBBY_SNAPSHOT_SEARCH=<text>
/// searches, LOBBY_SNAPSHOT_FILTER=<n> picks a filter, LOBBY_SNAPSHOT_INSTALLED=<game id>
/// shows that game as just installed, LOBBY_SNAPSHOT_ACCOUNTS=1 the accounts,
/// LOBBY_SNAPSHOT_SIGNIN=epic|gog a store's sign-in page, LOBBY_SNAPSHOT_UNINSTALL=<game id>
/// the question before uninstalling it, LOBBY_SNAPSHOT_FREE=1 the free games,
/// LOBBY_SNAPSHOT_CLAIM=<title> a free game's claim page, LOBBY_SNAPSHOT_SCROLL=protection|saves
/// scrolls a game's page to its Protection or Saves),
/// then quits.
fn snapshot_for_checking(ui: &Rc<Ui>) {
    let Some(file) = std::env::var_os("LOBBY_SNAPSHOT").map(std::path::PathBuf::from) else { return };
    let u = ui.clone();
    glib::spawn_future_local(async move {
        while u.scanning.get() {
            glib::timeout_future(std::time::Duration::from_millis(200)).await;
        }
        if let Ok(text) = std::env::var("LOBBY_SNAPSHOT_SEARCH") {
            u.search.set_text(&text);
        }
        // LOBBY_SNAPSHOT_PAGE=installed|library|downloads, LOBBY_SNAPSHOT_SOURCE=steam|epic|gog|
        // amazon|itch|other: pressed as the user would.
        if let Ok(name) = std::env::var("LOBBY_SNAPSHOT_PAGE") {
            let pages = u.window.content().and_then(|c| find_named(&c, &name)).and_downcast::<gtk::ToggleButton>();
            pages.inspect(|b| b.set_active(true));
        }
        // LOBBY_SNAPSHOT_SHOW=demos,favourites,hidden: the Show menu's switches (not saved).
        if let Ok(on) = std::env::var("LOBBY_SNAPSHOT_SHOW") {
            u.show.set(Show { demos: on.contains("demos"), favourites_only: on.contains("favourites"), hidden: on.contains("hidden") });
            u.refilter();
        }
        // LOBBY_SNAPSHOT_SORT=name|played|playtime|size|added.
        if let Some(i) = std::env::var("LOBBY_SNAPSHOT_SORT").ok().and_then(|n| Sort::ALL.iter().position(|(_, name, _)| *name == n)) {
            if let Some(d) = u.window.content().and_then(|c| find_type::<gtk::DropDown>(&c)) {
                d.set_selected(i as u32);
            }
            while u.measuring.get() {
                glib::timeout_future(std::time::Duration::from_millis(200)).await;
            }
        }
        if let Ok(name) = std::env::var("LOBBY_SNAPSHOT_SOURCE") {
            if let Some((_, b)) = u.origins.borrow().iter().find(|(_, b)| b.widget_name() == name) {
                b.set_active(true);
            }
        }
        if let Ok(id) = std::env::var("LOBBY_SNAPSHOT_INSTALLED") {
            u.just_installed.replace(Some((id, None)));
            u.fill();
        }
        if std::env::var_os("LOBBY_SNAPSHOT_ACCOUNTS").is_some() {
            u.accounts();
        }
        // LOBBY_SNAPSHOT_MOVE=<id>: where Move… offers to put it.
        if let Ok(id) = std::env::var("LOBBY_SNAPSHOT_MOVE") {
            u.move_game(&id);
            glib::timeout_future(std::time::Duration::from_secs(3)).await;
        }
        // The page itself as WebKit draws it (<file>.page.png): some pages don't show in the
        // window's snapshot.
        if let Some(store) = std::env::var("LOBBY_SNAPSHOT_SIGNIN").ok().and_then(|s| Store::from_id(&s)) {
            let web = u.sign_in(store);
            let page = file.with_extension("page.png");
            glib::timeout_future(std::time::Duration::from_secs(8)).await;
            web.snapshot(webkit6::SnapshotRegion::Visible, webkit6::SnapshotOptions::NONE, gio::Cancellable::NONE, move |r| {
                eprintln!("page snapshot: {:?}", r.map(|t| t.save_to_png(&page)));
            });
        }
        // The downloads of installed games are found after the scan: a page's row and the
        // clean-up list need them (LOBBY_SNAPSHOT_CLEANUP=1 opens the list).
        if std::env::var_os("LOBBY_SNAPSHOT_DETAILS").is_some() || std::env::var_os("LOBBY_SNAPSHOT_CLEANUP").is_some() {
            for _ in 0..150 {
                if !u.leftovers.borrow().is_empty() && !u.online_warnings.borrow().is_empty() {
                    break;
                }
                glib::timeout_future(std::time::Duration::from_millis(200)).await;
            }
        }
        if std::env::var_os("LOBBY_SNAPSHOT_CLEANUP").is_some() {
            u.clean_up();
        }
        if let Ok(id) = std::env::var("LOBBY_SNAPSHOT_DETAILS") {
            u.details(&id);
        }
        if let Some((id, details)) = std::env::var("LOBBY_SNAPSHOT_UNINSTALL").ok().and_then(|id| Some((id.clone(), u.details(&id)?))) {
            u.ask_uninstall(&id, &details);
        }
        // LOBBY_SNAPSHOT_NEED=1: the question before installing what a game needs.
        if std::env::var_os("LOBBY_SNAPSHOT_NEED").is_some() {
            let u2 = u.clone();
            glib::spawn_future_local(async move {
                u2.ask_install_need("Underground Exterminator", "Java 25", Some("62 MB")).await;
            });
        }
        if std::env::var_os("LOBBY_SNAPSHOT_PLACES").is_some() {
            u.places();
        }
        let searching = std::env::var_os("LOBBY_SNAPSHOT_DRIVES").is_some();
        if searching {
            u.search_drives();
        }
        if std::env::var_os("LOBBY_SNAPSHOT_FREE").is_some() {
            u.free_games();
        }
        // LOBBY_SNAPSHOT_CLAIM=<title>: that free game's claim page (signed in; nothing is
        // pressed), also saved as WebKit draws it (<out>.page.png).
        let claiming = std::env::var("LOBBY_SNAPSHOT_CLAIM").ok();
        if let Some(title) = &claiming {
            let t = title.clone();
            let games = gio::spawn_blocking(move || crate::free::list().unwrap_or_default().into_iter().filter(|g| t.split(',').any(|w| w == g.title)).collect::<Vec<_>>()).await.unwrap_or_default();
            if let Some(web) = u.claim(games) {
                glib::timeout_future(std::time::Duration::from_secs(35)).await;
                let page = file.with_extension("page.png");
                web.snapshot(webkit6::SnapshotRegion::Visible, webkit6::SnapshotOptions::NONE, gio::Cancellable::NONE, move |r| {
                    eprintln!("page snapshot: {:?}", r.map(|t| t.save_to_png(&page)));
                });
            }
        }
        // The cover picker's searches take a while.
        let picker = std::env::var("LOBBY_SNAPSHOT_PICKER").ok();
        if let Some((id, details)) = picker.as_ref().and_then(|id| Some((id, u.details(id)?))) {
            u.choose_cover(id, &details);
        }
        let slow = picker.is_some() || std::env::var_os("LOBBY_SNAPSHOT_FREE").is_some();
        // LOBBY_SNAPSHOT_PAD=<seconds>: time for a test to drive the window with a controller.
        let pad_wait = std::env::var("LOBBY_SNAPSHOT_PAD").ok().and_then(|s| s.parse::<u64>().ok());
        if pad_wait.is_some() {
            eprintln!("pad: ready");
        }
        glib::timeout_future(std::time::Duration::from_secs(pad_wait.unwrap_or(if searching { 100 } else if slow { 20 } else { 5 }))).await;
        if pad_wait.is_some() {
            let f = gtk::prelude::GtkWindowExt::focus(&u.window);
            eprintln!("pad: at the snapshot, focus {:?}, focus visible {}", f.map(|f| (pad::describe(&f), f.state_flags())), u.window.gets_focus_visible());
        }
        let (w, h) = (u.window.width(), u.window.height());
        let paintable = gtk::WidgetPaintable::new(Some(&u.window));
        let snapshot = gtk::Snapshot::new();
        paintable.snapshot(&snapshot, w as f64, h as f64);
        let saved = snapshot.to_node().zip(u.window.renderer()).map(|(node, renderer)| renderer.render_texture(&node, None).save_to_png(&file));
        eprintln!("snapshot: {saved:?}");
        if let Some(app) = u.window.application() {
            app.quit();
        }
    });
}

impl Ui {
    fn shows(&self, g: &Game) -> bool {
        let page = self.page.get();
        let origin = self.origin.get();
        if g.gone || g.missing || !(page.wants(g) || page == Page::Downloads && self.is_just_installed(&g.id)) || !(origin == Origin::All || Origin::of(g) == origin) || !self.show.get().lets_through(g) {
            return false;
        }
        let q = self.search.text().to_lowercase();
        q.split_whitespace().all(|w| g.name.to_lowercase().contains(w))
    }

    fn is_just_installed(&self, id: &str) -> bool {
        self.just_installed.borrow().as_ref().is_some_and(|(i, _)| i == id)
    }

    /// The game installed last goes back to being like any other.
    fn forget_installed(&self) {
        if let Some((_, Some(badge))) = self.just_installed.take() {
            badge.set_visible(false);
            if let Some(card) = badge.ancestor(gtk::FlowBoxChild::static_type()) {
                card.remove_css_class("just-installed");
            }
        }
        self.flow.invalidate_sort();
    }

    /// What a controller does (`pad`): only while this is the active window.
    fn pad(self: &Rc<Self>, action: pad::Action) {
        use pad::Action;
        let debug = std::env::var_os("LOBBY_DEBUG_PAD").is_some();
        if !self.window.is_active() {
            if debug {
                eprintln!("pad: {action:?} ignored: the window isn't active");
            }
            return;
        }
        if debug && !matches!(action, Action::Scroll(_)) {
            eprintln!("pad: {:.2} got {action:?}", glib::monotonic_time() as f64 / 1e6);
        }
        if self.pad_closing.get() {
            if matches!(action, pad::Action::Move(_)) {
                self.pad_pending.set(Some(action));
            }
            return;
        }
        self.window.set_focus_visible(true);
        // GTK hides the focus a few seconds after a key: with a controller it stays outlined
        // until the mouse moves (style.css, .using-pad).
        self.window.add_css_class("using-pad");
        // What's on top: an open popover, else the open dialog, else the window.
        let base: gtk::Widget = self.window.visible_dialog().map(|d| d.upcast()).unwrap_or_else(|| self.window.clone().upcast());
        let popover = pad::open_popover(&base);
        let root: gtk::Widget = popover.clone().map(|p| p.upcast()).unwrap_or_else(|| base.clone());
        let focus = gtk::prelude::GtkWindowExt::focus(&self.window).filter(|f| f.is_mapped() && (f == &root || f.is_ancestor(&root)));
        let card = focus.as_ref().and_then(|f| if f.is::<gtk::FlowBoxChild>() { Some(f.clone()) } else { f.ancestor(gtk::FlowBoxChild::static_type()) });
        match action {
            Action::Move(dir) => {
                let next = match &focus {
                    Some(f) => pad::spatial(&root, f, dir),
                    None => pad::first(&root),
                };
                if let Some(n) = next {
                    n.grab_focus();
                }
            }
            Action::Accept => match &focus {
                Some(f) => {
                    let before = self.window.visible_dialog();
                    f.activate();
                    // A page opened (a game's, a question): its main button, as a controller
                    // starts anywhere.
                    let after = self.window.visible_dialog();
                    if let Some(d) = after.filter(|a| before.as_ref() != Some(a)) {
                        self.pad_return.replace(card.as_ref().map(|c| c.downgrade()).unwrap_or_default());
                        glib::idle_add_local_once(move || {
                            pad::first(d.upcast_ref()).inspect(|w| {
                                w.grab_focus();
                            });
                        });
                    }
                }
                None => {
                    pad::first(&root).inspect(|w| {
                        w.grab_focus();
                    });
                }
            },
            Action::Back => {
                if let Some(p) = &popover {
                    p.popdown();
                } else if let Some(d) = self.window.visible_dialog() {
                    self.pad_closing.set(true);
                    let u = self.clone();
                    d.connect_closed(move |_| {
                        let u = u.clone();
                        glib::idle_add_local_once(move || {
                            u.pad_closing.set(false);
                            // Back where it was opened from (a card), if that's still there.
                            let back = u.pad_return.replace(Default::default()).upgrade().filter(|w| w.is_mapped());
                            if let Some(w) = back.filter(|_| u.window.visible_dialog().is_none()) {
                                w.grab_focus();
                            }
                            if let Some(a) = u.pad_pending.take() {
                                u.pad(a);
                            }
                        });
                    });
                    if !d.close() {
                        self.pad_closing.set(false);
                    }
                } else if !self.search.text().is_empty() {
                    self.search.set_text("");
                }
            }
            Action::Menu => {
                if let Some(menu) = card.as_ref().and_then(|c| {
                    let mut child = c.first_child();
                    while let Some(w) = child {
                        if let Ok(p) = w.clone().downcast::<gtk::Popover>() {
                            return Some(p);
                        }
                        child = w.next_sibling();
                    }
                    None
                }) {
                    menu.set_pointing_to(None);
                    menu.popup();
                    let m = menu.clone();
                    glib::idle_add_local_once(move || {
                        pad::first(m.upcast_ref()).inspect(|w| {
                            w.grab_focus();
                        });
                    });
                }
            }
            Action::Play => {
                // The card's Play (or Install), else the page's main button.
                let within = card.clone().unwrap_or_else(|| base.clone());
                let mut buttons = Vec::new();
                // In the page's order (the first is Play at its top), shown ones only.
                let mut todo = vec![within];
                while let Some(w) = todo.pop() {
                    if let Ok(b) = w.clone().downcast::<gtk::Button>() {
                        if b.is_mapped() && b.is_sensitive() && (b.has_css_class("play-button") || (card.is_none() && b.has_css_class("suggested-action"))) {
                            buttons.push(b);
                        }
                    }
                    let mut child = w.last_child();
                    while let Some(c) = child {
                        if !c.is::<gtk::Popover>() {
                            todo.push(c.clone());
                        }
                        child = c.prev_sibling();
                    }
                }
                if let Some(b) = buttons.first() {
                    // LOBBY_PAD_DRY: tests say what Start would press, and start nothing.
                    if std::env::var_os("LOBBY_PAD_DRY").is_some() {
                        eprintln!("pad: Start would press {}", pad::describe(b.upcast_ref()));
                    } else {
                        b.emit_clicked();
                    }
                }
            }
            Action::Page(step) if popover.is_none() && self.window.visible_dialog().is_none() => {
                let pages = self.pages.borrow().clone();
                let at = pages.iter().position(|b| b.is_active()).unwrap_or(0) as i32;
                if let Some(b) = pages.get((at + step).clamp(0, pages.len() as i32 - 1) as usize) {
                    b.set_active(true);
                    self.pad_to_first_card();
                }
            }
            Action::Source(step) if popover.is_none() && self.window.visible_dialog().is_none() => {
                let shown: Vec<gtk::ToggleButton> = self.origins.borrow().iter().map(|(_, b)| b.clone()).filter(|b| b.is_visible()).collect();
                let at = shown.iter().position(|b| b.is_active()).unwrap_or(0) as i32;
                if let Some(b) = shown.get((at + step).clamp(0, shown.len() as i32 - 1) as usize) {
                    b.set_active(true);
                    self.pad_to_first_card();
                }
            }
            Action::Page(_) | Action::Source(_) => {}
            Action::Scroll(v) => {
                // The nearest scrolled area around the focus that scrolls up and down, else the
                // first one on top.
                let mut at = focus.clone();
                let mut area = None;
                while let Some(w) = at {
                    if let Some(s) = w.downcast_ref::<gtk::ScrolledWindow>().filter(|s| s.vadjustment().upper() > s.vadjustment().page_size()) {
                        area = Some(s.clone());
                        break;
                    }
                    at = w.parent();
                }
                let area = area.or_else(|| {
                    let mut todo = vec![root.clone()];
                    while let Some(w) = todo.pop() {
                        if let Some(s) = w.downcast_ref::<gtk::ScrolledWindow>().filter(|s| s.is_mapped() && s.vadjustment().upper() > s.vadjustment().page_size()) {
                            return Some(s.clone());
                        }
                        let mut child = w.last_child();
                        while let Some(c) = child {
                            todo.push(c.clone());
                            child = c.prev_sibling();
                        }
                    }
                    None
                });
                if let Some(a) = area {
                    let adj = a.vadjustment();
                    adj.set_value(adj.value() + v * 24.0);
                    if debug {
                        eprintln!("pad: scrolled to {:.0} of {:.0}", adj.value(), adj.upper() - adj.page_size());
                    }
                }
            }
        }
        if debug && !matches!(action, Action::Scroll(_)) {
            let w = self.window.clone();
            // After what it started has settled (a dialog closing animates).
            glib::timeout_add_local_once(std::time::Duration::from_millis(300), move || {
                let f = gtk::prelude::GtkWindowExt::focus(&w).map(|f| pad::describe(&f)).unwrap_or_else(|| "nothing".into());
                eprintln!("pad: {:.2} {action:?} -> {f}", glib::monotonic_time() as f64 / 1e6);
            });
        }
    }

    /// After a page or source changes: the first card it shows, once the cards are in place
    /// (tried for up to a second).
    fn pad_to_first_card(self: &Rc<Self>) {
        let u = self.clone();
        let tries = Cell::new(0);
        glib::timeout_add_local(std::time::Duration::from_millis(50), move || {
            let window: gtk::Widget = u.window.clone().upcast();
            let landed = pad::first(&window).is_some_and(|w| w.is::<gtk::FlowBoxChild>() && w.grab_focus());
            tries.set(tries.get() + 1);
            if landed || tries.get() >= 20 { glib::ControlFlow::Break } else { glib::ControlFlow::Continue }
        });
    }

    fn refilter(&self) {
        self.show_leftover_banner();
        // Only the sources with games on this page; back to all when the one picked has none.
        let page = self.page.get();
        let show = self.show.get();
        let on_page: Vec<Origin> = self.lib.borrow().games.iter().filter(|g| !g.gone && !g.missing && page.wants(g) && show.lets_through(g)).map(Origin::of).collect();
        let mut fallback = None;
        for (origin, button) in self.origins.borrow().iter() {
            let any = *origin == Origin::All || on_page.contains(origin);
            button.set_visible(any);
            if !any && button.is_active() {
                fallback = self.origins.borrow().first().map(|(_, b)| b.clone());
            }
        }
        if let Some(all) = fallback {
            // Its toggled handler refilters.
            all.set_active(true);
            return;
        }
        self.flow.invalidate_filter();
        let lib = self.lib.borrow();
        // Continue playing: on Installed and Library, while not searching.
        let recent: Vec<String> = if page == Page::Downloads || !self.search.text().is_empty() {
            Vec::new()
        } else {
            let mut ids = Vec::new();
            let mut child = self.recent.first_child();
            while let Some(c) = child {
                let id = c.widget_name().to_string();
                if lib.games.iter().any(|g| g.id == id && self.shows(g)) {
                    ids.push(id);
                }
                child = c.next_sibling();
            }
            ids.truncate(8);
            ids
        };
        self.recent_box.set_visible(!recent.is_empty());
        self.recent_shown.replace(recent);
        self.recent.invalidate_filter();
        let shown = lib.games.iter().filter(|g| self.shows(g)).count();
        let total = on_page.len();
        let noun = if page == Page::Downloads { "downloads" } else { "games" };
        if !self.measuring.get() {
            self.count.set_label(&if shown == total { format!("{total} {noun}") } else { format!("{shown} of {total} {noun}") });
        }
        if !self.scanning.get() || !lib.games.is_empty() {
            self.stack.set_visible_child_name(if shown == 0 { "empty" } else { "games" });
        }
    }

    /// One card per game, rebuilt from the library.
    fn fill(self: &Rc<Self>) {
        // The card with focus (a controller's or the keyboard's) is rebuilt too: focus goes to
        // its new self after, or a rescan in the background would lose the place.
        let focused = gtk::prelude::GtkWindowExt::focus(&self.window)
            .and_then(|f| if f.is::<gtk::FlowBoxChild>() { Some(f) } else { f.ancestor(gtk::FlowBoxChild::static_type()) })
            .map(|c| (c.widget_name().to_string(), c.parent().is_some_and(|p| &p == self.recent.upcast_ref::<gtk::Widget>())));
        self.flow.remove_all();
        self.recent.remove_all();
        self.bars.borrow_mut().clear();
        self.values.borrow_mut().clear();
        self.run_widgets.borrow_mut().clear();
        let lib = self.lib.borrow();
        self.facts.replace(Facts::of(&lib.games));
        for g in lib.games.iter().filter(|g| matches!(g.kind, Kind::Game | Kind::Download | Kind::WrongSystem | Kind::Owned)) {
            self.flow.append(&card(self, g));
        }
        drop(lib);
        self.fill_recent();
        self.refilter();
        if let Some((id, recent)) = focused {
            let flow = if recent { self.recent.clone() } else { self.flow.clone() };
            glib::idle_add_local_once(move || {
                let mut child = flow.first_child();
                while let Some(c) = child {
                    if c.widget_name() == id && c.is_mapped() {
                        c.grab_focus();
                        break;
                    }
                    child = c.next_sibling();
                }
            });
        }
        // The new cards say at once whether their game is running.
        self.running.borrow_mut().clear();
        self.look_for_running();
        if self.sort.get() == Sort::Size {
            self.measure();
        }
    }

    fn look_for_running(self: &Rc<Self>) {
        if self.looking_for_running.replace(true) {
            return;
        }
        let games = self.lib.borrow().games.clone();
        let u = self.clone();
        glib::spawn_future_local(async move {
            let found: std::collections::HashSet<String> = gio::spawn_blocking(move || crate::running::find(&games).into_keys().collect()).await.unwrap_or_default();
            u.looking_for_running.set(false);
            if *u.running.borrow() != found {
                u.running.replace(found);
                u.show_running();
            }
            // Starts that ended without showing anything since the last look (`lobby sandbox`
            // writes them down; one failing within 2 s is never seen running).
            let since = u.failures_seen.get();
            let new = gio::spawn_blocking(move || crate::starts::all().into_iter().filter(|(_, f)| f.when > since).collect::<Vec<_>>()).await.unwrap_or_default();
            if let Some(latest) = new.iter().map(|(_, f)| f.when).max() {
                u.failures_seen.set(latest);
            }
            {
                for (id, f) in new {
                    let name = u.game(&id).map(|g| g.name).unwrap_or(id.clone());
                    let toast = adw::Toast::builder().title(format!("{name} closed {} without showing anything", crate::starts::after(f.seconds))).button_label("Why?").timeout(10).build();
                    let u2 = u.clone();
                    toast.connect_button_clicked(move |_| {
                        u2.details(&id);
                    });
                    u.toasts.add_toast(toast);
                }
            }
        });
    }

    fn is_running(&self, id: &str) -> bool {
        self.running.borrow().contains(id)
    }

    /// Badges and buttons as the games are now: Stop in place of Play while one runs.
    fn show_running(&self) {
        self.run_widgets.borrow_mut().retain(|(_, _, b, _)| b.upgrade().is_some());
        for (id, badge, button, page) in self.run_widgets.borrow().iter() {
            let on = self.is_running(id);
            if let Some(b) = badge {
                b.set_visible(on);
            }
            let Some(button) = button.upgrade() else { continue };
            match (page, on) {
                (true, true) => button.set_label("Stop"),
                (true, false) => button.set_label("Play"),
                (false, true) => button.set_icon_name("media-playback-stop-symbolic"),
                (false, false) => button.set_icon_name("media-playback-start-symbolic"),
            }
            // The page's Stop is red; a card's is dark, to show on any cover.
            let stop = if *page { "destructive-action" } else { "stop-button" };
            if on {
                button.add_css_class(stop);
                button.remove_css_class("suggested-action");
            } else {
                button.add_css_class("suggested-action");
                button.remove_css_class(stop);
            }
        }
    }

    /// Play, or Stop while it runs (asked first: whatever the game hasn't saved is lost).
    fn play_or_stop(self: &Rc<Self>, id: &str) {
        if !self.is_running(id) {
            self.play(id);
            return;
        }
        let Some(game) = self.game(id) else { return };
        let dialog = adw::AlertDialog::new(Some(&format!("Stop {}?", game.name)), Some("Anything the game hasn't saved is lost. Use this when it's stuck; a game that doesn't quit within 5 seconds is forced to."));
        dialog.add_response("cancel", "Cancel");
        dialog.add_response("stop", "Stop");
        dialog.set_response_appearance("stop", adw::ResponseAppearance::Destructive);
        let u = self.clone();
        dialog.connect_response(None, move |_, response| {
            if response != "stop" {
                return;
            }
            let (u, game) = (u.clone(), game.clone());
            u.toast(&format!("Stopping {}…", game.name));
            glib::spawn_future_local(async move {
                let games = u.lib.borrow().games.clone();
                let g = game.clone();
                let result = gio::spawn_blocking(move || crate::running::stop(&g, &games, std::time::Duration::from_secs(5))).await.unwrap_or_else(|_| Err("it stopped with an error".into()));
                match result {
                    Ok(s) if s.gone => match s.held_by {
                        Some(holder) => u.toast(&format!("{} stopped. {holder} still holds one of its ended processes; it goes when {holder} lets go.", game.name)),
                        None => u.toast(&format!("{} stopped.", game.name)),
                    },
                    Ok(_) => u.alert(&format!("{} didn't stop", game.name), "Some of its processes are still running even after being forced to quit."),
                    Err(e) => u.toast(&e),
                }
                u.look_for_running();
            });
        });
        dialog.present(Some(&self.window));
    }

    /// Continue playing: installed games by when they were played, the newest first, favourites
    /// before the rest.
    fn fill_recent(self: &Rc<Self>) {
        self.recent.remove_all();
        let lib = self.lib.borrow();
        let facts = self.facts.borrow();
        let mut recent: Vec<&Game> = lib.games.iter().filter(|g| g.kind == Kind::Game && !g.gone && !g.missing && facts.get(&g.id).is_some_and(|f| f.last > 0)).collect();
        recent.sort_by_key(|g| (!g.favourite, std::cmp::Reverse(facts[&g.id].last)));
        recent.truncate(40);
        drop(facts);
        for g in recent {
            self.recent.append(&card(self, g));
        }
    }

    /// Marks a game a favourite or not, hides it or brings it back.
    fn mark(self: &Rc<Self>, id: &str, change: impl FnOnce(&mut Game)) {
        if let Err(e) = self.change_lib(|lib| {
            if let Some(g) = lib.get_mut(id) {
                change(g);
            }
        }) {
            self.toast(&format!("Couldn't save the library: {e}"));
            return;
        }
        // Its card again (the star, dimmed when hidden), then the rows.
        let mut child = self.flow.first_child();
        while let Some(c) = child {
            child = c.next_sibling();
            if c.widget_name() == id {
                self.flow.remove(&c);
            }
        }
        if let Some(g) = self.game(id) {
            self.flow.append(&card(self, &g));
        }
        self.fill_recent();
        self.refilter();
    }

    /// The grid in the order picked, each card saying what it's sorted by; sizes not known
    /// yet are measured first.
    fn resort(self: &Rc<Self>) {
        self.show_order();
        if self.sort.get() == Sort::Size {
            self.measure();
        }
    }

    fn show_order(&self) {
        let sort = self.sort.get();
        let facts = self.facts.borrow();
        for (id, label) in self.values.borrow().iter() {
            let text = facts.get(id).and_then(|f| f.text(sort));
            label.set_visible(text.is_some());
            label.set_label(text.as_deref().unwrap_or_default());
        }
        drop(facts);
        self.flow.invalidate_sort();
    }

    /// Measures the sizes not known yet in the background (the first time can take a minute or
    /// two: every file of every game), then sorts again.
    fn measure(self: &Rc<Self>) {
        if self.measuring.replace(true) {
            return;
        }
        let games = self.lib.borrow().games.clone();
        let u = self.clone();
        self.count.set_label("Measuring sizes…");
        glib::spawn_future_local(async move {
            let games2 = games.clone();
            let measured = gio::spawn_blocking(move || crate::sizes::measure(&games2)).await.unwrap_or(0);
            if measured > 0 {
                let sizes = gio::spawn_blocking(move || crate::sizes::known(&games)).await.unwrap_or_default();
                for (id, f) in u.facts.borrow_mut().iter_mut() {
                    f.size = sizes.get(id).copied();
                }
            }
            u.measuring.set(false);
            u.show_order();
            u.refilter();
        });
    }

    /// The downloads whose game is installed, looked for again (in the background: repack
    /// folders are measured); then the banner, and the offer after an install.
    fn find_leftovers(self: &Rc<Self>) {
        let games = self.lib.borrow().games.clone();
        let u = self.clone();
        glib::spawn_future_local(async move {
            let all = games.clone();
            let found = gio::spawn_blocking(move || crate::log::timed("leftovers", || crate::leftovers::find(&games))).await.unwrap_or_default();
            u.leftovers.replace(found);
            let warnings = gio::spawn_blocking(move || crate::log::timed("anti-cheat", || crate::anticheat::warnings(&all))).await.unwrap_or_default();
            u.online_warnings.replace(warnings);
            u.show_leftover_banner();
            let Some(id) = u.offer_leftover.take() else { return };
            let mine: Vec<_> = u.leftovers.borrow().iter().filter(|l| l.id == id).cloned().collect();
            if let Some(first) = mine.first() {
                let file = first.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                let toast = adw::Toast::builder().title(format!("{} is installed. Its download, {file}, is still there.", first.name)).button_label("Delete…").timeout(12).build();
                let u2 = u.clone();
                toast.connect_button_clicked(move |_| u2.confirm_delete_leftovers(mine.clone()));
                u.toasts.add_toast(toast);
            }
        });
    }

    fn show_leftover_banner(&self) {
        let found = self.leftovers.borrow();
        let bytes: u64 = found.iter().map(|l| l.bytes).sum();
        self.leftover_banner.set_title(&format!("{} {} of installed games take {}", found.len(), if found.len() == 1 { "download" } else { "downloads" }, crate::windows_saves::size_text(bytes)));
        self.leftover_banner.set_revealed(self.page.get() == Page::Downloads && !found.is_empty() && self.search.text().is_empty());
    }

    /// The downloads whose game is installed, each with a box: what's ticked is deleted. Ones
    /// Lobby can't say where to get again start unticked.
    fn clean_up(self: &Rc<Self>) {
        use crate::leftovers::Again;
        let found = self.leftovers.borrow().clone();
        let no_trash = found.iter().any(|l| !l.trash);
        let group = adw::PreferencesGroup::builder()
            .description(format!(
                "Archives, installers and repacks whose game is installed: the games stay as they are. Ones Lobby doesn't know where to get again start unticked.{}",
                if no_trash { " Ones marked \"no Trash\" are on a drive without a Trash: deleting them is for good." } else { "" }
            ))
            .build();
        let delete = gtk::Button::builder().label("Delete").build();
        delete.add_css_class("destructive-action");
        let ticks: Rc<RefCell<Vec<(gtk::CheckButton, crate::leftovers::Leftover)>>> = Rc::default();
        let update = {
            let (ticks, delete) = (ticks.clone(), delete.clone());
            move || {
                let chosen: Vec<u64> = ticks.borrow().iter().filter(|(c, _)| c.is_active()).map(|(_, l)| l.bytes).collect();
                delete.set_label(&if chosen.is_empty() { "Delete".to_string() } else { format!("Delete {} ({})", chosen.len(), crate::windows_saves::size_text(chosen.iter().sum())) });
                delete.set_sensitive(!chosen.is_empty());
            }
        };
        for l in found {
            let file = l.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let gone_for_good = if l.trash { "" } else { " · no Trash" };
            let row = adw::ActionRow::builder().title(&l.name).subtitle(format!("{file} · {} · {}{gone_for_good}", crate::windows_saves::size_text(l.bytes), l.again.describe())).subtitle_lines(0).use_markup(false).build();
            let check = gtk::CheckButton::builder().active(l.again != Again::Unknown).valign(gtk::Align::Center).build();
            row.add_prefix(&check);
            row.set_activatable_widget(Some(&check));
            if let Some(url) = l.again.url().map(str::to_string) {
                let open = gtk::Button::builder().icon_name("adw-external-link-symbolic").tooltip_text("Open where it can be downloaded again").valign(gtk::Align::Center).build();
                open.add_css_class("flat");
                let w = self.window.clone();
                open.connect_clicked(move |_| gtk::UriLauncher::new(&url).launch(Some(&w), gio::Cancellable::NONE, |_| {}));
                row.add_suffix(&open);
            }
            let up = update.clone();
            check.connect_toggled(move |_| up());
            ticks.borrow_mut().push((check, l));
            group.add(&row);
        }
        update();
        let page = adw::PreferencesPage::new();
        page.add(&group);
        let header = adw::HeaderBar::new();
        header.pack_end(&delete);
        let view = adw::ToolbarView::new();
        view.add_top_bar(&header);
        view.set_content(Some(&page));
        let dialog = adw::Dialog::builder().title("Downloads of installed games").content_width(680).content_height(640).child(&view).build();
        let (u, d) = (self.clone(), dialog.clone());
        delete.connect_clicked(move |_| {
            let chosen: Vec<_> = ticks.borrow().iter().filter(|(c, _)| c.is_active()).map(|(_, l)| l.clone()).collect();
            d.close();
            u.confirm_delete_leftovers(chosen);
        });
        dialog.present(Some(&self.window));
    }

    /// Asks, saying what's deleted for good (drives without a Trash), then deletes and looks
    /// again.
    fn confirm_delete_leftovers(self: &Rc<Self>, chosen: Vec<crate::leftovers::Leftover>) {
        if chosen.is_empty() {
            return;
        }
        let bytes: u64 = chosen.iter().map(|l| l.bytes).sum();
        let for_good = chosen.iter().filter(|l| !l.trash).count();
        let heading = match chosen.as_slice() {
            [one] => format!("Delete the download of {}?", one.name),
            more => format!("Delete {} downloads?", more.len()),
        };
        let trash = match (for_good, chosen.len()) {
            (0, 1) => "It goes to the Trash.".to_string(),
            (0, _) => "They go to the Trash.".to_string(),
            (1, 1) => "It's deleted for good: its drive has no Trash.".to_string(),
            (f, n) if f == n => "They're deleted for good: their drive has no Trash.".to_string(),
            (f, _) => format!("{f} of them are deleted for good (their drive has no Trash); the others go to the Trash."),
        };
        let dialog = adw::AlertDialog::new(Some(&heading), Some(&format!("This frees {}. The {} stay installed. {trash}", crate::windows_saves::size_text(bytes), if chosen.len() == 1 { "game" } else { "games" })));
        dialog.add_response("cancel", "Cancel");
        dialog.add_response("delete", "Delete");
        dialog.set_response_appearance("delete", adw::ResponseAppearance::Destructive);
        dialog.set_close_response("cancel");
        let u = self.clone();
        dialog.connect_response(None, move |_, response| {
            if response != "delete" {
                return;
            }
            let (u, chosen) = (u.clone(), chosen.clone());
            glib::spawn_future_local(async move {
                let errors = gio::spawn_blocking(move || {
                    let games = Library::load().map(|l| l.games).unwrap_or_default();
                    chosen.iter().filter_map(|l| crate::leftovers::delete(&l.path, &games).err()).collect::<Vec<_>>()
                })
                .await
                .unwrap_or_else(|_| vec!["It stopped with an error.".into()]);
                if errors.is_empty() {
                    u.toast(&format!("{} freed.", crate::windows_saves::size_text(bytes)));
                } else {
                    u.alert("Some downloads couldn't be deleted", &errors.join("\n"));
                }
                u.rescan();
            });
        });
        dialog.present(Some(&self.window));
    }

    fn toast(&self, text: &str) {
        self.toasts.add_toast(adw::Toast::builder().title(text).timeout(4).build());
    }

    fn alert(&self, heading: &str, body: &str) {
        let dialog = adw::AlertDialog::new(Some(heading), Some(body));
        dialog.add_response("ok", "OK");
        dialog.present(Some(&self.window));
    }

    fn rescan(self: &Rc<Self>) {
        if self.scanning.replace(true) {
            self.scan_again.set(true);
            return;
        }
        self.rescan_button.set_sensitive(false);
        let u = self.clone();
        glib::spawn_future_local(async move {
            crate::log::line("scan starts");
            let found = gio::spawn_blocking(|| crate::log::timed("scan", crate::find_games)).await;
            u.scanning.set(false);
            u.rescan_button.set_sensitive(true);
            match found {
                Ok(Ok(games)) => {
                    if let Err(e) = u.change_lib(|lib| crate::log::timed("scan: matching the saved library", || lib.update(games))) {
                        u.toast(&format!("Couldn't save the library: {e}"));
                    }
                    let games = u.lib.borrow().games.clone();
                    let played = games.clone();
                    gio::spawn_blocking(move || crate::playtime::take_in(&played)).await.ok();
                    u.fill();
                    u.find_leftovers();
                    // Lutris's, Faugus's and Steam shortcuts' games become Lobby's own (shown as
                    // such after a rescan), and app-menu entries start games as Play does.
                    let taking = games.clone();
                    let taken = gio::spawn_blocking(move || crate::taken::take_over(&taking)).await.unwrap_or_default();
                    if !taken.is_empty() {
                        u.scan_again.set(true);
                    }
                    gio::spawn_blocking(move || crate::log::timed("app-menu entries' protection", || crate::sandbox::sync_shortcuts(&games)));
                }
                Ok(Err(e)) => u.alert("Couldn't look for games", &e),
                Err(_) => u.alert("Couldn't look for games", "The scan stopped with an error."),
            }
            if u.scan_again.take() {
                u.rescan();
            } else {
                u.refresh_stores();
                u.check_free();
            }
        });
    }

    /// Store lists and update checks more than a few hours old are fetched again in the
    /// background (a minute or so), then the library is looked at again.
    fn refresh_stores(self: &Rc<Self>) {
        if !crate::stores::stale() || self.refreshing_stores.replace(true) {
            return;
        }
        let u = self.clone();
        glib::spawn_future_local(async move {
            let _ = gio::spawn_blocking(|| crate::log::timed("store lists and updates", crate::stores::refresh_stale)).await;
            u.refreshing_stores.set(false);
            u.rescan();
        });
    }

    fn game(&self, id: &str) -> Option<Game> {
        self.lib.borrow().games.iter().find(|g| g.id == id).cloned()
    }

    fn played(&self, id: &str, entry: Option<Entry>) {
        let _ = self.change_lib(|lib| {
            if let Some(g) = lib.get_mut(id) {
                g.played = Some(store::now());
                g.entries.extend(entry);
            }
        });
    }

    /// Changes the library as it is saved now, saves it and keeps it: a `lobby` command
    /// (launch options, protection, a scan) may have changed it since this window read it,
    /// and saving the window's own copy would undo that (a launch option set in a terminal
    /// was lost when a game was played from an open window).
    fn change_lib(&self, change: impl FnOnce(&mut Library)) -> Result<(), String> {
        let mut lib = self.lib.borrow_mut();
        if let Some(saved) = Library::load() {
            *lib = saved;
        }
        change(&mut lib);
        lib.save()
    }

    fn is_installing(&self, id: &str) -> bool {
        self.installing.borrow().iter().any(|(i, ..)| i == id)
    }

    /// Unpacks or installs a download, or downloads a game owned in a store, sets it up with
    /// gamefix, then looks for games again, so it shows as installed. The card shows how far
    /// it is meanwhile.
    fn install(self: &Rc<Self>, id: &str) {
        let Some(game) = self.game(id) else { return };
        if self.is_installing(&game.id) {
            return;
        }
        if let Some(appid) = crate::install::steam_owned(&game) {
            match crate::install::install_with_steam(&game, appid) {
                Ok(()) => self.toast(&format!("Steam's install window for {} is opening. Once Steam has installed it, it shows here as installed (F5 looks again).", game.name)),
                Err(e) => self.alert(&format!("Steam couldn't be opened for {}", game.name), &e),
            }
            return;
        }
        if game.kind == Kind::Owned {
            let store = crate::install::owned_in(&game).map_or("the store", |(s, _)| s.name());
            self.toast(&format!("Downloading {} from {store}…", game.name));
        } else {
            let Some(file) = crate::install::download(&game) else {
                self.alert(&format!("{} can't be installed", game.name), "Its download isn't there any more.");
                return;
            };
            let installer = !file.extension().is_some_and(|e| ["zip", "rar", "7z", "gz", "tgz", "xz", "bz2"].contains(&e.to_string_lossy().to_lowercase().as_str()));
            self.toast(&if installer { format!("Installing {}: its installer opens in a moment; click through it.", game.name) } else { format!("Unpacking {}…", game.name) });
        }
        let u = self.clone();
        glib::spawn_future_local(async move {
            let g = game.clone();
            // A store's install reports how much is downloaded; unpacking an archive doesn't.
            let what = if game.kind == Kind::Owned { "Downloading…" } else { "Installing…" };
            let placed = u.with_progress(&game.id, what, move |p| crate::install::place(&g, p)).await.unwrap_or_else(|| Err("The install stopped with an error.".into()));
            let result = match placed {
                Ok(placed) => {
                    // Java or a runner from Flathub first, when the game needs one: asked.
                    let target = placed.target.clone();
                    let need = gio::spawn_blocking(move || launch::setup_need(&target).map(|n| {
                        let described = n.describe();
                        (n, described)
                    }))
                    .await
                    .ok()
                    .flatten();
                    if let Some((need, (what, size))) = need {
                        if u.ask_install_need(&game.name, &what, size.as_deref()).await {
                            let installed = u.with_progress(&game.id, "Installing…", move |_| need.install()).await.unwrap_or_else(|| Err("it stopped with an error".into()));
                            if let Err(e) = installed {
                                u.alert(&format!("{what} couldn't be installed"), &e);
                            }
                        }
                    }
                    u.with_progress(&game.id, "Setting up…", move |_| crate::install::set_up(placed)).await.ok_or_else(|| "gamefix stopped with an error.".to_string())
                }
                Err(e) => Err(e),
            };
            if result.is_err() {
                u.fill();
            }
            match result {
                Ok(done) => {
                    let failed = done.lines.iter().any(|l| l.starts_with('✗'));
                    match (&done.entry, failed) {
                        (Some(_), false) => u.toast(&format!("{} is installed. Press Play to start it.", game.name)),
                        (Some(_), true) => u.alert(&format!("{} is installed, with a warning", game.name), &done.lines.join("\n")),
                        (None, _) => u.alert(&format!("{} couldn't be set up", game.name), &done.lines.join("\n")),
                    }
                    if done.entry.is_some() {
                        u.just_installed.replace(Some((game.id.clone(), None)));
                        u.offer_leftover.replace(Some(game.id.clone()));
                    }
                    u.rescan();
                }
                Err(e) => u.alert(&format!("{} couldn't be installed", game.name), &e),
            }
        });
    }

    /// Where the game's files are, with Move… when Lobby can move them (else why not).
    fn files_group(self: &Rc<Self>, game: &Game, page: &Rc<RefCell<Option<adw::Dialog>>>) -> adw::PreferencesGroup {
        let group = adw::PreferencesGroup::new();
        let row = adw::ActionRow::builder().title("Files").use_markup(false).build();
        match crate::moving::plan(game) {
            Ok(plan) => {
                row.set_subtitle(&format!("{}{}", plan.from.display(), if plan.prefix { " (its Windows setup, with the game in it)" } else { "" }));
                let button = gtk::Button::builder().label("Move…").valign(gtk::Align::Center).tooltip_text("Move it to another folder or drive").build();
                let (u, id, page) = (self.clone(), game.id.clone(), page.clone());
                button.connect_clicked(move |_| {
                    if let Some(d) = page.borrow().as_ref() {
                        d.close();
                    }
                    u.move_game(&id);
                });
                row.add_suffix(&button);
            }
            Err(why) => row.set_subtitle(&why),
        }
        group.add(&row);
        // The download it came from, still on the drive.
        for l in self.leftovers.borrow().iter().filter(|l| l.id == game.id) {
            let gone_for_good = if l.trash { "" } else { " · its drive has no Trash: deleting is for good" };
            let row = adw::ActionRow::builder().title("Its download").subtitle(format!("{} · {} · {}{gone_for_good}", l.path.display(), crate::windows_saves::size_text(l.bytes), l.again.describe())).subtitle_lines(0).use_markup(false).build();
            let button = gtk::Button::builder().label("Delete…").valign(gtk::Align::Center).tooltip_text("The game stays installed").build();
            let (u, l, page) = (self.clone(), l.clone(), page.clone());
            button.connect_clicked(move |_| {
                if let Some(d) = page.borrow().as_ref() {
                    d.close();
                }
                u.confirm_delete_leftovers(vec![l.clone()]);
            });
            row.add_suffix(&button);
            group.add(&row);
        }
        group
    }

    /// Asks where to (each drive's games folder with its free room, or any folder), confirms,
    /// then moves it with a bar on its card.
    fn move_game(self: &Rc<Self>, id: &str) {
        let Some(game) = self.game(id) else { return };
        let Ok(plan) = crate::moving::plan(&game) else { return };
        let u = self.clone();
        glib::spawn_future_local(async move {
            let from = plan.from.clone();
            let (choices, (_, bytes)) = gio::spawn_blocking(move || (move_choices(&from), crate::moving::measure(&from))).await.unwrap_or_default();
            let windows = if plan.prefix { " It lives in its Windows setup (Wine prefix), which Proton needs on a Linux-format drive, so Windows-format ones are greyed out." } else { "" };
            let group = adw::PreferencesGroup::builder().description(format!("{} takes {}. It goes into a folder of its own name in the folder you pick.{windows}", game.name, crate::windows_saves::size_text(bytes))).build();
            let page = adw::PreferencesPage::new();
            page.add(&group);
            let view = adw::ToolbarView::new();
            view.add_top_bar(&adw::HeaderBar::new());
            view.set_content(Some(&page));
            let dialog = adw::Dialog::builder().title(format!("Move {}", game.name)).content_width(560).content_height(460).child(&view).build();
            for (title, folder, free, windows) in choices {
                let row = adw::ActionRow::builder().title(&title).subtitle(format!("{} · {} free{}", folder.display(), crate::windows_saves::size_text(free), if windows { " · Windows format" } else { "" })).activatable(true).use_markup(false).build();
                row.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));
                let tight = free < bytes + bytes / 50 + (100 << 20) && !plan.from.starts_with(&folder);
                row.set_sensitive(!tight && !(windows && plan.prefix));
                let (u, d, id) = (u.clone(), dialog.clone(), game.id.clone());
                row.connect_activated(move |_| {
                    d.close();
                    u.move_to(&id, folder.clone(), bytes);
                });
                group.add(&row);
            }
            let other = adw::ActionRow::builder().title("Choose a folder…").activatable(true).build();
            other.add_suffix(&gtk::Image::from_icon_name("folder-open-symbolic"));
            let (u2, d, id) = (u.clone(), dialog.clone(), game.id.clone());
            other.connect_activated(move |_| {
                let (u, d, id) = (u2.clone(), d.clone(), id.clone());
                glib::spawn_future_local(async move {
                    if let Some(into) = gtk::FileDialog::builder().title("Move it into").modal(true).build().select_folder_future(Some(&u.window)).await.ok().and_then(|f| f.path()) {
                        d.close();
                        u.move_to(&id, into, bytes);
                    }
                });
            });
            group.add(&other);
            dialog.present(Some(&u.window));
        });
    }

    /// Confirms moving the game into `into`, then moves it with a bar on its card.
    fn move_to(self: &Rc<Self>, id: &str, into: std::path::PathBuf, bytes: u64) {
        let Some(game) = self.game(id) else { return };
        let plan = match crate::moving::plan(&game) {
            Ok(p) => p,
            Err(e) => return self.alert(&format!("{} can't be moved", game.name), &e),
        };
        let Some(to) = crate::moving::target(&plan, &into) else { return };
        let u = self.clone();
        glib::spawn_future_local(async move {
            let dev = |p: &Path| std::fs::metadata(p).ok().map(|m| std::os::unix::fs::MetadataExt::dev(&m));
            let how = if dev(&plan.from) == dev(&into) { "It's the same drive, so it's quick.".to_string() } else { format!("Its files ({}) are copied there and checked; then the old copy in {} is deleted. It can't be played until that's done.", crate::windows_saves::size_text(bytes), plan.from.display()) };
            let alert = adw::AlertDialog::new(Some(&format!("Move {} to {}?", game.name, to.display())), Some(&how));
            alert.add_responses(&[("cancel", "Cancel"), ("move", "Move")]);
            alert.set_response_appearance("move", adw::ResponseAppearance::Suggested);
            if alert.choose_future(Some(&u.window)).await != "move" {
                return;
            }
            let g = game.clone();
            let moved = u.with_progress(&game.id, "Moving…", move |p| crate::moving::move_game(&g, &into, p)).await.unwrap_or_else(|| Err("it stopped with an error".into()));
            match moved {
                Ok(_) => u.toast(&format!("{} is in {} now.", game.name, to.display())),
                Err(e) => u.alert(&format!("{} wasn't moved", game.name), &e),
            }
            u.rescan();
        });
    }

    /// Uninstalling, at the bottom of a game's page: a button when lobby can, else who can.
    fn uninstall_group(self: &Rc<Self>, game: &Game, page: &Rc<RefCell<Option<adw::Dialog>>>) -> adw::PreferencesGroup {
        use crate::uninstall::How;
        let group = adw::PreferencesGroup::new();
        let row = adw::ActionRow::builder().title("Uninstall").build();
        match crate::uninstall::how(game) {
            How::Not(why) => row.set_subtitle(&why),
            how => {
                row.set_subtitle(match how {
                    How::Steam(_) => "Steam uninstalls it (it asks first).",
                    How::Found(_) => "Lobby found it on your drive: deletes its folder, its app-menu entry, and if you like its Windows setup.",
                    _ => "Deletes its files, its app-menu entry, and if you like its Wine prefix.",
                });
                let button = gtk::Button::builder().label("Uninstall…").valign(gtk::Align::Center).build();
                button.add_css_class("destructive-action");
                row.add_suffix(&button);
                let (u, page, id, steam) = (self.clone(), page.clone(), game.id.clone(), matches!(how, How::Steam(_)));
                button.connect_clicked(move |_| {
                    let Some(page) = page.borrow().clone() else { return };
                    if steam {
                        page.close();
                        u.uninstall(&id, false);
                    } else {
                        u.ask_uninstall(&id, &page);
                    }
                });
            }
        }
        group.add(&row);
        group
    }

    /// Asks before uninstalling: what goes, and whether its Wine prefix goes too (not when
    /// another game runs in it).
    fn ask_uninstall(self: &Rc<Self>, id: &str, page: &adw::Dialog) {
        let Some(game) = self.game(id) else { return };
        let folder = game.folder().or_else(|| game.program().and_then(Path::parent)).map(|f| f.display().to_string()).unwrap_or_default();
        let alert = adw::AlertDialog::new(Some(&format!("Uninstall {}?", game.name)), Some(&format!("Its files in {folder} are deleted.")));
        alert.add_responses(&[("cancel", "Cancel"), ("uninstall", "Uninstall")]);
        alert.set_response_appearance("uninstall", adw::ResponseAppearance::Destructive);
        alert.set_default_response(Some("cancel"));
        let check = gtk::CheckButton::builder().label("Also delete its Wine prefix").build();
        if let Some(prefix) = crate::log::timed("question: its prefix", || crate::uninstall::prefix(&game)) {
            let shared = crate::log::timed("question: other games in its prefix", || crate::uninstall::prefix_shared_with(&prefix, &game, &self.lib.borrow().games));
            let cloud = crate::log::timed("question: cloud saves?", || crate::stores::installed_in(&game).filter(|(s, i)| crate::stores::saves::supported(*s, i)).map(|(s, _)| s.name()));
            let note = if !shared.is_empty() {
                check.set_sensitive(false);
                let names = if shared.len() > 3 { format!("{} other games ({}, …)", shared.len(), shared[..3].join(", ")) } else { shared.join(", ") };
                format!("Its Wine prefix stays: {names} run in it too.")
            } else {
                check.set_active(true);
                match cloud {
                    Some(store) => format!("{}: its settings and saves on this computer. Its saves go to {store}'s cloud first.", prefix.display()),
                    None => format!("{}: its settings and saves on this computer, which go with it.", prefix.display()),
                }
            };
            let label = gtk::Label::builder().label(&note).wrap(true).xalign(0.0).build();
            label.add_css_class("dim-label");
            let extra = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(6).build();
            extra.append(&check);
            extra.append(&label);
            alert.set_extra_child(Some(&extra));
        }
        let (u, page, id) = (self.clone(), page.clone(), id.to_string());
        alert.connect_response(Some("uninstall"), move |_, _| {
            page.close();
            u.uninstall(&id, check.is_active());
        });
        alert.present(Some(&self.window));
    }

    fn uninstall(self: &Rc<Self>, id: &str, with_prefix: bool) {
        let Some(game) = self.game(id) else { return };
        if self.is_installing(&game.id) {
            return;
        }
        let all = self.lib.borrow().games.clone();
        let u = self.clone();
        glib::spawn_future_local(async move {
            let g = game.clone();
            let result = u.with_progress(&game.id, "Uninstalling…", move |_| crate::uninstall::uninstall(&g, &all, with_prefix)).await.unwrap_or_else(|| Err("it stopped with an error".into()));
            // Its card goes now; the scan that follows shows what's left (a store game as not
            // installed).
            if result.is_ok() && !matches!(crate::uninstall::how(&game), crate::uninstall::How::Steam(_)) {
                if let Some(g) = u.lib.borrow_mut().get_mut(&game.id) {
                    g.gone = true;
                }
                u.refilter();
            }
            match result {
                Ok(done) if done.iter().all(|l| l.starts_with('✓') || l.starts_with("Steam")) => u.toast(&format!("{}: {}", game.name, if done.iter().any(|l| l.starts_with("Steam")) { "Steam asks you to confirm." } else { "uninstalled." })),
                Ok(done) => u.alert(&format!("{} is uninstalled", game.name), &done.join("\n")),
                Err(e) => u.alert(&format!("{} couldn't be uninstalled", game.name), &e),
            }
            u.rescan();
        });
    }

    /// A game's DLC on its page, for games lobby installed from a store: what the account owns,
    /// installed or with an Install button. Looked up when the page opens (GOG asks the store);
    /// hidden when there's none.
    /// Also says, in `info`, whether lobby syncs the game's cloud saves.
    /// The game's saves found in a Windows user folder on the drives, and bringing them over.
    /// Where the game keeps its saves here, each with Open (looked up off the main thread: the
    /// manifest's index is a few MB).
    fn fill_save_folders(self: &Rc<Self>, game: &Game, group: &adw::PreferencesGroup) {
        let (u, group, g) = (self.clone(), group.clone(), game.clone());
        glib::spawn_future_local(async move {
            let found = gio::spawn_blocking(move || crate::save_folders::find(&g)).await.unwrap_or_default();
            for f in found {
                let title = match f.what {
                    "Saves" => "Save folder".to_string(),
                    cloud => format!("Save folder ({cloud})"),
                };
                let row = adw::ActionRow::builder().title(title).subtitle(f.path.display().to_string()).subtitle_selectable(true).use_markup(false).build();
                row.add_css_class("property");
                let open = gtk::Button::builder().label("Open").valign(gtk::Align::Center).build();
                let (w, path) = (u.window.clone(), f.path.clone());
                open.connect_clicked(move |_| {
                    gtk::FileLauncher::new(Some(&gio::File::for_path(&path))).launch(Some(&w), gio::Cancellable::NONE, |_| {});
                });
                row.add_suffix(&open);
                group.add(&row);
                group.set_visible(true);
            }
        });
    }

    fn fill_windows_saves(self: &Rc<Self>, game: &Game, group: &adw::PreferencesGroup) {
        use crate::windows_saves::{size_text, Saves};
        let (u, group, g) = (self.clone(), group.clone(), game.clone());
        glib::spawn_future_local(async move {
            let id = g.id.clone();
            let saves = gio::spawn_blocking(move || crate::windows_saves::find(&g)).await.unwrap_or(Saves::None);
            let what = |f: &crate::windows_saves::Found| {
                let user = std::env::var("USER").unwrap_or_default();
                let place = f.windows.display().to_string();
                let place = ["/run/media/", "/media/"].iter().find_map(|m| place.strip_prefix(&format!("{m}{user}/"))).map_or(place.clone(), String::from);
                let changed = f.newest.and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| format!(", last changed {}", when(d.as_secs()))).unwrap_or_default();
                format!("{} file{} ({}) in {place}{changed}.", f.files, if f.files == 1 { "" } else { "s" }, size_text(f.bytes))
            };
            let row = adw::ActionRow::builder().title("Found in a Windows user folder").use_markup(false).build();
            row.add_css_class("property");
            match saves {
                Saves::None => return,
                Saves::Cloud(cloud, f) => row.set_subtitle(&format!("{} {cloud} keeps them, so they come back by themselves.", what(&f))),
                Saves::NoPrefix(f) => row.set_subtitle(&format!("{} Play it once first (that makes its Windows setup); then Lobby can bring them over.", what(&f))),
                Saves::Found { found, .. } if found.copies.is_empty() => row.set_subtitle(&format!("{} They're all here already.", what(&found))),
                Saves::Found { found, .. } => {
                    let newer = found.copies.iter().filter(|c| c.replaces).count();
                    let missing = found.copies.len() - newer;
                    let mut parts = Vec::new();
                    if missing > 0 {
                        parts.push(format!("{missing} aren't here yet"));
                    }
                    if newer > 0 {
                        parts.push(format!("{newer} are newer than the copies here (those are kept)"));
                    }
                    row.set_subtitle(&format!("{} {}.", what(&found), capitalize(&parts.join("; "))));
                    let button = gtk::Button::builder().label("Bring over").valign(gtk::Align::Center).tooltip_text("Copy them into this game's Windows setup").build();
                    button.add_css_class("suggested-action");
                    let (u, row2) = (u.clone(), row.clone());
                    let found = std::sync::Arc::new(found);
                    button.connect_clicked(move |b| {
                        let found = found.clone();
                        b.set_sensitive(false);
                        let (u, row, b, id) = (u.clone(), row2.clone(), b.clone(), id.clone());
                        glib::spawn_future_local(async move {
                            match gio::spawn_blocking(move || crate::windows_saves::bring_over(&id, &found)).await.unwrap_or_else(|_| Err("it stopped with an error".into())) {
                                Ok((copied, replaced)) => {
                                    row.remove(&b);
                                    let kept = if replaced > 0 { format!(", {replaced} replacing older ones (kept in Lobby's save-backups folder)") } else { String::new() };
                                    row.set_subtitle(&format!("Brought over {copied} file{}{kept}.", if copied == 1 { "" } else { "s" }));
                                }
                                Err(e) => {
                                    b.set_sensitive(true);
                                    u.alert("The saves couldn't all be brought over", &e);
                                }
                            }
                        });
                    });
                    row.add_suffix(&button);
                }
            }
            group.add(&row);
            group.set_visible(true);
        });
    }

    fn fill_dlc(self: &Rc<Self>, game: &Game, group: &adw::PreferencesGroup, info: &adw::PreferencesGroup, dialog: &adw::Dialog) {
        let Some((store, sid)) = crate::stores::installed_in(game) else {
            group.set_visible(false);
            return;
        };
        let looking = adw::ActionRow::builder().title("Looking for DLC you own…").build();
        looking.add_suffix(&adw::Spinner::new());
        group.add(&looking);
        let (u, group, info, dialog, id) = (self.clone(), group.clone(), info.clone(), dialog.clone(), game.id.clone());
        glib::spawn_future_local(async move {
            let (found, cloud) = gio::spawn_blocking(move || (store.dlc(&sid), crate::stores::saves::supported(store, &sid))).await.unwrap_or_else(|_| (Err("it stopped with an error".into()), false));
            if cloud {
                let row = adw::ActionRow::builder().title("Cloud saves").subtitle(format!("Lobby brings them down from {} before you play and puts them back after you quit (when you play it from Lobby).", store.name())).build();
                row.add_css_class("property");
                info.add(&row);
            }
            group.remove(&looking);
            match found {
                Ok(list) if list.is_empty() => group.set_visible(false),
                Ok(list) => {
                    for d in list {
                        let row = adw::ActionRow::builder().title(&d.title).build();
                        if d.installed {
                            let label = gtk::Label::new(Some("Installed"));
                            label.add_css_class("dim-label");
                            row.add_suffix(&label);
                        } else {
                            let button = gtk::Button::builder().label("Install").valign(gtk::Align::Center).build();
                            let (u, dialog, id) = (u.clone(), dialog.clone(), id.clone());
                            button.connect_clicked(move |_| {
                                dialog.close();
                                u.install_dlc(&id, d.clone());
                            });
                            row.add_suffix(&button);
                        }
                        group.add(&row);
                    }
                }
                Err(e) => group.add(&adw::ActionRow::builder().title("Couldn't look for DLC").subtitle(e.as_str()).build()),
            }
        });
    }

    /// Runs a long job for a game off the main thread (an install, an update, DLC), its card
    /// showing how far it is meanwhile. None: the job stopped with an error.
    /// `what` is said while the job doesn't say how far it is ("Installing…").
    async fn with_progress<T: Send + 'static>(self: &Rc<Self>, id: &str, what: &'static str, task: impl FnOnce(&dyn Fn(f64)) -> T + Send + 'static) -> Option<T> {
        let progress = Arc::new(Mutex::new(-1.0));
        self.installing.borrow_mut().push((id.to_string(), progress.clone(), what));
        self.fill();
        let u = self.clone();
        glib::timeout_add_local(std::time::Duration::from_millis(300), move || {
            u.show_progress();
            if u.installing.borrow().is_empty() { glib::ControlFlow::Break } else { glib::ControlFlow::Continue }
        });
        let result = gio::spawn_blocking(move || task(&|p| *progress.lock().unwrap() = p)).await.ok();
        self.installing.borrow_mut().retain(|(i, ..)| i != id);
        // Its card without the bar now, not after the next scan (which can take minutes).
        self.fill();
        result
    }

    /// Downloads the store's newer version of a game lobby installed.
    fn update(self: &Rc<Self>, id: &str) {
        let Some(game) = self.game(id) else { return };
        let Some((store, sid)) = crate::stores::installed_in(&game) else { return };
        if self.is_installing(&game.id) {
            return;
        }
        self.toast(&format!("Updating {}…", game.name));
        let u = self.clone();
        glib::spawn_future_local(async move {
            match u.with_progress(&game.id, "Updating…", move |p| store.update(&sid, p)).await.unwrap_or_else(|| Err("it stopped with an error".into())) {
                Ok(()) => u.toast(&format!("{} is up to date.", game.name)),
                Err(e) => u.alert(&format!("{} couldn't be updated", game.name), &e),
            }
            u.rescan();
        });
    }

    /// Downloads DLC the account owns into a game lobby installed.
    fn install_dlc(self: &Rc<Self>, id: &str, dlc: crate::stores::Dlc) {
        let Some(game) = self.game(id) else { return };
        let Some((store, sid)) = crate::stores::installed_in(&game) else { return };
        if self.is_installing(&game.id) {
            self.toast(&format!("{} is busy downloading: try again when it's done.", game.name));
            return;
        }
        self.toast(&format!("Downloading {}…", dlc.title));
        let u = self.clone();
        glib::spawn_future_local(async move {
            let ids = vec![dlc.id.clone()];
            match u.with_progress(&game.id, "Downloading…", move |p| store.install_dlc(&sid, &ids, p)).await.unwrap_or_else(|| Err("it stopped with an error".into())) {
                Ok(()) => u.toast(&format!("{} is installed.", dlc.title)),
                Err(e) => u.alert(&format!("{} couldn't be installed", dlc.title), &e),
            }
            u.fill();
        });
    }

    /// Each installing game's card: how far it is.
    fn show_progress(&self) {
        let installing = self.installing.borrow();
        for (id, bar) in self.bars.borrow().iter() {
            let Some((_, p, what)) = installing.iter().find(|(i, ..)| i == id) else { continue };
            let p = *p.lock().unwrap();
            if p < 0.0 {
                bar.pulse();
                bar.set_text(Some(what));
            } else {
                bar.set_fraction(p / 100.0);
                // "Moving 37%", "Downloading 37%": the job's own word.
                bar.set_text(Some(&format!("{} {p:.0}%", what.trim_end_matches('…'))));
            }
        }
    }

    /// Folders added to look in (they're looked in from the next scan, which starts now).
    fn add_places(self: &Rc<Self>, folders: &[std::path::PathBuf]) {
        let mut places = crate::places::Places::load();
        for f in folders {
            places.add(f);
        }
        match places.save() {
            Ok(()) => {
                let names: Vec<String> = folders.iter().map(|f| f.display().to_string()).collect();
                self.toast(&format!("Lobby looks in {} now.", names.join(", ")));
                self.rescan();
            }
            Err(e) => self.alert("Couldn't save the folder", &e),
        }
    }

    /// Where lobby looks: the folders it looks in by itself, the ones added, the games taken
    /// from the search, and the search of all drives.
    fn places(self: &Rc<Self>) {
        let page = adw::PreferencesPage::new();
        let view = adw::ToolbarView::new();
        view.add_top_bar(&adw::HeaderBar::new());
        view.set_content(Some(&page));
        let dialog = adw::Dialog::builder().title("Where Lobby looks").content_width(640).content_height(720).child(&view).build();
        let places = crate::places::Places::load();
        let home = crate::sources::Home::find();

        let added = adw::PreferencesGroup::builder().title("Folders you added").description("Each folder in them is looked at as a game or a folder of games. You can also drop a folder on Lobby's window.").build();
        let add = gtk::Button::builder().label("Add a folder…").valign(gtk::Align::Center).build();
        add.add_css_class("suggested-action");
        added.set_header_suffix(Some(&add));
        let remove_row = |group: &adw::PreferencesGroup, path: &std::path::Path, which: fn(&mut crate::places::Places) -> &mut Vec<std::path::PathBuf>| {
            let row = adw::ActionRow::builder().title(path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned())).subtitle(path.display().to_string()).build();
            let remove = gtk::Button::builder().icon_name("list-remove-symbolic").tooltip_text("Stop looking here").valign(gtk::Align::Center).build();
            remove.add_css_class("flat");
            row.add_suffix(&remove);
            group.add(&row);
            let (u, path, row, group) = (self.clone(), path.to_path_buf(), row.clone(), group.clone());
            remove.connect_clicked(move |_| {
                let mut places = crate::places::Places::load();
                which(&mut places).retain(|p| p != &path);
                if places.save().is_ok() {
                    group.remove(&row);
                    u.rescan();
                }
            });
        };
        for f in &places.added {
            remove_row(&added, f, |p| &mut p.added);
        }
        page.add(&added);
        let (u, d) = (self.clone(), dialog.clone());
        add.connect_clicked(move |_| {
            let chooser = gtk::FileDialog::builder().title("A folder to look for games in").build();
            let (window, u, d) = (u.window.clone(), u.clone(), d.clone());
            chooser.select_folder(Some(&window), gio::Cancellable::NONE, move |r| {
                if let Some(path) = r.ok().and_then(|f| f.path()) {
                    d.close();
                    u.add_places(&[path]);
                }
            });
        });

        if !places.games.is_empty() {
            let games = adw::PreferencesGroup::builder().title("Games taken from the search").build();
            for g in &places.games {
                remove_row(&games, g, |p| &mut p.games);
            }
            page.add(&games);
        }

        let search = adw::PreferencesGroup::builder()
            .title("Search all drives")
            .description("Looks through every drive for games outside these folders (a while on big drives) and shows what it finds, for you to pick.")
            .build();
        let start = gtk::Button::builder().label("Search…").valign(gtk::Align::Center).build();
        search.set_header_suffix(Some(&start));
        page.add(&search);
        let (u, d) = (self.clone(), dialog.clone());
        start.connect_clicked(move |_| {
            d.close();
            u.search_drives();
        });

        let auto = adw::PreferencesGroup::builder().title("Looked in by itself").description("Games folders, Downloads, and folders on each drive named like them (\"Downloads-old\", \"Old Games\"); in those on other drives, only what shows a game engine counts.").build();
        if let Some(home) = &home {
            let mine = &places.added;
            for r in crate::sources::folders::roots(home).into_iter().filter(|r| !mine.iter().any(|m| crate::library::norm(m) == *r)) {
                auto.add(&adw::ActionRow::builder().title(r.file_name().map_or_else(|| r.display().to_string(), |n| n.to_string_lossy().into_owned())).subtitle(r.display().to_string()).build());
            }
        }
        page.add(&auto);
        dialog.present(Some(&self.window));
    }

    /// Copies a game from a Steam library Steam doesn't use into one it does, after asking
    /// (its size, where to), then offers to restart Steam so it shows there.
    fn move_into_steam(self: &Rc<Self>, id: &str, folder: std::path::PathBuf, appid: u32) {
        let Some(game) = self.game(id) else { return };
        let u = self.clone();
        glib::spawn_future_local(async move {
            let f = folder.clone();
            let plan = gio::spawn_blocking(move || {
                let home = crate::sources::Home::find().ok_or("no home folder")?;
                let size = crate::steam_move::size_text(&f);
                let target = crate::steam_move::target(&home, 0)?;
                Ok::<_, String>((size, target))
            })
            .await
            .unwrap_or_else(|_| Err("it stopped with an error".into()));
            let (size, target) = match plan {
                Ok(p) => p,
                Err(e) => return u.alert(&format!("{} can't be moved into Steam", game.name), &e),
            };
            let alert = adw::AlertDialog::new(Some(&format!("Move {} into Steam?", game.name)), Some(&format!("Its files ({size}) are copied into Steam's library {}; the old copy stays in {} until you delete it. Steam restarts afterwards to show it.", target.display(), folder.parent().map(|p| p.display().to_string()).unwrap_or_default())));
            alert.add_responses(&[("cancel", "Cancel"), ("move", "Move into Steam")]);
            alert.set_response_appearance("move", adw::ResponseAppearance::Suggested);
            if alert.choose_future(Some(&u.window)).await != "move" {
                return;
            }
            let f = folder.clone();
            let moved = u.with_progress(&game.id, "Copying…", move |p| crate::sources::Home::find().ok_or("no home folder".to_string()).and_then(|h| crate::steam_move::move_into_steam(&h, &f, appid, p))).await.unwrap_or_else(|| Err("it stopped with an error".into()));
            match moved {
                Ok(lib) => {
                    u.toast(&format!("{} is in Steam's library {} now. Restarting Steam…", game.name, lib.display()));
                    if let Ok(Err(e)) = gio::spawn_blocking(crate::steam_move::restart_steam).await {
                        u.alert("Steam didn't restart", &e);
                    }
                }
                Err(e) => u.alert(&format!("{} couldn't be moved into Steam", game.name), &e),
            }
            u.rescan();
        });
    }

    /// The search of all drives: what it goes through as it goes, then what it found, each
    /// with a box (ticked); the ticked ones are looked at from then on, the others not
    /// suggested again. Steam libraries it found are added to lobby's places by themselves.
    fn search_drives(self: &Rc<Self>) {
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
        let status = adw::StatusPage::builder().title("Searching your drives…").description("Looked through 0 folders").build();
        status.set_child(Some(&adw::Spinner::builder().width_request(32).height_request(32).build()));
        let stack = gtk::Stack::new();
        stack.add_named(&status, Some("searching"));
        let list = adw::PreferencesGroup::new();
        let page = adw::PreferencesPage::new();
        page.add(&list);
        stack.add_named(&page, Some("found"));
        let header = adw::HeaderBar::new();
        let stop_button = gtk::Button::builder().label("Stop").build();
        let add_button = gtk::Button::builder().label("Add the ticked games").visible(false).build();
        add_button.add_css_class("suggested-action");
        header.pack_start(&stop_button);
        header.pack_end(&add_button);
        let view = adw::ToolbarView::new();
        view.add_top_bar(&header);
        view.set_content(Some(&stack));
        let dialog = adw::Dialog::builder().title("Search all drives").content_width(720).content_height(720).child(&view).build();
        dialog.present(Some(&self.window));

        let seen = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let s = stop.clone();
        stop_button.connect_clicked(move |b| {
            s.store(true, Ordering::Relaxed);
            b.set_sensitive(false);
        });
        let (sn, st, d) = (seen.clone(), status.clone(), dialog.clone());
        glib::timeout_add_local(std::time::Duration::from_millis(500), move || {
            st.set_description(Some(&format!("Looked through {} folders", sn.load(Ordering::Relaxed))));
            if d.root().is_some() && st.is_mapped() { glib::ControlFlow::Continue } else { glib::ControlFlow::Break }
        });
        // Closing the window stops the search.
        let s = stop.clone();
        dialog.connect_closed(move |_| s.store(true, Ordering::Relaxed));

        let places = crate::places::Places::load();
        let mut skip: Vec<std::path::PathBuf> = places.games.iter().chain(&places.dismissed).cloned().collect();
        let lib = self.lib.borrow();
        skip.extend(lib.games.iter().flat_map(|g| g.entries.iter().filter_map(|e| e.folder.clone())));
        drop(lib);
        let u = self.clone();
        glib::spawn_future_local(async move {
            let found = gio::spawn_blocking(move || {
                let Some(home) = crate::sources::Home::find() else { return None };
                let mut skip = skip;
                skip.extend(crate::sources::folders::roots(&home));
                let starts = crate::search::starts(&home);
                Some(crate::log::timed("search of all drives", || crate::search::search(&starts, &skip, &seen, &stop)))
            })
            .await
            .ok()
            .flatten();
            let Some(found) = found else { return };
            stop_button.set_visible(false);
            // Steam libraries: noted, their games show under Steam.
            let mut places = crate::places::Places::load();
            let known_libs = crate::sources::Home::find().map(|h| crate::sources::steam::known_libraries(&h)).unwrap_or_default();
            let new_libs: Vec<std::path::PathBuf> = found.libraries.into_iter().filter(|l| !known_libs.contains(&std::fs::canonicalize(l).unwrap_or(l.clone())) && !places.libraries.contains(l)).collect();
            if !new_libs.is_empty() {
                places.libraries.extend(new_libs.iter().cloned());
                let _ = places.save();
            }
            list.set_title(&format!("Found {} games outside the folders Lobby looks in", found.games.len()));
            if !new_libs.is_empty() {
                list.set_description(Some(&format!("Also {} Steam {} Steam doesn't use ({}): {} games show under Steam, where they can be moved into Steam.", new_libs.len(), if new_libs.len() == 1 { "library" } else { "libraries" }, new_libs.iter().map(|l| l.display().to_string()).collect::<Vec<_>>().join(", "), if new_libs.len() == 1 { "its" } else { "their" })));
            }
            let mut checks = Vec::new();
            for f in &found.games {
                let name = f.folder.file_name().map_or_else(|| f.folder.display().to_string(), |n| n.to_string_lossy().into_owned());
                let row = adw::ActionRow::builder().title(&name).subtitle(format!("{} · {}", f.why, f.folder.display())).build();
                let check = gtk::CheckButton::builder().active(true).valign(gtk::Align::Center).build();
                row.add_prefix(&check);
                row.set_activatable_widget(Some(&check));
                list.add(&row);
                checks.push((f.folder.clone(), check));
            }
            add_button.set_visible(!checks.is_empty());
            stack.set_visible_child_name("found");
            let (u2, d) = (u.clone(), dialog.clone());
            add_button.connect_clicked(move |_| {
                let u = &u2;
                let mut places = crate::places::Places::load();
                for (folder, check) in &checks {
                    if check.is_active() { places.games.push(folder.clone()) } else { places.dismissed.push(folder.clone()) }
                }
                match places.save() {
                    Ok(()) => {
                        d.close();
                        u.toast("Added: Lobby looks at them now.");
                        u.rescan();
                    }
                    Err(e) => u.alert("Couldn't save them", &e),
                }
            });
            if !new_libs.is_empty() {
                u.rescan();
            }
        });
    }

    /// What's free to keep now and next, each with whether the account has it and a way to
    /// claim it: Epic's in lobby's window (the user places the order), the rest in the browser.
    fn free_games(self: &Rc<Self>) {
        // Three groups: free now and not yours yet, free now and already in your library,
        // free soon. Each shows only with games in it.
        let group = |title: &str| {
            let flow = gtk::FlowBox::builder().valign(gtk::Align::Start).homogeneous(true).min_children_per_line(2).max_children_per_line(6).column_spacing(12).row_spacing(18).selection_mode(gtk::SelectionMode::None).build();
            let heading = gtk::Label::builder().label(title).xalign(0.0).build();
            heading.add_css_class("title-4");
            let section = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(12).visible(false).build();
            section.append(&heading);
            section.append(&flow);
            (section, flow)
        };
        let (claimable, flow) = group("Free now");
        let (yours, yours_flow) = group("Free now, already in your library");
        let (coming, coming_flow) = group("Coming next");
        let nothing = gtk::Label::builder().label("Nothing new to claim right now: every game free today is already in your library.").xalign(0.0).wrap(true).visible(false).build();
        nothing.add_css_class("dim-label");
        let status = adw::StatusPage::builder().title("Looking for free games…").build();
        status.set_child(Some(&adw::Spinner::builder().width_request(32).height_request(32).build()));
        let stack = gtk::Stack::new();
        stack.add_named(&status, Some("looking"));
        let page = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(18).margin_top(12).margin_bottom(24).margin_start(24).margin_end(24).build();
        let daily = adw::SwitchRow::builder().title("Look for new ones every day").subtitle("A notification when something new is free, even with Lobby closed; its Claim button opens the checkout.").active(crate::free::daily_on()).build();
        let settings = adw::PreferencesGroup::new();
        settings.add(&daily);
        page.append(&settings);
        page.append(&nothing);
        page.append(&claimable);
        page.append(&yours);
        page.append(&coming);
        let u = self.clone();
        daily.connect_active_notify(move |row| {
            let on = row.is_active();
            let u = u.clone();
            glib::spawn_future_local(async move {
                if let Ok(Err(e)) = gio::spawn_blocking(move || crate::free::set_daily(on)).await {
                    u.alert("Couldn't change the daily look", &e);
                }
            });
        });
        stack.add_named(&gtk::ScrolledWindow::builder().child(&page).hscrollbar_policy(gtk::PolicyType::Never).vexpand(true).build(), Some("list"));
        let header = adw::HeaderBar::new();
        let claim_all = gtk::Button::builder().label("Claim all").visible(false).build();
        claim_all.add_css_class("suggested-action");
        header.pack_start(&claim_all);
        let view = adw::ToolbarView::new();
        view.add_top_bar(&header);
        view.set_content(Some(&stack));
        let dialog = adw::Dialog::builder().title("Free games").content_width(860).content_height(700).child(&view).build();
        dialog.present(Some(&self.window));

        let u = self.clone();
        glib::spawn_future_local(async move {
            let found = gio::spawn_blocking(|| crate::free::list().map(|games| (games, crate::stores::epic::owned_items()))).await.unwrap_or_else(|_| Err("it stopped with an error".into()));
            let (games, items) = match found {
                Ok(f) => f,
                Err(e) => {
                    status.set_title("Couldn't look for free games");
                    status.set_description(Some(&e));
                    status.set_child(None::<&gtk::Widget>);
                    return;
                }
            };
            let signed_in = crate::stores::Store::Epic.account().is_some();
            let mut to_claim = Vec::new();
            for g in &games {
                let owned = crate::free::owned(g, &items);
                if !owned && g.from.is_none() && signed_in && g.store == crate::stores::Store::Epic.name() {
                    to_claim.push(g.clone());
                }
                let (section, into) = match (g.from.is_some(), owned) {
                    (true, _) => (&coming, &coming_flow),
                    (false, true) => (&yours, &yours_flow),
                    (false, false) => (&claimable, &flow),
                };
                section.set_visible(true);
                into.append(&u.free_card(g, owned, signed_in, &dialog));
            }
            nothing.set_visible(!claimable.is_visible() && yours.is_visible());
            if !to_claim.is_empty() {
                claim_all.set_label(&format!("Claim all on Epic ({})", to_claim.len()));
                claim_all.set_visible(true);
                let (u, d) = (u.clone(), dialog.clone());
                claim_all.connect_clicked(move |_| {
                    d.close();
                    u.claim(to_claim.clone());
                });
            }
            stack.set_visible_child_name("list");
        });
    }

    /// Claims every free-now Epic game the account doesn't have (the notification's Claim);
    /// with none, the free games are shown.
    fn claim_unclaimed(self: &Rc<Self>) {
        self.window.present();
        let u = self.clone();
        glib::spawn_future_local(async move {
            let games = gio::spawn_blocking(|| {
                let epic = crate::stores::epic::owned_items();
                crate::free::list().unwrap_or_default().into_iter().filter(|g| g.from.is_none() && g.store == crate::stores::Store::Epic.name() && !crate::free::owned(g, &epic)).collect::<Vec<_>>()
            })
            .await
            .unwrap_or_default();
            if games.is_empty() || crate::stores::Store::Epic.account().is_none() {
                u.free_games();
            } else {
                u.claim(games);
            }
        });
    }

    /// How many games are free to keep now that the account doesn't have (on the header's
    /// button), and a desktop notification for new ones (`lobby free --notify`, whose Claim
    /// button opens the checkout).
    fn check_free(self: &Rc<Self>) {
        let u = self.clone();
        glib::spawn_future_local(async move {
            // A checking run (LOBBY_SNAPSHOT) neither notifies nor marks anything announced.
            let checking = std::env::var_os("LOBBY_SNAPSHOT").is_some();
            let Ok(Ok((count, new))) = gio::spawn_blocking(move || {
                crate::free::list().map(|games| {
                    let epic = crate::stores::epic::owned_items();
                    let count = games.iter().filter(|g| g.from.is_none() && !crate::free::owned(g, &epic)).count();
                    (count, !checking && crate::free::any_not_announced(&games, &epic))
                })
            })
            .await
            else {
                return;
            };
            u.free_button.set_label(&if count > 0 { format!("Free games ({count})") } else { "Free games".to_string() });
            if new && !checking {
                if let Ok(lobby) = std::env::current_exe() {
                    let _ = std::process::Command::new(lobby).args(["free", "--notify"]).stdin(std::process::Stdio::null()).process_group(0).spawn();
                }
            }
        });
    }

    fn free_card(self: &Rc<Self>, g: &crate::free::FreeGame, owned: bool, signed_in: bool, dialog: &adw::Dialog) -> gtk::FlowBoxChild {
        use crate::free::Claim;
        let overlay = gtk::Overlay::builder().width_request(120).height_request(180).overflow(gtk::Overflow::Hidden).halign(gtk::Align::Center).build();
        overlay.add_css_class("cover");
        let back = gtk::Box::new(gtk::Orientation::Vertical, 0);
        back.add_css_class("cover-placeholder");
        back.add_css_class("tint-0");
        overlay.set_child(Some(&back));
        if let Some(cover) = &g.cover {
            add_cover(&overlay, cover, 120, 180);
        }
        let title = gtk::Label::builder().label(&g.title).xalign(0.0).ellipsize(gtk::pango::EllipsizeMode::End).max_width_chars(1).build();
        title.add_css_class("caption-heading");
        let when = gtk::Label::builder().label(format!("{} · {}", g.store, crate::free::when(g))).xalign(0.0).wrap(true).max_width_chars(1).build();
        when.add_css_class("dim-label");
        when.add_css_class("caption");
        let body = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(4).width_request(140).halign(gtk::Align::Center).build();
        body.append(&overlay);
        body.append(&title);
        body.append(&when);
        let epic = matches!(g.claim, Claim::EpicCheckout { .. } | Claim::EpicPage(_));
        // Owned or coming games' group says it all: only games to claim get a button.
        // Epic's giveaway can be another store item of a game the account already has (Sker
        // Ritual, Home): its checkout still adds it, but it's the same game, so it isn't offered.
        if !owned && g.from.is_none() {
            let label = if epic && signed_in { "Claim" } else if epic { "Sign in to Epic to claim" } else { "Open in browser" };
            let button = gtk::Button::builder().label(label).margin_top(4).sensitive(!epic || signed_in).build();
            button.add_css_class("pill");
            if epic {
                button.add_css_class("suggested-action");
            }
            body.append(&button);
            let (u, d, game) = (self.clone(), dialog.clone(), g.clone());
            button.connect_clicked(move |_| match &game.claim {
                Claim::Browser(url) => gtk::UriLauncher::new(url).launch(Some(&u.window), gio::Cancellable::NONE, |_| {}),
                _ => {
                    d.close();
                    u.claim(vec![game.clone()]);
                }
            });
        }
        gtk::FlowBoxChild::builder().child(&body).tooltip_text(&g.title).build()
    }

    /// Claims Epic's free games in lobby's window, signed in to Epic's site with lobby's Epic
    /// sign-in. Games known only by name (GamerPower's) are looked up in the store's search
    /// from inside the signed-in page; then one checkout holds every game, the user places the
    /// order (and answers Epic's captcha when it asks), and the window sees Epic's thanks and
    /// closes. Lobby never presses anything on Epic's pages. Games whose offer wasn't found
    /// open at their store page afterwards, one by one. Closing fetches the owned list again
    /// and says what was added.
    fn claim(self: &Rc<Self>, games: Vec<crate::free::FreeGame>) -> Option<webkit6::WebView> {
        use crate::free::Claim;
        if games.is_empty() {
            return None;
        }
        let session = webkit6::NetworkSession::new_ephemeral();
        let web = webkit6::WebView::builder().network_session(&session).vexpand(true).hexpand(true).build();
        web.connect_create(|web, action| {
            if let Some(uri) = action.request().and_then(|r| r.uri()) {
                web.load_uri(&uri);
            }
            None
        });
        let header = adw::HeaderBar::new();
        let status = gtk::Label::builder().label("Signing in to Epic…").build();
        status.add_css_class("dim-label");
        header.pack_start(&status);
        let next = gtk::Button::builder().label("Next game").visible(false).build();
        next.add_css_class("suggested-action");
        header.pack_end(&next);
        let view = adw::ToolbarView::new();
        view.add_top_bar(&header);
        view.set_content(Some(&web));
        let title = if games.len() == 1 { format!("Claim {}", games[0].title) } else { format!("Claim {} free games", games.len()) };
        let dialog = adw::Dialog::builder().title(title).content_width(1000).content_height(820).child(&view).build();
        dialog.present(Some(&self.window));

        // Signed in at the store's home page, where the search can run.
        let w = web.clone();
        let u = self.clone();
        glib::spawn_future_local(async move {
            match gio::spawn_blocking(|| crate::stores::epic::web_sign_in("https://store.epicgames.com/en-US/")).await.unwrap_or_else(|_| Err("it stopped with an error".into())) {
                Ok(url) => w.load_uri(&url),
                Err(e) => u.alert("Couldn't sign in to Epic's store", &e),
            }
        });
        let started = Rc::new(Cell::new(false));
        let pages: Rc<RefCell<Vec<String>>> = Rc::default();
        let claimed = games.clone();
        let (st, s, d, p, nx) = (started.clone(), status.clone(), dialog.clone(), pages.clone(), next.clone());
        web.connect_load_changed(move |web, event| {
            if event != webkit6::LoadEvent::Finished || st.get() || !web.uri().is_some_and(|u| u.starts_with("https://store.epicgames.com/")) {
                return;
            }
            st.set(true);
            let (web, s, d, p, nx, games) = (web.clone(), s.clone(), d.clone(), p.clone(), nx.clone(), games.clone());
            glib::spawn_future_local(async move {
                let mut offers = Vec::new();
                let mut in_checkout = Vec::new();
                for g in &games {
                    match &g.claim {
                        Claim::EpicCheckout { namespace, offer } => {
                            offers.push((namespace.clone(), offer.clone()));
                            in_checkout.push(g.clone());
                        }
                        Claim::EpicPage(url) | Claim::Browser(url) => {
                            s.set_label(&format!("Finding {}…", g.title));
                            let found = search_store(&web, &crate::free::epic_search_js(&g.title)).await.and_then(|r| crate::free::free_offer(&g.title, &r));
                            crate::log::line(&format!("claim: {}'s offer {}", g.title, if found.is_some() { "found" } else { "not found: its page instead" }));
                            match found {
                                Some(o) => {
                                    offers.push(o);
                                    in_checkout.push(g.clone());
                                }
                                None => p.borrow_mut().push(url.clone()),
                            }
                        }
                    }
                }
                if offers.is_empty() {
                    open_next_page(&web, &p, &s, &nx, &d);
                    return;
                }
                s.set_label(&format!("Place the order to add {} to your library", if offers.len() == 1 { "the game".to_string() } else { format!("all {} games", offers.len()) }));
                web.load_uri(&crate::free::epic_checkout(&offers));
                watch_for_thanks(&web, &p, &s, &nx, &d, in_checkout);
            });
        });
        let (w, s, d, p) = (web.clone(), status.clone(), dialog.clone(), pages.clone());
        next.connect_clicked(move |b| open_next_page(&w, &p, &s, b, &d));

        let u = self.clone();
        dialog.connect_closed(move |_| {
            let (u, games) = (u.clone(), claimed.clone());
            u.toast("Checking your Epic library…");
            glib::spawn_future_local(async move {
                let owned = gio::spawn_blocking(|| crate::stores::Store::Epic.refresh_owned().map(|_| crate::stores::epic::owned_items())).await.unwrap_or_else(|_| Err("it stopped with an error".into()));
                match owned {
                    Ok(items) => {
                        let (got, not): (Vec<_>, Vec<_>) = games.iter().partition(|g| crate::free::owned(g, &items));
                        let names = |l: &[&crate::free::FreeGame]| l.iter().map(|g| g.title.clone()).collect::<Vec<_>>().join(", ");
                        let mut said = Vec::new();
                        if !got.is_empty() {
                            said.push(format!("{} {} in your Epic library now: Not installed has {}.", names(&got), if got.len() == 1 { "is" } else { "are" }, if got.len() == 1 { "it" } else { "them" }));
                        }
                        if !not.is_empty() {
                            said.push(format!("Not added: {}.", names(&not)));
                        }
                        u.toast(&said.join(" "));
                    }
                    Err(e) => u.toast(&format!("Couldn't check your Epic library: {e}")),
                }
                u.rescan();
                u.check_free();
            });
        });
        Some(web)
    }

    /// The stores lobby can sign in to, signed in or not.
    fn accounts(self: &Rc<Self>) {
        let group = adw::PreferencesGroup::builder()
            .title("Store accounts")
            .description("Sign in to Epic and GOG to see the games you own and install them from here. Steam's come from the Steam app signed in on this computer, which keeps doing its own downloads, updates and cloud saves.")
            .build();
        let view = adw::ToolbarView::new();
        view.add_top_bar(&adw::HeaderBar::new());
        let page = adw::PreferencesPage::new();
        page.add(&group);
        view.set_content(Some(&page));
        let dialog = adw::Dialog::builder().title("Accounts").content_width(560).content_height(560).child(&view).build();
        // Steam: the Steam app's own sign-in, read from its files; nothing to sign in to here.
        let steam = crate::sources::Home::find().and_then(|h| crate::sources::steam_owned::account(&h));
        let row = adw::ActionRow::builder().title("Steam").build();
        match steam {
            Some((name, count)) => {
                let who = if name.is_empty() { String::new() } else { format!(" as {name}") };
                row.set_subtitle(&format!("Signed in to the Steam app{who} · {count} games owned"));
            }
            None => {
                row.set_subtitle("Sign in to the Steam app to see your Steam games here");
                let button = gtk::Button::builder().label("Open Steam").valign(gtk::Align::Center).build();
                button.connect_clicked(|_| {
                    let _ = crate::launch::start(&["steam".to_string()], None, &crate::store::log_file("steam"));
                });
                row.add_suffix(&button);
            }
        }
        group.add(&row);
        for store in Store::ALL {
            let row = adw::ActionRow::builder().title(store.name()).build();
            let account = store.account();
            let button = gtk::Button::builder().label(if account.is_some() { "Sign out" } else { "Sign in" }).valign(gtk::Align::Center).build();
            match &account {
                Some(name) => {
                    let who = if name.is_empty() { String::new() } else { format!(" as {name}") };
                    row.set_subtitle(&format!("Signed in{who} · {} games owned", crate::stores::owned(store).len()));
                }
                None => {
                    row.set_subtitle("Not signed in");
                    button.add_css_class("suggested-action");
                }
            }
            row.add_suffix(&button);
            group.add(&row);
            let u = self.clone();
            let d = dialog.clone();
            let signed_in = account.is_some();
            button.connect_clicked(move |_| {
                d.close();
                if signed_in {
                    let u = u.clone();
                    glib::spawn_future_local(async move {
                        match gio::spawn_blocking(move || store.sign_out()).await.unwrap_or_else(|_| Err("it stopped with an error".into())) {
                            Ok(()) => u.toast(&format!("Signed out of {}.", store.name())),
                            Err(e) => u.alert(&format!("Couldn't sign out of {}", store.name()), &e),
                        }
                        u.rescan();
                    });
                } else {
                    u.sign_in(store);
                }
            });
        }
        // Games Heroic installed, which lobby can make its own so Heroic can be removed.
        let heroic = crate::sources::Home::find().map(|h| crate::stores::takeover::heroic_games(&h)).unwrap_or_default();
        if !heroic.is_empty() {
            let group = adw::PreferencesGroup::builder()
                .title("Games installed by Heroic")
                .description("Lobby can take them over: it updates them, syncs their cloud saves and starts them itself, in the same Wine prefixes with the same Proton, so their settings and saves stay. Heroic can be removed afterwards.")
                .build();
            let row = adw::ActionRow::builder().title(format!("{} games", heroic.len())).subtitle(heroic.join(", ")).subtitle_lines(3).build();
            let button = gtk::Button::builder().label("Take over").valign(gtk::Align::Center).build();
            button.add_css_class("suggested-action");
            row.add_suffix(&button);
            group.add(&row);
            page.add(&group);
            let (u, d) = (self.clone(), dialog.clone());
            button.connect_clicked(move |_| {
                d.close();
                let u = u.clone();
                glib::spawn_future_local(async move {
                    let result = gio::spawn_blocking(|| crate::sources::Home::find().ok_or("no home folder".to_string()).and_then(|h| crate::stores::takeover::take_over(&h))).await.unwrap_or_else(|_| Err("it stopped with an error".into()));
                    match result {
                        Ok(done) => u.alert(&format!("Lobby took over {} games from Heroic", done.len()), &done.iter().map(|l| format!("✓ {l}")).collect::<Vec<_>>().join("\n")),
                        Err(e) => u.alert("Couldn't take over Heroic's games", &e),
                    }
                    u.rescan();
                });
            });
        }
        dialog.present(Some(&self.window));
    }

    /// The store's own sign-in page, in a browser view that keeps nothing (no cookies, no
    /// history): once it hands over the sign-in code, the store tool signs in with it.
    fn sign_in(self: &Rc<Self>, store: Store) -> webkit6::WebView {
        let session = webkit6::NetworkSession::new_ephemeral();
        let web = webkit6::WebView::builder().network_session(&session).vexpand(true).hexpand(true).build();
        let view = adw::ToolbarView::new();
        view.add_top_bar(&adw::HeaderBar::new());
        view.set_content(Some(&web));
        let dialog = adw::Dialog::builder().title(format!("Sign in to {}", store.name())).content_width(600).content_height(800).child(&view).build();
        let done = Rc::new(Cell::new(false));
        let u = self.clone();
        let d = dialog.clone();
        web.connect_load_changed(move |web, event| {
            let Some(url) = web.uri().map(|u| u.to_string()) else { return };
            if done.get() {
                return;
            }
            let finish = {
                let (u, d, done) = (u.clone(), d.clone(), done.clone());
                move |code: String| {
                    if !done.replace(true) {
                        d.close();
                        u.finish_sign_in(store, code);
                    }
                }
            };
            if let Some(code) = crate::stores::code_in(store, &url, None) {
                finish(code);
            } else if store == Store::Epic && event == webkit6::LoadEvent::Finished && url.starts_with("https://www.epicgames.com/id/api/redirect") {
                // Epic's last page is JSON holding the code.
                web.evaluate_javascript("document.body.innerText", None, None, gio::Cancellable::NONE, move |r| {
                    if let Some(code) = r.ok().and_then(|v| crate::stores::code_in(store, &url, Some(&v.to_str()))) {
                        finish(code);
                    }
                });
            }
        });
        // "Sign in with Google/Steam/Xbox" opens a new window: shown in this one instead.
        web.connect_create(|web, action| {
            if let Some(uri) = action.request().and_then(|r| r.uri()) {
                web.load_uri(&uri);
            }
            None
        });
        web.load_uri(store.sign_in_page());
        dialog.present(Some(&self.window));
        web
    }

    fn finish_sign_in(self: &Rc<Self>, store: Store, code: String) {
        self.toast(&format!("Signing in to {}…", store.name()));
        let u = self.clone();
        glib::spawn_future_local(async move {
            // Signed in, and then the list of games, which can fail on its own (it's tried
            // again by the next scan).
            let result = gio::spawn_blocking(move || store.sign_in(&code).map(|()| store.refresh_owned().map(|g| g.len())))
                .await
                .unwrap_or_else(|_| Err("it stopped with an error".into()));
            match result {
                Ok(Ok(n)) => u.toast(&format!("Signed in to {}: you own {n} games there. The ones not installed are under Not installed.", store.name())),
                Ok(Err(e)) => u.alert(&format!("Signed in to {}", store.name()), &format!("But the list of your games didn't come through yet ({e}). Lobby asks again when it looks for games: press Refresh (F5) in a moment.")),
                Err(e) => u.alert(&format!("Couldn't sign in to {}", store.name()), &e),
            }
            u.rescan();
        });
    }

    fn play(self: &Rc<Self>, id: &str) {
        // As saved now: its launch options or protection may have been changed elsewhere.
        if let Some(saved) = Library::load() {
            *self.lib.borrow_mut() = saved;
        }
        let Some(game) = self.game(id) else { return };
        if self.is_just_installed(id) {
            self.forget_installed();
            self.refilter();
        }
        // The protection it starts with is kept (a default can change once it's been played).
        let _ = self.change_lib(|lib| {
            lib.get_mut(id).map(crate::sandbox::settle);
        });
        self.sync_shortcut(id);
        match launch::plan(&game) {
            Plan::Run { command, cwd, protect, .. } => {
                // A store app's game playing in Lobby's prefix of the app for the first time: its
                // saves from where it ran before come too.
                if let Some((client, _)) = crate::clients::needed(&game).filter(|(c, _)| c.installed()) {
                    match crate::clients::bring_saves(&game, client) {
                        Ok(0) => {}
                        Ok(n) => self.toast(&format!("Brought {n} save files of {} over from where it ran before.", game.name)),
                        Err(e) => self.toast(&format!("Its saves couldn't be brought over: {e}")),
                    }
                }
                match launch::final_command(&game, command, protect).and_then(|command| launch::start(&command, cwd.as_deref(), &store::log_file(&game.id))) {
                    Ok(()) => {
                        self.toast(&format!("Starting {}…", game.name));
                        self.played(&game.id, None);
                    }
                    Err(e) => self.alert(&format!("{} didn't start", game.name), &e),
                }
            }
            Plan::Cannot(why) => self.alert(&format!("{} can't be started", game.name), &why),
            Plan::NeedsClient { client, .. } => {
                let Some(client) = crate::clients::by_id(client) else { return };
                let (u, id) = (self.clone(), game.id.clone());
                glib::spawn_future_local(async move {
                    let alert = adw::AlertDialog::new(
                        Some(&format!("{} needs the {}", game.name, client.name)),
                        Some(&format!("It only starts with the {0}, which checks your copy. Lobby can install the {0} into a Wine setup of its own. Its installer opens: follow it, then sign in to your account in the {0}.", client.name)),
                    );
                    alert.add_responses(&[("cancel", "Not now"), ("install", &format!("Install the {}", client.name))]);
                    alert.set_response_appearance("install", adw::ResponseAppearance::Suggested);
                    alert.set_default_response(Some("install"));
                    if alert.choose_future(Some(&u.window)).await != "install" {
                        return;
                    }
                    u.toast(&format!("Installing the {}: follow its installer…", client.name));
                    if let Err(e) = gio::spawn_blocking(move || client.install()).await.unwrap_or_else(|_| Err("it stopped with an error".into())) {
                        u.alert(&format!("The {} couldn't be installed", client.name), &e);
                        return;
                    }
                    let alert = adw::AlertDialog::new(Some(&format!("Sign in to the {}", client.name)), Some(&format!("The {0} is installed. Sign in to your account in its window if it asks, then play {1}.", client.name, game.name)));
                    alert.add_responses(&[("cancel", "Later"), ("play", &format!("Play {}", game.name))]);
                    alert.set_response_appearance("play", adw::ResponseAppearance::Suggested);
                    alert.set_default_response(Some("play"));
                    if alert.choose_future(Some(&u.window)).await == "play" {
                        u.play(&id);
                    }
                });
            }
            Plan::BattleNet { folder, launch } => {
                let u = self.clone();
                glib::spawn_future_local(async move {
                    if !battlenet::installed() {
                        let alert = adw::AlertDialog::new(
                            Some(&format!("{} needs Battle.net", game.name)),
                            Some("Blizzard's games only start through Blizzard's Battle.net app, which signs you in. Lobby can install it (5 MB, then it downloads the rest) into a Wine setup of its own. Its installer opens: follow it, then sign in to your Blizzard account in Battle.net."),
                        );
                        alert.add_responses(&[("cancel", "Not now"), ("install", "Install Battle.net")]);
                        alert.set_response_appearance("install", adw::ResponseAppearance::Suggested);
                        alert.set_default_response(Some("install"));
                        if alert.choose_future(Some(&u.window)).await != "install" {
                            return;
                        }
                        u.toast("Installing Battle.net: follow its installer…");
                        if let Err(e) = gio::spawn_blocking(battlenet::install).await.unwrap_or_else(|_| Err("it stopped with an error".into())) {
                            u.alert("Battle.net couldn't be installed", &e);
                            return;
                        }
                        // Battle.net opens once installed; it's restarted with the game added.
                        let alert = adw::AlertDialog::new(Some("Sign in to Battle.net"), Some(&format!("Battle.net is installed. Sign in to your Blizzard account in its window; then Lobby restarts it with {} added and starts the game.", game.name)));
                        alert.add_responses(&[("cancel", "Later"), ("play", &format!("Play {}", game.name))]);
                        alert.set_response_appearance("play", adw::ResponseAppearance::Suggested);
                        alert.set_default_response(Some("play"));
                        if alert.choose_future(Some(&u.window)).await != "play" {
                            return;
                        }
                    }
                    if !battlenet::knows(&folder) {
                        u.toast(&format!("Adding {} to Battle.net…", game.name));
                    }
                    let log = store::log_file(&game.id);
                    let g = u.game(&game.id).unwrap_or(game.clone());
                    match gio::spawn_blocking(move || battlenet::start(&g, &folder, &launch, &log)).await.unwrap_or_else(|_| Err("it stopped with an error".into())) {
                        Ok(()) => {
                            u.toast(&format!("Starting {} with Battle.net…", game.name));
                            u.played(&game.id, None);
                        }
                        Err(e) => u.alert(&format!("{} didn't start", game.name), &e),
                    }
                });
            }
            Plan::SetUp(folder) => {
                let u = self.clone();
                glib::spawn_future_local(async move {
                    // Java or a runner from Flathub first, when the game needs one: asked.
                    let f = folder.clone();
                    let need = gio::spawn_blocking(move || launch::setup_need(&f).map(|n| {
                        let described = n.describe();
                        (n, described)
                    }))
                    .await
                    .ok()
                    .flatten();
                    if let Some((need, (what, size))) = need {
                        if !u.ask_install_need(&game.name, &what, size.as_deref()).await {
                            return;
                        }
                        u.toast(&format!("Installing {what}…"));
                        if let Err(e) = gio::spawn_blocking(move || need.install()).await.unwrap_or_else(|_| Err("it stopped with an error".into())) {
                            u.alert(&format!("{what} couldn't be installed"), &e);
                            return;
                        }
                    }
                    u.toast(&format!("Setting up {} with gamefix (only the first time)…", game.name));
                    let name = game.name.clone();
                    let result = gio::spawn_blocking(move || launch::set_up(&folder, Some(&name))).await.unwrap_or_else(|_| Err("gamefix stopped with an error.".into()));
                    match result {
                        Ok((lines, Some(entry))) => {
                            let started = std::fs::read_to_string(&entry).ok().and_then(|t| launch::desktop_exec(&t)).ok_or("gamefix's entry has no command".to_string()).and_then(|(command, cwd)| launch::final_command(&game, command, true).and_then(|command| launch::start(&command, cwd.as_deref(), &store::log_file(&game.id))));
                            let mut e = Entry::new(Source::AppMenu, game.name.clone());
                            e.keys.push(Key::Desktop(entry));
                            u.played(&game.id, Some(e));
                            u.sync_shortcut(&game.id);
                            match started {
                                Ok(()) => u.toast(&format!("Starting {}…", game.name)),
                                Err(e) => u.alert(&format!("{} didn't start", game.name), &e),
                            }
                            // What couldn't be fixed is worth saying.
                            if lines.iter().any(|l| l.starts_with('✗')) {
                                u.alert(&format!("{}: set up, with a warning", game.name), &lines.join("\n"));
                            }
                        }
                        Ok((lines, None)) => u.alert(&format!("{} couldn't be set up", game.name), &lines.join("\n")),
                        Err(e) => u.alert(&format!("{} couldn't be set up", game.name), &e),
                    }
                });
            }
        }
    }

    /// Asks before installing what a game needs to run (Java, a runner from Flathub).
    async fn ask_install_need(self: &Rc<Self>, game: &str, what: &str, size: Option<&str>) -> bool {
        let size = size.map(|s| format!(" ({s} to download)")).unwrap_or_default();
        let alert = adw::AlertDialog::new(Some(&format!("{game} needs {what}")), Some(&format!("It isn't installed. Lobby can install {what}{size} for you only: no password needed, and other games can use it too.")));
        alert.add_responses(&[("cancel", "Not now"), ("install", &format!("Install {what}"))]);
        alert.set_response_appearance("install", adw::ResponseAppearance::Suggested);
        alert.set_default_response(Some("install"));
        alert.choose_future(Some(&self.window)).await == "install"
    }

    /// A game's page: its cover, Play, where it was found and how it starts.
    fn details(self: &Rc<Self>, id: &str) -> Option<adw::Dialog> {
        let game = self.game(id)?;
        let plan = launch::plan(&game);

        let cover = cover_widget(&game, 200, 300);
        cover.set_valign(gtk::Align::Start);
        let name = gtk::Label::builder().label(&game.name).wrap(true).xalign(0.0).build();
        name.add_css_class("title-1");
        let found = gtk::Label::builder().label(format!("Found in: {}", game.found_in())).wrap(true).xalign(0.0).build();
        found.add_css_class("dim-label");

        // The page's dialog, for buttons made before it is.
        let dialog_slot: Rc<RefCell<Option<adw::Dialog>>> = Rc::default();
        let download = matches!(game.kind, Kind::Download | Kind::Owned);
        let play = gtk::Button::builder().label(if download { "Install" } else { "Play" }).halign(gtk::Align::Start).build();
        play.add_css_class("pill");
        play.add_css_class("suggested-action");
        play.set_sensitive(download || !matches!(plan, Plan::Cannot(_)) || self.is_running(&game.id));
        if !download {
            self.run_widgets.borrow_mut().push((game.id.clone(), None, play.downgrade(), true));
            self.show_running();
        }
        let buttons = gtk::Box::builder().orientation(gtk::Orientation::Horizontal).spacing(12).margin_top(12).build();
        buttons.append(&play);
        // In a Steam library Steam doesn't use, and not installed in Steam's own.
        let stray = game.entries.iter().find(|e| e.source == Source::SteamElsewhere).filter(|_| !game.entries.iter().any(|e| e.source == Source::Steam && e.kind != Kind::Owned)).and_then(|e| Some((e.folder.clone()?, e.keys.iter().find_map(|k| if let Key::SteamApp(id) = k { Some(*id) } else { None })?)));
        if let Some((folder, appid)) = stray.clone() {
            play.set_visible(false);
            let b = gtk::Button::builder().label("Move into Steam").tooltip_text("Copy it into one of Steam's libraries, where Steam starts it").build();
            b.add_css_class("pill");
            b.add_css_class("suggested-action");
            buttons.prepend(&b);
            let (u, d, id) = (self.clone(), dialog_slot.clone(), game.id.clone());
            b.connect_clicked(move |_| {
                if let Some(d) = d.borrow().as_ref() {
                    d.close();
                }
                u.move_into_steam(&id, folder.clone(), appid);
            });
        }
        let update = game.update().map(|_| {
            let b = gtk::Button::builder().label("Update").tooltip_text("Download the store's newer version").build();
            b.add_css_class("pill");
            buttons.append(&b);
            b
        });
        let change = gtk::Button::builder().label("Change cover").build();
        change.add_css_class("pill");
        buttons.append(&change);
        // Favourite and Hide, as icons at the end.
        let star = gtk::ToggleButton::builder().icon_name(if game.favourite { "starred-symbolic" } else { "non-starred-symbolic" }).active(game.favourite).tooltip_text("Favourite").valign(gtk::Align::Center).build();
        star.add_css_class("circular");
        let (u, id) = (self.clone(), game.id.clone());
        star.connect_toggled(move |b| {
            let on = b.is_active();
            b.set_icon_name(if on { "starred-symbolic" } else { "non-starred-symbolic" });
            u.mark(&id, |g| g.favourite = on);
        });
        // An open eye while it shows, a crossed one once it's hidden.
        let eye = |hidden: bool| if hidden { "view-conceal-symbolic" } else { "view-reveal-symbolic" };
        let hide = gtk::ToggleButton::builder().icon_name(eye(game.hidden)).active(game.hidden).tooltip_text(if game.hidden { "Hidden: press to show it again" } else { "Hide it (Show → Hidden games brings it back)" }).valign(gtk::Align::Center).build();
        hide.add_css_class("circular");
        let (u, id) = (self.clone(), game.id.clone());
        hide.connect_toggled(move |b| {
            let on = b.is_active();
            b.set_icon_name(eye(on));
            b.set_tooltip_text(Some(if on { "Hidden: press to show it again" } else { "Hide it (Show → Hidden games brings it back)" }));
            u.mark(&id, |g| g.hidden = on);
        });
        let folder = game.folder().map(Path::to_path_buf).or_else(|| game.program().and_then(Path::parent).map(Path::to_path_buf)).filter(|f| f.is_dir());
        if let Some(folder) = folder.clone() {
            let open = gtk::Button::builder().label("Open folder").build();
            open.add_css_class("pill");
            let w = self.window.clone();
            open.connect_clicked(move |_| {
                gtk::FileLauncher::new(Some(&gio::File::for_path(&folder))).launch(Some(&w), gio::Cancellable::NONE, |_| {});
            });
            buttons.append(&open);
        }
        buttons.append(&star);
        buttons.append(&hide);

        let info = adw::PreferencesGroup::new();
        let row = |title: &str, value: &str| {
            let r = adw::ActionRow::builder().title(title).subtitle(value).subtitle_selectable(true).build();
            r.add_css_class("property");
            info.add(&r);
        };
        if let Some(version) = game.update() {
            row("Update", &if version.is_empty() { "A newer version is ready to download.".to_string() } else { format!("Version {version} is ready to download.") });
        }
        row("How it starts", &plan.describe());
        if let Some(p) = game.program() {
            row(if download { "Download" } else { "Program" }, &p.display().to_string());
        }
        if let Some(f) = &folder {
            row("Folder", &f.display().to_string());
        }
        if let Some(p) = game.entries.iter().find_map(|e| e.prefix.as_ref()) {
            row("Windows setup (Wine prefix)", &p.display().to_string());
        }
        let played = crate::playtime::of(&crate::playtime::load(), &game);
        if played.last > 0 {
            row("Last played", &when(played.last));
        }
        if played.seconds() > 0 {
            row("Play time", &crate::playtime::text(played.seconds()));
        }
        if let Some(size) = crate::sizes::known(std::slice::from_ref(&game)).get(&game.id) {
            row("Size", &crate::windows_saves::size_text(*size));
        }
        if game.since() > 0 && game.kind == Kind::Game {
            row("Added", &when(game.since()));
        }

        let text = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(6).hexpand(true).build();
        text.append(&name);
        text.append(&found);
        text.append(&buttons);
        let top = gtk::Box::builder().orientation(gtk::Orientation::Horizontal).spacing(24).build();
        top.append(&cover);
        top.append(&text);
        let content = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(24).margin_top(12).margin_bottom(24).margin_start(24).margin_end(24).build();
        content.append(&top);
        let online = self.online_warnings.borrow().get(&game.id).cloned().unwrap_or_default();
        let failed = crate::starts::failed(&game.id).map(|f| {
            let when = glib::DateTime::from_unix_local(f.when as i64).ok().and_then(|d| d.format("%-d %b, %H:%M").ok()).map(|d| d.to_string()).unwrap_or_default();
            format!("Its last start, {when}, ended {} without showing anything. {}", crate::starts::after(f.seconds), f.why.unwrap_or_else(|| "Its log may say why.".into()))
        });
        for (warning, log) in crate::drives::warnings(&game, &crate::drives::mounts()).into_iter().map(|w| (w, false)).chain(online.into_iter().map(|w| (w, false))).chain(failed.map(|w| (w, true))) {
            let icon = gtk::Image::builder().icon_name("dialog-warning-symbolic").valign(gtk::Align::Start).build();
            let text = gtk::Label::builder().label(&warning).wrap(true).xalign(0.0).hexpand(true).build();
            let row = gtk::Box::builder().orientation(gtk::Orientation::Horizontal).spacing(12).build();
            row.add_css_class("drive-warning");
            row.append(&icon);
            row.append(&text);
            // A failed start: what the game printed.
            if log {
                let button = gtk::Button::builder().label("Show log").valign(gtk::Align::Center).build();
                let (file, w) = (store::log_file(&game.id), self.window.clone());
                button.connect_clicked(move |_| gtk::FileLauncher::new(Some(&gio::File::for_path(&file))).launch(Some(&w), gio::Cancellable::NONE, |_| {}));
                row.append(&button);
            }
            content.append(&row);
        }
        content.append(&info);
        let protection = self.protection_group(&game, &plan);
        if game.kind == Kind::Game {
            content.append(&self.frame_rate_group(&game, &plan));
            content.append(&self.options_group(&game, &plan));
            content.append(&protection);
        }
        let dlc = adw::PreferencesGroup::builder().title("DLC").build();
        content.append(&dlc);
        let files = self.files_group(&game, &dialog_slot);
        let save_folders = adw::PreferencesGroup::builder().title("Saves").visible(false).build();
        content.append(&save_folders);
        if game.kind == Kind::Game && !game.gone {
            self.fill_save_folders(&game, &save_folders);
        }
        let saves = adw::PreferencesGroup::builder().title("Saves from Windows").visible(false).build();
        content.append(&saves);
        if game.kind == Kind::Game && !game.gone {
            self.fill_windows_saves(&game, &saves);
        }
        if game.kind == Kind::Game {
            content.append(&files);
            content.append(&self.uninstall_group(&game, &dialog_slot));
        }

        let view = adw::ToolbarView::new();
        view.add_top_bar(&adw::HeaderBar::new());
        let scrolled = gtk::ScrolledWindow::builder().child(&content).propagate_natural_height(true).hscrollbar_policy(gtk::PolicyType::Never).build();
        view.set_content(Some(&scrolled));
        // A checking run's snapshot of the page further down (LOBBY_SNAPSHOT_SCROLL=protection|saves).
        if let Some(target) = std::env::var("LOBBY_SNAPSHOT_SCROLL").ok().and_then(|s| match s.as_str() {
            "protection" => Some(protection.clone().upcast::<gtk::Widget>()),
            "saves" => Some(save_folders.clone().upcast::<gtk::Widget>()),
            "files" => Some(files.clone().upcast::<gtk::Widget>()),
            _ => None,
        }) {
            let (scrolled, content) = (scrolled.clone(), content.clone());
            glib::timeout_add_local_once(std::time::Duration::from_secs(2), move || {
                if let Some(at) = target.compute_point(&content, &gtk::graphene::Point::zero()) {
                    scrolled.vadjustment().set_value(at.y() as f64 - 120.0);
                }
            });
        }
        let dialog = adw::Dialog::builder().title(&game.name).content_width(720).child(&view).build();
        dialog_slot.replace(Some(dialog.clone()));
        let u = self.clone();
        let d = dialog.clone();
        let id = game.id.clone();
        play.connect_clicked(move |_| {
            if !u.is_running(&id) {
                d.close();
            }
            if download { u.install(&id) } else { u.play_or_stop(&id) }
        });
        let u = self.clone();
        let d = dialog.clone();
        let id = game.id.clone();
        change.connect_clicked(move |_| u.choose_cover(&id, &d));
        if let Some(update) = update {
            let (u, d, id) = (self.clone(), dialog.clone(), game.id.clone());
            update.connect_clicked(move |_| {
                d.close();
                u.update(&id);
            });
        }
        self.fill_dlc(&game, &dlc, &info, &dialog);
        if self.is_just_installed(&game.id) {
            let u = self.clone();
            dialog.connect_closed(move |_| {
                u.forget_installed();
                u.refilter();
            });
        }
        dialog.present(Some(&self.window));
        Some(dialog)
    }

    /// Show frame rate on a game's page: MangoHud's overlay when Lobby starts it.
    fn frame_rate_group(self: &Rc<Self>, game: &Game, plan: &Plan) -> adw::PreferencesGroup {
        let unsupported = launch::frame_rate_unsupported(plan);
        let help = unsupported.clone().unwrap_or_else(|| "MangoHud's overlay in the corner: frames per second, frame times, CPU and GPU load. Right Shift+F12 hides it while playing. From Play, and from its app-menu and desktop shortcuts.".into());
        let group = adw::PreferencesGroup::new();
        let row = adw::SwitchRow::builder().title("Show frame rate").subtitle(&help).subtitle_lines(0).active(game.frame_rate).sensitive(unsupported.is_none() || game.frame_rate).build();
        let (u, id) = (self.clone(), game.id.clone());
        row.connect_active_notify(move |row| {
            let on = row.is_active();
            match u.change_lib(|lib| {
                if let Some(g) = lib.get_mut(&id) {
                    g.frame_rate = on;
                }
            }) {
                Ok(()) => u.toast(if on { "Frame rate shown from the next Play." } else { "Frame rate not shown any more." }),
                Err(e) => u.toast(&format!("Couldn't save the library: {e}")),
            }
        });
        group.add(&row);
        group
    }

    /// Launch options on a game's page: what's added to its command when Lobby starts it.
    fn options_group(self: &Rc<Self>, game: &Game, plan: &Plan) -> adw::PreferencesGroup {
        let unsupported = launch::options_unsupported(plan);
        let help = unsupported.clone().unwrap_or_else(|| "Added after the game's program, like -dx11 -windowed. Variables go before %command%: PROTON_LOG=1 %command% -dx11. Used from Play, and from its app-menu and desktop shortcuts.".into());
        let group = adw::PreferencesGroup::builder().title("Launch options").description(&help).build();
        let row = adw::EntryRow::builder().title("Options").text(game.launch_options.as_deref().unwrap_or_default()).show_apply_button(true).sensitive(unsupported.is_none()).build();
        let (u, id) = (self.clone(), game.id.clone());
        row.connect_apply(move |row| {
            let text = row.text().trim().to_string();
            if let Err(e) = launch::parse_options(&text) {
                u.alert("Those launch options can't be used", &e);
                return;
            }
            let saved = u.change_lib(|lib| {
                if let Some(g) = lib.get_mut(&id) {
                    g.launch_options = (!text.is_empty()).then_some(text.clone());
                }
            });
            match saved {
                Ok(()) => u.toast(if text.is_empty() { "Launch options cleared." } else { "Launch options saved: Play uses them." }),
                Err(e) => u.toast(&format!("Couldn't save the library: {e}")),
            }
        });
        group.add(&row);
        group
    }

    /// Protection on a game's page: its level, internet, and (isolated) the folders it may use.
    fn protection_group(self: &Rc<Self>, game: &Game, plan: &Plan) -> gtk::Box {
        use crate::sandbox::{self, Level};
        let unsupported = launch::protection_unsupported(plan);
        let help = unsupported.clone().unwrap_or_else(|| "What the game can reach when it starts: from Play, and from its app-menu and desktop shortcuts too (not from a shortcut in Steam's library).".into());
        let level = sandbox::level(game);
        let group = adw::PreferencesGroup::builder().title("Protection").description(&help).build();
        let names: Vec<&str> = Level::ALL.iter().map(|l| l.name()).collect();
        let combo = adw::ComboRow::builder()
            .title("Level")
            .model(&gtk::StringList::new(&names))
            .selected(Level::ALL.iter().position(|l| *l == level).unwrap_or(0) as u32)
            .subtitle(level.describe())
            .subtitle_lines(0)
            .build();
        let internet = adw::SwitchRow::builder()
            .title("Internet")
            .subtitle("When off, it can't reach the internet or your network.")
            .active(sandbox::internet(game))
            .sensitive(level != Level::Off)
            .build();
        if unsupported.is_none() {
            group.add(&combo);
            group.add(&internet);
        }

        let add = gtk::Button::builder().icon_name("list-add-symbolic").tooltip_text("Share a folder with it").valign(gtk::Align::Center).build();
        add.add_css_class("flat");
        let folders = adw::PreferencesGroup::builder()
            .title("Folders it can use")
            .description("Isolated, it sees only its own folder and these (a mods folder, say).")
            .header_suffix(&add)
            .visible(unsupported.is_none() && level == Level::Isolated)
            .build();
        let (u, id, list) = (self.clone(), game.id.clone(), folders.clone());
        let add_row: Rc<dyn Fn(std::path::PathBuf)> = Rc::new(move |folder| {
            let row = adw::ActionRow::builder().title(folder.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()).subtitle(folder.display().to_string()).build();
            let remove = gtk::Button::builder().icon_name("user-trash-symbolic").tooltip_text("Stop sharing it").valign(gtk::Align::Center).build();
            remove.add_css_class("flat");
            let (u, id, l, r) = (u.clone(), id.clone(), list.clone(), row.clone());
            remove.connect_clicked(move |_| {
                u.change_protection(&id, |p| p.folders.retain(|f| *f != folder));
                l.remove(&r);
            });
            row.add_suffix(&remove);
            list.add(&row);
        });
        for f in &game.protection.folders {
            add_row(f.clone());
        }
        let (u, id) = (self.clone(), game.id.clone());
        add.connect_clicked(move |_| {
            let chooser = gtk::FileDialog::builder().title("A folder the game can use").build();
            let (u, id, add_row) = (u.clone(), id.clone(), add_row.clone());
            chooser.select_folder(Some(&u.window.clone()), gio::Cancellable::NONE, move |r| {
                let Some(path) = r.ok().and_then(|f| f.path()) else { return };
                let path = std::fs::canonicalize(&path).unwrap_or(path);
                let mut added = false;
                u.change_protection(&id, |p| {
                    if !p.folders.contains(&path) {
                        p.folders.push(path.clone());
                        added = true;
                    }
                });
                if added {
                    add_row(path);
                }
            });
        });

        let (u, id, net, list) = (self.clone(), game.id.clone(), internet.clone(), folders.clone());
        combo.connect_selected_notify(move |row| {
            let level = Level::ALL[row.selected() as usize];
            row.set_subtitle(level.describe());
            net.set_sensitive(level != Level::Off);
            list.set_visible(level == Level::Isolated);
            u.change_protection(&id, |p| p.level = Some(level));
            // Internet not chosen yet: the new level's default (off when isolated).
            if u.game(&id).is_some_and(|g| g.protection.internet.is_none()) {
                net.set_active(level != Level::Isolated);
            }
        });
        let (u, id) = (self.clone(), game.id.clone());
        internet.connect_active_notify(move |row| {
            let on = row.is_active();
            u.change_protection(&id, |p| p.internet = Some(on));
        });

        let both = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(24).build();
        both.append(&group);
        both.append(&folders);
        both
    }

    fn change_protection(&self, id: &str, change: impl FnOnce(&mut crate::sandbox::Protection)) {
        let saved = self.change_lib(|lib| {
            if let Some(g) = lib.get_mut(id) {
                change(&mut g.protection);
            }
        });
        if let Err(e) = saved {
            self.toast(&format!("Couldn't save the library: {e}"));
        }
        self.sync_shortcut(id);
    }

    /// The game's app-menu shortcut made to start it as Play does (in the background: gamefix
    /// rebuilds the menu).
    fn sync_shortcut(&self, id: &str) {
        if let Some(game) = self.game(id) {
            gio::spawn_blocking(move || {
                if let Err(e) = crate::sandbox::sync_shortcut(&game) {
                    crate::log::line(&format!("protection: {}'s app-menu entry: {e}", game.name));
                }
            });
        }
    }

    /// Saves the user's cover for a game (None: back to the one found), and shows it.
    fn set_cover(self: &Rc<Self>, id: &str, cover: Option<String>, details: &adw::Dialog) {
        let saved = self.change_lib(|lib| {
            if let Some(g) = lib.get_mut(id) {
                g.chosen_cover = cover;
            }
        });
        if let Err(e) = saved {
            self.toast(&format!("Couldn't save the library: {e}"));
        }
        self.fill();
        details.close();
        self.details(id);
    }

    /// Covers to pick from: the user's own games, Steam's and itch.io's search results for a
    /// name (the game's at first), an image file, or back to the automatic one.
    fn choose_cover(self: &Rc<Self>, id: &str, details: &adw::Dialog) {
        let Some(game) = self.game(id) else { return };
        let search = gtk::SearchEntry::builder().text(&game.name).hexpand(true).build();
        let file = gtk::Button::builder().label("Image file…").build();
        let auto = gtk::Button::builder().label("Automatic").tooltip_text("The cover lobby found by itself").sensitive(game.chosen_cover.is_some()).build();
        let bar = gtk::Box::builder().orientation(gtk::Orientation::Horizontal).spacing(6).margin_start(12).margin_end(12).margin_bottom(6).build();
        bar.append(&search);
        bar.append(&file);
        bar.append(&auto);

        let flow = gtk::FlowBox::builder().valign(gtk::Align::Start).homogeneous(true).min_children_per_line(2).max_children_per_line(8).column_spacing(12).row_spacing(12).selection_mode(gtk::SelectionMode::None).activate_on_single_click(true).margin_top(12).margin_bottom(12).margin_start(12).margin_end(12).build();
        let stack = gtk::Stack::new();
        let spinner = adw::Spinner::builder().width_request(32).height_request(32).halign(gtk::Align::Center).valign(gtk::Align::Center).build();
        stack.add_named(&spinner, Some("searching"));
        stack.add_named(&gtk::ScrolledWindow::builder().child(&flow).hscrollbar_policy(gtk::PolicyType::Never).vexpand(true).build(), Some("results"));
        stack.add_named(&adw::StatusPage::builder().icon_name("edit-find-symbolic").title("No covers found").description("Try another name, or pick an image file.").build(), Some("none"));

        let view = adw::ToolbarView::new();
        view.add_top_bar(&adw::HeaderBar::new());
        view.add_top_bar(&bar);
        view.set_content(Some(&stack));
        let picker = adw::Dialog::builder().title(format!("Cover for {}", game.name)).content_width(820).content_height(640).child(&view).build();

        // Each search's results, in the order the searches were made: a slow one can't
        // overwrite a newer one's.
        let searches = Rc::new(Cell::new(0u32));
        // The covers shown, in the grid's order.
        let shown: Rc<RefCell<Vec<String>>> = Rc::default();
        let (u, p, d, i, sh) = (self.clone(), picker.clone(), details.clone(), id.to_string(), shown.clone());
        flow.connect_child_activated(move |_, child| {
            let cover = sh.borrow().get(child.index() as usize).cloned();
            if let Some(cover) = cover {
                p.close();
                u.set_cover(&i, Some(cover), &d);
            }
        });
        let run = {
            let (flow, stack, searches, shown) = (flow.clone(), stack.clone(), searches.clone(), shown.clone());
            move |query: String| {
                let n = searches.get() + 1;
                searches.set(n);
                stack.set_visible_child_name("searching");
                let (flow, stack, searches, shown) = (flow.clone(), stack.clone(), searches.clone(), shown.clone());
                glib::spawn_future_local(async move {
                    let found = gio::spawn_blocking(move || crate::artwork::candidates(&query)).await.unwrap_or_default();
                    if searches.get() != n {
                        return;
                    }
                    flow.remove_all();
                    for c in &found {
                        flow.append(&candidate_card(c));
                    }
                    stack.set_visible_child_name(if found.is_empty() { "none" } else { "results" });
                    *shown.borrow_mut() = found.into_iter().map(|c| c.cover).collect();
                });
            }
        };
        run(game.name.clone());
        let r = run.clone();
        search.connect_activate(move |s| r(s.text().to_string()));

        let (u, p, d, i) = (self.clone(), picker.clone(), details.clone(), id.to_string());
        auto.connect_clicked(move |_| {
            p.close();
            u.set_cover(&i, None, &d);
        });
        let (u, p, d, i) = (self.clone(), picker.clone(), details.clone(), id.to_string());
        file.connect_clicked(move |_| {
            let filter = gtk::FileFilter::new();
            filter.add_mime_type("image/*");
            filter.set_name(Some("Images"));
            let filters = gio::ListStore::new::<gtk::FileFilter>();
            filters.append(&filter);
            let chooser = gtk::FileDialog::builder().title("Choose a cover").filters(&filters).modal(true).build();
            let (u, p, d, i) = (u.clone(), p.clone(), d.clone(), i.clone());
            chooser.open(Some(&u.window.clone()), gio::Cancellable::NONE, move |result| {
                let Some(path) = result.ok().and_then(|f| f.path()) else { return };
                // A copy of its own, so the cover stays if the file is moved or deleted.
                let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_else(|| "img".into());
                let kept = store::dir().join("covers").join(format!("{i}.{ext}"));
                let saved = std::fs::create_dir_all(kept.parent().unwrap()).and_then(|_| std::fs::copy(&path, &kept));
                match saved {
                    Ok(_) => {
                        p.close();
                        u.set_cover(&i, Some(kept.display().to_string()), &d);
                    }
                    Err(e) => u.toast(&format!("Couldn't use that image: {e}")),
                }
            });
        });
        picker.present(Some(&self.window));
    }
}

/// A cover to pick in the cover picker: the art, its title and where it's from.
fn candidate_card(c: &crate::artwork::Candidate) -> gtk::FlowBoxChild {
    let overlay = gtk::Overlay::builder().width_request(120).height_request(180).overflow(gtk::Overflow::Hidden).halign(gtk::Align::Center).build();
    overlay.add_css_class("cover");
    let back = gtk::Box::new(gtk::Orientation::Vertical, 0);
    back.add_css_class("cover-placeholder");
    back.add_css_class("tint-5");
    overlay.set_child(Some(&back));
    add_cover(&overlay, &c.cover, 120, 180);
    let title = gtk::Label::builder().label(&c.title).xalign(0.0).ellipsize(gtk::pango::EllipsizeMode::End).max_width_chars(1).build();
    title.add_css_class("caption-heading");
    let from = gtk::Label::builder().label(&c.from).xalign(0.0).build();
    from.add_css_class("dim-label");
    from.add_css_class("caption");
    let body = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(4).width_request(120).halign(gtk::Align::Center).build();
    body.append(&overlay);
    body.append(&title);
    body.append(&from);
    gtk::FlowBoxChild::builder().child(&body).tooltip_text(format!("{} ({})", c.title, c.from)).build()
}

/// Runs the store search's JavaScript in the (signed-in) page and returns what it returned.
async fn search_store(web: &webkit6::WebView, js: &str) -> Option<String> {
    let answer: Rc<RefCell<Option<Option<String>>>> = Rc::default();
    let a = answer.clone();
    web.call_async_javascript_function(js, None, None, None, gio::Cancellable::NONE, move |r| {
        *a.borrow_mut() = Some(r.ok().map(|v| v.to_str().to_string()));
    });
    // Up to 20 s.
    for _ in 0..200 {
        if let Some(got) = answer.borrow_mut().take() {
            return got;
        }
        glib::timeout_future(std::time::Duration::from_millis(100)).await;
    }
    None
}

/// Opens the next store page lobby couldn't put in the checkout (the user presses Get and
/// places the order there); with none left, the window closes.
fn open_next_page(web: &webkit6::WebView, pages: &Rc<RefCell<Vec<String>>>, status: &gtk::Label, next: &gtk::Button, dialog: &adw::Dialog) {
    let page = {
        let mut p = pages.borrow_mut();
        (!p.is_empty()).then(|| p.remove(0))
    };
    match page {
        Some(url) => {
            status.set_label("Lobby couldn't put this one in the checkout: press Get on its page");
            next.set_label(if pages.borrow().is_empty() { "Done" } else { "Next game" });
            next.set_visible(true);
            web.load_uri(&url);
        }
        None => {
            dialog.close();
        }
    }
}

/// Watches the checkout (reading the page, pressing nothing): once Epic's order button
/// ("Add to library", "Place order") has been there and is gone, the order was most likely
/// placed (Epic's checkout then goes blank: it's made for Epic's launcher, which closes it),
/// so the Epic library is checked; with every game in it, the window says so and goes on to
/// any store pages left, or closes. A captcha over the button doesn't fool it: the library
/// decides.
fn watch_for_thanks(web: &webkit6::WebView, pages: &Rc<RefCell<Vec<String>>>, status: &gtk::Label, next: &gtk::Button, dialog: &adw::Dialog, games: Vec<crate::free::FreeGame>) {
    // 0: no order button seen yet, 1: seen, 2: checking the library, 3: done.
    let state = Rc::new(Cell::new(0u8));
    let (web, pages, status, next, dialog) = (web.clone(), pages.clone(), status.clone(), next.clone(), dialog.clone());
    let asking = status.label().to_string();
    glib::timeout_add_local(std::time::Duration::from_millis(1500), move || {
        if dialog.root().is_none() || state.get() == 3 {
            return glib::ControlFlow::Break;
        }
        if state.get() == 2 {
            return glib::ControlFlow::Continue;
        }
        let (state, pages, status, next, dialog, w, games, asking) = (state.clone(), pages.clone(), status.clone(), next.clone(), dialog.clone(), web.clone(), games.clone(), asking.clone());
        web.evaluate_javascript("document.body ? document.body.innerText : ''", None, None, gio::Cancellable::NONE, move |r| {
            let text = r.map(|v| v.to_str().to_lowercase()).unwrap_or_default();
            let button = crate::free::order_button(&text);
            match state.get() {
                0 if button => {
                    crate::log::line("claim: the checkout's order button is up");
                    state.set(1);
                }
                1 if !button || crate::free::order_done(&text) => {
                    crate::log::line("claim: the order button went: checking the Epic library");
                    state.set(2);
                    status.set_label("Checking your Epic library…");
                    glib::spawn_future_local(async move {
                        let owned = gio::spawn_blocking(|| crate::stores::Store::Epic.refresh_owned().map(|_| crate::stores::epic::owned_items())).await;
                        let all = matches!(&owned, Ok(Ok(items)) if games.iter().all(|g| crate::free::owned(g, items)));
                        if all {
                            crate::log::line("claim: every game is in the library");
                            state.set(3);
                            status.set_label(&format!("Added to your Epic library: {}", games.iter().map(|g| g.title.as_str()).collect::<Vec<_>>().join(", ")));
                            glib::timeout_add_local_once(std::time::Duration::from_secs(3), move || open_next_page(&w, &pages, &status, &next, &dialog));
                        } else {
                            crate::log::line("claim: not in the library yet: watching on");
                            state.set(1);
                            status.set_label(&asking);
                        }
                    });
                }
                _ => {}
            }
        });
        glib::ControlFlow::Continue
    });
}

/// "today", "yesterday", "3 days ago", or the date.
fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
}

fn when(t: u64) -> String {
    let days = store::now().saturating_sub(t) / 86_400;
    match days {
        0 => "Today".into(),
        1 => "Yesterday".into(),
        d if d < 30 => format!("{d} days ago"),
        _ => glib::DateTime::from_unix_local(t as i64).ok().and_then(|d| d.format("%-d %B %Y").ok()).map_or_else(String::new, |s| s.to_string()),
    }
}

/// The cover, or a tinted placeholder with the game's name (and icon, if it has one).
fn cover_widget(g: &Game, width: i32, height: i32) -> gtk::Overlay {
    let overlay = gtk::Overlay::builder().width_request(width).height_request(height).overflow(gtk::Overflow::Hidden).halign(gtk::Align::Center).build();
    overlay.add_css_class("cover");
    let placeholder = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(12).valign(gtk::Align::Fill).build();
    placeholder.add_css_class("cover-placeholder");
    let tint = g.name.bytes().fold(0u32, |h, b| h.wrapping_mul(31).wrapping_add(b as u32)) % 6;
    placeholder.add_css_class(&format!("tint-{tint}"));
    let inner = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(12).valign(gtk::Align::Center).vexpand(true).margin_start(14).margin_end(14).build();
    if let Some(icon) = g.icon() {
        let image = gtk::Image::from_file(icon);
        image.set_pixel_size(64);
        inner.append(&image);
    }
    let label = gtk::Label::builder().label(&g.name).wrap(true).wrap_mode(gtk::pango::WrapMode::WordChar).justify(gtk::Justification::Center).lines(5).ellipsize(gtk::pango::EllipsizeMode::End).max_width_chars(8).build();
    label.add_css_class("cover-name");
    inner.append(&label);
    placeholder.append(&inner);
    overlay.set_child(Some(&placeholder));
    if let Some(cover) = g.cover() {
        add_cover(&overlay, cover, width, height);
    }
    overlay
}

/// The cover over what's under it, once it's read. Both pictures are made at once, so overlays
/// added after (Play) stay on top. A cover wider than the card (itch.io's are 315x250) is shown
/// whole over a blurred copy of itself.
fn add_cover(overlay: &gtk::Overlay, cover: &str, width: i32, height: i32) {
    {
        let backdrop = gtk::Picture::builder().content_fit(gtk::ContentFit::Cover).can_shrink(true).visible(false).build();
        backdrop.add_css_class("cover-backdrop");
        let picture = gtk::Picture::builder().content_fit(gtk::ContentFit::Cover).can_shrink(true).visible(false).build();
        overlay.add_overlay(&backdrop);
        overlay.add_overlay(&picture);
        let ratio = width as f64 / height as f64;
        covers::load(cover.to_string(), move |texture| {
            let Some(texture) = texture else { return };
            let wide = texture.width() as f64 / texture.height() as f64 > ratio * 1.15;
            if wide {
                backdrop.set_paintable(Some(&texture));
                backdrop.set_visible(true);
                picture.set_content_fit(gtk::ContentFit::Contain);
            }
            picture.set_paintable(Some(&texture));
            picture.set_visible(true);
        });
    }
}

/// What a card says under the name: where the game came from, in a word or two.
fn origin(g: &Game) -> String {
    if g.kind == Kind::WrongSystem {
        return g.entries.iter().find_map(|e| e.note.clone()).unwrap_or_else(|| "For another system".into());
    }
    if g.kind == Kind::Download {
        return "Not installed yet".into();
    }
    if g.kind == Kind::Owned {
        let store = if crate::install::steam_owned(g).is_some() { "Steam" } else { crate::install::owned_in(g).map_or("A store", |(s, _)| s.name()) };
        return format!("{store} · not installed");
    }
    // A store's game is labelled with the store's name, taken over or not.
    let store = g.entries.iter().any(|e| matches!(e.source, Source::Steam | Source::SteamElsewhere | Source::Epic | Source::Gog | Source::Amazon | Source::Heroic));
    if g.entries.iter().any(|e| e.source == Source::Lobby) && !store {
        return "Lobby".into();
    }
    if g.unknown_to_launchers() && !store {
        return if g.entries.iter().any(|e| e.source == Source::Itch) { "itch.io · on your drive".into() } else { "On your drive".into() };
    }
    let s = g.sources();
    let label = |src: Source| match src {
        Source::Steam => "Steam",
        Source::SteamElsewhere => "Steam (not added)",
        Source::Epic => "Epic",
        Source::Gog => "GOG",
        Source::Amazon => "Amazon",
        Source::Heroic => "Heroic",
        Source::Lobby => "Lobby",
        Source::Lutris => "Lutris",
        Source::Faugus => "Faugus",
        Source::Itch => "itch.io",
        Source::Artwork => "On your drive",
        Source::SteamShortcut => "Steam shortcut",
        Source::Gamefix | Source::AppMenu => "App menu",
        Source::Prefix | Source::Folder => "On your drive",
    };
    s.first().map(|&src| label(src).to_string()).unwrap_or_default()
}

fn card(ui: &Rc<Ui>, g: &Game) -> gtk::FlowBoxChild {
    let cover = cover_widget(g, 180, 270);
    // Play, or Install for a download.
    let download = matches!(g.kind, Kind::Download | Kind::Owned);
    let (icon, tip) = if download { ("folder-download-symbolic", format!("Install {}", g.name)) } else { ("media-playback-start-symbolic", format!("Play {}", g.name)) };
    let play = gtk::Button::builder().icon_name(icon).tooltip_text(tip).halign(gtk::Align::End).valign(gtk::Align::End).margin_end(10).margin_bottom(10).build();
    play.add_css_class("circular");
    play.add_css_class("suggested-action");
    play.add_css_class("play-button");
    if g.kind == Kind::WrongSystem {
        play.set_visible(false);
    }
    let u = ui.clone();
    let id = g.id.clone();
    play.connect_clicked(move |_| if download { u.install(&id) } else { u.play_or_stop(&id) });
    // Running: a badge, and Stop in place of Play (`Ui::show_running`).
    let running_badge = (!download).then(|| {
        let b = gtk::Label::builder().label("Running").halign(gtk::Align::Start).valign(gtk::Align::End).margin_start(10).margin_bottom(14).visible(false).build();
        b.add_css_class("running-badge");
        cover.add_overlay(&b);
        b.upcast::<gtk::Widget>()
    });
    if !download {
        ui.run_widgets.borrow_mut().push((g.id.clone(), running_badge, play.downgrade(), false));
    }
    cover.add_overlay(&play);
    if ui.is_installing(&g.id) {
        play.set_visible(false);
        let bar = gtk::ProgressBar::builder().show_text(true).text("Installing…").valign(gtk::Align::End).margin_start(10).margin_end(10).margin_bottom(10).build();
        bar.add_css_class("install-progress");
        cover.add_overlay(&bar);
        ui.bars.borrow_mut().push((g.id.clone(), bar));
    }
    if g.favourite {
        let star = gtk::Image::builder().icon_name("starred-symbolic").halign(gtk::Align::End).valign(gtk::Align::Start).margin_end(10).margin_top(10).tooltip_text("A favourite").build();
        star.add_css_class("favourite-badge");
        cover.add_overlay(&star);
    }
    let just_installed = ui.is_just_installed(&g.id);
    if g.update().is_some() && !just_installed {
        let badge = gtk::Label::builder().label("Update").halign(gtk::Align::Start).valign(gtk::Align::Start).margin_start(10).margin_top(10).tooltip_text("A newer version is ready to download: open the game's page").build();
        badge.add_css_class("update-badge");
        cover.add_overlay(&badge);
    }
    if just_installed {
        let badge = gtk::Label::builder().label("Installed").halign(gtk::Align::Start).valign(gtk::Align::Start).margin_start(10).margin_top(10).build();
        badge.add_css_class("installed-badge");
        cover.add_overlay(&badge);
        if let Some((_, b)) = ui.just_installed.borrow_mut().as_mut() {
            *b = Some(badge.upcast());
        }
    }

    // Up to two lines before it's cut ("A Game About Feeding A Black Hole"); the whole name
    // is the card's tooltip.
    let name = gtk::Label::builder().label(&g.name).xalign(0.0).yalign(0.0).wrap(true).wrap_mode(gtk::pango::WrapMode::WordChar).lines(2).ellipsize(gtk::pango::EllipsizeMode::End).max_width_chars(1).hexpand(true).build();
    name.add_css_class("card-name");
    let from = gtk::Label::builder().label(origin(g)).xalign(0.0).ellipsize(gtk::pango::EllipsizeMode::End).max_width_chars(1).hexpand(true).build();
    from.add_css_class("dim-label");
    from.add_css_class("caption");
    // What it's sorted by (when not by name): "3 days ago", "12 h", "4.2 GB".
    let text = ui.facts.borrow().get(&g.id).and_then(|f| f.text(ui.sort.get()));
    let value = gtk::Label::builder().label(text.as_deref().unwrap_or_default()).visible(text.is_some()).xalign(1.0).build();
    value.add_css_class("dim-label");
    value.add_css_class("caption");
    ui.values.borrow_mut().push((g.id.clone(), value.clone()));
    let line = gtk::Box::builder().spacing(6).build();
    line.append(&from);
    line.append(&value);
    // At the top of its cell: a row's cards are as tall as its tallest (a two-line name), and
    // the cover must not grow to fill a shorter one's.
    let body = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(4).width_request(180).halign(gtk::Align::Center).valign(gtk::Align::Start).build();
    body.append(&cover);
    body.append(&name);
    body.append(&line);
    let child = gtk::FlowBoxChild::builder().child(&body).name(&g.id).tooltip_text(&g.name).build();
    child.add_css_class("game-card");
    if g.hidden {
        child.add_css_class("is-hidden");
    }
    // Right-click (or a long press): favourite, hide.
    let menu = gtk::Box::builder().orientation(gtk::Orientation::Vertical).build();
    let popover = gtk::Popover::builder().child(&menu).has_arrow(false).build();
    popover.set_parent(&child);
    for (label, action) in [(if g.favourite { "Remove from favourites" } else { "Add to favourites" }, 0), (if g.hidden { "Show again" } else { "Hide" }, 1)] {
        let b = gtk::Button::builder().label(label).build();
        b.add_css_class("flat");
        b.child().and_downcast::<gtk::Label>().inspect(|l| l.set_xalign(0.0));
        let (u, id, p) = (ui.clone(), g.id.clone(), popover.clone());
        b.connect_clicked(move |_| {
            p.popdown();
            // After the menu has closed: marking rebuilds the card it belongs to.
            let (u, id) = (u.clone(), id.clone());
            glib::idle_add_local_once(move || u.mark(&id, |g| if action == 0 { g.favourite = !g.favourite } else { g.hidden = !g.hidden }));
        });
        menu.append(&b);
    }
    let pop = popover.clone();
    let right = gtk::GestureClick::builder().button(3).build();
    right.connect_pressed(move |_, _, x, y| {
        pop.set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
        pop.popup();
    });
    child.add_controller(right);
    let pop = popover.clone();
    let long = gtk::GestureLongPress::new();
    long.connect_pressed(move |_, x, y| {
        pop.set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
        pop.popup();
    });
    child.add_controller(long);
    // The popover goes with the card.
    child.connect_destroy(move |_| popover.unparent());
    if g.kind == Kind::Owned {
        child.add_css_class("not-installed");
    }
    if just_installed {
        child.add_css_class("just-installed");
    }
    child
}

/// The widget with this name inside `w` (the snapshot's way to press a button).
fn find_named(w: &gtk::Widget, name: &str) -> Option<gtk::Widget> {
    if w.widget_name() == name {
        return Some(w.clone());
    }
    let mut child = w.first_child();
    while let Some(c) = child {
        if let Some(found) = find_named(&c, name) {
            return Some(found);
        }
        child = c.next_sibling();
    }
    None
}

/// The first widget of this type inside `w`.
fn find_type<T: IsA<gtk::Widget>>(w: &gtk::Widget) -> Option<T> {
    if let Some(t) = w.downcast_ref::<T>() {
        return Some(t.clone());
    }
    let mut child = w.first_child();
    while let Some(c) = child {
        if let Some(found) = find_type(&c) {
            return Some(found);
        }
        child = c.next_sibling();
    }
    None
}

/// Where a game can go: each drive's games folder (its Games, else its top), and Lobby's games
/// folder at home; with the free room there and whether it's a Windows-format drive.
fn move_choices(from: &Path) -> Vec<(String, std::path::PathBuf, u64, bool)> {
    let user = std::env::var("USER").unwrap_or_default();
    let mounts = crate::drives::mounts();
    let mut out = Vec::new();
    let windows = |f: &Path| crate::drives::mount_of(f, &mounts).is_some_and(|m| m.windows_format());
    let home = std::path::PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    if let Some(free) = crate::moving::free_space(&home.join("Games")).filter(|_| home.join("Games").is_dir()) {
        out.push(("Games in your home folder".to_string(), home.join("Games"), free, windows(&home.join("Games"))));
    }
    let mut drives: Vec<&crate::drives::Mount> = mounts.iter().filter(|m| m.point.parent() == Some(Path::new(&format!("/run/media/{user}")))).collect();
    drives.sort_by_key(|m| m.point.clone());
    drives.dedup_by_key(|m| m.point.clone());
    for m in drives {
        let name = m.point.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let folder = ["Games", "games"].iter().map(|g| m.point.join(g)).find(|g| g.is_dir()).unwrap_or_else(|| m.point.clone());
        if let Some(free) = crate::moving::free_space(&folder) {
            out.push((format!("The {name} drive"), folder, free, m.windows_format()));
        }
    }
    // gamefix's games folder, when it's somewhere else again.
    let gamefix = gamefix::setup::games_folder();
    if gamefix.is_dir() && !out.iter().any(|(_, f, ..)| crate::library::norm(f) == crate::library::norm(&gamefix)) {
        if let Some(free) = crate::moving::free_space(&gamefix) {
            out.push(("gamefix's games folder".to_string(), gamefix.clone(), free, windows(&gamefix)));
        }
    }
    // Not where it already is.
    out.retain(|(_, f, ..)| from.parent().map(crate::library::norm) != Some(crate::library::norm(f)));
    out
}
