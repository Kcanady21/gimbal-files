// SPDX-License-Identifier: GPL-3.0-or-later
//! What each model event touches. The runtime uses this map to call one
//! function per event kind; the tests pin which widgets an event may change.
#![allow(dead_code)]

use fm_core::workspace::{Event, WindowId};

/// The widgets one event changes. A window is always named for an event that
/// belongs to one, even when the process handles it first (opening/closing).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Touch {
    pub window: Option<WindowId>,
    pub header: bool,
    pub content: bool,
    pub status: bool,
    pub create_window: bool,
    pub destroy_window: bool,
    pub focus_window: bool,
    pub quit: bool,
}

fn window(window: WindowId, header: bool, content: bool) -> Touch {
    Touch {
        window: Some(window),
        header,
        content,
        ..Touch::default()
    }
}

/// The event-to-UI map, in one pure function.
pub fn touch(event: &Event) -> Touch {
    match event {
        Event::WindowOpened(id) => Touch {
            window: Some(*id),
            header: true,
            content: true,
            status: true,
            create_window: true,
            ..Touch::default()
        },
        Event::WindowClosed(id) => Touch {
            window: Some(*id),
            destroy_window: true,
            ..Touch::default()
        },
        Event::WindowFocused(id) => Touch {
            window: Some(*id),
            focus_window: true,
            ..Touch::default()
        },
        Event::LastWindowClosed => Touch {
            quit: true,
            ..Touch::default()
        },
        Event::TabAdded { window: id, .. } => window(*id, true, true),
        Event::TabClosed { window: id, .. } => window(*id, true, true),
        Event::TabMoved { window: id, .. } => window(*id, true, true),
        Event::GroupFormed { window: id, .. } => window(*id, true, true),
        Event::GroupDissolved { window: id, .. } => window(*id, true, true),
        Event::GroupRenamed { window: id, .. } => window(*id, true, false),
        Event::FocusChanged { window: id, .. } => window(*id, true, true),
        Event::PillMoved { window: id, .. } => window(*id, true, false),
        Event::LayoutChanged { window: id, .. } => window(*id, false, true),
        Event::SelectionChanged { window: id } => window(*id, true, false),
        Event::Navigated { window: id, .. } => window(*id, true, true),
        Event::ScrollAnchorChanged { window: id, .. } => window(*id, false, true),
        Event::TabScopeChanged { window: id, .. } => window(*id, true, true),
    }
}

/// The two events that fade a group pill in or out.
pub fn fades_a_group(event: &Event) -> bool {
    matches!(
        event,
        Event::GroupFormed { .. } | Event::GroupDissolved { .. }
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use fm_core::workspace::{GroupId, TabId};

    fn w() -> WindowId {
        WindowId::from_raw(7)
    }

    fn g() -> GroupId {
        GroupId::from_raw(8)
    }

    fn t() -> TabId {
        TabId::from_raw(9)
    }

    #[test]
    fn a_window_event_opens_or_closes_the_one_window() {
        let opened = touch(&Event::WindowOpened(w()));
        assert_eq!(opened.window, Some(w()));
        assert!(opened.create_window && opened.header && opened.content && opened.status);
        assert!(!opened.destroy_window && !opened.quit);

        let closed = touch(&Event::WindowClosed(w()));
        assert_eq!(closed.window, Some(w()));
        assert!(closed.destroy_window);
        assert!(!closed.header && !closed.content && !closed.status);

        let focused = touch(&Event::WindowFocused(w()));
        assert!(focused.focus_window);
        assert!(!focused.header && !focused.content);

        assert!(touch(&Event::LastWindowClosed).quit);
        assert_eq!(touch(&Event::LastWindowClosed).window, None);
    }

    #[test]
    fn tab_group_and_focus_events_touch_the_header_and_content() {
        let events = [
            Event::TabAdded {
                window: w(),
                group: g(),
                tab: t(),
                index: 2,
            },
            Event::TabClosed {
                window: w(),
                group: g(),
                tab: t(),
            },
            Event::TabMoved {
                window: w(),
                tab: t(),
                from: g(),
                to: GroupId::from_raw(10),
            },
            Event::GroupFormed {
                window: w(),
                group: g(),
            },
            Event::GroupDissolved {
                window: w(),
                group: g(),
            },
            Event::FocusChanged {
                window: w(),
                tab: t(),
            },
            Event::Navigated {
                window: w(),
                tab: t(),
                folder: "/tmp".into(),
            },
        ];
        for event in events {
            let touch = touch(&event);
            assert_eq!(touch.window, Some(w()), "{event:?}");
            assert!(touch.header, "{event:?} must refresh the header");
            assert!(touch.content, "{event:?} must refresh the content");
            assert!(!touch.create_window && !touch.destroy_window && !touch.quit);
        }
    }

    #[test]
    fn the_other_events_touch_only_what_they_can_change() {
        let renamed = touch(&Event::GroupRenamed {
            window: w(),
            group: g(),
        });
        assert!(renamed.header && !renamed.content);

        let moved = touch(&Event::PillMoved {
            window: w(),
            from: 0,
            to: 1,
        });
        assert!(moved.header && !moved.content);

        let layout = touch(&Event::LayoutChanged {
            window: w(),
            group: g(),
        });
        assert!(!layout.header && layout.content);

        let selected = touch(&Event::SelectionChanged { window: w() });
        assert!(selected.header && !selected.content);

        let scroll = touch(&Event::ScrollAnchorChanged {
            window: w(),
            tab: t(),
        });
        assert!(!scroll.header && scroll.content);
    }

    #[test]
    fn only_group_formation_and_dissolution_fade() {
        assert!(fades_a_group(&Event::GroupFormed {
            window: w(),
            group: g()
        }));
        assert!(fades_a_group(&Event::GroupDissolved {
            window: w(),
            group: g()
        }));
        for event in [
            Event::TabAdded {
                window: w(),
                group: g(),
                tab: t(),
                index: 0,
            },
            Event::TabClosed {
                window: w(),
                group: g(),
                tab: t(),
            },
            Event::SelectionChanged { window: w() },
        ] {
            assert!(!fades_a_group(&event), "{event:?}");
        }
    }
}
