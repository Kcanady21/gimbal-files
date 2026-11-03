// SPDX-License-Identifier: GPL-3.0-or-later
//! Hotkey bindings and the UI's command handler.
//!
//! The mapping from the config's hotkey table to invocations is pure and
//! tested. GTK's controller only adds the widget plumbing; every hotkey runs
//! through its own window, which is focused before the command dispatches.

use std::collections::BTreeMap;
use std::rc::Weak;

use fm_core::command::{Caller, CheckedCommand, Handler, Invocation, Outcome};
use fm_core::config::Hotkey;
use fm_core::workspace::WindowId;
use gtk::prelude::WidgetExt;

use crate::app::Process;
use crate::frame::WindowUi;

/// One accelerator and the invocation it runs, in the config's sorted order.
pub fn bindings(hotkeys: &BTreeMap<String, Hotkey>) -> Vec<(String, Invocation)> {
    hotkeys
        .iter()
        .map(|(accelerator, hotkey)| (accelerator.clone(), hotkey.invocation()))
        .collect()
}

/// The handler gets the caller and the window for the dispatch that reached
/// it. `Handler::handle` does not pass either, so they travel here.
pub struct UiHandler {
    pub caller: Caller,
    pub window: WindowId,
    pub process: Weak<Process>,
}

impl Handler for UiHandler {
    fn handle(&mut self, command: &CheckedCommand) -> Outcome {
        match self.process.upgrade() {
            Some(process) => process.handle_command(self.window, self.caller, command),
            None => Outcome::Done,
        }
    }
}

/// Bind every hotkey from the config. An accelerator GTK cannot parse is
/// logged and skipped; the rest still install.
pub fn install_hotkeys(app: &std::rc::Rc<WindowUi>, window: &gtk::Window) {
    let Some(process) = app.process_upgrade() else {
        return;
    };
    let controller = gtk::ShortcutController::new();
    controller.set_scope(gtk::ShortcutScope::Global);
    let hotkeys = process.hotkeys();
    for (accelerator, invocation) in bindings(&hotkeys) {
        let Some((key, modifiers)) = gtk::accelerator_parse(accelerator.as_str()) else {
            crate::log!(
                "hotkey",
                "cannot parse accelerator {accelerator:?}; skipped"
            );
            continue;
        };
        let Some(trigger) = gtk::ShortcutTrigger::parse_string(&accelerator) else {
            crate::log!(
                "hotkey",
                "GTK has no trigger for accelerator {accelerator:?}; skipped"
            );
            continue;
        };
        let name = invocation.name().to_string();
        let weak = std::rc::Rc::downgrade(app);
        let action = gtk::CallbackAction::new(move |_, _| {
            let Some(app) = weak.upgrade() else {
                return gtk::glib::Propagation::Proceed;
            };
            if app
                .process_upgrade()
                .is_some_and(|process| process.hotkeys_blocked(app.id))
            {
                return gtk::glib::Propagation::Proceed;
            }
            // The path bar, search field, rename and new-folder entries keep
            // the editing keys. The other hotkeys still run.
            if app.text_entry_focused() && crate::fileops::keys::entry_owns_key(key, modifiers) {
                return gtk::glib::Propagation::Proceed;
            }
            if app.preview_text_focused()
                && crate::fileops::keys::text_view_owns_key(key, modifiers)
            {
                return gtk::glib::Propagation::Proceed;
            }
            app.dispatch(Caller::Hotkey, invocation.clone());
            gtk::glib::Propagation::Stop
        });
        controller.add_shortcut(gtk::Shortcut::new(Some(trigger), Some(action)));
        crate::log!("hotkey", "bound {accelerator:?} to {name}");
    }
    window.add_controller(controller);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hotkey(command: &str, arguments: &[(&str, &str)]) -> Hotkey {
        Hotkey {
            command: command.to_string(),
            arguments: arguments
                .iter()
                .map(|(key, value)| (key.to_string(), value.to_string()))
                .collect(),
        }
    }

    #[test]
    fn hotkey_mapping_keeps_the_command_and_its_arguments() {
        let mut hotkeys = BTreeMap::new();
        hotkeys.insert("<Control>equal".to_string(), hotkey("view.zoom-in", &[]));
        hotkeys.insert(
            "<Control>Page_Down".to_string(),
            hotkey("view.set-opacity", &[("opacity", "0.9")]),
        );
        let bound = bindings(&hotkeys);
        assert_eq!(bound.len(), 2);
        assert_eq!(bound[0].0, "<Control>Page_Down");
        assert_eq!(bound[0].1.name(), "view.set-opacity");
        assert_eq!(
            bound[0].1.arguments(),
            &[("opacity".to_string(), "0.9".to_string())]
        );
        assert_eq!(bound[1].0, "<Control>equal");
        assert_eq!(bound[1].1.name(), "view.zoom-in");
        assert!(bound[1].1.arguments().is_empty());
    }
}
