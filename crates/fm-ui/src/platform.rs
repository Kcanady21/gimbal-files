// SPDX-License-Identifier: GPL-3.0-or-later
//! The desktop's window-control decision and the one niri adapter.
//!
//! The decision is pure and tested: under niri the Float button toggles
//! niri's floating mode and there is no Minimize button; anywhere else Float
//! toggles maximize and Minimize is shown.

use gtk::gio;
use gtk::prelude::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FloatAction {
    /// niri IPC: `niri msg action toggle-window-floating`.
    NiriToggle,
    /// Maximize or restore the window.
    Maximize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WindowControls {
    pub float: FloatAction,
    pub minimize: bool,
}

/// The one platform decision. `niri` is whether `NIRI_SOCKET` is set.
pub const fn controls(niri: bool) -> WindowControls {
    if niri {
        WindowControls {
            float: FloatAction::NiriToggle,
            minimize: false,
        }
    } else {
        WindowControls {
            float: FloatAction::Maximize,
            minimize: true,
        }
    }
}

pub fn niri() -> bool {
    std::env::var_os("NIRI_SOCKET").is_some()
}

pub fn controls_here() -> WindowControls {
    controls(niri())
}

/// Run the Float button's action for this window.
pub fn toggle_float(window: &gtk::Window, action: FloatAction) {
    match action {
        FloatAction::NiriToggle => toggle_niri_floating(),
        FloatAction::Maximize => {
            if window.is_maximized() {
                window.unmaximize();
            } else {
                window.maximize();
            }
        }
    }
}

fn toggle_niri_floating() {
    let argv = ["niri", "msg", "action", "toggle-window-floating"].map(std::ffi::OsStr::new);
    match gio::Subprocess::newv(&argv, gio::SubprocessFlags::NONE) {
        Ok(process) => process.wait_check_async(None::<&gio::Cancellable>, |result| match result {
            Ok(()) => crate::log!("float", "niri toggle-window-floating: exit 0"),
            Err(error) => crate::log!("float", "niri toggle-window-floating failed: {error}"),
        }),
        Err(error) => crate::log!("float", "could not run niri: {error}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn niri_floats_through_ipc_and_has_no_minimize() {
        assert_eq!(
            controls(true),
            WindowControls {
                float: FloatAction::NiriToggle,
                minimize: false
            }
        );
    }

    #[test]
    fn other_desktops_maximize_and_show_minimize() {
        assert_eq!(
            controls(false),
            WindowControls {
                float: FloatAction::Maximize,
                minimize: true
            }
        );
    }
}
