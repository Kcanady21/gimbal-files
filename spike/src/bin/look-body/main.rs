// SPDX-License-Identifier: GPL-3.0-or-later
//! P1j-r2: the body look — split view with tile identity and a live drag
//! (the gesture rides the static gtk::Fixed), saved sidebar widths that a
//! drag updates and a scale change keeps (scaled), width-driven grid
//! columns, both sidebars, live scaling, all in our own `gl-` CSS.
//! `#[rustfmt::skip]` stays only on the constant tables, per the review.
mod grid;
mod preview;
mod sidebar;
mod split;
mod style;

use adw::prelude::*;
use gtk::STYLE_PROVIDER_PRIORITY_APPLICATION as APP_PRIORITY;
use gtk::{Paned, gio, glib};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::OnceLock;
use std::time::Instant;
use style::{GAP_2XL, PREVIEW_MIN, PREVIEW_W, SIDEBAR_MIN, SIDEBAR_W, scaled};

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
            eprintln!("[gimbal-look-body {:.3}] {}: {}", $crate::elapsed_secs(), $module, format_args!($($arg)*));
        }
    }};
}

const USAGE: &str = "Usage: look-body [--tiles N] [--items N] [--pins N] [--drives F,F,...] [--scale S] [--font FAMILY]";

#[derive(Debug, PartialEq)]
pub struct Args {
    pub tiles: usize,
    pub items: u32,
    pub pins: usize,
    pub drives: Vec<f64>,
    pub scale: f64,
    pub font: Option<String>,
}

fn parse_drives(raw: &str) -> Result<Vec<f64>, String> {
    let bad = || format!("bad --drives: {raw}");
    let mut drives = vec![];
    for part in raw.split(',') {
        let f: f64 = part.trim().parse().map_err(|_| bad())?;
        if !(0.0..=1.0).contains(&f) {
            return Err(format!("--drives out of range: {raw}"));
        }
        drives.push(f);
    }
    Ok(drives)
}

fn parse_scale(raw: &str) -> Result<f64, String> {
    let scale: f64 = raw.parse().map_err(|_| "bad --scale".to_string())?;
    if !(0.5..=1.0).contains(&scale) {
        return Err(format!("--scale out of range: {scale}"));
    }
    Ok(scale)
}

