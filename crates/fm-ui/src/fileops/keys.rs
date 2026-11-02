// SPDX-License-Identifier: GPL-3.0-or-later
//! Which keys belong to a focused text entry.
//!
//! The window's shortcuts are global. While a path bar, search field, rename
//! or new-folder entry has focus, the editing keys must reach the entry
//! instead of running a command. The decision is pure and tested here; the
//! window only supplies "is an entry focused".

use gtk::gdk;

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

#[cfg(test)]
mod tests {
    use super::*;

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
