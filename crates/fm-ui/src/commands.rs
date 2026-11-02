// SPDX-License-Identifier: GPL-3.0-or-later
//! Hotkey bindings and the UI's command handler.
//!
//! The mapping from the config's hotkey table to invocations is pure and
//! tested. GTK's controller only adds the widget plumbing.

use std::collections::BTreeMap;
use std::rc::Weak;

use fm_core::command::{Caller, CheckedCommand, Handler, Invocation, Outcome};
use fm_core::config::Hotkey;
use gtk::prelude::WidgetExt;

use crate::frame::App;

/// One accelerator and the invocation it runs, in the config's sorted order.
pub fn bindings(hotkeys: &BTreeMap<String, Hotkey>) -> Vec<(String, Invocation)> {
    hotkeys
        .iter()
        .map(|(accelerator, hotkey)| (accelerator.clone(), hotkey.invocation()))
        .collect()
}

/// The handler gets the caller for the dispatch that reached it. P2b's
/// `Handler::handle` does not pass one, so the caller travels here instead.
pub struct UiHandler {
    pub caller: Caller,
    pub app: Weak<App>,
}

impl Handler for UiHandler {
    fn handle(&mut self, command: &CheckedCommand) -> Outcome {
        match self.app.upgrade() {
            Some(app) => app.handle_command(self.caller, command),
            None => Outcome::Done,
        }
    }
}

/// Bind every hotkey from the config. An accelerator GTK cannot parse is
/// logged and skipped; the rest still install.
pub fn install_hotkeys(app: &std::rc::Rc<App>, window: &gtk::Window) {
    let controller = gtk::ShortcutController::new();
    controller.set_scope(gtk::ShortcutScope::Global);
    let hotkeys = app.hotkeys();
    for (accelerator, invocation) in bindings(&hotkeys) {
        if gtk::accelerator_parse(accelerator.as_str()).is_none() {
            crate::log!(
                "hotkey",
                "cannot parse accelerator {accelerator:?}; skipped"
            );
            continue;
        }
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
