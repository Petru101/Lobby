//! lobby's window: every game as a card with its cover, a search box and a few filters; a
//! click shows where the game is and how it starts, Play starts it.

mod covers;

use crate::launch::{self, Plan};
use crate::library::{Entry, Game, Key, Kind, Source};
use crate::store::{self, Library};
use adw::prelude::*;
use gtk::{gdk, gio, glib};
use std::cell::{Cell, RefCell};
use std::path::Path;
use std::rc::Rc;

const APP_ID: &str = "io.github.Petru101.Lobby";

pub fn run() -> glib::ExitCode {
    let app = adw::Application::builder().application_id(APP_ID).build();
    app.connect_startup(|_| {
        let css = gtk::CssProvider::new();
        css.load_from_string(include_str!("style.css"));
        gtk::style_context_add_provider_for_display(&gdk::Display::default().expect("a display"), &css, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
    });
    app.connect_activate(build);
    app.set_accels_for_action("win.search", &["<Control>f"]);
    app.set_accels_for_action("win.rescan", &["<Control>r", "F5"]);
    app.set_accels_for_action("window.close", &["<Control>w", "<Control>q"]);
    app.run_with_args::<&str>(&[])
}

/// Which games the grid shows.
#[derive(Clone, Copy, PartialEq)]
enum Filter {
    All,
    Steam,
    Stores,
    Launchers,
    Drives,
    Downloads,
}

impl Filter {
    const ALL: [(Filter, &'static str, &'static str); 6] = [
        (Filter::All, "All", "Every game on this computer"),
        (Filter::Steam, "Steam", "Installed with Steam"),
        (Filter::Stores, "Epic · GOG · Amazon", "Installed through Heroic"),
        (Filter::Launchers, "Other launchers", "Lutris, Faugus, Steam shortcuts, the app menu and gamefix"),
        (Filter::Drives, "Only on your drives", "Games no launcher knows about: found in folders and Windows setups"),
        (Filter::Downloads, "Downloads", "Downloaded but not installed yet"),
    ];

    fn wants(self, g: &Game) -> bool {
        let has = |s: &[Source]| g.entries.iter().any(|e| s.contains(&e.source));
        if self == Filter::Downloads {
            return g.kind == Kind::Download;
        }
        g.kind == Kind::Game
            && match self {
                Filter::All => true,
                Filter::Steam => has(&[Source::Steam]),
                Filter::Stores => has(&[Source::Epic, Source::Gog, Source::Amazon, Source::Heroic]),
                Filter::Launchers => !has(&[Source::Steam, Source::Epic, Source::Gog, Source::Amazon, Source::Heroic]) && !g.unknown_to_launchers(),
                Filter::Drives => g.unknown_to_launchers(),
                Filter::Downloads => unreachable!(),
            }
    }
}

struct Ui {
    window: adw::ApplicationWindow,
    title: adw::WindowTitle,
    toasts: adw::ToastOverlay,
    stack: gtk::Stack,
    flow: gtk::FlowBox,
    search: gtk::SearchEntry,
    rescan_button: gtk::Button,
    lib: RefCell<Library>,
    filter: Cell<Filter>,
    scanning: Cell<bool>,
}

fn build(app: &adw::Application) {
    let window = adw::ApplicationWindow::builder().application(app).title("Lobby").default_width(1280).default_height(860).build();
    let title = adw::WindowTitle::new("Lobby", "");
    let header = adw::HeaderBar::builder().title_widget(&title).build();
    let rescan_button = gtk::Button::builder().icon_name("view-refresh-symbolic").tooltip_text("Look for games again (F5)").build();
    header.pack_start(&rescan_button);
    let search = gtk::SearchEntry::builder().placeholder_text("Search games").width_chars(28).build();
    header.pack_end(&search);

    // Filters, as a row of toggle buttons.
    let filters = gtk::Box::builder().orientation(gtk::Orientation::Horizontal).spacing(6).halign(gtk::Align::Center).margin_top(6).margin_bottom(6).build();
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
    let scrolled = gtk::ScrolledWindow::builder().hscrollbar_policy(gtk::PolicyType::Never).child(&flow).vexpand(true).build();

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
    toolbar.set_content(Some(&stack));
    let toasts = adw::ToastOverlay::new();
    toasts.set_child(Some(&toolbar));
    window.set_content(Some(&toasts));

    let ui = Rc::new(Ui { window: window.clone(), title, toasts, stack, flow: flow.clone(), search: search.clone(), rescan_button: rescan_button.clone(), lib: RefCell::new(Library::load().unwrap_or_default()), filter: Cell::new(Filter::All), scanning: Cell::new(false) });

    let mut group: Option<gtk::ToggleButton> = None;
    for (f, label, tip) in Filter::ALL {
        let button = gtk::ToggleButton::builder().label(label).tooltip_text(tip).build();
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
                u.filter.set(f);
                u.refilter();
            }
        });
        filters.append(&button);
    }

    let u = ui.clone();
    flow.set_filter_func(move |child| u.lib.borrow().games.iter().find(|g| g.id == child.widget_name()).is_some_and(|g| u.shows(g)));
    let u = ui.clone();
    flow.set_sort_func(move |a, b| {
        let lib = u.lib.borrow();
        let name = |c: &gtk::FlowBoxChild| lib.games.iter().find(|g| g.id == c.widget_name()).map(|g| g.name.to_lowercase()).unwrap_or_default();
        name(a).cmp(&name(b)).into()
    });
    let u = ui.clone();
    flow.connect_child_activated(move |_, child| u.details(&child.widget_name()));
    let u = ui.clone();
    search.connect_search_changed(move |_| u.refilter());
    search.set_key_capture_widget(Some(&window));
    let u = ui.clone();
    rescan_button.connect_clicked(move |_| u.rescan());

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
}

