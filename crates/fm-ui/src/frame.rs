// SPDX-License-Identifier: GPL-3.0-or-later
//! The empty Gimbal window: header strip, left sidebar, content area and
//! preview sidebar, with Phase 1's resizable borders and r9's animated
//! collapse. Settings come from the one `ConfigFile` and go back to it.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use adw::prelude::*;
use fm_core::command::{Caller, CheckedCommand, Effect, Invocation, Outcome, Registry};
use fm_core::config::{self, Change, ConfigFile, Side};

use crate::commands;
use crate::layout;
use crate::look::Look;
use crate::look::motion;
use crate::look::tokens::*;

pub struct App {
    window: adw::ApplicationWindow,
    config: RefCell<ConfigFile>,
    look: RefCell<Look>,

    /// Left sidebar | (content | preview): two nested horizontal Paneds.
    outer: gtk::Paned,
    inner: gtk::Paned,

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

    /// Set while the preview width must be (re-)applied: at startup and after
    /// every collapse/expand animation, when max-position changes.
    right_dirty: Cell<bool>,
    /// Saved sidebar widths in design px.
    left_width: Cell<f64>,
    right_width: Cell<f64>,
    /// Border drags update these cells live; the config is written once, on
    /// button release.
    pending_left: Cell<Option<i32>>,
    pending_right: Cell<Option<i32>>,
    /// The positions `apply_positions` last applied: a `position` notify with
    /// any other value is a user drag.
    expected_left: Cell<i32>,
    expected_right: Cell<i32>,
}

/// One sidebar collapse/expand run. `start`/`end` are visible widths; every
/// frame moves the paned edge to `motion_width`. `expanded` is the width the
/// clipped panel keeps its own layout at, and `current` is where the edge is
/// now (a reverse or a retarget starts from it). `generation` makes stale
/// animation callbacks no-ops.
struct SideAnim {
    generation: u64,
    end: Cell<i32>,
    expanded: Cell<i32>,
    current: Cell<i32>,
    scale: f64,
    _animation: adw::TimedAnimation,
}

impl App {
    pub fn new(application: &adw::Application, config: ConfigFile) -> Rc<Self> {
        let scale = config.config().look.base * config.config().look.scale;
        let opacity = config.config().look.opacity;
        let font = Some(config.config().look.font.clone()).filter(|font| !font.is_empty());
        let left_config = config.config().sidebars.left;
        let right_config = config.config().sidebars.right;

        let window = adw::ApplicationWindow::builder()
            .application(application)
            .title("Gimbal Files")
            .build();
        window.set_default_size(
            scaled(f64::from(DEFAULT_W), scale),
            scaled(f64::from(DEFAULT_H), scale),
        );
        window.add_css_class("gimbal-look");

        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        window.set_content(Some(&root));

        // The header strip is empty until P2h.
        let header = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        header.add_css_class("gl-header");
        header.set_hexpand(true);
        root.append(&header);

        // Body frame: left sidebar | content | preview in two nested Paneds.
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

        // The content area: status line placeholder and nothing else.
        let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let spacer = gtk::Box::new(gtk::Orientation::Vertical, 0);
        spacer.set_vexpand(true);
        content.append(&spacer);
        let status_rule = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        status_rule.add_css_class("gl-rule");
        status_rule.add_css_class("inset");
        content.append(&status_rule);
        let status = gtk::Label::new(Some("Files: 0, Folders: 0"));
        status.add_css_class("gl-status");
        status.set_hexpand(true);
        status.set_xalign(0.5);
        content.append(&status);

        // Left sidebar and preview sidebar, each a panel with a title row and
        // a collapse button that lives in the host overlay.
        let (left_root, left_button) = panel("Pinned Folders", "sidebar-show-symbolic", true);
        let (left_host, left_clip, left_outline) = sidebar_host(&left_root, &left_button, true);
        let (right_root, right_button) = panel("File Name", "sidebar-show-right-symbolic", false);
        let (right_host, right_clip, right_outline) =
            sidebar_host(&right_root, &right_button, false);

        outer.set_start_child(Some(&left_host));
        inner.set_start_child(Some(&content));
        inner.set_end_child(Some(&right_host));

        let look = Look::new(window.upcast_ref::<gtk::Window>(), scale, opacity, font);

        let app = Rc::new(Self {
            window,
            config: RefCell::new(config),
            look: RefCell::new(look),
            outer: outer.clone(),
            inner: inner.clone(),
            left_root,
            left_host,
            left_clip,
            left_outline,
            left_button,
            left_collapsed: Cell::new(left_config.collapsed),
            left_anim: RefCell::new(None),
            left_gen: Rc::new(Cell::new(0)),
            right_root,
            right_host,
            right_clip,
            right_outline,
            right_button,
            right_collapsed: Cell::new(right_config.collapsed),
            right_anim: RefCell::new(None),
            right_gen: Rc::new(Cell::new(0)),
            right_dirty: Cell::new(true),
            left_width: Cell::new(f64::from(left_config.width)),
            right_width: Cell::new(f64::from(right_config.width)),
            pending_left: Cell::new(None),
            pending_right: Cell::new(None),
            expected_left: Cell::new(0),
            expected_right: Cell::new(0),
        });
        app.wire();
        app.apply_scale();
        app.log_startup();
        app
    }

