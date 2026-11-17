// SPDX-License-Identifier: GPL-3.0-or-later
//! The icon drawn for a file without a thumbnail: its default app's icon.
//!
//! A folder keeps the mockup folder SVG. Every other file draws the icon of
//! the application that opens its content type by default, or the generic
//! placeholder when there is no such app, the app has no icon, or there is no
//! content type. The lookup is the same one Open uses, so the tile and a
//! double-click agree. Answers (including "no app") are cached per content
//! type on the GTK main thread, because `gio::Icon` is not `Send`. A change to
//! the default apps clears the cache and wakes every registered view.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::gio;
use gtk::prelude::*;

/// What one file's type icon should be.
#[derive(Clone, Debug)]
pub enum FileIcon {
    /// The mockup folder SVG.
    Folder,
    /// The default app's icon.
    App(gio::Icon),
    /// `text-x-generic`.
    Placeholder,
}

/// The decision, with the app lookup injected so it runs without GIO's real
/// app database. A folder never consults the lookup.
pub fn decide<F>(is_folder: bool, content_type: Option<&str>, lookup: F) -> FileIcon
where
    F: FnOnce(&str) -> Option<gio::Icon>,
{
    if is_folder {
        return FileIcon::Folder;
    }
    let Some(content_type) = content_type.filter(|content_type| !content_type.is_empty()) else {
        return FileIcon::Placeholder;
    };
    match lookup(content_type) {
        Some(icon) => FileIcon::App(icon),
        None => FileIcon::Placeholder,
    }
}

thread_local! {
    /// One answer per content type, including the "no default app" answers.
    /// `gio::Icon` is not `Send`, so this lives on the GTK main thread.
    static CACHE: RefCell<Vec<(String, Option<gio::Icon>)>> = const { RefCell::new(Vec::new()) };
    /// The process-wide monitor, kept alive by this slot. Dropping the only
    /// reference would disconnect its `changed` handler.
    static MONITOR: RefCell<Option<gio::AppInfoMonitor>> = const { RefCell::new(None) };
    /// Views that redraw their visible file icons after a default-app change.
    static LISTENERS: RefCell<Vec<Listener>> = const { RefCell::new(Vec::new()) };
    static NEXT_LISTENER: Cell<u64> = const { Cell::new(1) };
}

type Listener = (u64, Rc<dyn Fn()>);

/// Draw a folder as a folder, otherwise use the app that opens `content_type`.
pub fn for_content_type(content_type: Option<&str>) -> FileIcon {
    ensure_monitor();
    decide(false, content_type, |content_type| {
        cached(content_type, || lookup_default_app(content_type))
    })
}

/// The icon for a listing's `gio::FileInfo`, which already carries
/// `standard::content-type`. No query and no file read happen here.
pub fn for_file_info(info: &gio::FileInfo) -> FileIcon {
    if info.file_type() == gio::FileType::Directory {
        return FileIcon::Folder;
    }
    for_content_type(info.content_type().as_deref())
}

/// The icon for a name alone, as the Trash view has: guess the type from the
/// file name with no read and no content sniffing.
pub fn for_name(name: &str) -> FileIcon {
    let (content_type, _uncertain) = gio::content_type_guess(Some(name), None);
    for_content_type(Some(content_type.as_str()))
}

/// Register a callback for default-app changes. Keep the id and pass it to
/// [`disconnect_changed`] when the owner goes away.
pub fn connect_changed(callback: impl Fn() + 'static) -> u64 {
    let id = NEXT_LISTENER.with(|next| {
        let id = next.get();
        next.set(id.wrapping_add(1));
        id
    });
    LISTENERS.with(|listeners| listeners.borrow_mut().push((id, Rc::new(callback))));
    id
}

pub fn disconnect_changed(id: u64) {
    LISTENERS.with(|listeners| {
        listeners
            .borrow_mut()
            .retain(|(candidate, _)| *candidate != id);
    });
}

