// SPDX-License-Identifier: GPL-3.0-or-later
// P1f: GTK loads ~/.config/gtk-4.0/gtk.css once at startup (gtksettings.c:1147-1161)
// and never reloads it, so Noctalia rewrites of noctalia.css are invisible until
// restart. We watch the file and reload a USER-priority provider ourselves.
use adw::prelude::*;
use gtk::gio;
use gtk::glib;
use std::cell::Cell;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

pub fn init(app: &adw::Application) {
    let _ = app;
    let Some(display) = gtk::gdk::Display::default() else {
        crate::log!("theme", "no default display, not watching");
        return;
    };
    let path = match std::env::var("GIMBAL_SPIKE_THEME_CSS") {
        Ok(path) => PathBuf::from(path),
        Err(_) => glib::home_dir().join(".config/gtk-4.0/noctalia.css"),
    };
    let file = gio::File::for_path(&path);
    let provider = gtk::CssProvider::new();
    provider.connect_parsing_error(|_, section, error| {
        crate::log!("theme", "parse error at {}: {error}", section.to_string());
    });
    gtk::style_context_add_provider_for_display(
        &display,
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_USER,
    );
    reload(&provider, &file);

    let pending: Rc<Cell<Option<glib::SourceId>>> = Rc::new(Cell::new(None));
    // A dropped FileMonitor cancels itself, so the monitors must outlive init.
    let monitors: &mut Vec<gio::FileMonitor> = Box::leak(Box::default());
    for watch_path in watched(&path) {
        let watch = gio::File::for_path(&watch_path);
        let Ok(monitor) = watch.monitor_file(
            gio::FileMonitorFlags::NONE,
            Option::<&gio::Cancellable>::None,
        ) else {
            crate::log!("theme", "cannot watch {}", watch_path.display());
            continue;
        };
        let provider = provider.clone();
        // Always reload the main file. Loading an imported file on its own would
        // replace the provider with the colors that file happens to define.
        let target = file.clone();
        let watched_uri = watch.uri();
        let pending = pending.clone();
        monitor.connect_changed(move |_, _, _, event| {
            crate::log!("theme", "change {:?} on {}", event, watched_uri);
            if let Some(id) = pending.take() {
                id.remove();
            }
            let provider = provider.clone();
            let target = target.clone();
            let pending = pending.clone();
            let inner = pending.clone();
            pending.set(Some(glib::timeout_add_local_once(
                Duration::from_millis(200),
                move || {
                    inner.set(None);
                    reload(&provider, &target);
                },
            )));
        });
        crate::log!("theme", "watching {}", watch_path.display());
        monitors.push(monitor);
    }
}

fn reload(provider: &gtk::CssProvider, file: &gio::File) {
    provider.load_from_file(file);
    crate::log!("theme", "reloaded {}", file.uri());
}

// The file itself plus what it @imports (one level; Noctalia imports
// noctalia-extensions.css, which imports nothing).
fn watched(path: &PathBuf) -> Vec<PathBuf> {
    let mut paths = vec![path.clone()];
    let Ok(text) = std::fs::read_to_string(path) else {
        return paths;
    };
    let dir = path.parent().map(PathBuf::from).unwrap_or_default();
    for line in text.lines() {
        let Some(rest) = line.trim().strip_prefix("@import url(") else {
            continue;
        };
        if let Some(name) = rest
            .trim_end_matches([';', ')'])
            .trim()
            .strip_prefix('"')
            .and_then(|s| s.strip_suffix('"'))
        {
            paths.push(dir.join(name));
        }
    }
    paths
}