    pub fn present(&self) {
        self.window.present();
    }

    pub fn window(&self) -> &adw::ApplicationWindow {
        &self.window
    }

    pub fn hotkeys(&self) -> std::collections::BTreeMap<String, fm_core::config::Hotkey> {
        self.config.borrow().config().hotkeys.clone()
    }

    /// The only command path: build the invocation, validate and dispatch
    /// through the registry, and let the handler see who called.
    pub fn dispatch(self: &Rc<Self>, caller: Caller, invocation: Invocation) {
        let mut handler = commands::UiHandler {
            caller,
            app: Rc::downgrade(self),
        };
        let outcome = Registry::new().dispatch(caller, &invocation, &mut handler);
        match outcome {
            Outcome::Error(error) => crate::log!("command", "{error}"),
            Outcome::DryRun(lines) => {
                crate::log!(
                    "command",
                    "{} dry run: {} lines",
                    invocation.name(),
                    lines.len()
                )
            }
            Outcome::Done => {}
        }
    }

    pub fn handle_command(self: &Rc<Self>, caller: Caller, command: &CheckedCommand) -> Outcome {
        crate::log!(
            "command",
            "{} via {caller} ({})",
            command.name(),
            command.effect()
        );
        if command.effect() != Effect::Settings {
            crate::log!("command", "command: {} not built yet", command.name());
            return Outcome::Done;
        }

        let change = {
            let config = self.config.borrow();
            let current = &config.config().home;
            config::change_for(command, config.config(), current)
        };
        let Some(change) = change else {
            crate::log!(
                "command",
                "{}: no config change could be made",
                command.name()
            );
            return Outcome::Done;
        };
        let applied = self.config.borrow_mut().apply(change.clone());
        match &applied {
            Ok(()) => crate::log!("command", "{} applied", command.name()),
            // `apply` still updates the in-memory config when the file cannot
            // be written; the current session keeps the change (P2c).
            Err(error) => crate::log!("config", "{error}"),
        }
        self.after_change(&change);
        Outcome::Done
    }

    fn after_change(self: &Rc<Self>, change: &Change) {
        match change {
            Change::SetScale(_) => self.apply_scale(),
            Change::SetOpacity(opacity) => {
                self.look.borrow_mut().set_opacity(*opacity);
                crate::log!("opacity", "setting {opacity:.2}: blur follows live");
            }
            Change::SetSidebarCollapsed { side, collapsed } => {
                self.toggle_sidebar_to(*side, *collapsed);
            }
            Change::SetSidebarWidth { side, width } => {
                let design = f64::from(*width);
                if *side == Side::Left {
                    self.left_width.set(design);
                } else {
                    self.right_width.set(design);
                }
                self.apply_positions(true);
                crate::log!("sidebar", "saved {} width {width} design px", side.as_str());
            }
            other => crate::log!("command", "change applied: {other:?}"),
        }
    }

    fn log_startup(&self) {
        let config = self.config.borrow();
        let config = config.config();
        crate::log!(
            "config",
            "scale {:.2} (base {:.2}), opacity {:.2}, font {:?}, left collapsed {} width {}, right collapsed {} width {}",
            config.look.scale,
            config.look.base,
            config.look.opacity,
            config.look.font,
            config.sidebars.left.collapsed,
            config.sidebars.left.width,
            config.sidebars.right.collapsed,
            config.sidebars.right.width,
        );
    }

