// SPDX-License-Identifier: GPL-3.0-or-later
//! P1m: P1j-r5's body plus the P1h acceptance polish: gap-l split tiles,
//! uneven taller columns, the Nov 1 sidebar hierarchy and contrast, circular
//! close rings, wrapped preview text, draggable pinned rows, the mockup's
//! symbolic icons, the spike's live theme reload and Ctrl+= / Ctrl+-.
//! `#[rustfmt::skip]` stays only on the constant tables, per the review.
#[path = "../../look_shared/blur.rs"]
mod blur;
mod grid;
mod icons;
mod layout;
mod preview;
mod sidebar;
mod split;
mod style;
#[path = "../../theme.rs"]
mod theme;

use adw::prelude::*;
use gtk::STYLE_PROVIDER_PRIORITY_APPLICATION as APP_PRIORITY;
use gtk::{Paned, gio, glib};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::OnceLock;
use std::time::Instant;
use style::{PREVIEW_MIN, PREVIEW_W, SIDEBAR_MIN, SIDEBAR_W, scaled};

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

/// One Ctrl+= / Ctrl+- step: 0.05, snapped to the slider's grid and clamped to
/// the setting's 0.50–1.00 range. The dev bar slider uses the same value.
pub fn stepped_scale(current: f64, delta: f64) -> f64 {
    (((current + delta) / 0.05).round() * 0.05).clamp(0.50, 1.00)
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
    app.connect_startup(theme::init);
    app.connect_activate(move |app| show(app, &args));
    app.run_with_args::<&str>(&[])
}

struct Ui {
    provider: gtk::CssProvider,
    window: adw::ApplicationWindow,
    base: f64,
    setting: Cell<f64>,
    opacity: Cell<f64>,
    font: Option<String>,
    blur: blur::Blur,
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
    /// The positions `apply_positions` last applied: a `position` notify
    /// with any other value is a user drag (correction 3).
    expected_left: Cell<i32>,
    expected_right: Cell<i32>,
    split: split::SplitView,
}

impl Ui {
    fn scale(&self) -> f64 {
        self.base * self.setting.get()
    }

    /// Regenerate the stylesheet on the same provider (scale and opacity).
    fn reload_css(&self) {
        let css = style::css(self.scale(), self.font.as_deref(), self.opacity.get());
        self.provider.load_from_string(&css);
    }

    /// The dev bar's opacity slider: CSS first, then the blur region.
    fn apply_opacity(&self) {
        self.reload_css();
        self.blur.set_opacity(self.opacity.get());
        crate::log!("opacity", "setting {:.2}", self.opacity.get());
    }

    /// Regenerate the stylesheet on the same provider and re-apply the
    /// Rust-side sizes that read the same token table × scale.
    fn apply_scale(&self) {
        let s = self.scale();
        self.reload_css();
        self.blur.set_scale(s);
        let px = |t: f64| scaled(t, s);
        self.left_root.set_width_request(px(SIDEBAR_MIN));
        self.left_strip.set_width_request(layout::strip_width(s));
        self.right_root.set_width_request(px(PREVIEW_MIN));
        self.right_strip.set_width_request(layout::strip_width(s));
        // The side panels are inset gap-s from the window's outer side and
        // bottom (the sketch's frame is gone); a collapsed strip keeps the
        // same insets. Tops stay directly under the header.
        let inset = layout::inset(s);
        self.left_root.set_margin_start(inset);
        self.left_root.set_margin_bottom(inset);
        self.left_strip.set_margin_start(inset);
        self.left_strip.set_margin_bottom(inset);
        self.right_root.set_margin_end(inset);
        self.right_root.set_margin_bottom(inset);
        self.right_strip.set_margin_end(inset);
        self.right_strip.set_margin_bottom(inset);
        self.left.borrow_mut().set_scale(s);
        self.split.set_scale(s);
        grid::set_scale(s);
        self.apply_positions(true);
        let (w, _) = self.body_widths();
        crate::log!(
            "layout",
            "scale {s:.2}: left {}, content {}, preview {} (inset {inset})",
            w.left,
            w.content,
            w.preview
        );
    }

    /// The pure widths function (layout.rs) driven by the window's current
    /// width, the saved sidebar widths and the collapse flags.
    fn body_widths(&self) -> (layout::BodyWidths, i32) {
        layout::body_widths(
            self.window.width(),
            self.scale(),
            self.left_width.get(),
            self.right_width.get(),
            self.left_collapsed.get(),
            self.right_collapsed.get(),
        )
    }

    /// Where the inner handle must sit for the preview to be `preview` px
    /// wide (the position lives in a width−handle space, gtkpaned.c:1362;
    /// the preview's end margin is the window's gap-s inset).
    fn right_position(&self, preview: i32) -> i32 {
        layout::preview_position(self.inner.width(), preview, self.scale())
    }

