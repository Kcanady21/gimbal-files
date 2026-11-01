// SPDX-License-Identifier: GPL-3.0-or-later
//! Compositor blur for the two look prototypes, through the staging
//! `ext-background-effect-v1` Wayland protocol. GTK 4.22 has no API for it, so
//! this module speaks the protocol on GDK's own `wl_display`.
//!
//! The protocol's `set_blur_region` request is double-buffered and applied on
//! the next `wl_surface.commit`
//! (`wayland-protocols-0.32.13:protocols/staging/ext-background-effect/ext-background-effect-v1.xml:116-117`,
//! "The blur region is double-buffered state, and will be applied on the next
//! wl_surface.commit"); every change here ends with `GdkSurface::queue_render`
//! so a frame, and with it a commit, follows.

use gdk4_wayland::prelude::*;
use gtk::gdk;
use gtk::glib;
use gtk::prelude::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use wayland_client::protocol::{
    wl_compositor::WlCompositor, wl_region::WlRegion, wl_registry::WlRegistry,
    wl_surface::WlSurface,
};
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle, WEnum};
use wayland_protocols::ext::background_effect::v1::client::{
    ext_background_effect_manager_v1::{self, Capability, ExtBackgroundEffectManagerV1},
    ext_background_effect_surface_v1::ExtBackgroundEffectSurfaceV1,
};

/// `radius` from the style spec, at scale 1.
pub const RADIUS: f64 = 12.0;
/// Rows of strips used to approximate each rounded corner.
const CORNER_STRIPS: i32 = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

/// A rounded rectangle approximated by axis-aligned rectangles, inside
/// `w × h` at (0, 0). One full-width body between the corners, `strips` rows
/// per corner; each corner strip uses the inset of its outer edge, so the
/// approximation always stays inside the rounded rectangle (it cuts a little
/// more corner than an exact circle would).
pub fn rounded_region_rects(w: i32, h: i32, radius: i32, strips: i32) -> Vec<Rect> {
    if w <= 0 || h <= 0 {
        return Vec::new();
    }
    let r = radius.clamp(0, w.min(h) / 2);
    if r == 0 {
        return vec![Rect { x: 0, y: 0, w, h }];
    }
    let n = strips.max(1);
    let mut rects = Vec::with_capacity(2 * n as usize + 1);
    if h > 2 * r {
        rects.push(Rect {
            x: 0,
            y: r,
            w,
            h: h - 2 * r,
        });
    }
    for i in 0..n {
        let y0 = i * r / n;
        let y1 = (i + 1) * r / n;
        let hh = y1 - y0;
        if hh <= 0 {
            continue;
        }
        let dy = r - y0;
        let inset = r - isqrt(i64::from(r) * i64::from(r) - i64::from(dy) * i64::from(dy));
        let width = w - 2 * inset;
        if width <= 0 {
            continue;
        }
        rects.push(Rect {
            x: inset,
            y: y0,
            w: width,
            h: hh,
        });
        rects.push(Rect {
            x: inset,
            y: h - y1,
            w: width,
            h: hh,
        });
    }
    rects
}

fn isqrt(n: i64) -> i32 {
    let mut x = (n.max(0) as f64).sqrt() as i64;
    while (x + 1) * (x + 1) <= n {
        x += 1;
    }
    while x * x > n {
        x -= 1;
    }
    x as i32
}

/// One global advertised on GDK's registry.
struct State {
    globals: Vec<(u32, String, u32)>,
    capabilities: Option<Capability>,
}

impl Dispatch<WlRegistry, ()> for State {
    fn event(
        state: &mut Self,
        _proxy: &WlRegistry,
        event: <WlRegistry as Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        if let wayland_client::protocol::wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        {
            state.globals.push((name, interface, version));
        }
    }
}

impl Dispatch<ExtBackgroundEffectManagerV1, ()> for State {
    fn event(
        state: &mut Self,
        _proxy: &ExtBackgroundEffectManagerV1,
        event: <ExtBackgroundEffectManagerV1 as Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        if let ext_background_effect_manager_v1::Event::Capabilities { flags } = event
            && let WEnum::Value(capabilities) = flags
        {
            state.capabilities = Some(capabilities);
        }
    }
}