    fn scale(&self) -> f64 {
        let config = self.config.borrow();
        config.config().look.base * config.config().look.scale
    }

    fn wire(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.outer
            .connect_notify_local(Some("max-position"), move |_, _| {
                if let Some(app) = weak.upgrade()
                    && app.outer.max_position() > 0
                {
                    app.apply_positions(false);
                }
            });
        let weak = Rc::downgrade(self);
        self.inner
            .connect_notify_local(Some("max-position"), move |_, _| {
                if let Some(app) = weak.upgrade() {
                    app.on_inner_max_position();
                }
            });
        let weak = Rc::downgrade(self);
        self.outer
            .connect_notify_local(Some("position"), move |paned, _| {
                if let Some(app) = weak.upgrade() {
                    app.on_outer_position(paned);
                }
            });
        let weak = Rc::downgrade(self);
        self.inner
            .connect_notify_local(Some("position"), move |paned, _| {
                if let Some(app) = weak.upgrade() {
                    app.on_inner_position(paned);
                }
            });

        for (button, left) in [(&self.left_button, true), (&self.right_button, false)] {
            let weak = Rc::downgrade(self);
            let side = if left { "left" } else { "right" };
            button.connect_clicked(move |_| {
                if let Some(app) = weak.upgrade() {
                    app.dispatch(
                        Caller::Ui,
                        Invocation::new(
                            "view.toggle-sidebar",
                            vec![("side".to_string(), side.to_string())],
                        ),
                    );
                }
            });
        }

        // GtkPaned has no drag-end signal. A capture-phase legacy controller
        // sees the button release without claiming the sequence the handle's
        // own gesture uses; that is when a border drag's width is written.
        let release = gtk::EventControllerLegacy::new();
        release.set_propagation_phase(gtk::PropagationPhase::Capture);
        let weak = Rc::downgrade(self);
        release.connect_event(move |_, event| {
            if event.event_type() == gtk::gdk::EventType::ButtonRelease
                && let Some(app) = weak.upgrade()
            {
                app.flush_widths();
            }
            gtk::glib::Propagation::Proceed
        });
        self.window.add_controller(release);

        // A drag that ends by losing the window also keeps what it reached.
        let weak = Rc::downgrade(self);
        self.window
            .connect_notify_local(Some("is-active"), move |window, _| {
                if !window.is_active()
                    && let Some(app) = weak.upgrade()
                {
                    app.flush_widths();
                }
            });

        commands::install_hotkeys(self, self.window.upcast_ref::<gtk::Window>());
    }

    fn apply_scale(self: &Rc<Self>) {
        let scale = self.scale();
        self.look.borrow_mut().set_scale(scale);
        let px = |token: f64| scaled(token, scale);
        if !self.animating(true) {
            self.left_root.set_width_request(px(SIDEBAR_MIN));
        }
        if !self.animating(false) {
            self.right_root.set_width_request(px(PREVIEW_MIN));
        }
        let strip = layout::strip_width(scale);
        self.left_host.set_width_request(strip);
        self.right_host.set_width_request(strip);
        let inset = layout::inset(scale);
        self.left_host.set_margin_start(inset);
        self.left_host.set_margin_bottom(inset);
        self.right_host.set_margin_end(inset);
        self.right_host.set_margin_bottom(inset);
        let top = layout::panel_button_inset(scale);
        self.left_button.set_margin_top(top);
        self.right_button.set_margin_top(top);
        for left in [true, false] {
            if self.animating(left) {
                continue;
            }
            let collapsed = self.collapsed(left);
            let p = if collapsed { 1.0 } else { 0.0 };
            let h = layout::button_inset(scale, left, p);
            if left {
                self.left_button.set_margin_start(h);
            } else {
                self.right_button.set_margin_end(h);
            }
            self.clip(left).set_visible(!collapsed);
            self.outline(left).set_visible(false);
            self.outline(left).set_opacity(1.0);
        }
        self.apply_positions(true);
        let (w, _) = self.body_widths();
        crate::log!(
            "layout",
            "scale {scale:.2}: left {}, content {}, preview {} (inset {inset})",
            w.left,
            w.content,
            w.preview
        );
    }

