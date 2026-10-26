//! Menus: the main menu bar, the account and add menus of the toolbar, and the action
//! menu of a connection.

use super::app::{self, App};
use super::main_window::ViewMode;
use super::util::{self, menu_item, plain_item};
use crate::frontend::common::library::{Item, ItemAction, MenuItem};
use crate::frontend::common::text;
use objc2::rc::Retained;
use objc2::{sel, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{NSApplication, NSEventModifierFlags, NSMenu, NSMenuItem};
use std::rc::Rc;

fn add_submenu(bar: &NSMenu, submenu: &NSMenu) {
    let item = NSMenuItem::new(submenu.mtm());
    item.setSubmenu(Some(submenu));
    bar.addItem(&item);
}

fn separator(menu: &NSMenu) {
    menu.addItem(&NSMenuItem::separatorItem(menu.mtm()));
}

/// Runs `f` with the application state.
fn on_app(f: fn(&Rc<App>)) -> impl Fn() + 'static {
    move || app::with(f)
}

/// Installs the main menu bar.
pub(super) fn install_main_menu(mtm: MainThreadMarker) {
    let ns_app = NSApplication::sharedApplication(mtm);
    let bar = util::menu(mtm, "");

    // App menu.
    let app_menu = util::menu(mtm, text::APP_NAME);
    app_menu.addItem(&menu_item(
        mtm,
        &format!("About {}", text::APP_NAME),
        "",
        move || super::dialogs::about(mtm),
    ));
    separator(&app_menu);
    app_menu.addItem(&menu_item(
        mtm,
        "Settings…",
        ",",
        on_app(super::forms::show_settings),
    ));
    separator(&app_menu);
    let services = util::menu(mtm, "Services");
    let services_item = plain_item(mtm, "Services", None, "");
    services_item.setSubmenu(Some(&services));
    app_menu.addItem(&services_item);
    ns_app.setServicesMenu(Some(&services));
    separator(&app_menu);
    app_menu.addItem(&plain_item(
        mtm,
        &format!("Hide {}", text::APP_NAME),
        Some(sel!(hide:)),
        "h",
    ));
    let hide_others = plain_item(mtm, "Hide Others", Some(sel!(hideOtherApplications:)), "h");
    hide_others
        .setKeyEquivalentModifierMask(NSEventModifierFlags::Command | NSEventModifierFlags::Option);
    app_menu.addItem(&hide_others);
    app_menu.addItem(&plain_item(
        mtm,
        "Show All",
        Some(sel!(unhideAllApplications:)),
        "",
    ));
    separator(&app_menu);
    app_menu.addItem(&plain_item(
        mtm,
        &format!("Quit {}", text::APP_NAME),
        Some(sel!(terminate:)),
        "q",
    ));
    add_submenu(&bar, &app_menu);

    // File.
    let file = util::menu(mtm, "File");
    file.addItem(&menu_item(
        mtm,
        "New Remote PC…",
        "n",
        on_app(|app| app.edit_remote_pc(None)),
    ));
    file.addItem(&menu_item(
        mtm,
        "Import RDP File…",
        "o",
        on_app(App::import_rdp_file),
    ));
    separator(&file);
    file.addItem(&menu_item(mtm, "Sign In…", "", on_app(App::sign_in)));
    separator(&file);
    file.addItem(&plain_item(mtm, "Close", Some(sel!(performClose:)), "w"));
    add_submenu(&bar, &file);

    // Edit.
    let edit = util::menu(mtm, "Edit");
    edit.addItem(&plain_item(mtm, "Undo", Some(sel!(undo:)), "z"));
    edit.addItem(&plain_item(mtm, "Redo", Some(sel!(redo:)), "Z"));
    separator(&edit);
    edit.addItem(&plain_item(mtm, "Cut", Some(sel!(cut:)), "x"));
    edit.addItem(&plain_item(mtm, "Copy", Some(sel!(copy:)), "c"));
    edit.addItem(&plain_item(mtm, "Paste", Some(sel!(paste:)), "v"));
    edit.addItem(&plain_item(mtm, "Select All", Some(sel!(selectAll:)), "a"));
    separator(&edit);
    edit.addItem(&menu_item(
        mtm,
        "Find",
        "f",
        on_app(|app| {
            app.present();
            if let Some(window) = app.window() {
                window.focus_search();
            }
        }),
    ));
    add_submenu(&bar, &edit);

    // View.
    let view = util::menu(mtm, "View");
    view.addItem(&menu_item(
        mtm,
        "as Grid",
        "1",
        on_app(|app| app.set_mode(ViewMode::Grid)),
    ));
    view.addItem(&menu_item(
        mtm,
        "as List",
        "2",
        on_app(|app| app.set_mode(ViewMode::List)),
    ));
    separator(&view);
    view.addItem(&menu_item(mtm, "Refresh", "r", on_app(App::load)));
    separator(&view);
    let sidebar = plain_item(mtm, "Show Sidebar", Some(sel!(toggleSidebar:)), "s");
    sidebar.setKeyEquivalentModifierMask(
        NSEventModifierFlags::Command | NSEventModifierFlags::Control,
    );
    view.addItem(&sidebar);
    let full_screen = plain_item(mtm, "Enter Full Screen", Some(sel!(toggleFullScreen:)), "f");
    full_screen.setKeyEquivalentModifierMask(
        NSEventModifierFlags::Command | NSEventModifierFlags::Control,
    );
    view.addItem(&full_screen);
    add_submenu(&bar, &view);

    // Window.
    let window = util::menu(mtm, "Window");
    window.addItem(&plain_item(
        mtm,
        "Minimize",
        Some(sel!(performMiniaturize:)),
        "m",
    ));
    window.addItem(&plain_item(mtm, "Zoom", Some(sel!(performZoom:)), ""));
    separator(&window);
    window.addItem(&menu_item(
        mtm,
        "Connections",
        "0",
        on_app(|app| app.present()),
    ));
    separator(&window);
    window.addItem(&plain_item(
        mtm,
        "Bring All to Front",
        Some(sel!(arrangeInFront:)),
        "",
    ));
    add_submenu(&bar, &window);
    ns_app.setWindowsMenu(Some(&window));

    // Help.
    let help = util::menu(mtm, "Help");
    help.addItem(&menu_item(
        mtm,
        &format!("{} Help", text::APP_NAME),
        "?",
        || util::open_url(text::REPOSITORY_URL),
    ));
    help.addItem(&menu_item(mtm, "Report an Issue", "", || {
        util::open_url(&format!("{}/issues", text::REPOSITORY_URL));
    }));
    add_submenu(&bar, &help);
    ns_app.setHelpMenu(Some(&help));

    ns_app.setMainMenu(Some(&bar));
}

