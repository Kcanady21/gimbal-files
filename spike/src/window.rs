// SPDX-License-Identifier: GPL-3.0-or-later
use crate::{listing, tabs};
use adw::prelude::*;
use gtk::{gio, glib};
pub fn new_window(app: &adw::Application) -> (adw::ApplicationWindow, adw::TabView) {
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("Gimbal Spike")
        .default_width(900)
        .default_height(600)
        .build();
    let tab_view = adw::TabView::new();
    let tab_bar = adw::TabBar::new();
    tab_bar.set_view(Some(&tab_view));
    let header = adw::HeaderBar::new();
    header.set_title_widget(Some(&tab_bar));
    let up = gtk::Button::from_icon_name("go-up-symbolic");
    up.set_tooltip_text(Some("Go Up"));
    let tv = tab_view.clone();
    up.connect_clicked(move |_| {
        if let Some(page) = tv.selected_page() {
            listing::navigate_up(&page.child());
        }
    });
    header.pack_start(&up);
    let new_tab = gtk::Button::from_icon_name("tab-new-symbolic");
    new_tab.set_tooltip_text(Some("New Tab (Ctrl+T)"));
    let tv = tab_view.clone();
    new_tab.connect_clicked(move |_| open_home(&tv));
    header.pack_end(&new_tab);
    let menu = gio::Menu::new();
    menu.append(Some("Open in New Window"), Some("win.open-in-new-window"));
    menu.append(Some("New Tab"), Some("win.new-tab"));
    menu.append(Some("Close Tab"), Some("win.close-tab"));
    let menu_button = gtk::MenuButton::new();
    menu_button.set_icon_name("open-menu-symbolic");
    menu_button.set_menu_model(Some(&menu));
    header.pack_end(&menu_button);
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(&tab_view));
    window.set_content(Some(&toolbar));
    install_actions(&window, &tab_view);
    tabs::setup_tab_view(&window, &tab_view);
    crate::log!("window", "window created");
    (window, tab_view)
}
pub fn open_dir_in_tab(tab_view: &adw::TabView, dir: &gio::File) -> adw::TabPage {
    open_dir_in_tab_select(tab_view, dir, &[])
}
pub fn open_dir_in_tab_select(
    tab_view: &adw::TabView,
    dir: &gio::File,
    select: &[String],
) -> adw::TabPage {
    let view = listing::build(dir, select);
    let page = tab_view.add_page(&view, None);
    set_title(&page, dir);
    let weak_page = page.downgrade();
    listing::set_on_navigate(view.upcast_ref(), move |dir| {
        if let Some(page) = weak_page.upgrade() {
            set_title(&page, dir);
        }
    });
    tab_view.set_selected_page(&page);
    crate::log!("window", "tab opened: {}", listing::display_path(dir));
    page
}
fn install_actions(window: &adw::ApplicationWindow, tab_view: &adw::TabView) {
    let tv = tab_view.clone();
    let new_tab = gio::ActionEntry::builder("new-tab")
        .activate(move |_window: &adw::ApplicationWindow, _, _| open_home(&tv))
        .build();
    let tv = tab_view.clone();
    let close_tab = gio::ActionEntry::builder("close-tab")
        .activate(move |window: &adw::ApplicationWindow, _, _| {
            let Some(page) = tv.selected_page() else {
                return;
            };
            let path = page
                .tooltip()
                .map(|tip| tip.to_string())
                .unwrap_or_default();
            crate::log!("window", "tab closed: {}", path);
            let last = tv.n_pages() <= 1;
            tv.close_page(&page);
            if last {
                window.close();
            }
        })
        .build();
    let tv = tab_view.clone();
    let open_new = gio::ActionEntry::builder("open-in-new-window")
        .activate(move |window: &adw::ApplicationWindow, _, _| {
            let page = tv.selected_page().unwrap();
            let dir = listing::dir_of(&page.child()).unwrap();
            let app = window.application().unwrap();
            let app = app.downcast::<adw::Application>().unwrap();
            let (new_window, new_tab_view) = new_window(&app);
            open_dir_in_tab(&new_tab_view, &dir);
            new_window.present();
        })
        .build();
    window.add_action_entries([new_tab, close_tab, open_new]);
}
fn open_home(tab_view: &adw::TabView) {
    open_dir_in_tab(tab_view, &gio::File::for_path(glib::home_dir()));
}
fn set_title(page: &adw::TabPage, dir: &gio::File) {
    let name = dir.basename().map_or_else(
        || listing::display_path(dir),
        |name| name.to_string_lossy().into_owned(),
    );
    page.set_title(&name);
    page.set_tooltip(&listing::display_path(dir));
}
