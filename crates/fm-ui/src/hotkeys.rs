// SPDX-License-Identifier: GPL-3.0-or-later
//! P2as: the Keyboard Shortcuts dialog.
//!
//! The dialog lists every command a hotkey can run, shows its current keys in
//! libadwaita's `ShortcutLabel`, and adds, removes, replaces and resets them.
//! A change is one of fm-core's `hotkey.*` commands; the settings write goes
//! through `ConfigFile::apply`, and `Process::after_change` reinstalls every
//! window's controller.
//!
//! The capture rule is a pure function over a GDK keyval and modifier state,
//! so it is tested without a display. GTK only supplies the event.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use adw::prelude::*;
use fm_core::command::{Caller, Invocation, Outcome};
use fm_core::config::{Hotkey, default_hotkeys};
use gtk::{gdk, glib};

use crate::frame::WindowUi;
use crate::look::tokens::{GAP_M, GAP_S, GAP_XL, PREVIEW_W, SEARCH_W, scaled};

const HINT: &str = "A shortcut your desktop already uses (Mango, niri…) never reaches \
                    Gimbal. Pick another one.";

const SAVE_ERROR: &str = "Can't save: config.toml has an error";

const PRINTABLE_REFUSAL: &str = "Add Ctrl, Alt or Super to a letter or number.";
const TAB_REFUSAL: &str = "Tab and Return need Ctrl, Alt or Super.";
const BARE_REFUSAL: &str = "Use a function, arrow, editing or media key for a bare shortcut.";

/// What one key press means while a row is capturing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Capture {
    /// A lone modifier: keep waiting for a real key.
    Ignore,
    /// Escape or Backspace ends capture without a change.
    Cancel,
    /// A printable key with no Ctrl, Alt or Super would break typing.
    Refused(&'static str),
    /// A usable accelerator, already normalized with the lowercase keyval and
    /// the masked modifier state.
    Allowed {
        key: gdk::Key,
        modifiers: gdk::ModifierType,
    },
}

/// The capture rule. `default_mask` is `gtk::accelerator_get_default_mod_mask`
/// from the UI; the tests pass the same four modifiers directly, so the rule
/// stays pure. Shift is always kept, even if the caller's mask omits it.
pub fn capture(
    key: gdk::Key,
    modifiers: gdk::ModifierType,
    default_mask: gdk::ModifierType,
) -> Capture {
    if key == gdk::Key::Escape || key == gdk::Key::BackSpace {
        return Capture::Cancel;
    }
    if is_lone_modifier(key) {
        return Capture::Ignore;
    }

    let effective = (modifiers & default_mask) | (modifiers & gdk::ModifierType::SHIFT_MASK);
    let command_modifier = effective.intersects(
        gdk::ModifierType::CONTROL_MASK
            | gdk::ModifierType::ALT_MASK
            | gdk::ModifierType::SUPER_MASK,
    );

    if !command_modifier {
        if matches!(key, gdk::Key::Tab | gdk::Key::Return | gdk::Key::KP_Enter) {
            return Capture::Refused(TAB_REFUSAL);
        }
        if key.to_unicode().is_some_and(|c| !c.is_control()) {
            return Capture::Refused(PRINTABLE_REFUSAL);
        }
        if !bare_allowed(key) {
            return Capture::Refused(BARE_REFUSAL);
        }
    }

    Capture::Allowed {
        key: key.to_lower(),
        modifiers: effective,
    }
}

fn is_lone_modifier(key: gdk::Key) -> bool {
    matches!(
        key,
        gdk::Key::Shift_L
            | gdk::Key::Shift_R
            | gdk::Key::Control_L
            | gdk::Key::Control_R
            | gdk::Key::Alt_L
            | gdk::Key::Alt_R
            | gdk::Key::Super_L
            | gdk::Key::Super_R
            | gdk::Key::Hyper_L
            | gdk::Key::Hyper_R
            | gdk::Key::Meta_L
            | gdk::Key::Meta_R
            | gdk::Key::ISO_Level3_Shift
            | gdk::Key::ISO_Level5_Shift
            | gdk::Key::Caps_Lock
            | gdk::Key::Shift_Lock
            | gdk::Key::Num_Lock
    )
}

/// The keys a shortcut may use with no Ctrl, Alt or Super: the function keys,
/// the editing and arrow keys, and the media keys.
fn bare_allowed(key: gdk::Key) -> bool {
    if (gdk::Key::F1..=gdk::Key::F35).contains(&key) {
        return true;
    }
    matches!(
        key,
        gdk::Key::Delete
            | gdk::Key::Insert
            | gdk::Key::Home
            | gdk::Key::End
            | gdk::Key::Page_Up
            | gdk::Key::Page_Down
            | gdk::Key::Left
            | gdk::Key::Right
            | gdk::Key::Up
            | gdk::Key::Down
            | gdk::Key::AudioPlay
            | gdk::Key::AudioPause
            | gdk::Key::AudioStop
            | gdk::Key::AudioPrev
            | gdk::Key::AudioNext
            | gdk::Key::AudioRewind
            | gdk::Key::AudioForward
            | gdk::Key::AudioRepeat
            | gdk::Key::AudioRandomPlay
            | gdk::Key::AudioMedia
            | gdk::Key::AudioMute
            | gdk::Key::AudioRaiseVolume
            | gdk::Key::AudioLowerVolume
            | gdk::Key::AudioMicMute
            | gdk::Key::MonBrightnessUp
            | gdk::Key::MonBrightnessDown
            | gdk::Key::Eject
            | gdk::Key::Sleep
            | gdk::Key::Suspend
            | gdk::Key::WakeUp
            | gdk::Key::ScreenSaver
            | gdk::Key::Battery
            | gdk::Key::Bluetooth
            | gdk::Key::WLAN
            | gdk::Key::WWAN
            | gdk::Key::RFKill
    )
}