impl Dispatch<ExtBackgroundEffectSurfaceV1, ()> for State {
    fn event(
        _state: &mut Self,
        _proxy: &ExtBackgroundEffectSurfaceV1,
        _event: <ExtBackgroundEffectSurfaceV1 as Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WlRegion, ()> for State {
    fn event(
        _state: &mut Self,
        _proxy: &WlRegion,
        _event: <WlRegion as Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

/// The bound protocol objects. Kept alive for the life of the window.
struct Protocol {
    connection: Connection,
    _queue: EventQueue<State>,
    _registry: WlRegistry,
    qh: QueueHandle<State>,
    manager: ExtBackgroundEffectManagerV1,
    compositor: WlCompositor,
    /// Set once the window is realized.
    surface: Option<gdk::Surface>,
    wl_surface: Option<WlSurface>,
    effect: Option<ExtBackgroundEffectSurfaceV1>,
}

struct Inner {
    window: glib::WeakRef<gtk::Window>,
    opacity: Cell<f64>,
    /// The app's own scale (`base × setting`), not GDK's monitor scale.
    scale: Cell<f64>,
    protocol: RefCell<Option<Protocol>>,
    /// The surface size the last layout signal reported; the signal also
    /// fires for relayouts that do not change it.
    layout: Cell<Option<(i32, i32)>>,
    /// The one line a fallback logs.
    logged: Cell<bool>,
}

/// A handle both programs keep in their `Ui`.
pub struct Blur {
    inner: Rc<Inner>,
}

impl Blur {
    /// Bind the protocol on GDK's `wl_display` and listen for capabilities.
    /// On X11, or without the global/capability, this logs once and every
    /// later call is a no-op; the window stays transparent either way.
    pub fn attach(window: &gtk::Window, scale: f64, opacity: f64) -> Self {
        let inner = Rc::new(Inner {
            window: glib::WeakRef::new(),
            opacity: Cell::new(opacity),
            scale: Cell::new(scale),
            protocol: RefCell::new(None),
            layout: Cell::new(None),
            logged: Cell::new(false),
        });
        inner.window.set(Some(window));
        let blur = Self { inner };
        match blur.bind() {
            Ok(protocol) => {
                *blur.inner.protocol.borrow_mut() = Some(protocol);
            }
            Err(reason) => blur.inner.fallback(&reason),
        }
        if window.is_realized() {
            Inner::realize(&blur.inner);
        }
        window.connect_realize({
            let weak = Rc::downgrade(&blur.inner);
            move |_| {
                if let Some(inner) = weak.upgrade() {
                    Inner::realize(&inner);
                }
            }
        });
        blur
    }

    /// Opacity 1.00 removes the blur region; below 1.00 restores it.
    /// (look-header has no opacity slider, so its build does not call this.)
    #[allow(dead_code)]
    pub fn set_opacity(&self, opacity: f64) {
        self.inner.opacity.set(opacity);
        self.inner.update();
    }

    /// The app's scaled corner radius changed.
    pub fn set_scale(&self, scale: f64) {
        self.inner.scale.set(scale);
        self.inner.update();
    }

    fn bind(&self) -> Result<Protocol, String> {
        let display = gdk::Display::default().ok_or("no GDK display")?;
        let wayland = display
            .downcast::<gdk4_wayland::WaylandDisplay>()
            .map_err(|_| "not a Wayland display (X11 or another backend)".to_string())?;
        let wl_display = wayland
            .wl_display()
            .ok_or("GDK display has no wl_display")?;
        let compositor = wayland
            .wl_compositor()
            .ok_or("GDK display has no wl_compositor")?;
        let backend = wl_display
            .backend()
            .upgrade()
            .ok_or("GDK's Wayland backend is gone")?;
        let connection = Connection::from_backend(backend);
        let mut queue = connection.new_event_queue::<State>();
        let qh = queue.handle();
        let registry = wl_display.get_registry(&qh, ());
        let mut state = State {
            globals: Vec::new(),
            capabilities: None,
        };
        queue
            .roundtrip(&mut state)
            .map_err(|error| format!("registry roundtrip failed: {error}"))?;
        let (name, version) = state
            .globals
            .iter()
            .find(|(_, interface, _)| interface == "ext_background_effect_manager_v1")
            .map(|(name, _, version)| (*name, *version))
            .ok_or("ext_background_effect_manager_v1 is not advertised")?;
        crate::log!(
            "blur",
            "global ext_background_effect_manager_v1 v{version} (name {name})"
        );
        let manager: ExtBackgroundEffectManagerV1 = registry.bind(name, version.min(1), &qh, ());
        queue
            .roundtrip(&mut state)
            .map_err(|error| format!("capabilities roundtrip failed: {error}"))?;
        let capabilities = state
            .capabilities
            .ok_or("ext_background_effect_manager_v1 sent no capabilities")?;
        let blur = capabilities.contains(Capability::Blur);
        crate::log!("blur", "capabilities {capabilities:?} (blur {blur})");
        if !blur {
            return Err("the compositor does not offer blur".into());
        }
        Ok(Protocol {
            connection,
            _queue: queue,
            _registry: registry,
            qh,
            manager,
            compositor,
            surface: None,
            wl_surface: None,
            effect: None,
        })
    }
}

impl Inner {
    fn fallback(&self, reason: &str) {
        if !self.logged.replace(true) {
            crate::log!("blur", "{reason}; keeping the window transparent");
        }
    }

    fn realize(self: &Rc<Self>) {
        if self.protocol.borrow().as_ref().is_none() {
            return;
        }
        let Some(window) = self.window.upgrade() else {
            return;
        };
        let Some(surface) = window.surface() else {
            self.fallback("the window has no GDK surface");
            return;
        };
        let Some(wl_surface) = surface
            .clone()
            .downcast::<gdk4_wayland::WaylandSurface>()
            .ok()
            .and_then(|wayland| wayland.wl_surface())
        else {
            self.fallback("the toplevel surface is not a Wayland surface");
            return;
        };
        {
            let mut borrowed = self.protocol.borrow_mut();
            let Some(protocol) = borrowed.as_mut() else {
                return;
            };
            if protocol.effect.is_some() {
                return;
            }
            protocol.effect = Some(protocol.manager.get_background_effect(
                &wl_surface,
                &protocol.qh,
                (),
            ));
            protocol.wl_surface = Some(wl_surface);
            protocol.surface = Some(surface.clone());
        }
        self.layout.set(Some((surface.width(), surface.height())));
        let weak = Rc::downgrade(self);
        surface.connect_layout(move |_, width, height| {
            if let Some(inner) = weak.upgrade()
                && inner.layout.replace(Some((width, height))) != Some((width, height))
            {
                crate::log!("blur", "surface layout changed to {width}×{height}");
                inner.update();
            }
        });
        let weak = Rc::downgrade(self);
        surface.connect_notify_local(Some("scale-factor"), move |surface, _| {
            if let Some(inner) = weak.upgrade() {
                crate::log!("blur", "surface scale-factor {}", surface.scale_factor());
                inner.update();
            }
        });
        self.update();
    }

    fn update(&self) {
        let Some(window) = self.window.upgrade() else {
            return;
        };
        let Some(protocol) = self.protocol.borrow_mut().take() else {
            return;
        };
        let (Some(effect), Some(surface)) = (protocol.effect.as_ref(), protocol.surface.as_ref())
        else {
            *self.protocol.borrow_mut() = Some(protocol);
            return;
        };
        let opacity = self.opacity.get();
        if opacity >= 0.9995 {
            effect.set_blur_region(None);
            protocol.connection.flush().ok();
            surface.queue_render();
            crate::log!("blur", "opacity {opacity:.2}: blur region removed");
            *self.protocol.borrow_mut() = Some(protocol);
            return;
        }
        let Some((x, y, w, h)) = visible_geometry(&window) else {
            // The widget may not be allocated yet: let the next layout
            // signal retry even if the surface size did not change.
            self.layout.set(None);
            *self.protocol.borrow_mut() = Some(protocol);
            return;
        };
        let radius = (RADIUS * self.scale.get()).round() as i32;
        let rects = rounded_region_rects(w, h, radius, CORNER_STRIPS);
        let placed: Vec<Rect> = rects
            .iter()
            .map(|rect| Rect {
                x: rect.x + x,
                y: rect.y + y,
                ..*rect
            })
            .collect();
        let region = protocol.compositor.create_region(&protocol.qh, ());
        for rect in &placed {
            region.add(rect.x, rect.y, rect.w, rect.h);
        }
        effect.set_blur_region(Some(&region));
        region.destroy();
        protocol.connection.flush().ok();
        surface.queue_render();
        crate::log!(
            "blur",
            "opacity {opacity:.2}: visible {w}×{h} at ({x}, {y}), radius {radius}, {} rects: {placed:?}",
            placed.len()
        );
        *self.protocol.borrow_mut() = Some(protocol);
    }
}

/// The window's visible rectangle in surface coordinates: the GTK window's
/// widget rect, moved by the surface transform. The surface buffer also holds
/// GTK's client-side shadow; `gtk_window_native_get_surface_transform`
/// returns the shadow's top/left width minus the widget's margin origin
/// (`gtk-4.22.4:gtk/gtkwindow.c:2274-2287`), i.e. the widget's offset inside
/// the surface, so the shadow stays outside the region.
fn visible_geometry(window: &gtk::Window) -> Option<(i32, i32, i32, i32)> {
    let surface = window.surface()?;
    let (tx, ty) = window.surface_transform();
    let x = tx.round() as i32;
    let y = ty.round() as i32;
    let w = window.width().min(surface.width() - x);
    let h = window.height().min(surface.height() - y);
    if w <= 0 || h <= 0 || x < 0 || y < 0 {
        return None;
    }
    Some((x, y, w, h))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area(rects: &[Rect]) -> i64 {
        rects.iter().map(|r| i64::from(r.w) * i64::from(r.h)).sum()
    }

    #[test]
    fn region_rects_stay_inside_and_cut_the_corners() {
        let (w, h) = (2560, 1400);
        let rects = rounded_region_rects(w, h, 12, CORNER_STRIPS);
        assert_eq!(rects.len(), 2 * CORNER_STRIPS as usize + 1);
        for r in &rects {
            assert!(r.w > 0 && r.h > 0, "{r:?}");
            assert!(r.x >= 0 && r.y >= 0, "{r:?}");
            assert!(r.x + r.w <= w && r.y + r.h <= h, "{r:?}");
        }
        let covered = area(&rects);
        let whole = i64::from(w) * i64::from(h);
        assert!(
            covered < whole,
            "a rounded rectangle must cut the four corners"
        );
        assert!(
            covered > whole * 99 / 100,
            "corner strips should lose well under 1 %: {covered} of {whole}"
        );
        assert!(
            rects.iter().all(|r| !(r.x == 0 && r.y == 0)),
            "the top-left pixel must be outside the region"
        );
        assert!(
            rects
                .iter()
                .any(|r| r.x <= w / 2 && r.x + r.w >= w / 2 && r.y <= h / 2 && r.y + r.h >= h / 2),
            "the centre must be covered"
        );
    }

    #[test]
    fn region_rects_at_half_scale() {
        let (w, h) = (1600, 1000);
        let rects = rounded_region_rects(w, h, 6, CORNER_STRIPS);
        assert_eq!(rects.len(), 2 * CORNER_STRIPS as usize + 1);
        for r in &rects {
            assert!(r.x >= 0 && r.y >= 0, "{r:?}");
            assert!(r.x + r.w <= w && r.y + r.h <= h, "{r:?}");
        }
        assert!(area(&rects) < i64::from(w) * i64::from(h));
    }

    #[test]
    fn region_rects_degenerate_sizes() {
        assert!(rounded_region_rects(0, 100, 12, 4).is_empty());
        assert!(rounded_region_rects(100, 0, 12, 4).is_empty());
        assert_eq!(
            rounded_region_rects(100, 50, 0, 4),
            vec![Rect {
                x: 0,
                y: 0,
                w: 100,
                h: 50
            }]
        );
        // The radius is clamped to half the shorter side; one strip per
        // corner then leaves no body, only the two stair steps.
        let rects = rounded_region_rects(40, 20, 999, 1);
        for r in &rects {
            assert!(r.x + r.w <= 40 && r.y + r.h <= 20, "{r:?}");
        }
        assert_eq!(rects.len(), 2);
    }
}