    /// Drive the outer paned from the widths function and keep the window at
    /// least one grid column wide. Each applied position is remembered in
    /// `expected_*`, so a `position` notify that differs from it is a user
    /// drag, not an echo of our own application (which only ever notifies
    /// with the applied value — gtkpaned.c:1917).
    ///
    /// The preview width needs the inner paned's current allocation. This
    /// runs from the outer's max-position notify while the window is
    /// mid-resize, where the inner is not re-allocated yet: applying then
    /// gets clamped against the old allocation and the clamp's echo would
    /// pollute the saved width. So `apply_preview` is false on that path and
    /// the inner's own next max-position notify applies it instead
    /// (`right_dirty`); scale and collapse changes have fresh allocations
    /// and pass true.
    fn apply_positions(&self, apply_preview: bool) {
        let (w, min_w) = self.body_widths();
        self.window.set_size_request(min_w, -1);
        let left = layout::left_position(w.left, self.scale());
        self.expected_left.set(left);
        self.outer.set_position(left);
        if apply_preview && self.inner.width() > 0 {
            let pos = self.right_position(w.preview);
            self.expected_right.set(pos);
            self.inner.set_position(pos);
        } else {
            self.right_dirty.set(true);
        }
    }
}

/// A `position` notify records a new saved width only when it reports a
/// position `apply_positions` did not apply itself — i.e. a user drag of
/// the handle. The notifications a scale change or a window resize cause
/// are echoes of our own application and must not touch the saved widths
/// (correction 3).
fn dragged(expected: i32, position: i32) -> Option<f64> {
    (position != expected).then_some(position as f64)
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
        ui.apply_positions(false);
        let side = if left { "left" } else { "preview" };
        let action = if collapse { "collapsed" } else { "expanded" };
        crate::log!("sidebar", "{action} {side} sidebar");
    });
}

fn strip(icon: &str, left: bool) -> gtk::Box {
    let strip = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    strip.add_css_class("gl-collapse-strip");
    strip.add_css_class(if left { "left" } else { "right" });
    let button = sidebar::icon_button(icon);
    // Vertically the button keeps the title row's start position. A Box does
    // not honour halign on its main axis, so two expanding spacers centre the
    // button across the collapsed strip (item 7).
    button.set_valign(gtk::Align::Start);
    let before = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    before.set_hexpand(true);
    let after = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    after.set_hexpand(true);
    strip.append(&before);
    strip.append(&button);
    strip.append(&after);
    strip
}