/// The commands the dialog lists: fm-core's one bindable rule.
pub fn bindable_commands() -> Vec<&'static str> {
    fm_core::command::bindable_commands()
}

/// The readable name of every bindable command. One table, so a row can never
/// show a raw command name. The test below keeps it exactly in step with
/// [`bindable_commands`].
pub const NAMES: &[(&str, &str)] = &[
    ("app.quit", "Quit Gimbal Files"),
    ("app.disk-usage", "Show disk usage"),
    ("app.settings", "Open Settings"),
    ("window.new", "New window"),
    ("window.close", "Close window"),
    ("tab.new", "New tab"),
    ("tab.close", "Close tab"),
    ("tab.close-selected", "Close selected tabs"),
    ("tab.open-in-new-window", "Open in a new window"),
    ("tab.open-in-own-tab", "Open in its own tab"),
    ("group.split", "Split the view"),
    ("group.close", "Close the split group"),
    ("nav.back", "Back"),
    ("nav.forward", "Forward"),
    ("nav.up", "Go up a folder"),
    ("nav.refresh", "Refresh"),
    ("nav.focus-path", "Focus the path bar"),
    ("search.open", "Search files"),
    ("selection.all", "Select all"),
    ("selection.none", "Select none"),
    ("file.open", "Open selected files"),
    ("file.properties", "Show properties"),
    ("file.trash", "Move to Trash"),
    ("file.undo", "Undo"),
    ("file.cut", "Cut"),
    ("file.copy-to-clipboard", "Copy"),
    ("file.paste", "Paste"),
    ("trash.open", "Open the Trash"),
    ("preview.edit", "Edit in the preview"),
    ("preview.discard", "Discard preview edits"),
    ("preview.save", "Save preview edits"),
    ("view.zoom-in", "Zoom in"),
    ("view.zoom-out", "Zoom out"),
    (
        "view.clear-folder-sort",
        "Use the global sort for this folder",
    ),
    ("view.toggle-hidden", "Show or hide hidden files"),
    ("pins.add", "Pin this folder"),
];

pub fn name_for(command: &str) -> Option<&'static str> {
    NAMES
        .iter()
        .find(|(name, _)| *name == command)
        .map(|(_, label)| *label)
}

/// A readable name for a command that may be outside the editor's list, used
/// by the conflict sentence. A hand-written binding can name a command with
/// required arguments; its catalogue summary reads better than the raw name.
fn display_command(command: &str) -> String {
    if let Some(name) = name_for(command) {
        return name.to_string();
    }
    fm_core::command::Registry::new()
        .command(command)
        .map(|spec| spec.summary.trim_end_matches('.').to_string())
        .unwrap_or_else(|| command.to_string())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Group {
    Navigation,
    TabsAndWindows,
    Files,
    Selection,
    View,
    Search,
    Preview,
    App,
}

impl Group {
    pub const ALL: [Group; 8] = [
        Group::Navigation,
        Group::TabsAndWindows,
        Group::Files,
        Group::Selection,
        Group::View,
        Group::Search,
        Group::Preview,
        Group::App,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Self::Navigation => "Navigation",
            Self::TabsAndWindows => "Tabs and windows",
            Self::Files => "Files",
            Self::Selection => "Selection",
            Self::View => "View",
            Self::Search => "Search",
            Self::Preview => "Preview",
            Self::App => "App",
        }
    }
}

/// The area a command belongs to, from its catalogue prefix.
pub fn group_for(command: &str) -> Group {
    let area = command.split_once('.').map(|(area, _)| area).unwrap_or("");
    match area {
        "nav" => Group::Navigation,
        "tab" | "group" | "window" => Group::TabsAndWindows,
        "file" | "trash" => Group::Files,
        "selection" => Group::Selection,
        "view" => Group::View,
        "search" => Group::Search,
        "preview" => Group::Preview,
        _ => Group::App,
    }
}

/// Whether a search query matches a row by its name or by one of its keys.
pub fn matches(query: &str, name: &str, accelerators: &[&str]) -> bool {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return true;
    }
    name.to_lowercase().contains(&query)
        || accelerators
            .iter()
            .any(|accelerator| key_text(accelerator).contains(&query))
}

/// Accelerators as a person says them: `<Control>t` also matches "ctrl".
fn key_text(accelerator: &str) -> String {
    accelerator
        .to_lowercase()
        .replace("<control>", "ctrl ")
        .replace("<primary>", "ctrl ")
        .replace("<alt>", "alt ")
        .replace("<shift>", "shift ")
        .replace("<super>", "super ")
        .replace("<hyper>", "hyper ")
        .replace("<meta>", "meta ")
}

/// The keys currently bound to a command, in the config's sorted order, each
/// with whether the entry carries arguments (a hand-written binding).
pub fn shortcuts_for(hotkeys: &BTreeMap<String, Hotkey>, command: &str) -> Vec<(String, bool)> {
    hotkeys
        .iter()
        .filter(|(_, hotkey)| hotkey.command == command)
        .map(|(accelerator, hotkey)| (accelerator.clone(), !hotkey.arguments.is_empty()))
        .collect()
}

/// Whether the row differs from the defaults, so Reset is worth showing.
pub fn differs_from_defaults(hotkeys: &BTreeMap<String, Hotkey>, command: &str) -> bool {
    let now = shortcuts_for(hotkeys, command);
    let defaults = default_hotkeys();
    let expected = shortcuts_for(&defaults, command);

    now.len() != expected.len()
        || now
            .iter()
            .zip(expected.iter())
            .any(|((current, _), (default, _))| current != default)
        || now.iter().any(|(_, arguments)| *arguments)
}

