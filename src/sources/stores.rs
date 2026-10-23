//! The stores the user signed in to in lobby: games lobby installed from them, and the rest of
//! what the accounts own (Kind::Owned), all keyed by store and id, so an owned game merges with
//! its install whoever made it.

use crate::library::{Entry, Key, Kind};
use crate::stores::{self, Store};

pub fn games() -> Vec<Entry> {
    let mut entries = Vec::new();
    for store in Store::ALL {
        let owned = stores::owned(store);
        let installed = store.installed();
        for g in &installed {
            let mut e = Entry::new(store.source(), g.title.clone());
            e.folder = Some(g.folder.clone());
            e.program = g.program.clone();
            e.cover = owned.iter().find(|o| o.id == g.id).and_then(|o| o.cover.clone());
            e.keys.push(Key::Store(store.id().into(), g.id.clone()));
            entries.push(e);
        }
        if store.account().is_none() {
            continue;
        }
        for o in owned.into_iter().filter(|o| !installed.iter().any(|g| g.id == o.id)) {
            let mut e = Entry::new(store.source(), o.title);
            e.kind = Kind::Owned;
            e.cover = o.cover;
            e.keys.push(Key::Store(store.id().into(), o.id));
            entries.push(e);
        }
    }
    entries
}
