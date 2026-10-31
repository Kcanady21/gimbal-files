// SPDX-License-Identifier: GPL-3.0-or-later
mod dnd;
mod listing;
mod tabs;
mod theme;
mod window;
use gtk::prelude::*;
use gtk::{gio, glib};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::OnceLock;
use std::time::Instant;
static START: OnceLock<Instant> = OnceLock::new();
static ENABLED: OnceLock<bool> = OnceLock::new();
pub fn log_enabled() -> bool {
    *ENABLED.get_or_init(|| std::env::var("GIMBAL_SPIKE_LOG").as_deref() == Ok("1"))
}
pub fn elapsed_secs() -> f64 {
    START.get_or_init(Instant::now).elapsed().as_secs_f64()
}
#[macro_export]
macro_rules! log {
    ($module:literal, $($arg:tt)*) => {{
        if $crate::log_enabled() {
            eprintln!(
                "[gimbal-spike {:.3}] {}: {}",
                $crate::elapsed_secs(),
                $module,
                format_args!($($arg)*)
            );
        }
    }};
}
const USAGE: &str = "Usage: gimbal-spike [--select NAME[,NAME...]] [DIR...]";

#[derive(Debug, PartialEq)]
pub struct Args {
    pub dirs: Vec<PathBuf>,
    pub select: Vec<String>,
}

impl Args {
    pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Self, String> {
        let (mut dirs, mut select) = (Vec::new(), Vec::new());
        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            if arg == "--select" {
                let value = args.next().ok_or("--select needs a value")?;
                push_names(&mut select, &value);
            } else if let Some(value) = arg.strip_prefix("--select=") {
                push_names(&mut select, value);
            } else if arg.starts_with('-') && arg != "-" {
                return Err(format!("unknown flag: {arg}"));
            } else {
                dirs.push(PathBuf::from(arg));
            }
        }
        Ok(Self { dirs, select })
    }
}

fn push_names(names: &mut Vec<String>, value: &str) {
    names.extend(
        value
            .split(',')
            .filter(|n| !n.is_empty())
            .map(str::to_string),
    );
}

fn main() -> glib::ExitCode {
    START.get_or_init(Instant::now);
    let args = Args::parse(std::env::args().skip(1)).unwrap_or_else(|error| {
        eprintln!("{error}\n{USAGE}");
        std::process::exit(2);
    });

    let app = adw::Application::builder()
        .application_id("io.github.kcanady21.GimbalSpike")
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();
    app.set_accels_for_action("win.new-tab", &["<Control>t"]);
    app.set_accels_for_action("win.close-tab", &["<Control>w"]);
    app.set_accels_for_action("win.open-in-new-window", &["<Control>n"]);
    app.connect_startup(theme::init);

    let args = Rc::new(args);
    app.connect_activate(move |app| {
        let mut dirs = args.dirs.clone();
        if dirs.is_empty() {
            dirs.push(glib::home_dir());
        }
        let (window, tab_view) = window::new_window(app);
        let mut first = None;
        for (index, dir) in dirs.iter().enumerate() {
            let file = gio::File::for_path(dir);
            if index == 0 {
                first = Some(window::open_dir_in_tab_select(
                    &tab_view,
                    &file,
                    &args.select,
                ));
            } else {
                window::open_dir_in_tab(&tab_view, &file);
            }
        }
        // Show the first folder, the one --select applies to (review of P1b).
        if let Some(first) = first {
            tab_view.set_selected_page(&first);
        }
        window.present();
    });

    // The command line is ours; GTK must not try to parse it.
    app.run_with_args::<&str>(&[])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Args, String> {
        Args::parse(args.iter().map(|arg| arg.to_string()))
    }

    #[test]
    fn parses_cli() {
        let dirs = parse(&["/tmp/a", "/tmp/b"]).unwrap();
        assert_eq!(dirs.dirs, ["/tmp/a", "/tmp/b"].map(PathBuf::from));
        assert!(dirs.select.is_empty());
        assert_eq!(parse(&["--select", "b.txt"]).unwrap().select, ["b.txt"]);
        let many = parse(&["--select", "a.txt,b.txt,C.png"]).unwrap();
        assert_eq!(many.select, ["a.txt", "b.txt", "C.png"]);
        assert!(parse(&["--bogus"]).unwrap_err().contains("--bogus"));
    }
}
