//! App Menu Shortcuts: what can be in the app menu, each with a switch: games with an entry of
//! their own there or a Windows shortcut, and the Start Menu shortcuts Windows programs'
//! installers made in Wine prefixes. A window of its own (its own app-menu entry, `lobby
//! shortcuts`) and a dialog from Lobby's main menu.

use crate::start_menu::{self, Item};
use adw::prelude::*;
use gtk::{gdk, gio, glib};
use std::cell::RefCell;
use std::rc::Rc;

const ABOUT: &str = "Games and programs that can be in this computer's app menu. Installers of Windows programs make Start Menu shortcuts, but Proton keeps them out of the app menu, so programs like Battle.net don't show there on their own. Switch on what you want in it: everything here starts through Lobby, protected like its games.";

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
            let items = crate::log::timed("shortcuts: list", || start_menu::items(&lib));
            crate::log::timed("shortcuts: icons", || items.into_iter().map(|i| {
                let icon = i.icon.as_deref().and_then(picture);
                (i, icon)
            }).collect::<Vec<_>>())
        })
        .await
        .unwrap_or_default();
        toasts.set_child(Some(&page(found, &toasts)));
    });
    view
}

/// A picture file's bytes, or a program's own icon (.exe).
fn picture(path: &std::path::Path) -> Option<Vec<u8>> {
    match path.extension().map(|e| e.to_string_lossy().to_lowercase()).as_deref() {
        Some("exe") => gamefix::peicon::icon_png(path),
        Some("png" | "jpg" | "jpeg" | "svg" | "ico" | "webp") => std::fs::read(path).ok(),
        _ => None,
    }
}

fn page(found: Vec<(Item, Option<Vec<u8>>)>, toasts: &adw::ToastOverlay) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::new();
    page.add(&adw::PreferencesGroup::builder().description(ABOUT).build());
    if found.is_empty() {
        page.add(&adw::PreferencesGroup::builder().title("Nothing found").description("No game has an app-menu entry, and no Wine prefix Lobby knows has a Start Menu shortcut to a program.").build());
        return page;
    }
    let groups = [
        (start_menu::GAMES, "Games", "Switching one off hides its entry; Play in Lobby still starts the game."),
        ("", "Other Windows programs", ""),
        (start_menu::GONE, start_menu::GONE, "Uninstalled, or on a drive that isn't open."),
    ];
    let mut found = found;
    for (key, title, about) in groups {
        let (here, rest): (Vec<_>, Vec<_>) = found.into_iter().partition(|(i, _)| i.group == key || (key.is_empty() && i.group != start_menu::GAMES && i.group != start_menu::GONE));
        found = rest;
        if here.is_empty() {
            continue;
        }
        let group = adw::PreferencesGroup::builder().title(title).description(about).build();
        for (item, icon) in here {
            group.add(&row(item, icon, toasts));
        }
        page.add(&group);
    }
    page
}

fn row(item: Item, icon: Option<Vec<u8>>, toasts: &adw::ToastOverlay) -> adw::SwitchRow {
    // A Windows program's: whose prefix it's in, and its file.
    let subtitle = match &item.shortcut {
        Some(s) if item.game.is_none() => format!("{} · {}", s.place, item.detail),
        _ => item.detail.clone(),
    };
    let row = adw::SwitchRow::builder().title(glib::markup_escape_text(&item.name)).subtitle(glib::markup_escape_text(&subtitle)).active(item.shown()).build();
    let picture = icon.and_then(|png| gdk::Texture::from_bytes(&glib::Bytes::from_owned(png)).ok());
    let image = match &picture {
        Some(t) => gtk::Image::from_paintable(Some(t)),
        None => gtk::Image::from_icon_name("application-x-executable"),
    };
    image.set_pixel_size(32);
    row.add_prefix(&image);
    let state = Rc::new(RefCell::new(item));
    // Set back after a failure: that change isn't the player's.
    let undoing = Rc::new(std::cell::Cell::new(false));
    let toasts = toasts.clone();
    row.connect_active_notify(move |row| {
        if undoing.replace(false) {
            return;
        }
        let mut item = state.borrow_mut();
        if row.is_active() == item.shown() {
            return;
        }
        let done = match row.is_active() {
            true => item.show().map(|()| format!("{} is in the app menu", item.name)),
            false => item.hide().map(|()| format!("{} is out of the app menu", item.name)),
        };
        start_menu::refresh();
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
        let window = adw::ApplicationWindow::builder().application(app).title("App Menu Shortcuts").default_width(600).default_height(720).content(&content()).build();
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