// -------------------------------------------------------------------------
// The dialog
// -------------------------------------------------------------------------

#[derive(Debug, Clone)]
enum Notice {
    Refused {
        command: String,
        text: String,
    },
    Conflict {
        command: String,
        accelerator: String,
        other: String,
    },
    Save {
        command: Option<String>,
        text: String,
    },
}

pub(crate) struct Editor {
    ui: Rc<WindowUi>,
    scale: f64,
    list: gtk::Box,
    scroller: gtk::ScrolledWindow,
    search: gtk::SearchEntry,
    global: gtk::Label,
    capturing: RefCell<Option<String>>,
    notice: RefCell<Option<Notice>>,
}

/// Open the dialog on this window. It counts as a dialog, so the window's
/// hotkeys stay blocked while a key is being captured.
pub fn show(ui: &Rc<WindowUi>) {
    open(ui);
}

/// Build and present the dialog, handing back the editor and the dialog so a
/// widget test can drive the same buttons and key handler a person does.
fn open(ui: &Rc<WindowUi>) -> (Rc<Editor>, adw::Dialog) {
    let scale = ui
        .process_upgrade()
        .map(|process| process.scale())
        .unwrap_or(1.0);
    let dialog = adw::Dialog::new();
    dialog.add_css_class("gimbal-look");
    dialog.set_title("Keyboard Shortcuts");
    dialog.set_content_width(scaled(SEARCH_W + GAP_XL, scale));
    dialog.set_content_height(scaled(PREVIEW_W, scale));

    let toolbar = adw::ToolbarView::new();
    let header = adw::HeaderBar::new();
    let reset_all = gtk::Button::with_label("Reset All");
    reset_all.set_tooltip_text(Some("Restore every bindable command's default shortcuts"));
    reset_all.add_css_class("destructive-action");
    header.pack_start(&reset_all);
    toolbar.add_top_bar(&header);

    let body = gtk::Box::new(gtk::Orientation::Vertical, scaled(GAP_S, scale));
    let margin = scaled(GAP_M, scale);
    body.set_margin_top(margin);
    body.set_margin_bottom(margin);
    body.set_margin_start(margin);
    body.set_margin_end(margin);

    let hint = gtk::Label::builder()
        .label(HINT)
        .wrap(true)
        .xalign(0.0)
        .halign(gtk::Align::Start)
        .build();
    hint.add_css_class("dim-label");
    body.append(&hint);

    let search = gtk::SearchEntry::new();
    search.set_placeholder_text(Some("Search by name or key"));
    body.append(&search);

    let global = gtk::Label::builder()
        .wrap(true)
        .xalign(0.0)
        .halign(gtk::Align::Start)
        .build();
    global.add_css_class("error");
    global.set_visible(false);
    body.append(&global);

    let list = gtk::Box::new(gtk::Orientation::Vertical, scaled(GAP_S, scale));
    let scroller = gtk::ScrolledWindow::new();
    scroller.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    scroller.set_vexpand(true);
    scroller.set_child(Some(&list));
    body.append(&scroller);

    toolbar.set_content(Some(&body));
    dialog.set_child(Some(&toolbar));

    let editor = Rc::new(Editor {
        ui: ui.clone(),
        scale,
        list,
        scroller: scroller.clone(),
        search,
        global,
        capturing: RefCell::new(None),
        notice: RefCell::new(None),
    });

    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    let weak = Rc::downgrade(&editor);
    keys.connect_key_pressed(move |_, key, _, state| match weak.upgrade() {
        Some(editor) => editor.key_pressed(key, state),
        None => glib::Propagation::Proceed,
    });
    dialog.add_controller(keys);

    {
        let search = editor.search.clone();
        let editor = editor.clone();
        search.connect_search_changed(move |_| editor.rebuild());
    }
    {
        let editor = editor.clone();
        reset_all.connect_clicked(move |_| editor.confirm_reset_all());
    }

    editor.rebuild();
    ui.begin_dialog();
    let ui_closed = ui.clone();
    dialog.connect_closed(move |_| ui_closed.end_dialog());
    let parent: gtk::Widget = ui.window().clone().upcast();
    dialog.present(Some(&parent));
    (editor, dialog)
}

/// Test hooks for the real-GTK editor regression in `app.rs`. Key presses go
/// through the same `key_pressed` the dialog's capture controller calls; the
/// buttons are found in the real row widgets and clicked with `emit_clicked`.
/// No desktop input is injected.
#[cfg(test)]
pub(crate) mod testing {
    use super::*;

    pub(crate) struct Driver {
        pub editor: Rc<Editor>,
        pub dialog: adw::Dialog,
    }

    pub(crate) fn open(ui: &Rc<WindowUi>) -> Driver {
        let (editor, dialog) = super::open(ui);
        Driver { editor, dialog }
    }

    impl Driver {
        pub(crate) fn press(&self, key: gdk::Key, modifiers: gdk::ModifierType) -> bool {
            self.editor.key_pressed(key, modifiers) == glib::Propagation::Stop
        }

        pub(crate) fn search(&self, text: &str) {
            self.editor.search.set_text(text);
            self.editor.rebuild();
        }

        pub(crate) fn reset_all_confirmed(&self) {
            self.editor.reset_all();
        }

        /// The row for a readable command name: its texts and its buttons.
        fn row(&self, name: &str) -> gtk::Widget {
            let mut child = self.editor.list.first_child();
            while let Some(widget) = child {
                if let Some(line) = widget.first_child()
                    && line
                        .first_child()
                        .and_downcast::<gtk::Label>()
                        .is_some_and(|label| label.text() == name)
                {
                    return widget;
                }
                child = widget.next_sibling();
            }
            panic!("no row named {name:?}");
        }