fn cached<F>(content_type: &str, lookup: F) -> Option<gio::Icon>
where
    F: FnOnce() -> Option<gio::Icon>,
{
    let hit = CACHE.with(|cache| {
        cache
            .borrow()
            .iter()
            .find(|(candidate, _)| candidate == content_type)
            .map(|(_, icon)| icon.clone())
    });
    if let Some(icon) = hit {
        return icon;
    }
    let icon = lookup();
    CACHE.with(|cache| {
        cache
            .borrow_mut()
            .push((content_type.to_string(), icon.clone()));
    });
    icon
}

fn lookup_default_app(content_type: &str) -> Option<gio::Icon> {
    let app = gio::AppInfo::default_for_type(content_type, false);
    let icon = app.as_ref().and_then(|app| app.icon());
    match (&app, icon.as_ref()) {
        (Some(app), Some(_)) => {
            let name = app
                .id()
                .map(|id| id.trim_end_matches(".desktop").to_string())
                .unwrap_or_else(|| app.name().to_string());
            crate::log_verbose!("icons", "{content_type} -> {name}");
        }
        _ => crate::log_verbose!("icons", "{content_type} -> placeholder"),
    }
    icon
}

fn ensure_monitor() {
    if MONITOR.with(|monitor| monitor.borrow().is_some()) {
        return;
    }
    let monitor = gio::AppInfoMonitor::get();
    monitor.connect_changed(|_| {
        clear_cache();
        notify_changed();
    });
    MONITOR.with(|slot| *slot.borrow_mut() = Some(monitor));
}

fn clear_cache() {
    CACHE.with(|cache| cache.borrow_mut().clear());
    crate::log_verbose!("icons", "default-app icon cache cleared");
}

fn notify_changed() {
    let listeners: Vec<Rc<dyn Fn()>> = LISTENERS.with(|listeners| {
        listeners
            .borrow()
            .iter()
            .map(|(_, callback)| callback.clone())
            .collect()
    });
    for callback in listeners {
        callback();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn themed_icon() -> gio::Icon {
        gio::ThemedIcon::new("text-x-generic").upcast()
    }

    #[test]
    fn a_folder_is_folder_without_asking_for_a_type() {
        let icon = decide(true, Some("image/png"), |_| {
            panic!("a folder must not consult the app lookup")
        });
        assert!(matches!(icon, FileIcon::Folder));
    }

    #[test]
    fn an_app_icon_becomes_app() {
        let icon = decide(false, Some("text/x-python"), |_| Some(themed_icon()));
        assert!(matches!(icon, FileIcon::App(_)));
    }

    #[test]
    fn no_app_becomes_placeholder() {
        let icon = decide(false, Some("application/x-unknown"), |_| None);
        assert!(matches!(icon, FileIcon::Placeholder));
    }

    #[test]
    fn no_content_type_becomes_placeholder_without_a_lookup() {
        let icon = decide(false, None, |_| {
            panic!("no type must not consult the lookup")
        });
        assert!(matches!(icon, FileIcon::Placeholder));
        let icon = decide(false, Some(""), |_| {
            panic!("an empty type must not consult the lookup")
        });
        assert!(matches!(icon, FileIcon::Placeholder));
    }

    #[test]
    fn the_cache_runs_the_lookup_once_and_caches_none() {
        const TYPE: &str = "application/x-gimbal-cache-test";
        clear_cache();
        let calls = Cell::new(0);
        assert!(
            cached(TYPE, || {
                calls.set(calls.get() + 1);
                None
            })
            .is_none()
        );
        assert!(
            cached(TYPE, || {
                calls.set(calls.get() + 1);
                None
            })
            .is_none()
        );
        assert_eq!(calls.get(), 1);

        clear_cache();
        assert!(
            cached(TYPE, || {
                calls.set(calls.get() + 1);
                None
            })
            .is_none()
        );
        assert_eq!(calls.get(), 2);
    }

    #[test]
    fn the_cache_returns_an_icon_without_a_second_lookup() {
        const TYPE: &str = "application/x-gimbal-cache-icon-test";
        clear_cache();
        let calls = Cell::new(0);
        assert!(
            cached(TYPE, || {
                calls.set(calls.get() + 1);
                Some(themed_icon())
            })
            .is_some()
        );
        assert!(
            cached(TYPE, || {
                calls.set(calls.get() + 1);
                Some(themed_icon())
            })
            .is_some()
        );
        assert_eq!(calls.get(), 1);
        clear_cache();
    }
}