    fn on_inner_max_position(&self) {
        if self.animating(false) {
            return;
        }
        if self.right_dirty.get() && self.inner.max_position() > 0 {
            self.right_dirty.set(false);
            let (w, _) = self.body_widths();
            let pos = self.right_position(w.preview);
            self.expected_right.set(pos);
            if self.inner.position() != pos {
                self.inner.set_position(pos);
            }
        }
    }

    fn on_outer_position(&self, paned: &gtk::Paned) {
        let position = paned.position();
        let visible = layout::left_from_position(position, self.scale());
        if self.animating(true) {
            // An animation frame is ours: it is not a drag.
            self.expected_left.set(position);
            return;
        }
        if self.left_collapsed.get() {
            let target = layout::left_position(self.body_widths().0.left, self.scale());
            if position != target {
                paned.set_position(target);
            }
        } else {
            let min = layout::left_position(scaled(SIDEBAR_MIN, self.scale()), self.scale());
            if position < min {
                self.expected_left.set(min);
                if position != min {
                    paned.set_position(min);
                }
                return;
            }
            let implied = layout::left_position(self.body_widths().0.left, self.scale());
            if position != self.expected_left.get() && position != implied {
                self.left_width.set(visible as f64 / self.scale());
                self.pending_left.set(Some(visible));
                self.expected_left.set(position);
            }
        }
    }

    fn on_inner_position(&self, paned: &gtk::Paned) {
        let position = paned.position();
        if self.animating(false) {
            self.expected_right.set(position);
            return;
        }
        if self.right_collapsed.get() {
            let target = self.right_position(self.body_widths().0.preview);
            if position != target {
                paned.set_position(target);
            }
        } else {
            if self.inner.width() > 0 {
                let max = self.right_position(scaled(PREVIEW_MIN, self.scale()));
                if position > max {
                    self.expected_right.set(max);
                    if position != max {
                        paned.set_position(max);
                    }
                    return;
                }
            }
            let implied = self.right_position(self.body_widths().0.preview);
            if !self.right_dirty.get()
                && position != self.expected_right.get()
                && position != implied
            {
                let width =
                    layout::preview_from_position(self.inner.width(), position, self.scale());
                self.right_width.set(width as f64 / self.scale());
                self.pending_right.set(Some(width));
                self.expected_right.set(position);
            }
        }
    }