/// The "+" menu of the toolbar.
pub(super) fn add_menu(mtm: MainThreadMarker) -> Retained<NSMenu> {
    let menu = util::menu(mtm, "Add");
    menu.addItem(&menu_item(
        mtm,
        "Cloud PC or Azure Virtual Desktop…",
        "",
        on_app(App::sign_in),
    ));
    menu.addItem(&menu_item(
        mtm,
        "Remote PC…",
        "",
        on_app(|app| app.edit_remote_pc(None)),
    ));
    separator(&menu);
    menu.addItem(&menu_item(
        mtm,
        "Import RDP File…",
        "",
        on_app(App::import_rdp_file),
    ));
    menu
}

/// The account menu of the toolbar: who is signed in, other accounts, sign in/out.
pub(super) fn account_menu(app: &App) -> Retained<NSMenu> {
    let mtm = app.mtm;
    let menu = util::menu(mtm, "Account");
    menu.setAutoenablesItems(false);
    let account = app.auth.current_account();
    let active_upn = account.as_ref().and_then(|a| a.upn.clone());
    match &account {
        Some(account) => {
            menu.addItem(&util::caption_item(
                mtm,
                account.label().unwrap_or("Signed in"),
            ));
            if let Some(upn) = account
                .upn
                .as_deref()
                .filter(|u| Some(*u) != account.label())
            {
                menu.addItem(&util::caption_item(mtm, upn));
            }
        }
        None => menu.addItem(&util::caption_item(mtm, "Not signed in")),
    }
    let others: Vec<_> = app
        .auth
        .accounts()
        .into_iter()
        .filter(|a| a.upn != active_upn)
        .collect();
    if !others.is_empty() {
        separator(&menu);
        for other in others {
            let Some(upn) = other.upn else { continue };
            let label = match other.display_name.as_deref() {
                Some(name) if name != upn => format!("Switch to {name} ({upn})"),
                _ => format!("Switch to {upn}"),
            };
            menu.addItem(&menu_item(mtm, &label, "", move || {
                app::with(|app| app.switch_account(&upn));
            }));
        }
    }
    separator(&menu);
    if account.is_some() {
        menu.addItem(&menu_item(
            mtm,
            "Add Another Account…",
            "",
            on_app(App::sign_in),
        ));
        menu.addItem(&menu_item(mtm, "Sign Out", "", on_app(App::sign_out)));
    } else {
        menu.addItem(&menu_item(mtm, "Sign In…", "", on_app(App::sign_in)));
    }
    separator(&menu);
    menu.addItem(&menu_item(
        mtm,
        "Settings…",
        "",
        on_app(super::forms::show_settings),
    ));
    menu
}

/// macOS wording of an action (an ellipsis when more input follows).
fn action_title(action: ItemAction) -> &'static str {
    match action {
        ItemAction::Restart => "Restart…",
        ItemAction::Inspect => "Inspect Connection…",
        ItemAction::Details => "View Details…",
        ItemAction::Preferences => "Settings…",
        other => other.label(),
    }
}

/// The actions of `item` (context menu and the "…" button of cards).
pub(super) fn item_menu(app: &App, item: &Item) -> Retained<NSMenu> {
    let mtm = app.mtm;
    let menu = util::menu(mtm, item.title());
    menu.setAutoenablesItems(false);
    let busy = app.is_busy(item.id());
    let mut pending_separator = false;
    // Launcher and login entries integrate with Linux desktops only.
    for entry in item.menu(false, false, busy) {
        match entry {
            MenuItem::Separator => pending_separator = menu.numberOfItems() > 0,
            MenuItem::Toggle { .. } => {}
            MenuItem::Action { action, enabled } => {
                if pending_separator {
                    separator(&menu);
                    pending_separator = false;
                }
                let item = item.clone();
                let entry = menu_item(mtm, action_title(action), "", move || {
                    let item = item.clone();
                    // After the menu has closed (alerts and sheets may follow).
                    util::defer(move || app::with(|app| app.item_action(item, action)));
                });
                entry.setEnabled(enabled);
                menu.addItem(&entry);
            }
        }
    }
    menu
}