impl Args {
    pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Self, String> {
        let args: Vec<String> = args.into_iter().collect();
        let mut out = Self {
            tiles: 0,
            items: 44,
            pins: 7,
            drives: vec![0.50, 0.69, 0.99],
            scale: 1.0,
            font: None,
        };
        let mut i = 0;
        while i < args.len() {
            let (flag, inline) = match args[i].split_once('=') {
                Some((f, v)) => (f, Some(v.to_string())),
                None => (args[i].as_str(), None),
            };
            let mut value = || {
                inline
                    .clone()
                    .or_else(|| {
                        i += 1;
                        args.get(i).cloned()
                    })
                    .ok_or_else(|| format!("{flag} needs a value"))
            };
            match flag {
                "--tiles" => out.tiles = value()?.parse().map_err(|_| "bad --tiles".to_string())?,
                "--items" => out.items = value()?.parse().map_err(|_| "bad --items".to_string())?,
                "--pins" => out.pins = value()?.parse().map_err(|_| "bad --pins".to_string())?,
                "--drives" => out.drives = parse_drives(&value()?)?,
                "--scale" => out.scale = parse_scale(&value()?)?,
                "--font" => out.font = Some(value()?),
                flag if flag.starts_with('-') => return Err(format!("unknown flag: {flag}")),
                _ => return Err(format!("unexpected argument: {}", args[i])),
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
        .application_id("io.github.kcanady21.GimbalLookBody")
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();
    app.connect_activate(move |app| show(app, &args));
    app.run_with_args::<&str>(&[])
}

struct Ui {
    provider: gtk::CssProvider,
    base: f64,
    setting: Cell<f64>,
    font: Option<String>,
    /// Left sidebar | (content | preview): two nested horizontal Paneds.
    outer: gtk::Paned,
    inner: gtk::Paned,
    left: Rc<RefCell<sidebar::LeftPanel>>,
    left_root: gtk::Box,
    left_strip: gtk::Box,
    left_collapsed: Cell<bool>,
    right_root: gtk::Box,
    right_strip: gtk::Box,
    right_collapsed: Cell<bool>,
    /// Set while the preview pane's width must be (re-)applied: at startup
    /// and after every collapse/expand swap (it changes max-position).
    right_dirty: Cell<bool>,
    /// Saved sidebar widths in design px: dragging a border updates them, a
    /// scale change re-applies them scaled.
    left_width: Cell<f64>,
    right_width: Cell<f64>,
    split: split::SplitView,
}

impl Ui {
    fn scale(&self) -> f64 {
        self.base * self.setting.get()
    }

    /// Regenerate the stylesheet on the same provider and re-apply the
    /// Rust-side sizes that read the same token table × scale.
    fn apply_scale(&self) {
        let s = self.scale();
        let css = style::css(s, self.font.as_deref());
        self.provider.load_from_string(&css);
        let px = |t: f64| scaled(t, s);
        self.left_root.set_width_request(px(SIDEBAR_MIN));
        self.left_strip.set_width_request(px(GAP_2XL));
        self.right_root.set_width_request(px(PREVIEW_MIN));
        self.right_strip.set_width_request(px(GAP_2XL));
        self.left.borrow_mut().set_scale(s);
        self.split.set_scale(s);
        grid::set_scale(s);
        self.apply_positions();
        let (lw, pw) = (
            scaled(self.left_width.get(), s),
            scaled(self.right_width.get(), s),
        );
        crate::log!("layout", "scale {s:.2}: left {lw}, preview {pw}");
    }

    /// Where the left handle should sit: the strip width while collapsed,
    /// else the sidebar's saved width (design px, applied scaled).
    fn left_target(&self) -> i32 {
        let width = if self.left_collapsed.get() {
            GAP_2XL
        } else {
            self.left_width.get()
        };
        scaled(width, self.scale())
    }

    /// Where the right handle should sit: the preview is the end child, so
    /// its width = max-position + its minimum − the position.
    fn right_target(&self) -> i32 {
        let s = self.scale();
        let (min_w, pane_w) = (scaled(PREVIEW_MIN, s), scaled(self.right_width.get(), s));
        let extra = if self.right_collapsed.get() {
            0
        } else {
            min_w - pane_w
        };
        self.inner.max_position() + extra
    }

    fn apply_positions(&self) {
        self.outer.set_position(self.left_target());
        self.inner.set_position(self.right_target());
    }
}

/// `paned.connect_notify_local` with the `Rc<Ui>` wired in.
fn on_notify(ui: &Rc<Ui>, paned: &Paned, prop: &str, run: impl Fn(&Ui, &Paned) + 'static) {
    let ui = ui.clone();
    paned.connect_notify_local(Some(prop), move |paned, _| run(&ui, paned));
}

/// Collapse (or expand) one sidebar: hide the panel behind a `gap-2xl` strip
/// at the window edge holding the same button. The left sidebar is the outer
/// paned's start child, the preview the inner paned's end child.
fn wire_collapse(ui: &Rc<Ui>, trigger: &gtk::Button, left: bool, collapse: bool) {
    let ui = ui.clone();
    trigger.connect_clicked(move |_| {
        if left {
            ui.left_collapsed.set(collapse);
            let child = if collapse {
                ui.left_strip.clone()
            } else {
                ui.left_root.clone()
            };
            ui.outer.set_start_child(Some(&child));
        } else {
            ui.right_collapsed.set(collapse);
            ui.right_dirty.set(true);
            let child = if collapse {
                ui.right_strip.clone()
            } else {
                ui.right_root.clone()
            };
            ui.inner.set_end_child(Some(&child));
        }
        ui.apply_positions();
        let side = if left { "left" } else { "preview" };
        let action = if collapse { "collapsed" } else { "expanded" };
        crate::log!("sidebar", "{action} {side} sidebar");
    });
}

fn strip(icon: &str) -> gtk::Box {
    let strip = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    strip.add_css_class("gl-collapse-strip");
    strip.append(&sidebar::icon_button(icon));
    strip
}

fn expand_button(strip: &gtk::Box) -> gtk::Button {
    let button = strip.first_child().and_downcast::<gtk::Button>();
    button.expect("strip holds its expand button")
}

fn dev_slider(lo: f64, hi: f64, step: f64, value: f64) -> gtk::Scale {
    let slider = gtk::Scale::with_range(gtk::Orientation::Horizontal, lo, hi, step);
    slider.set_value(value);
    slider.set_size_request(200, -1);
    slider
}

fn show(app: &adw::Application, args: &Args) {
    let provider = gtk::CssProvider::new();
    let display = gtk::gdk::Display::default().expect("no display");
    gtk::style_context_add_provider_for_display(&display, &provider, APP_PRIORITY);
    let window = adw::ApplicationWindow::builder().application(app).build();
    window.set_default_size(style::DEFAULT_W, style::DEFAULT_H);
    window.add_css_class("gimbal-look");
    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    window.set_content(Some(&root));

    // Dev bar: plain default widgets, part of the harness, not the design.
    let dev = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let scale_ctl = dev_slider(0.50, 1.00, 0.05, args.scale);
    let drive_ctl = dev_slider(0.0, 1.0, 0.01, args.drives.first().copied().unwrap_or(0.5));
    dev.append(&scale_ctl);
    let split_buttons = [2usize, 3, 4, 6].map(|n| gtk::Button::with_label(&format!("Split {n}")));
    for b in &split_buttons {
        dev.append(b);
    }
    let single_btn = gtk::Button::with_label("Single");
    let pin_add = gtk::Button::with_label("+ Pin");
    let pin_remove = gtk::Button::with_label("− Pin");
    for b in [&single_btn, &pin_add, &pin_remove] {
        dev.append(b);
    }
    dev.append(&drive_ctl);
    root.append(&dev);

    // Empty strip standing in for P1i's header.
    let slot = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    slot.add_css_class("gl-header-slot");
    root.append(&slot);

    // Body frame: left sidebar | content | preview in two nested Paneds. The
    // sidebars never shrink on their own, the content area takes what's left.
    let frame = gtk::Box::new(gtk::Orientation::Vertical, 0);
    frame.add_css_class("gl-frame");
    frame.set_vexpand(true);
    root.append(&frame);
    let outer = gtk::Paned::new(gtk::Orientation::Horizontal);
    frame.append(&outer);
    outer.set_resize_start_child(false);
    outer.set_resize_end_child(true);
    outer.set_shrink_start_child(false);
    outer.set_shrink_end_child(true);
    let inner = gtk::Paned::new(gtk::Orientation::Horizontal);
    outer.set_end_child(Some(&inner));
    inner.set_resize_start_child(true);
    inner.set_resize_end_child(false);
    inner.set_shrink_start_child(true);
    inner.set_shrink_end_child(false);

    // Content column: folder view / split view, then rule and status line.
    let stack = gtk::Stack::new();
    stack.set_vexpand(true);
    let single = grid::folder_view(args.items);
    stack.add_named(&single, Some("single"));
    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    content.append(&stack);
    let status_rule = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    status_rule.add_css_class("gl-rule");
    status_rule.add_css_class("inset");
    content.append(&status_rule);
    content.append(&grid::status_label(args.items));

    // Left sidebar and its collapse strip.
    let left = Rc::new(RefCell::new(sidebar::LeftPanel::new(&args.drives)));
    left.borrow_mut().set_pin_count(args.pins);
    let left_root = left.borrow().root.clone();
    let left_strip = strip("sidebar-show-symbolic");

    // Preview sidebar and its collapse strip.
    let right = preview::PreviewPanel::new();
    let right_root = right.root.clone();
    let right_strip = strip("sidebar-show-right-symbolic");

    // Split view. A slot breaks the cycle: on_exit fades out and returns to
    // the single folder view. Every tile shows the same fake item count, so
    // one status update at startup covers single and split view.
    let view_slot: Rc<RefCell<Option<split::SplitView>>> = Rc::new(RefCell::new(None));
    let fade_to_single: Rc<dyn Fn()> = {
        let stack = stack.clone();
        let slot = view_slot.clone();
        Rc::new(move || {
            if let Some(view) = slot.borrow().as_ref() {
                let stack = stack.clone();
                view.fade_out(Box::new(move || stack.set_visible_child_name("single")));
            }
            crate::log!("split", "returning to single view");
        })
    };
    let split = split::SplitView::new(args.items, fade_to_single.clone());
    view_slot.replace(Some(split.clone()));
    stack.add_named(split.widget(), Some("split"));

    let ui = Rc::new(Ui {
        provider,
        base: style::base(),
        setting: Cell::new(args.scale),
        font: args.font.clone(),
        outer: outer.clone(),
        inner: inner.clone(),
        left: left.clone(),
        left_root,
        left_strip,
        left_collapsed: Cell::new(false),
        right_root,
        right_strip,
        right_collapsed: Cell::new(false),
        right_dirty: Cell::new(true),
        left_width: Cell::new(SIDEBAR_W),
        right_width: Cell::new(PREVIEW_W),
        split,
    });
    outer.set_start_child(Some(&ui.left_root));
    inner.set_start_child(Some(&content));
    inner.set_end_child(Some(&ui.right_root));

    // The saved widths are set before the first allocation (a position set
    // then is honoured, clamped, by gtk_paned_compute_position). The right
    // paned re-applies while right_dirty is set: a collapse/expand swap
    // changes max-position, which GTK only computes at allocation time.
    on_notify(&ui, &inner, "max-position", |ui, paned| {
        if ui.right_dirty.get() && paned.max_position() > 0 {
            ui.right_dirty.set(false);
            ui.inner.set_position(ui.right_target());
        }
    });
    // Dragging a sidebar border updates the saved width (kept in design px,
    // so a scale change re-applies it scaled). While a sidebar is collapsed
    // its strip must keep the strip width: snap the handle back whenever
    // anything moves it. gtk_paned_set_position only notifies on a real
    // change, so the guard converges. Recording is suppressed while
    // right_dirty is set: startup and collapse swaps notify with positions
    // that aren't the saved width's.
    on_notify(&ui, &outer, "position", |ui, paned| {
        if ui.left_collapsed.get() {
            let target = ui.left_target();
            if paned.position() != target {
                paned.set_position(target);
            }
        } else {
            ui.left_width.set(paned.position() as f64 / ui.scale());
        }
    });
    on_notify(&ui, &inner, "position", |ui, paned| {
        if ui.right_collapsed.get() {
            let target = ui.right_target();
            if paned.position() != target {
                paned.set_position(target);
            }
        } else if !ui.right_dirty.get() {
            // Inverse of right_target: width = max-position + min − position.
            let width = paned.max_position() + scaled(PREVIEW_MIN, ui.scale()) - paned.position();
            ui.right_width.set(width as f64 / ui.scale());
        }
    });
    // Dev bar wiring.
    let ui_scale = ui.clone();
    scale_ctl.connect_value_changed(move |s| {
        ui_scale.setting.set(s.value());
        ui_scale.apply_scale();
    });
    for (n, button) in [2usize, 3, 4, 6].iter().zip(&split_buttons) {
        let ui = ui.clone();
        let stack = stack.clone();
        button.connect_clicked(move |_| {
            ui.split.set_count(*n);
            stack.set_visible_child_name("split");
            ui.split.fade_in();
            crate::log!("split", "entered split view with {} tiles", n);
        });
    }
    single_btn.connect_clicked(move |_| fade_to_single());
    let (added, removed) = (left.clone(), left.clone());
    pin_add.connect_clicked(move |_| added.borrow_mut().add_pin());
    pin_remove.connect_clicked(move |_| removed.borrow_mut().remove_pin());
    let drive_left = left.clone();
    drive_ctl.connect_value_changed(move |s| drive_left.borrow_mut().set_drive(0, s.value()));
    wire_collapse(&ui, &left.borrow().collapse_button, true, true);
    wire_collapse(&ui, &expand_button(&ui.left_strip), true, false);
    wire_collapse(&ui, &right.collapse_button, false, true);
    wire_collapse(&ui, &expand_button(&ui.right_strip), false, false);

    // Start in split view when --tiles says so.
    if args.tiles > 1 {
        ui.split.set_count(args.tiles);
        stack.set_visible_child_name("split");
        ui.split.fade_in();
    }
    ui.apply_scale();
    window.present();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &str) -> Result<Args, String> {
        Args::parse(args.split(' ').filter(|a| !a.is_empty()).map(String::from))
    }

    #[test]
    fn parses_cli() {
        let got = parse("").unwrap();
        assert_eq!((got.tiles, got.items, got.pins), (0, 44, 7));
        assert_eq!((got.scale, got.font.as_deref()), (1.0, None));
        assert_eq!(got.drives, [0.50, 0.69, 0.99]);
        let cli = "--tiles 6 --items=10 --pins 14 --drives 0.25,1.0 --scale=0.5 --font Fira";
        let full = parse(cli).unwrap();
        assert_eq!((full.tiles, full.items, full.pins), (6, 10, 14));
        assert_eq!((full.scale, full.font.as_deref()), (0.5, Some("Fira")));
        assert_eq!(full.drives, [0.25, 1.0]);
    }

    #[test]
    fn rejects_bad_input() {
        assert!(parse("--bogus").unwrap_err().contains("--bogus"));
        let cases = "--tiles|--items x|--pins -1|--drives 0.5,oops|--drives 1.5|--scale 0.2|--scale 1.1|--font";
        for case in cases.split('|') {
            assert!(parse(case).is_err(), "{case:?} should be rejected");
        }
    }
}