/// For checking how the window looks without a screenshot of the desktop: with
/// LOBBY_SNAPSHOT=<file.png> the window renders itself into that file once the scan is done
/// and covers have had time to load (LOBBY_SNAPSHOT_DETAILS=<game id> opens that game's page
/// first, LOBBY_SNAPSHOT_SEARCH=<text> searches), then quits.
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
        if let Ok(id) = std::env::var("LOBBY_SNAPSHOT_DETAILS") {
            u.details(&id);
        }
        glib::timeout_future(std::time::Duration::from_secs(5)).await;
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
        if g.gone || g.missing || !self.filter.get().wants(g) {
            return false;
        }
        let q = self.search.text().to_lowercase();
        q.split_whitespace().all(|w| g.name.to_lowercase().contains(w))
    }

    fn refilter(&self) {
        self.flow.invalidate_filter();
        let lib = self.lib.borrow();
        let shown = lib.games.iter().filter(|g| self.shows(g)).count();
        let total = lib.games.iter().filter(|g| g.kind == Kind::Game && !g.gone && !g.missing).count();
        self.title.set_subtitle(&if shown == total { format!("{total} games") } else { format!("{shown} of {total} games") });
        if !self.scanning.get() || !lib.games.is_empty() {
            self.stack.set_visible_child_name(if shown == 0 { "empty" } else { "games" });
        }
    }

    /// One card per game, rebuilt from the library.
    fn fill(self: &Rc<Self>) {
        self.flow.remove_all();
        for g in self.lib.borrow().games.iter().filter(|g| matches!(g.kind, Kind::Game | Kind::Download)) {
            self.flow.append(&card(self, g));
        }
        self.refilter();
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
            return;
        }
        self.rescan_button.set_sensitive(false);
        let u = self.clone();
        glib::spawn_future_local(async move {
            let found = gio::spawn_blocking(crate::find_games).await;
            u.scanning.set(false);
            u.rescan_button.set_sensitive(true);
            match found {
                Ok(Ok(games)) => {
                    let mut lib = u.lib.borrow_mut();
                    lib.update(games);
                    if let Err(e) = lib.save() {
                        u.toast(&format!("Couldn't save the library: {e}"));
                    }
                    drop(lib);
                    u.fill();
                }
                Ok(Err(e)) => u.alert("Couldn't look for games", &e),
                Err(_) => u.alert("Couldn't look for games", "The scan stopped with an error."),
            }
        });
    }

    fn game(&self, id: &str) -> Option<Game> {
        self.lib.borrow().games.iter().find(|g| g.id == id).cloned()
    }

    fn played(&self, id: &str, entry: Option<Entry>) {
        let mut lib = self.lib.borrow_mut();
        if let Some(g) = lib.get_mut(id) {
            g.played = Some(store::now());
            g.entries.extend(entry);
        }
        let _ = lib.save();
    }

    fn play(self: &Rc<Self>, id: &str) {
        let Some(game) = self.game(id) else { return };
        match launch::plan(&game) {
            Plan::Run { command, cwd, .. } => match launch::start(&command, cwd.as_deref(), &store::log_file(&game.id)) {
                Ok(()) => {
                    self.toast(&format!("Starting {}…", game.name));
                    self.played(&game.id, None);
                }
                Err(e) => self.alert(&format!("{} didn't start", game.name), &e),
            },
            Plan::Cannot(why) => self.alert(&format!("{} can't be started", game.name), &why),
            Plan::SetUp(folder) => {
                self.toast(&format!("Setting up {} with gamefix (only the first time)…", game.name));
                let u = self.clone();
                glib::spawn_future_local(async move {
                    let result = gio::spawn_blocking(move || launch::set_up(&folder)).await.unwrap_or_else(|_| Err("gamefix stopped with an error.".into()));
                    match result {
                        Ok((lines, Some(entry))) => {
                            let started = std::fs::read_to_string(&entry).ok().and_then(|t| launch::desktop_exec(&t)).ok_or("gamefix's entry has no command".to_string()).and_then(|(command, cwd)| launch::start(&command, cwd.as_deref(), &store::log_file(&game.id)));
                            let mut e = Entry::new(Source::AppMenu, game.name.clone());
                            e.keys.push(Key::Desktop(entry));
                            u.played(&game.id, Some(e));
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

    /// A game's page: its cover, Play, where it was found and how it starts.
    fn details(self: &Rc<Self>, id: &str) {
        let Some(game) = self.game(id) else { return };
        let plan = launch::plan(&game);

        let cover = cover_widget(&game, 200, 300);
        cover.set_valign(gtk::Align::Start);
        let name = gtk::Label::builder().label(&game.name).wrap(true).xalign(0.0).build();
        name.add_css_class("title-1");
        let found = gtk::Label::builder().label(format!("Found in: {}", game.sources().iter().map(|s| s.label()).collect::<Vec<_>>().join(", "))).wrap(true).xalign(0.0).build();
        found.add_css_class("dim-label");

        let play = gtk::Button::builder().label("Play").halign(gtk::Align::Start).build();
        play.add_css_class("pill");
        play.add_css_class("suggested-action");
        play.set_sensitive(!matches!(plan, Plan::Cannot(_)));
        let buttons = gtk::Box::builder().orientation(gtk::Orientation::Horizontal).spacing(12).margin_top(12).build();
        buttons.append(&play);
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

        let info = adw::PreferencesGroup::new();
        let row = |title: &str, value: &str| {
            let r = adw::ActionRow::builder().title(title).subtitle(value).subtitle_selectable(true).build();
            r.add_css_class("property");
            info.add(&r);
        };
        row("How it starts", &plan.describe());
        if let Some(p) = game.program() {
            row("Program", &p.display().to_string());
        }
        if let Some(f) = &folder {
            row("Folder", &f.display().to_string());
        }
        if let Some(p) = game.entries.iter().find_map(|e| e.prefix.as_ref()) {
            row("Windows setup (Wine prefix)", &p.display().to_string());
        }
        if let Some(t) = game.played {
            row("Last played", &when(t));
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
        content.append(&info);

        let view = adw::ToolbarView::new();
        view.add_top_bar(&adw::HeaderBar::new());
        view.set_content(Some(&gtk::ScrolledWindow::builder().child(&content).propagate_natural_height(true).hscrollbar_policy(gtk::PolicyType::Never).build()));
        let dialog = adw::Dialog::builder().title(&game.name).content_width(720).child(&view).build();
        let u = self.clone();
        let d = dialog.clone();
        let id = game.id.clone();
        play.connect_clicked(move |_| {
            d.close();
            u.play(&id);
        });
        dialog.present(Some(&self.window));
    }
}

/// "today", "yesterday", "3 days ago", or the date.
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
        let picture = gtk::Picture::builder().content_fit(gtk::ContentFit::Cover).can_shrink(true).build();
        overlay.add_overlay(&picture);
        let p = picture.clone();
        covers::load(&picture, cover.to_string(), move || p.set_visible(false));
    }
    overlay
}

/// What a card says under the name: where the game came from, in a word or two.
fn origin(g: &Game) -> String {
    if g.kind == Kind::Download {
        return "Not installed yet".into();
    }
    if g.unknown_to_launchers() {
        return "On your drive".into();
    }
    let s = g.sources();
    let label = |src: Source| match src {
        Source::Steam => "Steam",
        Source::Epic => "Epic",
        Source::Gog => "GOG",
        Source::Amazon => "Amazon",
        Source::Heroic => "Heroic",
        Source::Lutris => "Lutris",
        Source::Faugus => "Faugus",
        Source::SteamShortcut => "Steam shortcut",
        Source::Gamefix | Source::AppMenu => "App menu",
        Source::Prefix | Source::Folder => "On your drive",
    };
    s.first().map(|&src| label(src).to_string()).unwrap_or_default()
}

fn card(ui: &Rc<Ui>, g: &Game) -> gtk::FlowBoxChild {
    let cover = cover_widget(g, 180, 270);
    let play = gtk::Button::builder().icon_name("media-playback-start-symbolic").tooltip_text(format!("Play {}", g.name)).halign(gtk::Align::End).valign(gtk::Align::End).margin_end(10).margin_bottom(10).build();
    play.add_css_class("circular");
    play.add_css_class("suggested-action");
    play.add_css_class("play-button");
    if g.kind == Kind::Download {
        play.set_visible(false);
    }
    let u = ui.clone();
    let id = g.id.clone();
    play.connect_clicked(move |_| u.play(&id));
    cover.add_overlay(&play);

    let name = gtk::Label::builder().label(&g.name).xalign(0.0).ellipsize(gtk::pango::EllipsizeMode::End).max_width_chars(1).hexpand(true).build();
    name.add_css_class("card-name");
    let from = gtk::Label::builder().label(origin(g)).xalign(0.0).ellipsize(gtk::pango::EllipsizeMode::End).max_width_chars(1).build();
    from.add_css_class("dim-label");
    from.add_css_class("caption");
    let body = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(4).width_request(180).halign(gtk::Align::Center).build();
    body.append(&cover);
    body.append(&name);
    body.append(&from);
    let child = gtk::FlowBoxChild::builder().child(&body).name(&g.id).tooltip_text(&g.name).build();
    child.add_css_class("game-card");
    child
}
