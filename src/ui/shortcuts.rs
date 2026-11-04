//! Windows Shortcuts: the Start Menu shortcuts Windows programs' installers made in Wine
//! prefixes, each with a switch putting it in the app menu. A window of its own (its own
//! app-menu entry, `lobby shortcuts`) and a dialog from Lobby's main menu.

use crate::start_menu::{self, Shortcut};
use adw::prelude::*;
use gtk::{gdk, gio, glib};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

const ABOUT: &str = "Installers of Windows programs make Start Menu shortcuts. Proton keeps them out of this computer's app menu, so programs like Battle.net don't show there. Switch on the ones you want in it: they start through Lobby, protected like its games.";

/// The picker: a toolbar view (header bar, toasts, the list once it's read).
pub fn content() -> adw::ToolbarView {
    let view = adw::ToolbarView::new();
    view.add_top_bar(&adw::HeaderBar::new());
    let toasts = adw::ToastOverlay::new();
    let spinner = gtk::Spinner::builder().spinning(true).width_request(32).height_request(32).halign(gtk::Align::Center).valign(gtk::Align::Center).build();
    toasts.set_child(Some(&spinner));
    view.set_content(Some(&toasts));
    glib::spawn_future_local(async move {
        let found = gio::spawn_blocking(|| {
            let lib = crate::store::Library::load().unwrap_or_default();
            start_menu::all(&lib).into_iter().map(|s| {
                let icon = (!s.gone()).then(|| gamefix::peicon::icon_png(&s.program)).flatten();
                (s, icon)
            }).collect::<Vec<_>>()
        })
        .await
        .unwrap_or_default();
        toasts.set_child(Some(&page(found, &toasts)));
    });
    view
}

fn page(found: Vec<(Shortcut, Option<Vec<u8>>)>, toasts: &adw::ToastOverlay) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::new();
    page.add(&adw::PreferencesGroup::builder().description(ABOUT).build());
    if found.is_empty() {
        page.add(&adw::PreferencesGroup::builder().title("No shortcuts found").description("No Wine prefix Lobby knows has a Start Menu shortcut to a program.").build());
        return page;
    }
    // A group for each prefix, by whose it is; the ones whose program is gone last.
    let mut places: BTreeMap<(bool, String), Vec<(Shortcut, Option<Vec<u8>>)>> = BTreeMap::new();
    for (s, icon) in found {
        let key = if s.gone() { (true, "No longer on this computer".to_string()) } else { (false, s.place.clone()) };
        places.entry(key).or_default().push((s, icon));
    }
    for ((_, title), list) in places {
        let group = adw::PreferencesGroup::builder().title(glib::markup_escape_text(&title)).build();
        for (s, icon) in list {
            group.add(&row(s, icon, toasts));
        }
        page.add(&group);
    }
    page
}

fn row(s: Shortcut, icon: Option<Vec<u8>>, toasts: &adw::ToastOverlay) -> adw::SwitchRow {
    let program = s.program.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let subtitle = if s.gone() { format!("{program}, in {}'s Wine prefix", s.place) } else { program };
    let row = adw::SwitchRow::builder().title(glib::markup_escape_text(&s.name)).subtitle(glib::markup_escape_text(&subtitle)).active(s.entry.is_some()).build();
    let picture = icon.and_then(|png| gdk::Texture::from_bytes(&glib::Bytes::from_owned(png)).ok());
    let image = match &picture {
        Some(t) => gtk::Image::from_paintable(Some(t)),
        None => gtk::Image::from_icon_name("application-x-executable"),
    };
    image.set_pixel_size(32);
    row.add_prefix(&image);
    let state = Rc::new(RefCell::new(s));
    // Set back after a failure: that change isn't the player's.
    let undoing = Rc::new(std::cell::Cell::new(false));
    let toasts = toasts.clone();
    row.connect_active_notify(move |row| {
        if undoing.replace(false) {
            return;
        }
        let mut s = state.borrow_mut();
        let done = match (row.is_active(), s.entry.clone()) {
            (true, None) => start_menu::add(&s).map(|file| {
                s.entry = Some(file);
                format!("{} is in the app menu", s.name)
            }),
            (false, Some(entry)) => start_menu::remove(&entry).map(|()| {
                s.entry = None;
                format!("{} is out of the app menu", s.name)
            }),
            _ => return,
        };
        let text = done.unwrap_or_else(|e| {
            undoing.set(true);
            row.set_active(!row.is_active());
            e
        });
        toasts.add_toast(adw::Toast::builder().title(glib::markup_escape_text(&text)).timeout(3).build());
    });
    row
}

/// `lobby shortcuts`: the picker in a window of its own; opened again, that window comes forward.
pub fn window() -> glib::ExitCode {
    crate::appimage::enter();
    let flags = if std::env::var_os("LOBBY_SNAPSHOT").is_some() { gio::ApplicationFlags::NON_UNIQUE } else { gio::ApplicationFlags::empty() };
    let app = adw::Application::builder().application_id("io.github.Petru101.Lobby.Shortcuts").flags(flags).build();
    app.connect_startup(|_| crate::appimage::host_env());
    app.connect_activate(|app| {
        if let Some(window) = app.active_window() {
            return window.present();
        }
        let window = adw::ApplicationWindow::builder().application(app).title("Windows Shortcuts").default_width(600).default_height(720).content(&content()).build();
        window.present();
        if let Some(file) = std::env::var_os("LOBBY_SNAPSHOT").map(std::path::PathBuf::from) {
            let w = window.clone();
            // Time to read the prefixes and icons.
            glib::timeout_add_local_once(std::time::Duration::from_secs(8), move || {
                let paintable = gtk::WidgetPaintable::new(Some(&w));
                let snapshot = gtk::Snapshot::new();
                paintable.snapshot(&snapshot, w.width() as f64, w.height() as f64);
                let saved = snapshot.to_node().zip(w.renderer()).map(|(node, r)| r.render_texture(&node, None).save_to_png(&file));
                eprintln!("snapshot: {saved:?}");
                w.close();
            });
        }
    });
    app.set_accels_for_action("window.close", &["<Control>w", "<Control>q"]);
    app.run_with_args::<&str>(&[])
}
