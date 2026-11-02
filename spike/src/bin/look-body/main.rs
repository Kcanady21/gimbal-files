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
#[path = "../../look_shared/motion.rs"]
mod motion;
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
    left_host: gtk::Overlay,
    left_clip: gtk::ScrolledWindow,
    left_outline: gtk::Box,
    left_button: gtk::Button,
    left_collapsed: Cell<bool>,
    left_anim: RefCell<Option<SideAnim>>,
    left_gen: Rc<Cell<u64>>,
    right_root: gtk::Box,
    right_host: gtk::Overlay,
    right_clip: gtk::ScrolledWindow,
    right_outline: gtk::Box,
    right_button: gtk::Button,
    right_collapsed: Cell<bool>,
    right_anim: RefCell<Option<SideAnim>>,
    right_gen: Rc<Cell<u64>>,
    /// Set while the preview pane's width must be (re-)applied: at startup
    /// and after every collapse/expand animation (it changes max-position).
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

/// One sidebar collapse/expand run. `start`/`end` are visible widths; every
/// frame moves the paned edge to `motion_width`. `expanded` is the width the
/// clipped panel keeps its own layout at, and `current` is where the edge is
/// now (a reverse or a retarget starts from it). `gen` is the generation the
/// animation's target and done handlers check, the same pattern the tile
/// slides use.
struct SideAnim {
    generation: u64,
    start: Cell<i32>,
    end: Cell<i32>,
    expanded: Cell<i32>,
    current: Cell<i32>,
    scale: f64,
    _animation: adw::TimedAnimation,
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
    fn apply_scale(self: &Rc<Self>) {
        let s = self.scale();
        self.reload_css();
        self.blur.set_scale(s);
        let px = |t: f64| scaled(t, s);
        // While a sidebar animates its panel keeps the pre-animation layout
        // width; the retarget in `apply_positions` scales or replaces it.
        if !self.animating(true) {
            self.left_root.set_width_request(px(SIDEBAR_MIN));
        }
        if !self.animating(false) {
            self.right_root.set_width_request(px(PREVIEW_MIN));
        }
        // The hosts are the paned children now; their floor is the strip, so
        // a collapsed sidebar and the handle's hard stop are the same width.
        let strip = layout::strip_width(s);
        self.left_host.set_width_request(strip);
        self.right_host.set_width_request(strip);
        // The side panels are inset gap-s from the window's outer side and
        // bottom (the sketch's frame is gone); a collapsed strip keeps the
        // same insets. Tops stay directly under the header.
        let inset = layout::inset(s);
        self.left_host.set_margin_start(inset);
        self.left_host.set_margin_bottom(inset);
        self.right_host.set_margin_end(inset);
        self.right_host.set_margin_bottom(inset);
        let top = layout::panel_button_inset(s);
        self.left_button.set_margin_top(top);
        self.right_button.set_margin_top(top);
        for left in [true, false] {
            if self.animating(left) {
                continue;
            }
            let p = if self.collapsed(left) { 1.0 } else { 0.0 };
            let h = layout::button_inset(s, left, p);
            if left {
                self.left_button.set_margin_start(h);
            } else {
                self.right_button.set_margin_end(h);
            }
        }
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

    fn anim_slot(&self, left: bool) -> &RefCell<Option<SideAnim>> {
        if left {
            &self.left_anim
        } else {
            &self.right_anim
        }
    }

    fn gen_cell(&self, left: bool) -> Rc<Cell<u64>> {
        if left {
            self.left_gen.clone()
        } else {
            self.right_gen.clone()
        }
    }

    fn animating(&self, left: bool) -> bool {
        self.anim_slot(left).borrow().is_some()
    }

    fn collapsed(&self, left: bool) -> bool {
        if left {
            self.left_collapsed.get()
        } else {
            self.right_collapsed.get()
        }
    }

    fn set_collapsed(&self, left: bool, value: bool) {
        if left {
            self.left_collapsed.set(value);
        } else {
            self.right_collapsed.set(value);
            self.right_dirty.set(true);
        }
    }

    fn panel_root(&self, left: bool) -> gtk::Box {
        if left {
            self.left_root.clone()
        } else {
            self.right_root.clone()
        }
    }

    fn clip(&self, left: bool) -> gtk::ScrolledWindow {
        if left {
            self.left_clip.clone()
        } else {
            self.right_clip.clone()
        }
    }

    fn outline(&self, left: bool) -> gtk::Box {
        if left {
            self.left_outline.clone()
        } else {
            self.right_outline.clone()
        }
    }

    /// The sidebar's visible width from the paned, which is where a reverse
    /// starts when no animation is running.
    fn current_width(&self, left: bool) -> i32 {
        if left {
            layout::left_from_position(self.outer.position(), self.scale())
        } else {
            layout::preview_from_position(self.inner.width(), self.inner.position(), self.scale())
        }
    }

    /// The visible width this sidebar expands to at the current window and
    /// scale, with the side itself treated as expanded (the widths function
    /// may still narrow it along with the other panel).
    fn expanded_width(&self, left: bool) -> i32 {
        let (w, _) = layout::body_widths(
            self.window.width(),
            self.scale(),
            self.left_width.get(),
            self.right_width.get(),
            if left {
                false
            } else {
                self.left_collapsed.get()
            },
            if left {
                self.right_collapsed.get()
            } else {
                false
            },
        );
        if left { w.left } else { w.preview }
    }

    /// Keep the right panel's content anchored to its right edge in the
    /// clipping scroller: the viewport translates by its adjustment, so the
    /// maximum value sits at `upper - page_size`. The left panel stays at 0.
    fn anchor_clip(&self, left: bool) {
        let adjustment = self.clip(left).hadjustment();
        {
            let anchor = if left {
                0.0
            } else {
                (adjustment.upper() - adjustment.page_size()).max(0.0)
            };
            if (adjustment.value() - anchor).abs() > 0.5 {
                adjustment.set_value(anchor);
            }
        }
    }

    /// One animation frame: move the paned edge to `width`, glide the button
    /// by the same collapse progress and fade the outline over the last part.
    fn frame_sidebar(&self, left: bool, width: i32) {
        let (generation, expanded) = {
            let slot = self.anim_slot(left);
            let borrowed = slot.borrow();
            let Some(anim) = borrowed.as_ref() else {
                return;
            };
            (anim.generation, anim.expanded.get())
        };
        if self.gen_cell(left).get() != generation {
            return;
        }
        let width = width.max(1);
        if let Some(anim) = self.anim_slot(left).borrow().as_ref() {
            anim.current.set(width);
        }
        let scale = self.scale();
        let progress = layout::collapse_progress(width, expanded, scale);
        if left {
            let position = layout::left_position(width, scale);
            self.expected_left.set(position);
            if self.outer.position() != position {
                self.outer.set_position(position);
            }
        } else {
            let position = layout::preview_position(self.inner.width(), width, scale);
            let position = position.max(0);
            self.expected_right.set(position);
            if self.inner.position() != position {
                self.inner.set_position(position);
            }
        }
        let inset = layout::button_inset(scale, left, progress);
        if left {
            self.left_button.set_margin_start(inset);
        } else {
            self.right_button.set_margin_end(inset);
        }
        self.outline(left)
            .set_opacity(layout::outline_opacity(progress));
        self.anchor_clip(left);
    }

    /// Start (or restart) one sidebar animation from `start` to `end`. The
    /// panel's own layout width is `expanded` throughout; a newer generation
    /// makes every older target and done handler a no-op.
    fn begin_motion(self: &Rc<Self>, left: bool, start: i32, end: i32, expanded: i32) {
        let generation = {
            let cell = self.gen_cell(left);
            let next = cell.get() + 1;
            cell.set(next);
            next
        };
        let gen_cell = self.gen_cell(left);
        let weak = Rc::downgrade(self);
        let target = adw::CallbackAnimationTarget::new(move |v| {
            if gen_cell.get() != generation {
                return;
            }
            let width = layout::motion_width(start, end, v);
            if let Some(ui) = weak.upgrade() {
                ui.frame_sidebar(left, width);
            }
        });
        let duration = if start == end {
            1
        } else {
            crate::motion::ms(200)
        };
        let animation = adw::TimedAnimation::new(&self.window, 0.0, 1.0, duration, target);
        let done_cell = self.gen_cell(left);
        let done_weak = Rc::downgrade(self);
        animation.connect_done(move |_| {
            if done_cell.get() != generation {
                return;
            }
            if let Some(ui) = done_weak.upgrade() {
                ui.finish_sidebar(left);
            }
        });
        *self.anim_slot(left).borrow_mut() = Some(SideAnim {
            generation,
            start: Cell::new(start),
            end: Cell::new(end),
            expanded: Cell::new(expanded),
            current: Cell::new(start),
            scale: self.scale(),
            _animation: animation.clone(),
        });
        animation.play();
    }

    /// A click, or a click mid-animation: the target is the opposite of the
    /// current direction, starting from the width the edge is at right now.
    fn toggle_sidebar(self: &Rc<Self>, left: bool) {
        let running = {
            let slot = self.anim_slot(left);
            let borrowed = slot.borrow();
            borrowed.as_ref().map(|a| {
                (
                    a.current.get(),
                    a.expanded.get(),
                    a.start.get(),
                    a.end.get(),
                )
            })
        };
        let collapsing_now = running.is_some_and(|(_, _, start, end)| end < start);
        let target_collapsed = if running.is_some() {
            !collapsing_now
        } else {
            !self.collapsed(left)
        };
        self.set_collapsed(left, target_collapsed);
        let scale = self.scale();
        let strip = layout::strip_width(scale);
        let start = match running {
            Some((current, _, _, _)) => current,
            None => self.current_width(left).max(1),
        };
        let end = if target_collapsed {
            strip
        } else {
            self.expanded_width(left).max(strip)
        };
        let expanded = match running {
            // The panel keeps the layout width it had before this animation
            // or any reverse of it.
            Some((_, expanded, _, _)) => expanded,
            None if target_collapsed => start.max(end),
            None => end,
        };
        if running.is_none() {
            self.panel_root(left).set_width_request(expanded);
        }
        self.panel_root(left).add_css_class("gl-anim");
        self.clip(left).set_visible(true);
        self.outline(left).set_visible(true);
        self.outline(left)
            .set_opacity(layout::outline_opacity(layout::collapse_progress(
                start, expanded, scale,
            )));
        let action = if target_collapsed {
            "collapsing"
        } else {
            "expanding"
        };
        crate::log!(
            "sidebar",
            "{action} {} sidebar from {start} to {end} (panel {expanded})",
            if left { "left" } else { "preview" }
        );
        self.begin_motion(left, start, end, expanded);
    }

    /// The animation reached its end width: drop the clipping animation
    /// state, put the button and outline back to their resting values and let
    /// the normal layout path take over. A stale generation does nothing.
    fn finish_sidebar(self: &Rc<Self>, left: bool) {
        let anim = self.anim_slot(left).borrow_mut().take();
        let Some(anim) = anim else {
            return;
        };
        if self.gen_cell(left).get() != anim.generation {
            return;
        }
        let collapsed = self.collapsed(left);
        let scale = self.scale();
        let root = self.panel_root(left);
        root.remove_css_class("gl-anim");
        root.set_width_request(if left {
            scaled(SIDEBAR_MIN, scale)
        } else {
            scaled(PREVIEW_MIN, scale)
        });
        let outline = self.outline(left);
        outline.set_visible(false);
        outline.set_opacity(1.0);
        self.clip(left).set_visible(!collapsed);
        let progress = if collapsed { 1.0 } else { 0.0 };
        let inset = layout::button_inset(scale, left, progress);
        if left {
            self.left_button.set_margin_start(inset);
        } else {
            self.right_button.set_margin_end(inset);
        }
        let action = if collapsed { "collapsed" } else { "expanded" };
        crate::log!(
            "sidebar",
            "{action} {} sidebar (panel {})",
            if left { "left" } else { "preview" },
            root.width()
        );
        self.apply_positions(true);
    }

    /// A window resize or scale change during an animation keeps the panel's
    /// layout width but moves the end width; start a new run from the current
    /// edge. A same-width call (every ordinary resize notify) is a no-op.
    fn retarget_side(self: &Rc<Self>, left: bool, end: i32) {
        let (current, old_end, expanded, old_scale) = {
            let slot = self.anim_slot(left);
            let borrowed = slot.borrow();
            let Some(anim) = borrowed.as_ref() else {
                return;
            };
            (
                anim.current.get(),
                anim.end.get(),
                anim.expanded.get(),
                anim.scale,
            )
        };
        let scale = self.scale();
        if end == old_end && (scale - old_scale).abs() < 1e-9 {
            return;
        }
        let new_expanded = if self.collapsed(left) {
            // Keep the panel's design width through a scale change.
            (expanded as f64 * scale / old_scale).round() as i32
        } else {
            end.max(current)
        };
        self.panel_root(left).set_width_request(new_expanded);
        self.begin_motion(left, current, end, new_expanded);
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
    fn apply_positions(self: &Rc<Self>, apply_preview: bool) {
        let (w, min_w) = self.body_widths();
        self.window.set_size_request(min_w, -1);
        if self.animating(true) {
            // The resize/scale retargets the running animation instead of
            // snapping to its end width.
            self.retarget_side(true, w.left);
        } else {
            let left = layout::left_position(w.left, self.scale());
            self.expected_left.set(left);
            if self.outer.position() != left {
                self.outer.set_position(left);
            }
        }
        if self.animating(false) {
            self.retarget_side(false, w.preview);
        } else if apply_preview && self.inner.width() > 0 {
            let pos = self.right_position(w.preview);
            self.expected_right.set(pos);
            self.right_dirty.set(false);
            if self.inner.position() != pos {
                self.inner.set_position(pos);
            }
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
fn on_notify(ui: &Rc<Ui>, paned: &Paned, prop: &str, run: impl Fn(&Rc<Ui>, &Paned) + 'static) {
    let ui = ui.clone();
    paned.connect_notify_local(Some(prop), move |paned, _| run(&ui, paned));
}

/// A sidebar host: a `GtkOverlay` whose main child is an `EXTERNAL`
/// horizontal scroller around the panel. EXTERNAL is the part that makes the
/// paned move a clipping viewport instead of squeezing the panel: the
/// scroller's horizontal measure is zero (`gtk-4.22.4:gtk/gtkscrolledwindow.c:1843-1855`,
/// `:1887-1922`), while `gtk_viewport_size_allocate` gives the panel its
/// `width_request` and clips it (`gtk-4.22.4:gtk/gtkviewport.c:496-566`).
/// The left panel is anchored at adjustment 0; the preview is anchored at
/// `upper - page_size` so its right edge stays on the window's right.
/// The outline and the button are overlay children, so the edge and the
/// button can move without being clipped.
fn sidebar_host(
    panel: &gtk::Box,
    button: &gtk::Button,
    left: bool,
) -> (gtk::Overlay, gtk::ScrolledWindow, gtk::Box) {
    let host = gtk::Overlay::new();
    let clip = gtk::ScrolledWindow::new();
    clip.set_policy(gtk::PolicyType::External, gtk::PolicyType::Never);
    clip.set_propagate_natural_width(false);
    clip.set_child(Some(panel));
    let outline = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    outline.add_css_class("gl-panel-outline");
    outline.set_hexpand(true);
    outline.set_vexpand(true);
    outline.set_halign(gtk::Align::Fill);
    outline.set_valign(gtk::Align::Fill);
    outline.set_can_target(false);
    outline.set_visible(false);
    button.set_valign(gtk::Align::Start);
    button.set_halign(if left {
        gtk::Align::Start
    } else {
        gtk::Align::End
    });
    host.set_child(Some(&clip));
    host.add_overlay(&outline);
    host.add_overlay(button);
    // A wheel or a trackpad must not slide the clipped panel: pin the
    // adjustment to its anchor whenever GTK or an input device moves it.
    let adjustment = clip.hadjustment();
    {
        let pin = move |adjustment: &gtk::Adjustment| {
            let anchor = if left {
                0.0
            } else {
                (adjustment.upper() - adjustment.page_size()).max(0.0)
            };
            if (adjustment.value() - anchor).abs() > 0.5 {
                adjustment.set_value(anchor);
            }
        };
        adjustment.connect_changed(pin);
        adjustment.connect_value_changed(pin);
    }
    (host, clip, outline)
}

/// One click on either button toggles the side; mid-animation it reverses.
fn wire_toggle(ui: &Rc<Ui>, button: &gtk::Button, left: bool) {
    let ui = ui.clone();
    button.connect_clicked(move |_| ui.toggle_sidebar(left));
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
    // The sidebar hosts keep the paned from allocating a panel below its
    // own minimum: each panel lives in an EXTERNAL horizontal scroller whose
    // measure ignores the child's minimum, so the paned only ever moves the
    // host and the panel is clipped at the moving edge (r9).

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

    // Left sidebar and its host (clipping holder + outline + button).
    let left = Rc::new(RefCell::new(sidebar::LeftPanel::new(&args.drives)));
    left.borrow_mut().set_pin_count(args.pins);
    let left_root = left.borrow().root.clone();
    let left_button = left.borrow().collapse_button.clone();
    let (left_host, left_clip, left_outline) = sidebar_host(&left_root, &left_button, true);

    // Preview sidebar and its host.
    let right = preview::PreviewPanel::new();
    let right_root = right.root.clone();
    let right_button = right.collapse_button.clone();
    let (right_host, right_clip, right_outline) = sidebar_host(&right_root, &right_button, false);

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
        left_host,
        left_clip,
        left_outline,
        left_button,
        left_collapsed: Cell::new(false),
        left_anim: RefCell::new(None),
        left_gen: Rc::new(Cell::new(0)),
        right_root,
        right_host,
        right_clip,
        right_outline,
        right_button,
        right_collapsed: Cell::new(false),
        right_anim: RefCell::new(None),
        right_gen: Rc::new(Cell::new(0)),
        right_dirty: Cell::new(true),
        left_width: Cell::new(SIDEBAR_W),
        right_width: Cell::new(PREVIEW_W),
        expected_left: Cell::new(0),
        expected_right: Cell::new(0),
        split,
    });
    outer.set_start_child(Some(&ui.left_host));
    inner.set_start_child(Some(&content));
    inner.set_end_child(Some(&ui.right_host));

    // The saved widths are set before the first allocation (a position set
    // then is honoured, clamped, by gtk_paned_compute_position). The inner
    // paned re-applies the preview width while right_dirty is set: at
    // startup and after every collapse/expand swap its max-position is only
    // computed at allocation time, and a window resize defers to it too
    // (see `apply_positions`).
    on_notify(&ui, &inner, "max-position", |ui, _| {
        if ui.animating(false) {
            return;
        }
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
        if ui.animating(true) {
            // An animation frame is ours: it is not a drag and must not be
            // snapped back to the end width.
            ui.expected_left.set(position);
            return;
        }
        if ui.left_collapsed.get() {
            let target = layout::left_position(ui.body_widths().0.left, ui.scale());
            if position != target {
                paned.set_position(target);
            }
        } else {
            let min = layout::left_position(scaled(SIDEBAR_MIN, ui.scale()), ui.scale());
            if position < min {
                ui.expected_left.set(min);
                if position != min {
                    paned.set_position(min);
                }
                return;
            }
            let implied = layout::left_position(ui.body_widths().0.left, ui.scale());
            if dragged(ui.expected_left.get(), position).is_some() && position != implied {
                ui.left_width.set(visible as f64 / ui.scale());
                ui.expected_left.set(position);
            }
        }
    });
    on_notify(&ui, &inner, "position", |ui, paned| {
        let position = paned.position();
        if ui.animating(false) {
            ui.expected_right.set(position);
            return;
        }
        if ui.right_collapsed.get() {
            let target = ui.right_position(ui.body_widths().0.preview);
            if position != target {
                paned.set_position(target);
            }
        } else {
            if ui.inner.width() > 0 {
                let max = ui.right_position(scaled(PREVIEW_MIN, ui.scale()));
                if position > max {
                    ui.expected_right.set(max);
                    if position != max {
                        paned.set_position(max);
                    }
                    return;
                }
            }
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
    // One button per side now: expanded it is the panel's collapse button,
    // collapsed the strip's expand button; mid-animation it reverses.
    wire_toggle(&ui, &ui.left_button, true);
    wire_toggle(&ui, &ui.right_button, false);

    // Start in split view when --tiles says so.
    if args.tiles > 1 {
        ui.split.set_count(args.tiles);
        stack.set_visible_child_name("split");
        ui.split.fade_in();
    }
    // Ctrl+= / Ctrl+- step the same slider the dev bar uses (±0.05), so the
    // scale setting and the whole stylesheet move together. Esc cancels an
    // active split drag, and losing the window's focus does the same, so no
    // preview rects are left behind (P1m-r8).
    let keys = gtk::EventControllerKey::new();
    let scale_keys = scale_ctl.clone();
    let esc_split = ui.split.clone();
    keys.connect_key_pressed(move |_, key, _, modifiers| {
        if key == gtk::gdk::Key::Escape {
            if esc_split.cancel_drag() {
                crate::log!("split", "Escape cancelled the drag");
                return glib::Propagation::Stop;
            }
            return glib::Propagation::Proceed;
        }
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
    let focus_split = ui.split.clone();
    window.connect_notify_local(Some("is-active"), move |window, _| {
        if !window.is_active() {
            focus_split.cancel_drag();
        }
    });
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
