// SPDX-License-Identifier: GPL-3.0-or-later
//! Which keys belong to a focused text entry.
//!
//! The window's shortcuts are global. While a path bar, search field, rename
//! or new-folder entry has focus, the editing keys must reach the entry
//! instead of running a command. The key rules are pure and tested here; the
//! window only supplies "is an entry focused", through [`entry_focused`], the
//! one shared implementation of that question.

use gtk::{gdk, prelude::*};

/// Whether the caret is in this entry. A `GtkEntry` hands its focus to the
/// `GtkText` inside it (`gtk/gtkentry.c:426-431`), so `has_focus()` on the
/// entry itself is always false; the entry and every other ancestor of the
/// focus widget carry `FOCUS_WITHIN` instead (`gtk/gtkwindow.c:5280-5310`).
///
/// The window only reports a logical focus here: `FOCUS_WITHIN` is set whether
/// or not the window is the active toplevel, which is what both the main
/// window's hotkey guard (`commands.rs`) and the portal dialog's own key
/// controller (`portal::picker`) need.
pub fn entry_focused(entry: &gtk::Entry) -> bool {
    entry.state_flags().contains(gtk::StateFlags::FOCUS_WITHIN)
}

/// Whether a global shortcut with this key and modifier state must be left to
/// a focused text entry.
pub fn entry_owns_key(key: gdk::Key, modifiers: gdk::ModifierType) -> bool {
    let ctrl = modifiers.contains(gdk::ModifierType::CONTROL_MASK);
    let alt = modifiers.contains(gdk::ModifierType::ALT_MASK);
    let sup = modifiers.contains(gdk::ModifierType::SUPER_MASK);
    let shift = modifiers.contains(gdk::ModifierType::SHIFT_MASK);

    // The editing commands: Ctrl+A, Ctrl+C, Ctrl+X, Ctrl+V and Ctrl+Z.
    if ctrl && !alt && !sup && !shift {
        return matches!(
            key,
            gdk::Key::a | gdk::Key::c | gdk::Key::x | gdk::Key::v | gdk::Key::z
        );
    }

    // Cursor movement and deleting a character, with or without Alt.
    if !ctrl
        && !sup
        && matches!(
            key,
            gdk::Key::Delete | gdk::Key::Left | gdk::Key::Right | gdk::Key::Up
        )
    {
        return true;
    }

    // Escape closes a popover or search field, never a command.
    if !ctrl && !alt && !sup && key == gdk::Key::Escape {
        return true;
    }

    false
}