    /// Write the widths a just-finished border drag reached, once each.
    fn flush_widths(&self) {
        let scale = self.scale();
        for left in [true, false] {
            let pending = if left {
                self.pending_left.take()
            } else {
                self.pending_right.take()
            };
            let Some(visible) = pending else {
                continue;
            };
            let min = if left { SIDEBAR_MIN } else { PREVIEW_MIN };
            let design = ((visible as f64) / scale).round().max(min) as u32;
            let side = if left { Side::Left } else { Side::Right };
            let applied = self.config.borrow_mut().apply(Change::SetSidebarWidth {
                side,
                width: design,
            });
            match applied {
                Ok(()) => crate::log!(
                    "sidebar",
                    "saved {} width {design} design px ({visible} visible)",
                    side.as_str()
                ),
                Err(error) => crate::log!("config", "{error}"),
            }
        }
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

    fn current_width(&self, left: bool) -> i32 {
        if left {
            layout::left_from_position(self.outer.position(), self.scale())
        } else {
            layout::preview_from_position(self.inner.width(), self.inner.position(), self.scale())
        }
    }

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

    fn anchor_clip(&self, left: bool) {
        let adjustment = self.clip(left).hadjustment();
        let anchor = if left {
            0.0
        } else {
            (adjustment.upper() - adjustment.page_size()).max(0.0)
        };
        if (adjustment.value() - anchor).abs() > 0.5 {
            adjustment.set_value(anchor);
        }
    }

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
            let position = layout::preview_position(self.inner.width(), width, scale).max(0);
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
            if let Some(app) = weak.upgrade() {
                app.frame_sidebar(left, width);
            }
        });
        let duration = if start == end { 1 } else { motion::ms(200) };
        let animation = adw::TimedAnimation::new(&self.window, 0.0, 1.0, duration, target);
        let done_cell = self.gen_cell(left);
        let done_weak = Rc::downgrade(self);
        animation.connect_done(move |_| {
            if done_cell.get() != generation {
                return;
            }
            if let Some(app) = done_weak.upgrade() {
                app.finish_sidebar(left);
            }
        });
        let scale = self.scale();
        *self.anim_slot(left).borrow_mut() = Some(SideAnim {
            generation,
            end: Cell::new(end),
            expanded: Cell::new(expanded),
            current: Cell::new(start),
            scale,
            _animation: animation.clone(),
        });
        animation.play();
    }

    /// Animate a sidebar to `target_collapsed`, starting at the edge it has
    /// right now (so a second click mid-animation reverses it).
    fn toggle_sidebar_to(self: &Rc<Self>, side: Side, target_collapsed: bool) {
        let left = side == Side::Left;
        let running = {
            let slot = self.anim_slot(left);
            let borrowed = slot.borrow();
            borrowed
                .as_ref()
                .map(|anim| (anim.current.get(), anim.expanded.get()))
        };
        self.set_collapsed(left, target_collapsed);
        let scale = self.scale();
        let strip = layout::strip_width(scale);
        let start = match running {
            Some((current, _)) => current,
            None => self.current_width(left).max(1),
        };
        let end = if target_collapsed {
            strip
        } else {
            self.expanded_width(left).max(strip)
        };
        let expanded = match running {
            Some((_, expanded)) => expanded,
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
        crate::log!(
            "sidebar",
            "{} {} sidebar (panel {})",
            if collapsed { "collapsed" } else { "expanded" },
            if left { "left" } else { "preview" },
            root.width()
        );
        self.apply_positions(true);
    }

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
            (expanded as f64 * scale / old_scale).round() as i32
        } else {
            end.max(current)
        };
        self.panel_root(left).set_width_request(new_expanded);
        self.begin_motion(left, current, end, new_expanded);
    }

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

    fn right_position(&self, preview: i32) -> i32 {
        layout::preview_position(self.inner.width(), preview, self.scale())
    }

    fn apply_positions(self: &Rc<Self>, apply_preview: bool) {
        let (w, min_w) = self.body_widths();
        self.window.set_size_request(min_w, -1);
        if self.animating(true) {
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

fn rule() -> gtk::Box {
    let rule = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    rule.add_css_class("gl-rule");
    rule
}

fn icon_button(icon: &str) -> gtk::Button {
    let button = gtk::Button::new();
    button.set_child(Some(&gtk::Image::from_icon_name(icon)));
    button.add_css_class("gl-icon-button");
    button
}

/// A panel title row with the collapse button's 48 × 48 slot. The button
/// itself is an overlay child of the sidebar host, so the row keeps its exact
/// size during a collapse.
fn title_row(title: &str, icon: &str, button_first: bool) -> (gtk::Box, gtk::Button) {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    let label = gtk::Label::new(Some(title));
    label.add_css_class("gl-title");
    label.set_hexpand(true);
    label.set_xalign(0.0);
    label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    let slot = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    slot.add_css_class("gl-icon-slot");
    let button = icon_button(icon);
    if button_first {
        row.append(&slot);
    }
    row.append(&label);
    if !button_first {
        row.append(&slot);
    }
    (row, button)
}

fn panel(title: &str, icon: &str, button_first: bool) -> (gtk::Box, gtk::Button) {
    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    root.add_css_class("gl-panel");
    root.set_vexpand(true);
    let (row, button) = title_row(title, icon, button_first);
    root.append(&row);
    root.append(&rule());
    (root, button)
}

/// A sidebar host: an overlay whose main child is an `EXTERNAL` horizontal
/// scroller around the panel. EXTERNAL is what makes the paned move a
/// clipping viewport instead of squeezing the panel
/// (gtk-4.22.4:gtk/gtkscrolledwindow.c:1843-1855,
/// gtk/gtkviewport.c:496-566). The outline and button are overlays, so they
/// are not clipped.
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
    // A wheel or trackpad must not slide the clipped panel: pin the adjustment
    // to its anchor whenever GTK or an input device moves it.
    let adjustment = clip.hadjustment();
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
    (host, clip, outline)
}