        /// Every label and shortcut text in a row, depth first.
        pub(crate) fn row_texts(&self, name: &str) -> Vec<String> {
            let mut texts = Vec::new();
            collect_texts(&self.row(name), &mut texts);
            texts
        }

        /// Click the row's button with this label, or (for `None`) the remove
        /// button of the chip showing `accelerator`.
        pub(crate) fn click(&self, name: &str, label: &str) {
            let button = find_button(&self.row(name), &|button| {
                button.label().is_some_and(|text| text == label)
            })
            .unwrap_or_else(|| panic!("row {name:?} has no {label:?} button"));
            button.emit_clicked();
        }

        pub(crate) fn remove_chip(&self, name: &str, accelerator: &str) {
            let row = self.row(name);
            let mut found = None;
            visit(&row, &mut |widget| {
                if found.is_none()
                    && let Some(label) = widget.downcast_ref::<adw::ShortcutLabel>()
                    && label.accelerator() == accelerator
                {
                    found = widget.next_sibling().and_downcast::<gtk::Button>();
                }
            });
            found
                .unwrap_or_else(|| panic!("row {name:?} has no {accelerator:?} chip"))
                .emit_clicked();
        }

        /// Render the dialog as it is drawn now into a PNG, when
        /// `GIMBAL_TEST_SNAPSHOTS` names a folder. A reviewer's eyes, not an
        /// assertion: the widget regression passes either way.
        pub(crate) fn snapshot(&self, name: &str) {
            let Some(folder) = std::env::var_os("GIMBAL_TEST_SNAPSHOTS") else {
                return;
            };
            let widget: gtk::Widget = self.dialog.clone().upcast();
            let (width, height) = (widget.width(), widget.height());
            if width <= 0 || height <= 0 {
                return;
            }
            // GTK paints on the compositor's next frame, so a node exists
            // only once a frame has drawn the dialog. Let the open fade and
            // any scroll animation finish first (about half a second), then
            // wait up to two seconds more for a painted frame.
            let context = glib::MainContext::default();
            for _ in 0..50 {
                context.iteration(false);
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            widget.queue_draw();
            let paintable = gtk::WidgetPaintable::new(Some(&widget));
            let mut node = None;
            for _ in 0..200 {
                context.iteration(false);
                let snapshot = gtk::Snapshot::new();
                paintable.snapshot(&snapshot, f64::from(width), f64::from(height));
                node = snapshot.to_node();
                if node.is_some() {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            let (Some(node), Some(renderer)) =
                (node, widget.native().and_then(|native| native.renderer()))
            else {
                return;
            };
            let texture = renderer.render_texture(&node, None);
            let path = std::path::Path::new(&folder).join(format!("{name}.png"));
            let _ = texture.save_to_png(&path);
        }

        pub(crate) fn global_text(&self) -> Option<String> {
            self.editor
                .global
                .is_visible()
                .then(|| self.editor.global.text().to_string())
        }

        pub(crate) fn has_row(&self, name: &str) -> bool {
            let mut child = self.editor.list.first_child();
            while let Some(widget) = child {
                if widget
                    .first_child()
                    .and_then(|line| line.first_child())
                    .and_downcast::<gtk::Label>()
                    .is_some_and(|label| label.text() == name)
                {
                    return true;
                }
                child = widget.next_sibling();
            }
            false
        }
    }

    fn visit(widget: &gtk::Widget, f: &mut dyn FnMut(&gtk::Widget)) {
        f(widget);
        let mut child = widget.first_child();
        while let Some(next) = child {
            visit(&next, f);
            child = next.next_sibling();
        }
    }

    fn collect_texts(widget: &gtk::Widget, texts: &mut Vec<String>) {
        visit(widget, &mut |widget| {
            if !widget.is_visible() {
                return;
            }
            if let Some(label) = widget.downcast_ref::<gtk::Label>() {
                texts.push(label.text().to_string());
            } else if let Some(label) = widget.downcast_ref::<adw::ShortcutLabel>() {
                texts.push(label.accelerator().to_string());
            }
        });
    }

    fn find_button(
        widget: &gtk::Widget,
        test: &dyn Fn(&gtk::Button) -> bool,
    ) -> Option<gtk::Button> {
        let mut found = None;
        visit(widget, &mut |widget| {
            if found.is_none()
                && let Some(button) = widget.downcast_ref::<gtk::Button>()
                && test(button)
            {
                found = Some(button.clone());
            }
        });
        found
    }
}

impl Editor {
    fn key_pressed(self: &Rc<Self>, key: gdk::Key, state: gdk::ModifierType) -> glib::Propagation {
        let Some(command) = self.capturing.borrow().clone() else {
            return glib::Propagation::Proceed;
        };

        match capture(key, state, gtk::accelerator_get_default_mod_mask()) {
            Capture::Ignore => glib::Propagation::Stop,
            Capture::Cancel => {
                self.cancel_capture();
                glib::Propagation::Stop
            }
            Capture::Refused(text) => {
                *self.notice.borrow_mut() = Some(Notice::Refused {
                    command,
                    text: text.to_string(),
                });
                self.rebuild();
                glib::Propagation::Stop
            }
            Capture::Allowed { key, modifiers } => {
                let accelerator = gtk::accelerator_name(key, modifiers).to_string();
                *self.capturing.borrow_mut() = None;
                self.finish_capture(&command, accelerator);
                glib::Propagation::Stop
            }
        }
    }

    fn finish_capture(self: &Rc<Self>, command: &str, accelerator: String) {
        let hotkeys = self
            .ui
            .process_upgrade()
            .map(|process| process.hotkeys())
            .unwrap_or_default();
        // Compare keys, not spellings: a hand-written `<Ctrl><Shift>b` is the
        // same key as the captured `<Shift><Control>b`. Reusing the existing
        // spelling keeps one entry per key in the file.
        let wanted = gtk::accelerator_parse(accelerator.as_str());
        let existing = hotkeys.iter().find(|(bound, _)| {
            *bound == &accelerator
                || (wanted.is_some() && gtk::accelerator_parse(bound.as_str()) == wanted)
        });
        let Some((bound, hotkey)) = existing else {
            self.bind(command, &accelerator);
            return;
        };
        if hotkey.command == command {
            // Already this command's key: nothing to save.
            *self.notice.borrow_mut() = None;
            self.rebuild();
            return;
        }
        *self.notice.borrow_mut() = Some(Notice::Conflict {
            command: command.to_string(),
            accelerator: bound.clone(),
            other: hotkey.command.clone(),
        });
        self.rebuild();
    }

    fn begin_capture(self: &Rc<Self>, command: &'static str) {
        *self.capturing.borrow_mut() = Some(command.to_string());
        *self.notice.borrow_mut() = None;
        self.rebuild();
    }

    fn cancel_capture(self: &Rc<Self>) {
        *self.capturing.borrow_mut() = None;
        *self.notice.borrow_mut() = None;
        self.rebuild();
    }

    fn cancel_notice(self: &Rc<Self>) {
        *self.notice.borrow_mut() = None;
        self.rebuild();
    }

    fn bind(self: &Rc<Self>, command: &str, accelerator: &str) {
        if !self.writable(command) {
            return;
        }
        let invocation = Invocation::new(
            "hotkey.set",
            vec![
                ("accelerator".to_string(), accelerator.to_string()),
                ("command".to_string(), command.to_string()),
            ],
        );
        self.dispatch(command, invocation);
    }

    fn remove(self: &Rc<Self>, command: &str, accelerator: &str) {
        if !self.writable(command) {
            return;
        }
        let invocation = Invocation::new(
            "hotkey.remove",
            vec![("accelerator".to_string(), accelerator.to_string())],
        );
        self.dispatch(command, invocation);
    }

    fn reset(self: &Rc<Self>, command: &str) {
        if !self.writable(command) {
            return;
        }
        let invocation = Invocation::new(
            "hotkey.reset",
            vec![("command".to_string(), command.to_string())],
        );
        self.dispatch(command, invocation);
    }

    fn reset_all(self: &Rc<Self>) {
        if !self.writable_global() {
            return;
        }
        self.dispatch_global(Invocation::new("hotkey.reset", Vec::new()));
    }

    fn confirm_reset_all(self: &Rc<Self>) {
        let editor = self.clone();
        crate::fileops::dialogs::confirm_admin(
            &self.ui,
            "Reset every shortcut?",
            "Every bindable command goes back to its default keys. Hand-written entries for \
             commands this editor does not list stay as they are.",
            "Reset All",
            Rc::new(move |confirmed| {
                if confirmed {
                    editor.reset_all();
                }
            }),
        );
    }

    fn writable(self: &Rc<Self>, command: &str) -> bool {
        if self
            .ui
            .process_upgrade()
            .is_some_and(|process| process.config_writable())
        {
            return true;
        }
        *self.notice.borrow_mut() = Some(Notice::Save {
            command: Some(command.to_string()),
            text: SAVE_ERROR.to_string(),
        });
        self.rebuild();
        false
    }

    fn writable_global(self: &Rc<Self>) -> bool {
        if self
            .ui
            .process_upgrade()
            .is_some_and(|process| process.config_writable())
        {
            return true;
        }
        *self.notice.borrow_mut() = Some(Notice::Save {
            command: None,
            text: SAVE_ERROR.to_string(),
        });
        self.rebuild();
        false
    }

    fn dispatch(self: &Rc<Self>, command: &str, invocation: Invocation) {
        let Some(process) = self.ui.process_upgrade() else {
            return;
        };
        let outcome = process.dispatch_with_outcome(self.ui.id, Caller::Ui, &invocation);
        match outcome {
            Outcome::Done => *self.notice.borrow_mut() = None,
            Outcome::Failed(message) => {
                *self.notice.borrow_mut() = Some(Notice::Save {
                    command: Some(command.to_string()),
                    text: save_message(&message),
                });
            }
            Outcome::Error(error) => {
                *self.notice.borrow_mut() = Some(Notice::Save {
                    command: Some(command.to_string()),
                    text: save_message(&error.to_string()),
                });
            }
            Outcome::DryRun(_) => {}
        }
        self.rebuild();
    }

    fn dispatch_global(self: &Rc<Self>, invocation: Invocation) {
        let Some(process) = self.ui.process_upgrade() else {
            return;
        };
        let outcome = process.dispatch_with_outcome(self.ui.id, Caller::Ui, &invocation);
        match outcome {
            Outcome::Done => *self.notice.borrow_mut() = None,
            Outcome::Failed(message) => {
                *self.notice.borrow_mut() = Some(Notice::Save {
                    command: None,
                    text: save_message(&message),
                });
            }
            Outcome::Error(error) => {
                *self.notice.borrow_mut() = Some(Notice::Save {
                    command: None,
                    text: save_message(&error.to_string()),
                });
            }
            Outcome::DryRun(_) => {}
        }
        self.rebuild();
    }

    fn rebuild(self: &Rc<Self>) {
        while let Some(child) = self.list.first_child() {
            self.list.remove(&child);
        }

        let Some(process) = self.ui.process_upgrade() else {
            return;
        };
        let hotkeys = process.hotkeys();
        let query = self.search.text().to_string();
        let notice = self.notice.borrow().clone();

        let global_text = match &notice {
            Some(Notice::Save {
                command: None,
                text,
            }) => text.clone(),
            _ => String::new(),
        };
        self.global.set_text(&global_text);
        self.global.set_visible(!global_text.is_empty());

        // The row a person is working on: the one capturing a key, or the
        // one showing a refusal, conflict or save message.
        let active = self.capturing.borrow().clone().or_else(|| match &notice {
            Some(Notice::Refused { command, .. }) | Some(Notice::Conflict { command, .. }) => {
                Some(command.clone())
            }
            Some(Notice::Save {
                command: Some(command),
                ..
            }) => Some(command.clone()),
            _ => None,
        });
        let mut reveal: Option<gtk::Widget> = None;
        let mut rows = 0;
        let bindable = bindable_commands();
        for group in Group::ALL {
            let mut commands: Vec<&'static str> = bindable
                .iter()
                .copied()
                .filter(|command| group_for(command) == group)
                .filter(|command| {
                    let name = name_for(command).unwrap_or(command);
                    let accelerators: Vec<String> = shortcuts_for(&hotkeys, command)
                        .into_iter()
                        .map(|(accelerator, _)| accelerator)
                        .collect();
                    let borrowed: Vec<&str> = accelerators.iter().map(String::as_str).collect();
                    matches(&query, name, &borrowed)
                })
                .collect();
            if commands.is_empty() {
                continue;
            }
            commands.sort_by_key(|command| name_for(command).unwrap_or(command).to_lowercase());
            rows += commands.len();

            let heading = gtk::Label::builder()
                .label(group.title())
                .halign(gtk::Align::Start)
                .xalign(0.0)
                .build();
            heading.add_css_class("heading");
            heading.set_margin_top(scaled(GAP_S, self.scale));
            self.list.append(&heading);
            for command in commands {
                let row = self.row(command, &hotkeys, notice.as_ref());
                self.list.append(&row);
                if Some(command) == active.as_deref() {
                    reveal = Some(row);
                }
            }
        }

        if let Some(row) = reveal {
            self.reveal(&row);
        }

        if rows == 0 {
            let empty = gtk::Label::builder()
                .label("No shortcut matches that search.")
                .halign(gtk::Align::Start)
                .xalign(0.0)
                .build();
            empty.add_css_class("dim-label");
            self.list.append(&empty);
        }
    }

    /// Scroll so the whole row is in view, its message line included. Focus
    /// alone scrolls only to the focused button, which left a refusal or a
    /// conflict's Replace button below the fold. A new row has no size until
    /// a frame lays it out, so this measures on the first tick after that
    /// (giving up after a few frames rather than scrolling to a guess).
    fn reveal(&self, row: &gtk::Widget) {
        let list = self.list.clone();
        let adjustment = self.scroller.vadjustment();
        let ticks = std::cell::Cell::new(0);
        row.add_tick_callback(move |row, _| {
            ticks.set(ticks.get() + 1);
            if row.width() <= 0 || row.height() <= 0 {
                return if ticks.get() < 10 {
                    glib::ControlFlow::Continue
                } else {
                    glib::ControlFlow::Break
                };
            }
            if let Some(bounds) = row.compute_bounds(&list) {
                let top = f64::from(bounds.y());
                let bottom = top + f64::from(bounds.height());
                // `clamp_page` would write the value under a running focus
                // scroll animation, which then carries on to its own target;
                // `set_value` ends that animation (gtkadjustment.c:538).
                let page = adjustment.page_size();
                let mut value = adjustment.value();
                if value + page < bottom {
                    value = bottom - page;
                }
                if value > top {
                    value = top;
                }
                adjustment.set_value(value);
            }
            glib::ControlFlow::Break
        });
    }

    fn row(
        self: &Rc<Self>,
        command: &'static str,
        hotkeys: &BTreeMap<String, Hotkey>,
        notice: Option<&Notice>,
    ) -> gtk::Widget {
        let outer = gtk::Box::new(gtk::Orientation::Vertical, scaled(GAP_S / 2.0, self.scale));
        let line = gtk::Box::new(gtk::Orientation::Horizontal, scaled(GAP_S, self.scale));
        line.set_margin_top(scaled(GAP_S / 4.0, self.scale));
        line.set_margin_bottom(scaled(GAP_S / 4.0, self.scale));

        let name = gtk::Label::builder()
            .label(name_for(command).unwrap_or(command))
            .halign(gtk::Align::Start)
            .xalign(0.0)
            .hexpand(true)
            .build();
        line.append(&name);

        let capturing = self.capturing.borrow().as_deref() == Some(command);
        if capturing {
            let prompt = gtk::Label::builder()
                .label("Press the new shortcut…")
                .halign(gtk::Align::Start)
                .xalign(0.0)
                .build();
            prompt.add_css_class("accent");
            line.append(&prompt);
            let cancel = gtk::Button::with_label("Cancel");
            cancel.set_valign(gtk::Align::Center);
            let editor = self.clone();
            cancel.connect_clicked(move |_| editor.cancel_capture());
            line.append(&cancel);
            outer.append(&line);
            outer.append(&self.notice_widget(command, notice));
            // The Add button is gone after this rebuild, so put the keyboard
            // back inside the dialog: the Capture key controller only sees
            // events that start in this widget tree.
            let focus = cancel;
            glib::idle_add_local_once(move || {
                focus.grab_focus();
            });
            return outer.upcast();
        }

        let shortcuts = shortcuts_for(hotkeys, command);
        let chips = gtk::Box::new(
            gtk::Orientation::Horizontal,
            scaled(GAP_S / 2.0, self.scale),
        );
        if shortcuts.is_empty() {
            let none = gtk::Label::builder()
                .label("Not set")
                .halign(gtk::Align::Start)
                .xalign(0.0)
                .build();
            none.add_css_class("dim-label");
            chips.append(&none);
        } else {
            for (accelerator, _) in &shortcuts {
                let chip = gtk::Box::new(
                    gtk::Orientation::Horizontal,
                    scaled(GAP_S / 4.0, self.scale),
                );
                chip.add_css_class("card");
                let label = adw::ShortcutLabel::new(accelerator);
                chip.append(&label);
                let remove = gtk::Button::from_icon_name("window-close-symbolic");
                remove.add_css_class("flat");
                remove.set_valign(gtk::Align::Center);
                remove.set_tooltip_text(Some("Remove this shortcut"));
                remove.update_property(&[gtk::accessible::Property::Label("Remove this shortcut")]);
                let editor = self.clone();
                let command = command.to_string();
                let accelerator = accelerator.clone();
                remove.connect_clicked(move |_| editor.remove(&command, &accelerator));
                chip.append(&remove);
                chips.append(&chip);
            }
        }
        line.append(&chips);

        let add = gtk::Button::with_label("Add");
        add.set_valign(gtk::Align::Center);
        add.set_tooltip_text(Some("Add a shortcut"));
        let editor = self.clone();
        add.connect_clicked(move |_| editor.begin_capture(command));
        line.append(&add);

        if differs_from_defaults(hotkeys, command) {
            let reset = gtk::Button::with_label("Reset");
            reset.set_valign(gtk::Align::Center);
            reset.set_tooltip_text(Some("Restore this command's default shortcuts"));
            let editor = self.clone();
            reset.connect_clicked(move |_| editor.reset(command));
            line.append(&reset);
        }

        outer.append(&line);
        outer.append(&self.notice_widget(command, notice));
        outer.upcast()
    }

    fn notice_widget(self: &Rc<Self>, command: &str, notice: Option<&Notice>) -> gtk::Widget {
        let Some(notice) = notice else {
            let empty = gtk::Box::new(gtk::Orientation::Horizontal, 0);
            empty.set_visible(false);
            return empty.upcast();
        };

        match notice {
            Notice::Refused {
                command: target,
                text,
            } if target == command => {
                let label = gtk::Label::builder()
                    .label(text)
                    .wrap(true)
                    .halign(gtk::Align::Start)
                    .xalign(0.0)
                    .build();
                label.add_css_class("error");
                label.upcast()
            }
            Notice::Conflict {
                command: target,
                accelerator,
                other,
            } if target == command => {
                let line = gtk::Box::new(gtk::Orientation::Horizontal, scaled(GAP_S, self.scale));
                let text = format!(
                    "{} is used by {}",
                    key_label(accelerator),
                    display_command(other)
                );
                let label = gtk::Label::builder()
                    .label(text)
                    .wrap(true)
                    .halign(gtk::Align::Start)
                    .xalign(0.0)
                    .hexpand(true)
                    .build();
                label.add_css_class("error");
                line.append(&label);

                let replace = gtk::Button::with_label("Replace");
                replace.add_css_class("destructive-action");
                replace.set_valign(gtk::Align::Center);
                line.append(&replace);
                let cancel = gtk::Button::with_label("Cancel");
                cancel.set_valign(gtk::Align::Center);
                line.append(&cancel);

                let editor_replace = self.clone();
                let command = command.to_string();
                let accelerator = accelerator.clone();
                replace.connect_clicked(move |_| {
                    editor_replace.bind(&command, &accelerator);
                });
                let editor_cancel = self.clone();
                cancel.connect_clicked(move |_| editor_cancel.cancel_notice());
                line.upcast()
            }
            Notice::Save {
                command: Some(target),
                text,
            } if target == command => {
                let label = gtk::Label::builder()
                    .label(text)
                    .wrap(true)
                    .halign(gtk::Align::Start)
                    .xalign(0.0)
                    .build();
                label.add_css_class("error");
                label.upcast()
            }
            _ => {
                let empty = gtk::Box::new(gtk::Orientation::Horizontal, 0);
                empty.set_visible(false);
                empty.upcast()
            }
        }
    }
}

fn save_message(message: &str) -> String {
    if message.contains("does not parse as TOML") {
        SAVE_ERROR.to_string()
    } else {
        format!("Can't save: {message}")
    }
}

/// An accelerator as a person reads it, for the conflict sentence.
fn key_label(accelerator: &str) -> String {
    gtk::accelerator_parse(accelerator)
        .map(|(key, modifiers)| gtk::accelerator_get_label(key, modifiers).to_string())
        .unwrap_or_else(|| accelerator.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn mask() -> gdk::ModifierType {
        gdk::ModifierType::CONTROL_MASK
            | gdk::ModifierType::ALT_MASK
            | gdk::ModifierType::SUPER_MASK
            | gdk::ModifierType::SHIFT_MASK
    }

    #[test]
    fn a_letter_needs_ctrl_alt_or_super() {
        assert_eq!(
            capture(gdk::Key::a, gdk::ModifierType::empty(), mask()),
            Capture::Refused(PRINTABLE_REFUSAL)
        );
        assert_eq!(
            capture(gdk::Key::a, gdk::ModifierType::SHIFT_MASK, mask()),
            Capture::Refused(PRINTABLE_REFUSAL)
        );
        assert!(matches!(
            capture(gdk::Key::a, gdk::ModifierType::CONTROL_MASK, mask()),
            Capture::Allowed { .. }
        ));
        assert!(matches!(
            capture(gdk::Key::a, gdk::ModifierType::SUPER_MASK, mask()),
            Capture::Allowed { .. }
        ));
    }

    #[test]
    fn shift_is_kept_and_the_keyval_is_lowercased() {
        let result = capture(
            gdk::Key::B,
            gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::SHIFT_MASK,
            mask(),
        );
        let Capture::Allowed { key, modifiers } = result else {
            panic!("Ctrl+Shift+B is a valid shortcut: {result:?}");
        };
        assert_eq!(key, gdk::Key::b);
        assert!(modifiers.contains(gdk::ModifierType::CONTROL_MASK));
        assert!(modifiers.contains(gdk::ModifierType::SHIFT_MASK));
    }

    #[test]
    fn escape_and_backspace_cancel_and_lone_modifiers_wait() {
        assert_eq!(
            capture(gdk::Key::Escape, gdk::ModifierType::empty(), mask()),
            Capture::Cancel
        );
        assert_eq!(
            capture(gdk::Key::BackSpace, gdk::ModifierType::CONTROL_MASK, mask()),
            Capture::Cancel
        );
        for key in [
            gdk::Key::Control_L,
            gdk::Key::Shift_R,
            gdk::Key::Alt_L,
            gdk::Key::Super_L,
            gdk::Key::Caps_Lock,
        ] {
            assert_eq!(
                capture(key, gdk::ModifierType::empty(), mask()),
                Capture::Ignore,
                "{key:?} is a lone modifier"
            );
        }
    }

    #[test]
    fn tab_and_return_need_a_command_modifier() {
        for key in [gdk::Key::Tab, gdk::Key::Return, gdk::Key::KP_Enter] {
            assert_eq!(
                capture(key, gdk::ModifierType::empty(), mask()),
                Capture::Refused(TAB_REFUSAL)
            );
            assert!(matches!(
                capture(key, gdk::ModifierType::ALT_MASK, mask()),
                Capture::Allowed { .. }
            ));
        }
    }

    #[test]
    fn bare_function_editing_arrow_and_media_keys_are_allowed() {
        for key in [
            gdk::Key::F6,
            gdk::Key::F35,
            gdk::Key::Delete,
            gdk::Key::Insert,
            gdk::Key::Home,
            gdk::Key::End,
            gdk::Key::Page_Up,
            gdk::Key::Page_Down,
            gdk::Key::Left,
            gdk::Key::Right,
            gdk::Key::Up,
            gdk::Key::Down,
            gdk::Key::AudioPlay,
            gdk::Key::AudioMute,
            gdk::Key::MonBrightnessUp,
        ] {
            assert!(
                matches!(
                    capture(key, gdk::ModifierType::empty(), mask()),
                    Capture::Allowed { .. }
                ),
                "{key:?} should be allowed with no modifier"
            );
        }
    }

    #[test]
    fn other_named_keys_need_a_modifier() {
        for key in [gdk::Key::Print, gdk::Key::Help, gdk::Key::Menu] {
            assert_eq!(
                capture(key, gdk::ModifierType::empty(), mask()),
                Capture::Refused(BARE_REFUSAL)
            );
            assert!(matches!(
                capture(key, gdk::ModifierType::CONTROL_MASK, mask()),
                Capture::Allowed { .. }
            ));
        }
    }

    #[test]
    fn the_search_matches_names_and_keys() {
        assert!(matches("", "Back", &["<Alt>Left"]));
        assert!(matches("back", "Back", &["<Alt>Left"]));
        assert!(matches("alt", "Back", &["<Alt>Left"]));
        assert!(matches("left", "Back", &["<Alt>Left"]));
        assert!(matches("ctrl", "New tab", &["<Control>t"]));
        assert!(!matches("paste", "Back", &["<Alt>Left"]));
    }

    #[test]
    fn every_bindable_command_has_exactly_one_name_and_no_extra_names() {
        let bindable: BTreeSet<&str> = bindable_commands().into_iter().collect();
        let named: BTreeSet<&str> = NAMES.iter().map(|(command, _)| *command).collect();
        assert_eq!(
            named, bindable,
            "NAMES must list exactly the bindable commands"
        );

        let mut labels = BTreeSet::new();
        for (command, label) in NAMES {
            assert!(!label.is_empty(), "{command} has no readable name");
            assert!(labels.insert(*label), "{label} is used twice");
            assert!(
                !label.contains('.'),
                "{label} looks like a raw command name"
            );
        }
    }

    #[test]
    fn a_row_differs_when_a_default_is_removed_or_stolen() {
        let defaults = default_hotkeys();
        assert!(!differs_from_defaults(&defaults, "nav.back"));

        let mut removed = defaults.clone();
        removed.remove("<Alt>Left");
        assert!(differs_from_defaults(&removed, "nav.back"));

        let mut stolen = defaults.clone();
        stolen.insert(
            "<Alt>Left".to_string(),
            Hotkey {
                command: "nav.forward".to_string(),
                arguments: Vec::new(),
            },
        );
        assert!(differs_from_defaults(&stolen, "nav.back"));

        let mut with_arguments = defaults.clone();
        with_arguments.insert(
            "F6".to_string(),
            Hotkey {
                command: "nav.back".to_string(),
                arguments: vec![("path".to_string(), "/tmp".to_string())],
            },
        );
        assert!(differs_from_defaults(&with_arguments, "nav.back"));
    }

    #[test]
    fn every_bindable_command_has_a_group_and_the_defaults_are_bindable() {
        let bindable: BTreeSet<&str> = bindable_commands().into_iter().collect();
        for (command, hotkey) in default_hotkeys() {
            assert!(
                bindable.contains(hotkey.command.as_str()),
                "default key {command} binds {}, which is not bindable",
                hotkey.command
            );
        }
    }
}
