// SPDX-License-Identifier: GPL-3.0-or-later
//! The Noctalia theme watcher, ported from P1f.
//!
//! GTK loads `~/.config/gtk-4.0/gtk.css` once at startup and never reloads it
//! (gtksettings.c:1147-1161), so Noctalia's rewrites of `noctalia.css` are
//! invisible until restart. We watch the file and reload a USER-priority
//! provider ourselves. The watcher owns its monitors: dropping this value
//! cancels them.
use gtk::gio;
use gtk::glib;
use gtk::prelude::*;
use std::cell::Cell;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

pub struct Watcher {
    /// The USER-priority provider for Noctalia's colours. It is also held by
    /// the display once registered; keeping it here keeps the reload path
    /// simple.
    _provider: gtk::CssProvider,
    /// A dropped `FileMonitor` cancels itself, so the watcher owns them.
    _monitors: Vec<gio::FileMonitor>,
    _pending: Rc<Cell<Option<glib::SourceId>>>,
}

impl Watcher {
    /// Register the theme provider and start watching Noctalia's file. On a
    /// missing file or a display without a theme file, the app still runs
    /// with the desktop's defaults.
    pub fn new() -> Self {
        let provider = gtk::CssProvider::new();
        let pending: Rc<Cell<Option<glib::SourceId>>> = Rc::new(Cell::new(None));
        let mut monitors = Vec::new();

        let Some(display) = gtk::gdk::Display::default() else {
            crate::log!("theme", "no default display, not watching");
            return Self {
                _provider: provider,
                _monitors: monitors,
                _pending: pending,
            };
        };

        let path = glib::home_dir().join(".config/gtk-4.0/noctalia.css");
        provider.connect_parsing_error(|_, section, error| {
            crate::log!("theme", "parse error at {}: {error}", section.to_string());
        });
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_USER,
        );
        reload(&provider, &path);

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
            // Always reload the main file. Loading an imported file on its own
            // would replace the provider with the colours that file happens to
            // define.
            let target = path.clone();
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

        Self {
            _provider: provider,
            _monitors: monitors,
            _pending: pending,
        }
    }
}

fn reload(provider: &gtk::CssProvider, path: &PathBuf) {
    if !path.is_file() {
        crate::log!(
            "theme",
            "{} not found; using theme defaults",
            path.display()
        );
        return;
    }
    provider.load_from_file(&gio::File::for_path(path));
    crate::log!("theme", "reloaded {}", path.display());
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn imports_are_watched_once_each() {
        let dir = std::env::temp_dir().join(format!(
            "gimbal-p2g-theme-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("noctalia.css");
        std::fs::write(
            &path,
            "@import url(\"noctalia-extensions.css\");\n@import url('ignored.css');\n",
        )
        .unwrap();
        let paths = watched(&path);
        assert_eq!(paths.len(), 2);
        assert_eq!(paths[0], path);
        assert_eq!(paths[1], dir.join("noctalia-extensions.css"));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
