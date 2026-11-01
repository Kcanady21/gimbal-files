// SPDX-License-Identifier: GPL-3.0-or-later
//! P1i: a standalone header look, built on adw::ApplicationWindow so libadwaita's
//! stylesheet is loaded, and drawn entirely by our own `gl-` CSS.
#[path = "../../look_shared/blur.rs"]
mod blur;
mod header;
mod strip;
mod style;
use gtk::prelude::*;
use gtk::{gio, glib};
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
            eprintln!("[gimbal-look-header {:.3}] {}: {}", $crate::elapsed_secs(), $module, format_args!($($arg)*));
        }
    }};
}

const USAGE: &str = "Usage: look-header [--tabs N] [--scale S] [--font FAMILY]";

#[derive(Debug, PartialEq)]
pub struct Args {
    pub tabs: usize,
    pub scale: f64,
    pub font: Option<String>,
}

impl Args {
    pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Self, String> {
        let args: Vec<String> = args.into_iter().collect();
        let mut out = Self {
            tabs: 2,
            scale: 1.0,
            font: None,
        };
        let mut i = 0;
        while i < args.len() {
            let (flag, inline) = match args[i].split_once('=') {
                Some((flag, value)) => (flag, Some(value.to_string())),
                None => (args[i].as_str(), None),
            };
            if !matches!(flag, "--tabs" | "--scale" | "--font") {
                return Err(if flag.starts_with('-') {
                    format!("unknown flag: {flag}")
                } else {
                    format!("unexpected argument: {}", args[i])
                });
            }
            let value = match inline {
                Some(value) => value,
                None => {
                    i += 1;
                    args.get(i)
                        .cloned()
                        .ok_or(format!("{flag} needs a value"))?
                }
            };
            match flag {
                "--tabs" => {
                    out.tabs = value.parse().map_err(|_| format!("bad --tabs: {value}"))?;
                    if out.tabs == 0 {
                        return Err("--tabs must be at least 1".into());
                    }
                }
                "--scale" => {
                    out.scale = value.parse().map_err(|_| format!("bad --scale: {value}"))?;
                    if !(0.5..=1.0).contains(&out.scale) {
                        return Err(format!("--scale out of range: {value}"));
                    }
                }
                _ => out.font = Some(value),
            }
            i += 1;
        }
        Ok(out)
    }
}

fn main() -> glib::ExitCode {
    START.get_or_init(Instant::now);
    let args = Args::parse(std::env::args().skip(1)).unwrap_or_else(|error| {
        eprintln!("{error}\n{USAGE}");
        std::process::exit(2);
    });
    let app = adw::Application::builder()
        .application_id("io.github.kcanady21.GimbalLookHeader")
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();
    app.connect_activate(move |app| header::show(app, &args));
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
        assert_eq!(
            parse(&[]).unwrap(),
            Args {
                tabs: 2,
                scale: 1.0,
                font: None
            }
        );
        assert_eq!(
            parse(&["--tabs", "5", "--scale=0.75", "--font", "Fira Sans"]).unwrap(),
            Args {
                tabs: 5,
                scale: 0.75,
                font: Some("Fira Sans".into())
            }
        );
        assert!(parse(&["--bogus"]).unwrap_err().contains("--bogus"));
        assert!(parse(&["--tabs", "x"]).is_err() && parse(&["--tabs", "0"]).is_err());
        assert!(parse(&["--scale", "0.2"]).is_err() && parse(&["--font"]).is_err());
    }
}