/// Whether a global shortcut must be left to a focused preview text view.
/// Text Undo/Redo and Delete belong to the editor, never to file selections.
pub fn text_view_owns_key(key: gdk::Key, modifiers: gdk::ModifierType) -> bool {
    let ctrl = modifiers.contains(gdk::ModifierType::CONTROL_MASK);
    let sup = modifiers.contains(gdk::ModifierType::SUPER_MASK);
    let shift = modifiers.contains(gdk::ModifierType::SHIFT_MASK);

    if sup {
        return false;
    }
    // Cursor movement and character/word deletion stay in the editor with any
    // Ctrl/Alt combination, so a configured file Delete or Undo cannot act on
    // the folder selection behind the preview.
    if matches!(
        key,
        gdk::Key::Delete
            | gdk::Key::BackSpace
            | gdk::Key::Left
            | gdk::Key::Right
            | gdk::Key::Up
            | gdk::Key::Down
            | gdk::Key::Home
            | gdk::Key::End
            | gdk::Key::Page_Up
            | gdk::Key::Page_Down
            | gdk::Key::Return
            | gdk::Key::KP_Enter
            | gdk::Key::Tab
            | gdk::Key::ISO_Left_Tab
    ) {
        return true;
    }
    if ctrl {
        return matches!(
            key,
            gdk::Key::a | gdk::Key::c | gdk::Key::x | gdk::Key::v | gdk::Key::z | gdk::Key::y
        );
    }
    shift && matches!(key, gdk::Key::Insert)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The focus checks need real widgets and a display, so they run in their
    /// own process: `gtk::init` takes over the process default main context for
    /// its lifetime, which would starve the other in-process GLib timer tests.
    fn gtk_test_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Runs `inner` in a fresh process and fails this test if it does not pass.
    fn run_widget_test(inner: &str, label: &str) {
        let output = std::process::Command::new(std::env::current_exe().expect("test executable"))
            .args(["--exact", inner, "--ignored", "--nocapture"])
            .output()
            .unwrap_or_else(|_| panic!("{label}: could not start the owned widget process"));
        assert!(
            output.status.success(),
            "{label} failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    /// The window's focus widget: the one widget GTK gave focus to. Read as a
    /// property because the binding generates no getter for it.
    fn window_focus(window: &gtk::Window) -> Option<gtk::Widget> {
        window.property::<Option<gtk::Widget>>("focus-widget")
    }

    #[test]
    fn entry_focused_follows_the_caret() {
        run_widget_test(
            "fileops::keys::tests::entry_focused_follows_the_caret_inner",
            "the entry-focus widget regression",
        );
    }

    #[test]
    #[ignore = "runs in its own process: gtk::init owns the default main context"]
    fn entry_focused_follows_the_caret_inner() {
        let _guard = gtk_test_lock();
        gtk::init().expect("the entry-focus regression needs a GTK display");

        // Nothing is presented or mapped: `gtk_widget_grab_focus` asks only for
        // a sensitive, focusable widget under a root (`gtk/gtkwidget.c:5154-5164`)
        // and a widget gets its root when it is parented
        // (`gtk/gtkwidget.c:2581-2588`). The window does not have to be active
        // either: `FOCUS_WITHIN` is set on the focus widget and every one of its
        // ancestors as part of the focus crossing (`gtk/gtkwindow.c:5280-5330`),
        // while `has_focus()` additionally needs `is_active`
        // (`gtk/gtkwindow.c:2256-2257`).
        let window = gtk::Window::new();
        let rows = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let entry = gtk::Entry::new();
        let button = gtk::Button::new();
        let text_view = gtk::TextView::new();
        rows.append(&entry);
        rows.append(&button);
        rows.append(&text_view);
        window.set_child(Some(&rows));

        entry.grab_focus();
        assert!(
            entry_focused(&entry),
            "the entry carries FOCUS_WITHIN from the GtkText inside it"
        );
        assert!(
            !entry.has_focus(),
            "the entry is never its own focus widget, so has_focus() is always false"
        );
        let focus = window_focus(&window).expect("the window has a focus widget");
        let entry_widget = entry.clone().upcast::<gtk::Widget>();
        assert!(
            focus != entry_widget && focus.is_ancestor(&entry_widget),
            "focus sits inside the entry, not on the entry itself"
        );

        button.grab_focus();
        assert!(
            !entry_focused(&entry),
            "moving focus out of the entry clears FOCUS_WITHIN on it"
        );
        assert_eq!(
            window_focus(&window).as_ref(),
            Some(&button.clone().upcast::<gtk::Widget>()),
            "a GtkButton is its own focus widget, so header.rs's has_focus() \
             on the accordion's choose button is the right question"
        );

        // The sweep's other compound widget: a `GtkTextView` sets no
        // `grab_focus` override and makes itself focusable
        // (`gtk/gtktextview.c:2110`), so preview.rs's `has_focus()` on it is
        // right for the same reason.
        text_view.grab_focus();
        assert_eq!(
            window_focus(&window).as_ref(),
            Some(&text_view.clone().upcast::<gtk::Widget>()),
            "a GtkTextView is its own focus widget"
        );
        assert!(
            !entry_focused(&entry),
            "focus elsewhere never makes the entry look focused"
        );
    }

    fn modifiers(ctrl: bool, alt: bool, shift: bool) -> gdk::ModifierType {
        let mut state = gdk::ModifierType::empty();
        if ctrl {
            state |= gdk::ModifierType::CONTROL_MASK;
        }
        if alt {
            state |= gdk::ModifierType::ALT_MASK;
        }
        if shift {
            state |= gdk::ModifierType::SHIFT_MASK;
        }
        state
    }

    #[test]
    fn editing_chords_belong_to_the_entry() {
        for key in [
            gdk::Key::a,
            gdk::Key::c,
            gdk::Key::x,
            gdk::Key::v,
            gdk::Key::z,
        ] {
            assert!(entry_owns_key(key, modifiers(true, false, false)));
            assert!(!entry_owns_key(key, modifiers(false, false, false)));
        }
        assert!(!entry_owns_key(gdk::Key::c, modifiers(true, false, true)));
    }

    #[test]
    fn delete_and_the_arrows_belong_to_the_entry_with_or_without_alt() {
        for key in [
            gdk::Key::Delete,
            gdk::Key::Left,
            gdk::Key::Right,
            gdk::Key::Up,
        ] {
            assert!(entry_owns_key(key, modifiers(false, false, false)));
            assert!(entry_owns_key(key, modifiers(false, true, false)));
        }
        assert!(!entry_owns_key(
            gdk::Key::Delete,
            modifiers(true, false, false)
        ));
        assert!(!entry_owns_key(
            gdk::Key::Down,
            modifiers(false, false, false)
        ));
    }

    #[test]
    fn text_view_owns_editing_and_cursor_keys() {
        for key in [
            gdk::Key::a,
            gdk::Key::c,
            gdk::Key::x,
            gdk::Key::v,
            gdk::Key::z,
            gdk::Key::y,
        ] {
            assert!(text_view_owns_key(key, modifiers(true, false, false)));
        }
        for key in [
            gdk::Key::Delete,
            gdk::Key::BackSpace,
            gdk::Key::Left,
            gdk::Key::Right,
            gdk::Key::Up,
            gdk::Key::Down,
            gdk::Key::Home,
            gdk::Key::End,
            gdk::Key::Page_Up,
            gdk::Key::Page_Down,
            gdk::Key::Return,
            gdk::Key::Tab,
        ] {
            assert!(text_view_owns_key(key, modifiers(false, false, false)));
            assert!(text_view_owns_key(key, modifiers(false, false, true)));
        }
        assert!(text_view_owns_key(
            gdk::Key::Delete,
            modifiers(true, false, false)
        ));
        assert!(text_view_owns_key(
            gdk::Key::Left,
            modifiers(false, true, false)
        ));
        assert!(!text_view_owns_key(
            gdk::Key::z,
            modifiers(false, false, false)
        ));
        assert!(!text_view_owns_key(
            gdk::Key::F5,
            modifiers(false, false, false)
        ));
    }

    #[test]
    fn escape_belongs_to_the_entry_and_other_hotkeys_do_not() {
        assert!(entry_owns_key(
            gdk::Key::Escape,
            modifiers(false, false, false)
        ));
        assert!(!entry_owns_key(
            gdk::Key::Escape,
            modifiers(true, false, false)
        ));
        assert!(!entry_owns_key(
            gdk::Key::F5,
            modifiers(false, false, false)
        ));
        assert!(!entry_owns_key(gdk::Key::n, modifiers(true, false, false)));
        assert!(!entry_owns_key(gdk::Key::t, modifiers(true, false, false)));
    }
}