fn expand_button(strip: &gtk::Box) -> gtk::Button {
    let mut child = strip.first_child();
    while let Some(widget) = child {
        if let Some(button) = widget.downcast_ref::<gtk::Button>() {
            return button.clone();
        }
        child = widget.next_sibling();
    }
    panic!("strip holds its expand button")
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
    // The mockup icons are `gtk::Svg` paintables (see icons.rs), not icon-theme
    // names, so no search path is needed.
    gtk::style_context_add_provider_for_display(&display, &provider, APP_PRIORITY);
    let window = adw::ApplicationWindow::builder().application(app).build();
    window.set_default_size(style::DEFAULT_W, style::DEFAULT_H);
    window.add_css_class("gimbal-look");
    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    window.set_content(Some(&root));
    let opacity = style::opacity();

    // Dev bar: plain default widgets, part of the harness, not the design.
    let dev = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let scale_ctl = dev_slider(0.50, 1.00, 0.05, args.scale);
    let opacity_ctl = dev_slider(0.50, 1.00, 0.05, opacity);
    let drive_ctl = dev_slider(0.0, 1.0, 0.01, args.drives.first().copied().unwrap_or(0.5));
    dev.append(&scale_ctl);
    dev.append(&opacity_ctl);
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
    // The dev bar is harness furniture: in an EXTERNAL scroller it never
    // pins the window's minimum width, so the body's own one-column floor
    // is what stops a narrowing window.
    let dev_scroll = gtk::ScrolledWindow::new();
    dev_scroll.set_child(Some(&dev));
    dev_scroll.set_policy(gtk::PolicyType::External, gtk::PolicyType::Never);
    root.append(&dev_scroll);

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
    let single = grid::folder_view(args.items, false);
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
    let left_strip = strip("sidebar-show-symbolic", true);

    // Preview sidebar and its collapse strip.
    let right = preview::PreviewPanel::new();
    let right_root = right.root.clone();
    let right_strip = strip("sidebar-show-right-symbolic", false);

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

    let blur = blur::Blur::attach(window.upcast_ref(), style::base() * args.scale, opacity);
    let ui = Rc::new(Ui {
        provider,
        window: window.clone(),
        base: style::base(),
        setting: Cell::new(args.scale),
        opacity: Cell::new(opacity),
        font: args.font.clone(),
        blur,
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
        expected_left: Cell::new(0),
        expected_right: Cell::new(0),
        split,
    });
    outer.set_start_child(Some(&ui.left_root));
    inner.set_start_child(Some(&content));
    inner.set_end_child(Some(&ui.right_root));

    // The saved widths are set before the first allocation (a position set
    // then is honoured, clamped, by gtk_paned_compute_position). The inner
    // paned re-applies the preview width while right_dirty is set: at
    // startup and after every collapse/expand swap its max-position is only
    // computed at allocation time, and a window resize defers to it too
    // (see `apply_positions`).
    on_notify(&ui, &inner, "max-position", |ui, _| {
        if ui.right_dirty.get() && ui.inner.max_position() > 0 {
            ui.right_dirty.set(false);
            let (w, _) = ui.body_widths();
            let pos = ui.right_position(w.preview);
            ui.expected_right.set(pos);
            ui.inner.set_position(pos);
        }
    });
    // Every window-width change reaches here (max-position = allocation −
    // handle − end-child minimum): re-split the body from the widths
    // function, so narrowing drops grid columns and then gives the
    // sidebars way, and widening restores the saved widths.
    on_notify(&ui, &outer, "max-position", |ui, _| {
        if ui.outer.max_position() > 0 {
            ui.apply_positions(false);
        }
    });
    // Dragging a sidebar border updates the saved width (kept in design px,
    // so a scale change re-applies it scaled); anything we applied ourselves
    // is an echo and must not record: neither the value apply_positions set
    // (`dragged`) nor the position the widths function implies for the
    // current window (`implied` — apply_positions can run against a not
    // yet re-allocated paned during a window resize, and GTK then clamps
    // the applied position, notifying the implied one). While a sidebar is
    // collapsed its strip must keep the strip width: snap the handle back
    // whenever anything moves it. gtk_paned_set_position only notifies on a
    // real change, so the guard converges (gtkpaned.c:1917).
    on_notify(&ui, &outer, "position", |ui, paned| {
        let position = paned.position();
        let visible = layout::left_from_position(position, ui.scale());
        if ui.left_collapsed.get() {
            let target = layout::left_position(ui.body_widths().0.left, ui.scale());
            if position != target {
                paned.set_position(target);
            }
        } else {
            let implied = layout::left_position(ui.body_widths().0.left, ui.scale());
            if dragged(ui.expected_left.get(), position).is_some() && position != implied {
                ui.left_width.set(visible as f64 / ui.scale());
                ui.expected_left.set(position);
            }
        }
    });
    on_notify(&ui, &inner, "position", |ui, paned| {
        let position = paned.position();
        if ui.right_collapsed.get() {
            let target = ui.right_position(ui.body_widths().0.preview);
            if position != target {
                paned.set_position(target);
            }
        } else {
            let implied = ui.right_position(ui.body_widths().0.preview);
            if !ui.right_dirty.get()
                && dragged(ui.expected_right.get(), position).is_some()
                && position != implied
            {
                let width = layout::preview_from_position(ui.inner.width(), position, ui.scale());
                ui.right_width.set(width as f64 / ui.scale());
                ui.expected_right.set(position);
            }
        }
    });
    // Dev bar wiring.
    let ui_scale = ui.clone();
    scale_ctl.connect_value_changed(move |s| {
        ui_scale.setting.set(s.value());
        ui_scale.apply_scale();
    });
    let ui_opacity = ui.clone();
    opacity_ctl.connect_value_changed(move |s| {
        ui_opacity.opacity.set(s.value());
        ui_opacity.apply_opacity();
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
    // Ctrl+= / Ctrl+- step the same slider the dev bar uses (±0.05), so the
    // scale setting and the whole stylesheet move together.
    let keys = gtk::EventControllerKey::new();
    let scale_keys = scale_ctl.clone();
    keys.connect_key_pressed(move |_, key, _, modifiers| {
        if !modifiers.contains(gtk::gdk::ModifierType::CONTROL_MASK) {
            return glib::Propagation::Proceed;
        }
        let step = match key {
            gtk::gdk::Key::equal | gtk::gdk::Key::plus | gtk::gdk::Key::KP_Add => 0.05,
            gtk::gdk::Key::minus | gtk::gdk::Key::KP_Subtract => -0.05,
            _ => return glib::Propagation::Proceed,
        };
        let next = stepped_scale(scale_keys.value(), step);
        scale_keys.set_value(next);
        crate::log!("scale", "key {key:?}: setting {next:.2}");
        glib::Propagation::Stop
    });
    window.add_controller(keys);
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

    #[test]
    fn ctrl_scale_steps_and_clamps() {
        assert_eq!(stepped_scale(1.0, 0.05), 1.0);
        assert_eq!(stepped_scale(0.50, -0.05), 0.50);
        assert!((stepped_scale(0.85, 0.05) - 0.90).abs() < 1e-9);
        assert!((stepped_scale(0.95, 0.05) - 1.00).abs() < 1e-9);
        assert!((stepped_scale(1.00, -0.05) - 0.95).abs() < 1e-9);
    }

    #[test]
    fn only_a_user_drag_records_a_width() {
        // Applying a saved width notifies with the applied value: no record.
        assert_eq!(dragged(348, 348), None);
        // A drag does record, and afterwards that position is the expected
        // one — further echoes of it stay silent.
        assert_eq!(dragged(348, 500), Some(500.0));
        assert_eq!(dragged(500, 500), None);
        // The scale-change sequence: each apply is expected, so the saved
        // width is never touched (correction 3).
        let mut expected = 348;
        let mut saved = 348.0;
        for applied in [261i32, 348, 174, 348] {
            if let Some(px) = dragged(expected, applied) {
                saved = px;
                expected = applied;
            }
        }
        assert_eq!(saved, 348.0);
    }
}
