//! Main window: header (add connection, layout, main menu), the Cloud PC, Azure Virtual
//! Desktop and Remote PC cards, and the dialogs behind their actions.

use super::connection_card::{CardAction, CardLayoutMode, ConnectionCard};
use super::credentials_dialog::{self, SignIn};
use super::login_dialog::LoginDialog;
use super::open_sessions;
use super::preferences_dialog::PreferencesDialog;
use super::session_window::{SessionWindow, SessionWindowOptions};
use crate::api::{self, AvdResource, CloudPcService, Snapshot, TroubleshootOutcome};
use crate::auth::{account, Account, AuthManager};
use crate::config::AppConfig;
use crate::connections::{self, ConnectionStore, SavedConnection};
use crate::frontend::common::library::{
    SearchFilter, SECTION_AVD, SECTION_CLOUD_PCS, SECTION_REMOTE_PCS,
};
use crate::frontend::common::text::{
    format_time, join_names, load_error_message, load_summary, or_not_reported, read_rdp_file,
    sanitize_file_name, snapshot_label, REPOSITORY_URL,
};
use crate::history::{self, RecentStore};
use crate::launcher::{LaunchEntry, LaunchTarget, SearchIndex};
use crate::models::{CloudPc, CloudPcAction};
use crate::overrides::{ConnectionOverrides, OverrideStore};
use crate::placement::PlacementStore;
use crate::platform::hooks::{self, HookContext};
use crate::{platform, rdp, rdp_file, session};
use adw::prelude::*;
use gtk4::{
    gio, glib, Box, Button, FlowBox, Image, Label, ListBox, MenuButton, Orientation, PopoverMenu,
    ScrolledWindow, SearchEntry, Spinner, Stack, ToggleButton,
};
use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::rc::Rc;
use std::sync::Arc;

/// Logical size of a new session window; the remote desktop follows the real size.
const SESSION_WIDTH: u32 = 1280;
const SESSION_HEIGHT: u32 = 800;

pub struct AppMainWindow {
    window: adw::ApplicationWindow,
    stack: Stack,
    empty_page: adw::StatusPage,
    content_stack: Stack,
    /// Section boxes of the grid and list views.
    grid_sections: Box,
    list_sections: Box,
    /// Ids of the collapsed sections.
    collapsed_sections: RefCell<HashSet<String>>,
    spinner: Spinner,
    status_label: Label,
    auth: Arc<AuthManager>,
    cloud_service: Arc<CloudPcService>,
    connection_store: ConnectionStore,
    cached_pcs: RefCell<Vec<CloudPc>>,
    avd_resources: RefCell<Vec<AvdResource>>,
    remote_pcs: RefCell<Vec<SavedConnection>>,
    loading_pcs: Cell<bool>,
    /// Connection requested (command line, search) before the lists finished loading.
    pending_target: RefCell<Option<LaunchTarget>>,
    search_index: SearchIndex,
    current_mode: RefCell<CardLayoutMode>,
    menu_button: MenuButton,
    grid_btn: ToggleButton,
    list_btn: ToggleButton,
    refresh_btn: Button,
    layout_box: Box,
    /// Cloud PCs and AVD resources with a connection or action in progress.
    connecting_pcs: RefCell<HashSet<String>>,
    search_bar: gtk4::SearchBar,
    search_entry: SearchEntry,
    recent: RecentStore,
    overrides: OverrideStore,
    /// The user chose what happens to the open sessions, so the window may close.
    close_confirmed: Cell<bool>,
}

/// A connection about to start, for hooks, history and the session window.
struct Launch {
    name: String,
    /// `None` for connections that are not saved (quick connect, opened `.rdp` files).
    target: Option<LaunchTarget>,
    address: String,
    login_hint: Option<String>,
    overrides: ConnectionOverrides,
    multi_monitor: bool,
    dynamic_resolution: bool,
}

impl AppMainWindow {
    pub fn new(
        app: &adw::Application,
        auth: Arc<AuthManager>,
        cloud_service: Arc<CloudPcService>,
    ) -> Rc<Self> {
        let window = adw::ApplicationWindow::builder()
            .application(app)
            .title("Windows App")
            .default_width(980)
            .default_height(680)
            .icon_name("com.microsoft.windowsapp.linux")
            .build();
        app.set_accels_for_action("win.add-connection", &["<Control>n"]);
        app.set_accels_for_action("win.find", &["<Control>f"]);
        app.set_accels_for_action("win.import-rdp", &["<Control>o"]);
        app.set_accels_for_action("window.close", &["<Control>q"]);

        let config = AppConfig::load();
        let initial_mode = if config.view_mode == "list" {
            CardLayoutMode::List
        } else {
            CardLayoutMode::Grid
        };

        // --- Header bar ---
        let header = adw::HeaderBar::new();

        let add_btn = Button::builder()
            .icon_name("list-add-symbolic")
            .tooltip_text("Add Connection (Ctrl+N)")
            .action_name("win.add-connection")
            .build();
        header.pack_start(&add_btn);

        // Visible while signed in.
        let refresh_btn = Button::from_icon_name("view-refresh-symbolic");
        refresh_btn.set_tooltip_text(Some("Refresh"));
        refresh_btn.set_visible(false);
        header.pack_start(&refresh_btn);

        // Grid / list switcher, visible while there is something to show. Grouped toggle
        // buttons in a linked box form a native segmented control.
        let layout_box = Box::new(Orientation::Horizontal, 0);
        layout_box.add_css_class("linked");
        layout_box.set_visible(false);
        let grid_btn = ToggleButton::builder()
            .icon_name("view-grid-symbolic")
            .tooltip_text("Grid View")
            .build();
        let list_btn = ToggleButton::builder()
            .icon_name("view-list-symbolic")
            .tooltip_text("List View")
            .group(&grid_btn)
            .build();
        if initial_mode == CardLayoutMode::Grid {
            grid_btn.set_active(true);
        } else {
            list_btn.set_active(true);
        }
        layout_box.append(&grid_btn);
        layout_box.append(&list_btn);
        header.pack_start(&layout_box);

        let menu_button = MenuButton::builder()
            .icon_name("open-menu-symbolic")
            .tooltip_text("Main Menu")
            .primary(true)
            .build();
        header.pack_end(&menu_button);
        let search_btn = ToggleButton::builder()
            .icon_name("system-search-symbolic")
            .tooltip_text("Search or Quick Connect (Ctrl+F)")
            .build();
        header.pack_end(&search_btn);

        // Filters the cards; Enter connects to the only match or to a typed address.
        let search_entry = SearchEntry::builder()
            .placeholder_text("Search, or enter a PC name or address to connect")
            .hexpand(true)
            .build();
        let search_clamp = adw::Clamp::builder()
            .maximum_size(560)
            .child(&search_entry)
            .build();
        let search_bar = gtk4::SearchBar::builder()
            .child(&search_clamp)
            .show_close_button(true)
            .build();
        search_bar.connect_entry(&search_entry);
        search_bar
            .bind_property("search-mode-enabled", &search_btn, "active")
            .bidirectional()
            .sync_create()
            .build();

        // --- Pages: loading, empty, content ---
        let stack = Stack::new();
        stack.set_vexpand(true);

        let spinner = Spinner::new();
        spinner.set_size_request(48, 48);
        spinner.set_halign(gtk4::Align::Center);
        let loading_page = adw::StatusPage::builder()
            .title("Loading Cloud PCs…")
            .child(&spinner)
            .build();
        stack.add_named(&loading_page, Some("loading"));

        let empty_page = adw::StatusPage::builder()
            .icon_name("com.microsoft.windowsapp.linux")
            .title("No Connections Yet")
            .build();
        stack.add_named(&empty_page, Some("empty"));

        let content_stack = Stack::new();
        content_stack.set_vexpand(true);

        let grid_scrolled = ScrolledWindow::builder()
            .hscrollbar_policy(gtk4::PolicyType::Never)
            .vscrollbar_policy(gtk4::PolicyType::Automatic)
            .build();
        let grid_sections = sections_box();
        grid_scrolled.set_child(Some(&grid_sections));
        content_stack.add_named(&grid_scrolled, Some("grid"));

        // valign Start keeps rows compact instead of stretching them vertically.
        let list_scrolled = ScrolledWindow::builder()
            .hscrollbar_policy(gtk4::PolicyType::Never)
            .vscrollbar_policy(gtk4::PolicyType::Automatic)
            .build();
        let list_sections = sections_box();
        let list_clamp = adw::Clamp::builder()
            .maximum_size(900)
            .child(&list_sections)
            .build();
        list_scrolled.set_child(Some(&list_clamp));
        content_stack.add_named(&list_scrolled, Some("list"));

        content_stack.set_visible_child_name(match initial_mode {
            CardLayoutMode::Grid => "grid",
            CardLayoutMode::List => "list",
        });
        stack.add_named(&content_stack, Some("content"));

        // --- Status bar ---
        let status_bar = Box::new(Orientation::Horizontal, 8);
        status_bar.set_margin_start(16);
        status_bar.set_margin_end(16);
        status_bar.set_margin_top(6);
        status_bar.set_margin_bottom(6);
        let status_label = Label::new(Some("Ready"));
        status_label.add_css_class("dim-label");
        status_label.set_halign(gtk4::Align::Start);
        status_label.set_hexpand(true);
        status_label.set_ellipsize(gtk4::pango::EllipsizeMode::End);
        status_bar.append(&status_label);

        let toolbar_view = adw::ToolbarView::new();
        toolbar_view.add_top_bar(&header);
        toolbar_view.add_top_bar(&search_bar);
        toolbar_view.set_content(Some(&stack));
        toolbar_view.add_bottom_bar(&status_bar);
        window.set_content(Some(&toolbar_view));
        search_bar.set_key_capture_widget(Some(&window));

        let connection_store = ConnectionStore::default();
        let remote_pcs = connection_store.load();

        let app_window = Rc::new(Self {
            window,
            stack,
            empty_page,
            content_stack,
            grid_sections,
            list_sections,
            collapsed_sections: RefCell::new(config.collapsed_sections.iter().cloned().collect()),
            spinner,
            status_label,
            auth,
            cloud_service,
            connection_store,
            cached_pcs: RefCell::new(Vec::new()),
            avd_resources: RefCell::new(Vec::new()),
            remote_pcs: RefCell::new(remote_pcs),
            loading_pcs: Cell::new(false),
            pending_target: RefCell::new(None),
            search_index: SearchIndex::default(),
            current_mode: RefCell::new(initial_mode),
            menu_button,
            grid_btn,
            list_btn,
            refresh_btn: refresh_btn.clone(),
            layout_box,
            connecting_pcs: RefCell::new(HashSet::new()),
            search_bar,
            search_entry,
            recent: RecentStore::default(),
            overrides: OverrideStore::default(),
            close_confirmed: Cell::new(false),
        });

        // Handlers only hold weak references, so the window state is owned here until the
        // window closes.
        let keep_alive = RefCell::new(Some(Rc::clone(&app_window)));
        app_window.window.connect_close_request(move |_| {
            let this = keep_alive.borrow().clone();
            if let Some(this) = this {
                if !this.close_confirmed.get() && !open_sessions::names().is_empty() {
                    this.confirm_close();
                    return glib::Propagation::Stop;
                }
            }
            keep_alive.take();
            glib::Propagation::Proceed
        });

        app_window.install_actions();
        app_window.install_search();

        let weak = Rc::downgrade(&app_window);
        refresh_btn.connect_clicked(move |_| {
            if let Some(this) = weak.upgrade() {
                this.load_cloud_pcs();
            }
        });
        for (button, mode) in [
            (&app_window.grid_btn, CardLayoutMode::Grid),
            (&app_window.list_btn, CardLayoutMode::List),
        ] {
            let weak = Rc::downgrade(&app_window);
            button.connect_toggled(move |btn| {
                if let Some(this) = weak.upgrade() {
                    if btn.is_active() && *this.current_mode.borrow() != mode {
                        this.set_layout_mode(mode);
                    }
                }
            });
        }

        app_window.load_cloud_pcs();
        app_window
    }

    pub fn present(&self) {
        self.window.present();
    }

    /// Asks whether to disconnect the open sessions or keep them running without the
    /// main window.
    fn confirm_close(self: &Rc<Self>) {
        let names = open_sessions::names();
        let (heading, body) = match names.as_slice() {
            [name] => (
                "Disconnect Session?".to_string(),
                format!(
                    "{name} is still connected. Quitting disconnects it; keeping it open \
                     closes only this window."
                ),
            ),
            _ => (
                format!("Disconnect {} Sessions?", names.len()),
                format!(
                    "{} are still connected. Quitting disconnects them; keeping them open \
                     closes only this window.",
                    join_names(&names)
                ),
            ),
        };
        let dialog = adw::AlertDialog::builder()
            .heading(heading)
            .body(body)
            .default_response("cancel")
            .close_response("cancel")
            .build();
        dialog.add_responses(&[
            ("cancel", "Cancel"),
            ("keep", "Keep Open"),
            ("quit", "Disconnect and Quit"),
        ]);
        dialog.set_response_appearance("quit", adw::ResponseAppearance::Destructive);
        // The window may be behind full-screen sessions (e.g. closed from the overview).
        self.window.present();
        let weak = Rc::downgrade(self);
        dialog.connect_response(None, move |_, response| {
            let Some(this) = weak.upgrade() else { return };
            match response {
                "keep" => open_sessions::keep_app_running(),
                "quit" => open_sessions::close_all(),
                _ => return,
            }
            this.close_confirmed.set(true);
            this.window.close();
        });
        dialog.present(Some(&self.window));
    }

    fn show_about(&self) {
        let about = adw::AboutDialog::builder()
            .application_name("Windows App")
            .application_icon("com.microsoft.windowsapp.linux")
            .version(env!("CARGO_PKG_VERSION"))
            .developer_name("Sudhir Dhumal")
            .developers(["Sudhir Dhumal"])
            .copyright("© 2026 Sudhir Dhumal")
            .license_type(gtk4::License::MitX11)
            .website(REPOSITORY_URL)
            .issue_url(format!("{REPOSITORY_URL}/issues"))
            .comments(
                "Connect to Windows 365 Cloud PCs, Azure Virtual Desktop and any Remote \
                 Desktop (RDP) computer.",
            )
            .debug_info(format!(
                "Windows App {}\nFreeRDP {}\nGTK {}.{}.{}\nlibadwaita {}.{}.{}\n",
                env!("CARGO_PKG_VERSION"),
                rdp::version(),
                gtk4::major_version(),
                gtk4::minor_version(),
                gtk4::micro_version(),
                adw::major_version(),
                adw::minor_version(),
                adw::micro_version(),
            ))
            .build();
        about.add_credit_section(
            Some("Built With"),
            &[
                "FreeRDP https://www.freerdp.com",
                "GTK https://www.gtk.org",
                "libadwaita https://gnome.pages.gitlab.gnome.org/libadwaita/",
                "gtk-rs https://gtk-rs.org",
            ],
        );
        about.add_legal_section(
            "Trademarks",
            None,
            gtk4::License::Unknown,
            Some(
                "Windows, Windows 365 and Azure Virtual Desktop are trademarks of the \
                 Microsoft group of companies. This app is not affiliated with or endorsed \
                 by Microsoft.",
            ),
        );
        about.present(Some(&self.window));
    }

    /// Connects to `target` now, or once the connection lists have loaded.
    pub fn connect_target(self: &Rc<Self>, target: LaunchTarget) {
        match &target {
            LaunchTarget::RemotePc(id) => {
                let connection = self
                    .remote_pcs
                    .borrow()
                    .iter()
                    .find(|c| &c.id == id)
                    .cloned();
                match connection {
                    Some(connection) => self.connect_saved_remote(connection),
                    None => self.report_missing(&target),
                }
                return;
            }
            LaunchTarget::CloudPc(id) => {
                let pc = self
                    .cached_pcs
                    .borrow()
                    .iter()
                    .find(|p| &p.id == id)
                    .cloned();
                if let Some(pc) = pc {
                    self.connect_cloud_pc(pc);
                    return;
                }
            }
            LaunchTarget::Avd(id) => {
                let resource = self
                    .avd_resources
                    .borrow()
                    .iter()
                    .find(|r| &r.id == id)
                    .cloned();
                if let Some(resource) = resource {
                    self.connect_avd(resource);
                    return;
                }
            }
        }
        if self.loading_pcs.get() {
            *self.pending_target.borrow_mut() = Some(target);
        } else if self.auth.is_signed_in() {
            *self.pending_target.borrow_mut() = Some(target);
            self.load_cloud_pcs();
        } else {
            self.status_label
                .set_text("Sign in to connect to your Cloud PCs.");
        }
    }

    fn report_missing(&self, target: &LaunchTarget) {
        let name = self
            .search_index
            .find(target)
            .map_or_else(|| target.to_string(), |e| e.name);
        log::warn!("Launch target {target} is not available");
        self.status_label
            .set_text(&format!("{name} is no longer available to this account."));
    }

    /// Mirrors the listed connections into the desktop search index.
    fn update_search_index(&self) {
        let entries: Vec<LaunchEntry> = self
            .cached_pcs
            .borrow()
            .iter()
            .map(LaunchEntry::cloud_pc)
            .chain(self.avd_resources.borrow().iter().map(LaunchEntry::avd))
            .chain(self.remote_pcs.borrow().iter().map(LaunchEntry::remote_pc))
            .collect();
        if let Err(e) = self.search_index.save(&entries) {
            log::warn!("Could not update the search index: {e}");
        }
    }

    fn launch_entry(&self, target: &LaunchTarget) -> Option<LaunchEntry> {
        match target {
            LaunchTarget::CloudPc(id) => self
                .cached_pcs
                .borrow()
                .iter()
                .find(|p| &p.id == id)
                .map(LaunchEntry::cloud_pc),
            LaunchTarget::Avd(id) => self
                .avd_resources
                .borrow()
                .iter()
                .find(|r| &r.id == id)
                .map(LaunchEntry::avd),
            LaunchTarget::RemotePc(id) => self
                .remote_pcs
                .borrow()
                .iter()
                .find(|c| &c.id == id)
                .map(LaunchEntry::remote_pc),
        }
    }

    /// Adds or removes the app launcher (`login == false`) or login entry for `target`.
    fn set_desktop_entry(self: &Rc<Self>, target: &LaunchTarget, login: bool, enable: bool) {
        let Some(entry) = self.launch_entry(target) else {
            return;
        };
        let result = match (login, enable) {
            (false, true) => platform::install_launcher(&entry).map(|_| ()),
            (false, false) => platform::remove_launcher(target),
            (true, enabled) => platform::set_autostart(&entry, enabled),
        };
        let message = match (result, login, enable) {
            (Err(e), ..) => {
                log::error!("Updating the desktop entry for {target} failed: {e}");
                format!("Could not update {}: {e}", entry.name)
            }
            (Ok(()), false, true) => format!("{} was added to the app launcher.", entry.name),
            (Ok(()), false, false) => {
                format!("{} was removed from the app launcher.", entry.name)
            }
            (Ok(()), true, true) => format!("{} will connect when you log in.", entry.name),
            (Ok(()), true, false) => {
                format!("{} will no longer connect at login.", entry.name)
            }
        };
        self.status_label.set_text(&message);
        self.render_cards();
    }

    fn set_layout_mode(self: &Rc<Self>, mode: CardLayoutMode) {
        *self.current_mode.borrow_mut() = mode;
        let (button, name) = match mode {
            CardLayoutMode::Grid => (&self.grid_btn, "grid"),
            CardLayoutMode::List => (&self.list_btn, "list"),
        };
        button.set_active(true);
        self.content_stack.set_visible_child_name(name);

        let mut config = AppConfig::load();
        name.clone_into(&mut config.view_mode);
        if let Err(e) = config.save() {
            log::warn!("Could not save the view mode: {e}");
        }
        self.render_cards();
    }

    /// Rebuilds the main menu: the account and Sign Out appear only while signed in;
    /// other remembered accounts can be switched to.
    fn update_account_header(&self) {
        let account = self.auth.current_account();
        let active_upn = account.as_ref().and_then(|a| a.upn.clone());

        let menu = gio::Menu::new();
        if account.is_some() {
            let account_item = gio::MenuItem::new(None, None);
            account_item.set_attribute_value("custom", Some(&"account".to_variant()));
            let account_section = gio::Menu::new();
            account_section.append_item(&account_item);
            menu.append_section(None, &account_section);
        }
        let others: Vec<Account> = self
            .auth
            .accounts()
            .into_iter()
            .filter(|a| a.upn != active_upn)
            .collect();
        if !others.is_empty() {
            let switch_section = gio::Menu::new();
            for other in &others {
                let Some(upn) = other.upn.as_deref() else {
                    continue;
                };
                let label = match other.display_name.as_deref() {
                    Some(name) if name != upn => format!("Switch to {name} ({upn})"),
                    _ => format!("Switch to {upn}"),
                };
                let item = gio::MenuItem::new(Some(&label), None);
                item.set_action_and_target_value(
                    Some("win.switch-account"),
                    Some(&upn.to_variant()),
                );
                switch_section.append_item(&item);
            }
            menu.append_section(None, &switch_section);
        }
        let options_section = gio::Menu::new();
        options_section.append(Some("Add Connection…"), Some("win.add-connection"));
        options_section.append(Some("Preferences"), Some("win.preferences"));
        options_section.append(Some("About Windows App"), Some("win.about"));
        menu.append_section(None, &options_section);
        if account.is_some() {
            let signout_section = gio::Menu::new();
            signout_section.append(Some("Add Another Account…"), Some("win.add-cloud-pc"));
            signout_section.append(Some("Sign Out"), Some("win.sign-out"));
            menu.append_section(None, &signout_section);
        }
        let quit_section = gio::Menu::new();
        quit_section.append(Some("Quit"), Some("window.close"));
        menu.append_section(None, &quit_section);

        let popover = PopoverMenu::from_model(Some(&menu));
        if let Some(account) = account {
            let account_box = Box::new(Orientation::Vertical, 2);
            account_box.set_margin_start(12);
            account_box.set_margin_end(12);
            account_box.set_margin_top(6);
            account_box.set_margin_bottom(6);
            let name_label = Label::new(Some(account.label().unwrap_or("Signed in")));
            name_label.set_halign(gtk4::Align::Start);
            name_label.add_css_class("heading");
            account_box.append(&name_label);
            if let Some(upn) = account
                .upn
                .as_deref()
                .filter(|u| Some(*u) != account.label())
            {
                let upn_label = Label::new(Some(upn));
                upn_label.set_halign(gtk4::Align::Start);
                upn_label.add_css_class("dim-label");
                account_box.append(&upn_label);
            }
            popover.add_child(&account_box, "account");
        }
        self.menu_button.set_popover(Some(&popover));
    }

    /// Registers the window actions used by the header and menus.
    fn install_actions(self: &Rc<Self>) {
        let add = |name: &str, f: fn(&Rc<Self>)| {
            let action = gio::SimpleAction::new(name, None);
            let weak = Rc::downgrade(self);
            action.connect_activate(move |_, _| {
                if let Some(this) = weak.upgrade() {
                    f(&this);
                }
            });
            self.window.add_action(&action);
        };
        add("add-connection", Self::show_add_connection);
        add("add-cloud-pc", Self::sign_in);
        add("add-remote-pc", |this| this.edit_remote_pc(None));
        add("import-rdp", Self::import_rdp_file);
        add("find", |this| {
            this.search_bar.set_search_mode(true);
            this.search_entry.grab_focus();
        });
        add("preferences", Self::show_preferences);
        add("sign-out", Self::sign_out);
        add("about", |this| this.show_about());

        let switch = gio::SimpleAction::new("switch-account", Some(glib::VariantTy::STRING));
        let weak = Rc::downgrade(self);
        switch.connect_activate(move |_, upn| {
            let (Some(this), Some(upn)) = (weak.upgrade(), upn.and_then(|v| v.get::<String>()))
            else {
                return;
            };
            this.switch_account(&upn);
        });
        self.window.add_action(&switch);
    }

    fn switch_account(self: &Rc<Self>, upn: &str) {
        match self.auth.switch_account(upn) {
            Ok(true) => {
                self.cached_pcs.borrow_mut().clear();
                self.avd_resources.borrow_mut().clear();
                self.status_label
                    .set_text(&format!("Switched to {upn}. Loading Cloud PCs…"));
                self.load_cloud_pcs();
            }
            Ok(false) => {
                self.status_label
                    .set_text(&format!("{upn} needs to sign in again."));
                self.update_account_header();
            }
            Err(e) => self
                .status_label
                .set_text(&format!("Could not switch accounts: {e}")),
        }
    }

    /// Asks which kind of connection to add.
    fn show_add_connection(self: &Rc<Self>) {
        let dialog = adw::Dialog::builder()
            .title("Add Connection")
            .content_width(440)
            .build();

        let cloud_subtitle = match account::current_upn().filter(|_| self.auth.is_signed_in()) {
            Some(upn) => format!(
                "Signed in as {upn}. Sign in again to use a different work or school account."
            ),
            None => {
                "Windows 365 or Azure Virtual Desktop, with a work or school account".to_string()
            }
        };
        let choices = ListBox::new();
        choices.add_css_class("boxed-list");
        choices.set_selection_mode(gtk4::SelectionMode::None);
        for (title, subtitle, icon, action) in [
            (
                "Microsoft Cloud PC",
                cloud_subtitle.as_str(),
                None,
                "win.add-cloud-pc",
            ),
            (
                "Remote PC",
                "A Windows PC on your network, by name or IP address",
                Some("preferences-desktop-remote-desktop-symbolic"),
                "win.add-remote-pc",
            ),
            (
                "Import RDP File",
                "Add a Remote PC from a .rdp connection file",
                Some("document-open-symbolic"),
                "win.import-rdp",
            ),
        ] {
            let row = adw::ActionRow::builder()
                .title(title)
                .subtitle(glib::markup_escape_text(subtitle).as_str())
                .activatable(true)
                .build();
            let prefix: gtk4::Widget = match icon {
                Some(name) => {
                    let image = Image::from_icon_name(name);
                    image.set_pixel_size(28);
                    image.upcast()
                }
                None => {
                    let glyph = Label::new(Some("⊞"));
                    glyph.add_css_class("title-2");
                    glyph.upcast()
                }
            };
            prefix.set_width_request(36);
            row.add_prefix(&prefix);
            row.add_suffix(&Image::from_icon_name("go-next-symbolic"));
            let dlg = dialog.downgrade();
            let window = self.window.downgrade();
            row.connect_activated(move |_| {
                if let Some(dialog) = dlg.upgrade() {
                    dialog.close();
                }
                if let Some(window) = window.upgrade() {
                    let _ = WidgetExt::activate_action(&window, action, None);
                }
            });
            choices.append(&row);
        }

        let content = Box::new(Orientation::Vertical, 12);
        for set_margin in [
            Box::set_margin_start,
            Box::set_margin_end,
            Box::set_margin_bottom,
        ] {
            set_margin(&content, 18);
        }
        content.set_margin_top(6);
        content.append(&choices);

        let toolbar_view = adw::ToolbarView::new();
        toolbar_view.add_top_bar(&adw::HeaderBar::new());
        toolbar_view.set_content(Some(&content));
        dialog.set_child(Some(&toolbar_view));
        dialog.present(Some(&self.window));
    }

    fn show_preferences(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        PreferencesDialog::show(&self.window, move |_| {
            if let Some(this) = weak.upgrade() {
                this.status_label.set_text("Preferences saved.");
            }
        });
    }

    fn sign_out(self: &Rc<Self>) {
        if let Err(e) = self.auth.sign_out() {
            log::error!("Sign-out failed: {e}");
            self.status_label
                .set_text(&format!("Could not remove the stored sign-in: {e}"));
            return;
        }
        AppConfig::remove_legacy_files();
        self.cached_pcs.borrow_mut().clear();
        self.avd_resources.borrow_mut().clear();
        self.update_search_index();
        self.update_account_header();
        self.refresh_view();
        self.status_label.set_text("Signed out.");
    }

    /// Microsoft sign-in; the dialog itself offers the alternative methods.
    fn sign_in(self: &Rc<Self>) {
        self.status_label.set_text("Opening Microsoft sign-in…");
        let weak = Rc::downgrade(self);
        LoginDialog::present(&self.window, Arc::clone(&self.auth), move |signed_in| {
            let Some(this) = weak.upgrade() else { return };
            if signed_in {
                // The new sign-in may be a different account.
                this.auth.forget_scoped_tokens();
                this.status_label.set_text("Signed in. Loading Cloud PCs…");
                this.load_cloud_pcs();
            } else {
                this.status_label.set_text("Ready");
            }
        });
    }

    /// Loads the Cloud PCs and then the Azure Virtual Desktop feed of the active account.
    fn load_cloud_pcs(self: &Rc<Self>) {
        self.update_account_header();
        if !self.auth.is_signed_in() {
            self.cached_pcs.borrow_mut().clear();
            self.avd_resources.borrow_mut().clear();
            self.refresh_view();
            if let Some(target) = self.pending_target.take() {
                self.connect_target(target);
            }
            return;
        }
        if self.loading_pcs.replace(true) {
            return;
        }
        self.spinner.start();
        self.status_label
            .set_text("Fetching Cloud PCs from Windows 365…");
        self.refresh_view();

        let service = Arc::clone(&self.cloud_service);
        let weak = Rc::downgrade(self);
        glib::MainContext::default().spawn_local(async move {
            let result = service.list_user_cloud_pcs().await;
            // The AVD feed is optional: most Windows 365 users have only Cloud PCs.
            let avd = match service.list_avd_resources().await {
                Ok(resources) => resources,
                Err(e) => {
                    log::warn!("Loading the Azure Virtual Desktop feed failed: {e}");
                    Vec::new()
                }
            };
            let Some(this) = weak.upgrade() else { return };
            this.loading_pcs.set(false);
            this.spinner.stop();
            match result {
                Ok(pcs) => {
                    // Cloud PCs are published in the feed too; show them only once.
                    let cloud_ids: HashSet<String> =
                        pcs.iter().filter_map(api::launch_resource_id).collect();
                    let avd: Vec<AvdResource> = avd
                        .into_iter()
                        .filter(|r| !cloud_ids.contains(&r.id))
                        .collect();
                    log::debug!(
                        "Loaded {} Cloud PCs and {} AVD resources",
                        pcs.len(),
                        avd.len()
                    );
                    this.status_label
                        .set_text(&load_summary(pcs.len(), avd.len()));
                    *this.cached_pcs.borrow_mut() = pcs;
                    *this.avd_resources.borrow_mut() = avd;
                }
                Err(e) => {
                    log::error!("Loading Cloud PCs failed: {e}");
                    this.cached_pcs.borrow_mut().clear();
                    this.status_label.set_text(&load_error_message(&e));
                    *this.avd_resources.borrow_mut() = avd;
                }
            }
            this.update_search_index();
            this.update_account_header();
            this.refresh_view();
            if let Some(target) = this.pending_target.take() {
                let available = this.launch_entry(&target).is_some();
                if available {
                    this.connect_target(target);
                } else {
                    this.report_missing(&target);
                }
            }
        });
    }

    /// Shows the cards, the loading page or the empty page, and the matching header buttons.
    fn refresh_view(self: &Rc<Self>) {
        let signed_in = self.auth.is_signed_in();
        let has_items = !self.cached_pcs.borrow().is_empty()
            || !self.avd_resources.borrow().is_empty()
            || !self.remote_pcs.borrow().is_empty();
        self.refresh_btn.set_visible(signed_in);
        self.refresh_btn.set_sensitive(!self.loading_pcs.get());
        self.layout_box.set_visible(has_items);

        if has_items {
            self.render_cards();
            self.stack.set_visible_child_name("content");
        } else if self.loading_pcs.get() {
            self.stack.set_visible_child_name("loading");
        } else {
            self.empty_page.set_description(Some(if signed_in {
                "No Cloud PCs are assigned to your account. Use the + button to add a Remote PC."
            } else {
                "Use the + button to add a Microsoft Cloud PC or a Remote PC."
            }));
            self.stack.set_visible_child_name("empty");
        }
    }

    fn render_cards(self: &Rc<Self>) {
        for container in [&self.grid_sections, &self.list_sections] {
            while let Some(child) = container.first_child() {
                container.remove(&child);
            }
        }

        let mode = *self.current_mode.borrow();
        let on_action = || {
            let weak = Rc::downgrade(self);
            move |action| {
                if let Some(this) = weak.upgrade() {
                    this.handle_card_action(action);
                }
            }
        };
        let connecting = self.connecting_pcs.borrow();
        let filter = SearchFilter::new(&self.search_entry.text());
        let recent = self.recent.load();
        let now = chrono::Utc::now();
        let last_used = |target: LaunchTarget| {
            recent
                .get(&target.to_string())
                .map(|when| history::relative_time(*when, now))
        };

        let mut cloud_pcs = Vec::new();
        for pc in self.cached_pcs.borrow().iter() {
            let account = pc.user_principal_name.as_deref().unwrap_or_default();
            if !filter.matches(&[&pc.display_name, account]) {
                continue;
            }
            cloud_pcs.push(ConnectionCard::cloud_pc(
                pc,
                mode,
                connecting.contains(&pc.id),
                last_used(LaunchTarget::CloudPc(pc.id.clone())),
                on_action(),
            ));
        }
        let mut avd = Vec::new();
        for resource in self.avd_resources.borrow().iter() {
            let folder = resource.folder.as_deref().unwrap_or_default();
            if !filter.matches(&[&resource.title, &resource.workspace, folder]) {
                continue;
            }
            avd.push(ConnectionCard::avd_resource(
                resource,
                mode,
                connecting.contains(&resource.id),
                last_used(LaunchTarget::Avd(resource.id.clone())),
                on_action(),
            ));
        }
        // Grouped connections first, by group, then by name.
        let mut remote_pcs = self.remote_pcs.borrow().clone();
        remote_pcs.sort_by_cached_key(|c| {
            (
                c.group.is_empty(),
                c.group.to_lowercase(),
                c.display_name().to_lowercase(),
            )
        });
        let mut remotes = Vec::new();
        for connection in &remote_pcs {
            if !filter.matches(&[
                connection.display_name(),
                &connection.address,
                &connection.group,
                &connection.username,
            ]) {
                continue;
            }
            remotes.push(ConnectionCard::remote_pc(
                connection,
                mode,
                last_used(LaunchTarget::RemotePc(connection.id.clone())),
                on_action(),
            ));
        }

        let shown = cloud_pcs.len() + avd.len() + remotes.len();
        let container = match mode {
            CardLayoutMode::Grid => &self.grid_sections,
            CardLayoutMode::List => &self.list_sections,
        };
        for (id, title, items) in [
            (SECTION_CLOUD_PCS, "Cloud PCs", cloud_pcs),
            (SECTION_AVD, "Azure Virtual Desktop", avd),
            (SECTION_REMOTE_PCS, "Remote PCs", remotes),
        ] {
            if !items.is_empty() {
                // Searching shows every match, even in collapsed sections.
                let expanded = !filter.is_empty() || !self.collapsed_sections.borrow().contains(id);
                container.append(&self.section(id, title, mode, expanded, items));
            }
        }
        if !filter.is_empty() {
            let text = self.search_entry.text();
            let message = match (shown, connections::parse_address(&text).is_ok()) {
                (0, true) => format!("No matches. Press Enter to connect to {}.", text.trim()),
                (0, false) => "No matches.".to_string(),
                (1, _) => "1 match. Press Enter to connect.".to_string(),
                (n, _) => format!("{n} matches."),
            };
            self.status_label.set_text(&message);
        }
    }

    /// A heading that collapses or expands `items`, laid out as cards or as a list.
    fn section(
        self: &Rc<Self>,
        id: &'static str,
        title: &str,
        mode: CardLayoutMode,
        expanded: bool,
        items: Vec<gtk4::Widget>,
    ) -> Box {
        let chevron = Image::from_icon_name(chevron_icon(expanded));
        let title_label = Label::new(Some(title));
        title_label.add_css_class("heading");
        let count_label = Label::new(Some(&items.len().to_string()));
        count_label.add_css_class("dim-label");
        count_label.add_css_class("numeric");
        let heading = Box::new(Orientation::Horizontal, 8);
        heading.append(&chevron);
        heading.append(&title_label);
        heading.append(&count_label);
        let header = Button::builder()
            .child(&heading)
            .halign(gtk4::Align::Start)
            .tooltip_text(if expanded { "Collapse" } else { "Expand" })
            .build();
        header.add_css_class("flat");
        header.update_state(&[gtk4::accessible::State::Expanded(Some(expanded))]);

        let content: gtk4::Widget = match mode {
            CardLayoutMode::Grid => {
                let flowbox = FlowBox::builder()
                    .valign(gtk4::Align::Start)
                    .max_children_per_line(6)
                    .min_children_per_line(1)
                    .selection_mode(gtk4::SelectionMode::None)
                    .column_spacing(16)
                    .row_spacing(16)
                    .homogeneous(true)
                    .build();
                for item in &items {
                    flowbox.append(item);
                }
                flowbox.upcast()
            }
            CardLayoutMode::List => {
                let list = ListBox::new();
                list.add_css_class("boxed-list");
                list.set_selection_mode(gtk4::SelectionMode::None);
                list.set_valign(gtk4::Align::Start);
                for item in &items {
                    list.append(item);
                }
                list.upcast()
            }
        };
        content.set_margin_top(6);
        let revealer = gtk4::Revealer::builder()
            .transition_type(gtk4::RevealerTransitionType::SlideDown)
            .reveal_child(expanded)
            .child(&content)
            .build();

        let weak = Rc::downgrade(self);
        let revealer_ref = revealer.downgrade();
        let chevron_ref = chevron.downgrade();
        header.connect_clicked(move |header| {
            let (Some(this), Some(revealer), Some(chevron)) = (
                weak.upgrade(),
                revealer_ref.upgrade(),
                chevron_ref.upgrade(),
            ) else {
                return;
            };
            let expanded = !revealer.reveals_child();
            revealer.set_reveal_child(expanded);
            chevron.set_icon_name(Some(chevron_icon(expanded)));
            header.set_tooltip_text(Some(if expanded { "Collapse" } else { "Expand" }));
            header.update_state(&[gtk4::accessible::State::Expanded(Some(expanded))]);
            this.set_section_collapsed(id, !expanded);
        });

        let section = Box::new(Orientation::Vertical, 0);
        section.append(&header);
        section.append(&revealer);
        section
    }

    fn set_section_collapsed(&self, id: &str, collapsed: bool) {
        let changed = {
            let mut sections = self.collapsed_sections.borrow_mut();
            if collapsed {
                sections.insert(id.to_string())
            } else {
                sections.remove(id)
            }
        };
        if !changed {
            return;
        }
        let mut config = AppConfig::load();
        let mut ids: Vec<String> = self.collapsed_sections.borrow().iter().cloned().collect();
        ids.sort();
        config.collapsed_sections = ids;
        if let Err(e) = config.save() {
            log::warn!("Could not save the collapsed sections: {e}");
        }
    }

    fn install_search(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.search_entry.connect_search_changed(move |_| {
            if let Some(this) = weak.upgrade() {
                this.render_cards();
            }
        });
        let weak = Rc::downgrade(self);
        self.search_entry.connect_activate(move |entry| {
            if let Some(this) = weak.upgrade() {
                this.connect_from_search(&entry.text());
            }
        });
        let weak = Rc::downgrade(self);
        self.search_bar
            .connect_search_mode_enabled_notify(move |bar| {
                if let Some(this) = weak.upgrade() {
                    if !bar.is_search_mode() && !this.search_entry.text().is_empty() {
                        this.search_entry.set_text("");
                    }
                }
            });
    }

    /// Enter in the search: connect to the only matching connection, or quick-connect
    /// to the typed address.
    fn connect_from_search(self: &Rc<Self>, text: &str) {
        let filter = SearchFilter::new(text);
        if filter.is_empty() {
            return;
        }
        let pcs: Vec<CloudPc> = self
            .cached_pcs
            .borrow()
            .iter()
            .filter(|pc| {
                filter.matches(&[
                    &pc.display_name,
                    pc.user_principal_name.as_deref().unwrap_or_default(),
                ])
            })
            .cloned()
            .collect();
        let avd: Vec<AvdResource> = self
            .avd_resources
            .borrow()
            .iter()
            .filter(|r| {
                filter.matches(&[
                    &r.title,
                    &r.workspace,
                    r.folder.as_deref().unwrap_or_default(),
                ])
            })
            .cloned()
            .collect();
        let remote: Vec<SavedConnection> = self
            .remote_pcs
            .borrow()
            .iter()
            .filter(|c| filter.matches(&[c.display_name(), &c.address, &c.group, &c.username]))
            .cloned()
            .collect();
        match (pcs.as_slice(), avd.as_slice(), remote.as_slice()) {
            ([pc], [], []) => self.connect_cloud_pc(pc.clone()),
            ([], [resource], []) => self.connect_avd(resource.clone()),
            ([], [], [connection]) => self.connect_saved_remote(connection.clone()),
            ([], [], []) => match SavedConnection::new("", text, "") {
                Ok(connection) => {
                    self.connect_remote_pc(connection, false, ConnectionOverrides::default());
                }
                Err(e) => self.status_label.set_text(&e.to_string()),
            },
            _ => {}
        }
    }

    fn handle_card_action(self: &Rc<Self>, action: CardAction) {
        match action {
            CardAction::Connect(pc) => self.connect_cloud_pc(pc),
            CardAction::Restart(pc) => self.restart_cloud_pc(&pc),
            CardAction::Rename(pc) => self.rename_cloud_pc(&pc),
            CardAction::Inspect(pc) => self.inspect_connection(&pc),
            CardAction::Details(pc) => self.view_details(&pc),
            CardAction::Troubleshoot(pc) => self.troubleshoot_cloud_pc(pc),
            CardAction::Restore(pc) => self.restore_cloud_pc(pc),
            CardAction::Reset(pc) => self.reset_cloud_pc(pc),
            CardAction::Power(pc, start) => self.set_cloud_pc_power(pc, start),
            CardAction::ConnectAvd(resource) => self.connect_avd(resource),
            CardAction::ConnectRemote(connection) => self.connect_saved_remote(connection),
            CardAction::EditRemote(connection) => self.edit_remote_pc(Some(connection)),
            CardAction::RemoveRemote(connection) => self.remove_remote_pc(connection),
            CardAction::ToggleLauncher(target, enable) => {
                self.set_desktop_entry(&target, false, enable);
            }
            CardAction::ToggleAutostart(target, enable) => {
                self.set_desktop_entry(&target, true, enable);
            }
            CardAction::Settings => self.show_preferences(),
            CardAction::ConnectionSettings(target, name) => {
                super::connection_settings_dialog::show(&self.window, target, &name);
            }
            CardAction::ExportRemote(connection) => self.export_remote_pc(connection),
            CardAction::ForgetPassword(mut connection) => {
                credentials_dialog::forget(&connection.id);
                connection.remember_password = false;
                self.save_remote_pc(connection);
                self.status_label
                    .set_text("The saved password was removed.");
            }
        }
    }

    fn set_connecting(self: &Rc<Self>, pc_id: &str, connecting: bool) {
        let changed = if connecting {
            self.connecting_pcs.borrow_mut().insert(pc_id.to_string())
        } else {
            self.connecting_pcs.borrow_mut().remove(pc_id)
        };
        if changed {
            self.render_cards();
        }
    }

    /// Session options from the preferences at the display's scale, sized for the
    /// session window (which opens fullscreen) so the host starts at the final size:
    /// a later resize may be lost while Windows completes the sign-in. The scale can then
    /// be changed from the session window.
    fn session_options(
        &self,
        config: &AppConfig,
        username: Option<String>,
    ) -> session::SessionOptions {
        let scale = session::clamp_scale(platform::desktop_scale_percent().unwrap_or(100));
        let (width, height) = self
            .monitor_device_size()
            .unwrap_or((SESSION_WIDTH * scale / 100, SESSION_HEIGHT * scale / 100));
        session::SessionOptions::from_config(config, width, height, scale, username)
    }

    /// Size in device pixels of the monitor showing this window.
    fn monitor_device_size(&self) -> Option<(u32, u32)> {
        let surface = self.window.surface()?;
        let monitor = WidgetExt::display(&self.window).monitor_at_surface(&surface)?;
        let geometry = monitor.geometry();
        let to_device = |v: i32| (f64::from(v) * monitor.scale()).round() as u32;
        let size = (to_device(geometry.width()), to_device(geometry.height()));
        (size.0 > 0 && size.1 > 0).then_some(size)
    }

    /// Applies the connection's settings to the preferences: the configuration for the
    /// session arguments and the session options.
    fn launch_options(
        &self,
        overrides: &ConnectionOverrides,
        username: Option<String>,
    ) -> (AppConfig, session::SessionOptions) {
        let config = overrides.apply(&AppConfig::load());
        let options = self
            .session_options(&config, username)
            .with_resolution(overrides.resolution);
        (config, options)
    }

    /// Runs the connection's command before connecting (if any) and opens the session.
    fn start_session(self: &Rc<Self>, launch: Launch, prepared: session::PreparedSession) {
        let command = launch.overrides.pre_command.clone();
        if command.is_empty() {
            self.open_session(launch, prepared);
            return;
        }
        self.status_label.set_text(&format!(
            "Running the command before connecting to {}…",
            launch.name
        ));
        let context = hook_context(&launch);
        let weak = Rc::downgrade(self);
        glib::MainContext::default().spawn_local(async move {
            let result = gio::spawn_blocking(move || {
                hooks::run_pre_connect(&command, &context, hooks::PRE_COMMAND_TIMEOUT)
            })
            .await;
            let Some(this) = weak.upgrade() else { return };
            match result {
                Ok(Ok(())) => this.open_session(launch, prepared),
                Ok(Err(e)) => {
                    log::warn!("Command before connecting to {} failed: {e}", launch.name);
                    this.status_label
                        .set_text(&format!("Not connecting to {}: {e}", launch.name));
                }
                Err(_) => this
                    .status_label
                    .set_text("The command before connecting failed."),
            }
        });
    }

    fn open_session(self: &Rc<Self>, launch: Launch, prepared: session::PreparedSession) {
        log::debug!(
            "Session arguments: {:?}",
            session::redact_args(&prepared.args)
        );
        self.status_label
            .set_text(&format!("Connecting to {}…", launch.name));
        if let Some(target) = &launch.target {
            if let Err(e) = self.recent.record(target, chrono::Utc::now()) {
                log::warn!("Could not record the connection time: {e}");
            }
            self.render_cards();
        }
        let weak = Rc::downgrade(self);
        let options = SessionWindowOptions {
            login_hint: launch.login_hint.clone(),
            multi_monitor: launch.multi_monitor,
            dynamic_resolution: launch.dynamic_resolution,
            remember_placement: launch.target.clone(),
        };
        let name = launch.name.clone();
        SessionWindow::open(&name, options, prepared, move || {
            let post_command = &launch.overrides.post_command;
            if !post_command.is_empty() {
                if let Err(e) = hooks::spawn_post_disconnect(post_command, &hook_context(&launch)) {
                    log::warn!(
                        "Command after disconnecting from {} failed: {e}",
                        launch.name
                    );
                }
            }
            if let Some(this) = weak.upgrade() {
                this.status_label
                    .set_text(&format!("Session with {} ended.", launch.name));
            }
        });
    }

    /// Fetches the connection details and opens a session window for `pc`.
    fn connect_cloud_pc(self: &Rc<Self>, pc: CloudPc) {
        if self.connecting_pcs.borrow().contains(&pc.id) {
            return;
        }
        self.set_connecting(&pc.id, true);
        self.status_label
            .set_text(&format!("Preparing connection to {}…", pc.display_name));

        let service = Arc::clone(&self.cloud_service);
        let weak = Rc::downgrade(self);
        glib::MainContext::default().spawn_local(async move {
            let launch = service.fetch_rdp_launch(&pc).await;
            let Some(this) = weak.upgrade() else { return };
            let login_hint = pc.user_principal_name.clone().or_else(account::current_upn);
            let target = LaunchTarget::CloudPc(pc.id.clone());
            let overrides = this.overrides.get(&target);
            let (config, options) = this.launch_options(&overrides, login_hint.clone());
            let prepared = launch.map_err(|e| e.to_string()).and_then(|launch| {
                session::prepare(&launch, &options)
                    .map_err(|e| format!("could not write the session file: {e}"))
            });
            match prepared {
                Ok(prepared) => this.start_session(
                    Launch {
                        name: pc.display_name.clone(),
                        target: Some(target),
                        address: String::new(),
                        login_hint,
                        overrides,
                        multi_monitor: config.multi_monitor,
                        dynamic_resolution: options.dynamic_resolution,
                    },
                    prepared,
                ),
                Err(e) => {
                    log::error!("Connecting to {} failed: {e}", pc.display_name);
                    this.status_label
                        .set_text(&format!("Could not connect: {e}"));
                }
            }
            this.set_connecting(&pc.id, false);
        });
    }

    /// Downloads the signed `.rdp` file of an Azure Virtual Desktop resource and opens it.
    fn connect_avd(self: &Rc<Self>, resource: AvdResource) {
        if self.connecting_pcs.borrow().contains(&resource.id) {
            return;
        }
        let Some(rdp_url) = resource.rdp_url.clone() else {
            self.status_label.set_text(&format!(
                "{} cannot be opened: the workspace did not publish a connection file.",
                resource.title
            ));
            return;
        };
        self.set_connecting(&resource.id, true);
        self.status_label
            .set_text(&format!("Preparing connection to {}…", resource.title));

        let service = Arc::clone(&self.cloud_service);
        let weak = Rc::downgrade(self);
        glib::MainContext::default().spawn_local(async move {
            let launch = service.fetch_rdp_file(&rdp_url).await;
            let Some(this) = weak.upgrade() else { return };
            let login_hint = account::current_upn();
            let target = LaunchTarget::Avd(resource.id.clone());
            let overrides = this.overrides.get(&target);
            let (config, options) = this.launch_options(&overrides, login_hint.clone());
            let prepared = launch.map_err(|e| e.to_string()).and_then(|launch| {
                session::prepare(&launch, &options)
                    .map_err(|e| format!("could not write the session file: {e}"))
            });
            match prepared {
                Ok(prepared) => this.start_session(
                    Launch {
                        name: resource.title.clone(),
                        target: Some(target),
                        address: String::new(),
                        login_hint,
                        overrides,
                        multi_monitor: config.multi_monitor,
                        dynamic_resolution: options.dynamic_resolution,
                    },
                    prepared,
                ),
                Err(e) => {
                    log::error!("Connecting to {} failed: {e}", resource.title);
                    this.status_label
                        .set_text(&format!("Could not connect: {e}"));
                }
            }
            this.set_connecting(&resource.id, false);
        });
    }

    /// Connects to a saved Remote PC with its own settings.
    fn connect_saved_remote(self: &Rc<Self>, connection: SavedConnection) {
        let overrides = self
            .overrides
            .get(&LaunchTarget::RemotePc(connection.id.clone()));
        self.connect_remote_pc(connection, true, overrides);
    }

    /// Signs in (saved password or prompt) and connects to `connection`. `saved`: the
    /// connection is in the list (so it has history and can remember passwords).
    fn connect_remote_pc(
        self: &Rc<Self>,
        connection: SavedConnection,
        saved: bool,
        overrides: ConnectionOverrides,
    ) {
        let remembered = connection.remember_password;
        let weak = Rc::downgrade(self);
        credentials_dialog::obtain(&self.window, connection, saved, move |sign_in| {
            let Some(this) = weak.upgrade() else { return };
            let SignIn {
                connection,
                credentials,
            } = sign_in;
            if saved && connection.remember_password != remembered {
                this.store_remote_pc(connection.clone());
            }
            let username = Some(connection.username.clone()).filter(|u| !u.is_empty());
            let (config, options) = this.launch_options(&overrides, username);
            match session::prepare_direct(&connection, &credentials, &options) {
                Ok(prepared) => this.start_session(
                    Launch {
                        name: connection.display_name().to_string(),
                        target: saved.then(|| LaunchTarget::RemotePc(connection.id.clone())),
                        address: connection.address.clone(),
                        login_hint: None,
                        overrides,
                        multi_monitor: config.multi_monitor,
                        dynamic_resolution: options.dynamic_resolution,
                    },
                    prepared,
                ),
                Err(e) => this
                    .status_label
                    .set_text(&format!("Could not connect: {e}")),
            }
        });
    }

    /// Adds a Remote PC (`None`) or edits a saved one.
    fn edit_remote_pc(self: &Rc<Self>, existing: Option<SavedConnection>) {
        let remembered = existing.as_ref().is_some_and(|c| c.remember_password);
        let weak = Rc::downgrade(self);
        super::remote_pc_dialog::show(&self.window, existing, move |connection| {
            if remembered && !connection.remember_password {
                credentials_dialog::forget(&connection.id);
            }
            if let Some(this) = weak.upgrade() {
                this.save_remote_pc(connection);
            }
        });
    }

    /// Imports a `.rdp` file chosen by the user as a new Remote PC.
    fn import_rdp_file(self: &Rc<Self>) {
        let dialog = gtk4::FileDialog::builder()
            .title("Import RDP File")
            .modal(true)
            .default_filter(&rdp_file_filter())
            .build();
        let weak = Rc::downgrade(self);
        dialog.open(Some(&self.window), gio::Cancellable::NONE, move |result| {
            let (Some(this), Some(path)) = (weak.upgrade(), result.ok().and_then(|f| f.path()))
            else {
                return;
            };
            match read_rdp_file(&path) {
                Ok(imported) => {
                    let target = LaunchTarget::RemotePc(imported.connection.id.clone());
                    if let Err(e) = this.overrides.set(&target, imported.overrides) {
                        log::warn!("Could not save the imported settings: {e}");
                    }
                    this.save_remote_pc(imported.connection);
                }
                Err(message) => this.status_label.set_text(&message),
            }
        });
    }

    /// Connects to the PC described by the `.rdp` file at `path` without saving it.
    pub fn open_rdp_file(self: &Rc<Self>, path: &std::path::Path) {
        match read_rdp_file(path) {
            Ok(imported) => {
                self.connect_remote_pc(imported.connection, false, imported.overrides);
            }
            Err(message) => self.status_label.set_text(&message),
        }
    }

    /// Saves a Remote PC with its settings as a `.rdp` file (without the password).
    fn export_remote_pc(self: &Rc<Self>, connection: SavedConnection) {
        let file_name = format!("{}.rdp", sanitize_file_name(connection.display_name()));
        let dialog = gtk4::FileDialog::builder()
            .title("Export RDP File")
            .modal(true)
            .initial_name(file_name)
            .default_filter(&rdp_file_filter())
            .build();
        let weak = Rc::downgrade(self);
        dialog.save(Some(&self.window), gio::Cancellable::NONE, move |result| {
            let (Some(this), Some(path)) = (weak.upgrade(), result.ok().and_then(|f| f.path()))
            else {
                return;
            };
            let overrides = this
                .overrides
                .get(&LaunchTarget::RemotePc(connection.id.clone()));
            let text = rdp_file::export(&connection, &overrides);
            match std::fs::write(&path, text) {
                Ok(()) => this.status_label.set_text(&format!(
                    "Exported {} to {}.",
                    connection.display_name(),
                    path.display()
                )),
                Err(e) => this.status_label.set_text(&format!(
                    "Could not export {}: {e}",
                    connection.display_name()
                )),
            }
        });
    }

    /// Updates a saved connection quietly (e.g. the "remember password" choice).
    fn store_remote_pc(self: &Rc<Self>, connection: SavedConnection) {
        match self.connection_store.upsert(connection) {
            Ok(list) => {
                *self.remote_pcs.borrow_mut() = list;
                self.render_cards();
            }
            Err(e) => log::warn!("Could not update the saved connection: {e}"),
        }
    }

    fn save_remote_pc(self: &Rc<Self>, connection: SavedConnection) {
        let name = connection.display_name().to_string();
        match self.connection_store.upsert(connection) {
            Ok(list) => {
                *self.remote_pcs.borrow_mut() = list;
                self.status_label.set_text(&format!("Saved {name}."));
                self.update_search_index();
            }
            Err(e) => {
                log::error!(
                    "Saving {} failed: {e}",
                    self.connection_store.path().display()
                );
                self.status_label
                    .set_text(&format!("Could not save {name}: {e}"));
            }
        }
        self.refresh_view();
    }

    fn remove_remote_pc(self: &Rc<Self>, connection: SavedConnection) {
        let dialog = adw::AlertDialog::builder()
            .heading(format!("Remove {}?", connection.display_name()))
            .body("The saved connection is removed from this computer.")
            .default_response("cancel")
            .close_response("cancel")
            .build();
        dialog.add_responses(&[("cancel", "Cancel"), ("remove", "Remove")]);
        dialog.set_response_appearance("remove", adw::ResponseAppearance::Destructive);
        let weak = Rc::downgrade(self);
        dialog.connect_response(Some("remove"), move |_, _| {
            let Some(this) = weak.upgrade() else { return };
            let name = connection.display_name();
            match this.connection_store.remove(&connection.id) {
                Ok(list) => {
                    *this.remote_pcs.borrow_mut() = list;
                    this.status_label.set_text(&format!("Removed {name}."));
                    let target = LaunchTarget::RemotePc(connection.id.clone());
                    platform::remove_desktop_integration(&target);
                    credentials_dialog::forget(&connection.id);
                    if let Err(e) = this.overrides.remove(&target) {
                        log::warn!("Could not remove the connection settings: {e}");
                    }
                    if let Err(e) = this.recent.remove(&target) {
                        log::warn!("Could not remove the connection history: {e}");
                    }
                    if let Err(e) = PlacementStore::default().remove(&target) {
                        log::warn!("Could not remove the session window placement: {e}");
                    }
                    this.update_search_index();
                }
                Err(e) => this
                    .status_label
                    .set_text(&format!("Could not remove {name}: {e}")),
            }
            this.refresh_view();
        });
        dialog.present(Some(&self.window));
    }

    fn restart_cloud_pc(self: &Rc<Self>, pc: &CloudPc) {
        let dialog = adw::AlertDialog::builder()
            .heading(format!("Restart {}?", pc.display_name))
            .body("Unsaved work on the Cloud PC is lost and open sessions are disconnected.")
            .default_response("cancel")
            .close_response("cancel")
            .build();
        dialog.add_responses(&[("cancel", "Cancel"), ("restart", "Restart")]);
        dialog.set_response_appearance("restart", adw::ResponseAppearance::Destructive);
        let weak = Rc::downgrade(self);
        let pc = pc.clone();
        dialog.connect_response(Some("restart"), move |_, _| {
            let Some(this) = weak.upgrade() else { return };
            let service = Arc::clone(&this.cloud_service);
            let id = pc.id.clone();
            this.run_pc_action(&pc, CloudPcAction::Restart, async move {
                service.restart_cloud_pc(&id).await
            });
        });
        dialog.present(Some(&self.window));
    }

    /// Runs a remote action on `pc`, marks the card busy meanwhile and refreshes the list
    /// so the new state shows.
    fn run_pc_action(
        self: &Rc<Self>,
        pc: &CloudPc,
        action: CloudPcAction,
        request: impl std::future::Future<Output = crate::error::Result<()>> + 'static,
    ) {
        let name = pc.display_name.clone();
        let id = pc.id.clone();
        self.set_connecting(&id, true);
        self.status_label
            .set_text(&format!("{} {name}…", action.progress_label()));
        let weak = Rc::downgrade(self);
        glib::MainContext::default().spawn_local(async move {
            let result = request.await;
            let Some(this) = weak.upgrade() else { return };
            this.set_connecting(&id, false);
            match result {
                Ok(()) => {
                    this.load_cloud_pcs();
                    this.status_label.set_text(&format!(
                        "{} {name} — this can take a few minutes.",
                        action.progress_label()
                    ));
                }
                Err(e) => {
                    log::error!("{action:?} on {name} failed: {e}");
                    this.show_error(&format!("Could not finish on {name}"), &e.to_string());
                }
            }
        });
    }

    fn show_error(&self, heading: &str, body: &str) {
        self.status_label.set_text(&format!("{heading}: {body}"));
        let dialog = adw::AlertDialog::builder()
            .heading(heading)
            .body(body)
            .build();
        dialog.add_response("close", "Close");
        dialog.present(Some(&self.window));
    }

    fn set_cloud_pc_power(self: &Rc<Self>, pc: CloudPc, start: bool) {
        let service = Arc::clone(&self.cloud_service);
        let id = pc.id.clone();
        let action = if start {
            CloudPcAction::Start
        } else {
            CloudPcAction::Stop
        };
        self.run_pc_action(
            &pc,
            action,
            async move { service.set_power(&id, start).await },
        );
    }

    /// Runs the service's connectivity check and repair, then reports what it found.
    fn troubleshoot_cloud_pc(self: &Rc<Self>, pc: CloudPc) {
        if !self.connecting_pcs.borrow_mut().insert(pc.id.clone()) {
            return;
        }
        self.render_cards();
        self.status_label.set_text(&format!(
            "Troubleshooting {} — this can take several minutes…",
            pc.display_name
        ));
        let service = Arc::clone(&self.cloud_service);
        let weak = Rc::downgrade(self);
        glib::MainContext::default().spawn_local(async move {
            let outcome = service.troubleshoot_cloud_pc(&pc.id).await;
            let Some(this) = weak.upgrade() else { return };
            this.set_connecting(&pc.id, false);
            let name = &pc.display_name;
            let (heading, body) = match outcome {
                Ok(TroubleshootOutcome::NoIssues) => (
                    format!("No issues found on {name}"),
                    "The Cloud PC passed all checks. If you still cannot connect, restart it."
                        .to_string(),
                ),
                Ok(TroubleshootOutcome::Fixed) => (
                    format!("Issues fixed on {name}"),
                    "Problems were found and fixed. Try connecting again.".to_string(),
                ),
                Ok(TroubleshootOutcome::Failed(detail)) => {
                    (format!("Issues remain on {name}"), detail)
                }
                Ok(TroubleshootOutcome::StillRunning) => (
                    format!("Still troubleshooting {name}"),
                    "The check is taking longer than usual. Look at the status again later."
                        .to_string(),
                ),
                Err(e) => (format!("Could not troubleshoot {name}"), e.to_string()),
            };
            this.status_label.set_text(&heading);
            let dialog = adw::AlertDialog::builder()
                .heading(heading)
                .body(body)
                .build();
            dialog.add_response("close", "Close");
            dialog.present(Some(&this.window));
            this.load_cloud_pcs();
        });
    }

    /// Lets the user pick a restore point and restores the Cloud PC to it.
    fn restore_cloud_pc(self: &Rc<Self>, pc: CloudPc) {
        self.status_label
            .set_text(&format!("Loading restore points of {}…", pc.display_name));
        let service = Arc::clone(&self.cloud_service);
        let weak = Rc::downgrade(self);
        glib::MainContext::default().spawn_local(async move {
            let snapshots = service.list_snapshots(&pc.id).await;
            let Some(this) = weak.upgrade() else { return };
            match snapshots {
                Ok(list) if list.is_empty() => this.show_error(
                    &format!("No restore points for {}", pc.display_name),
                    "Restore points are created automatically; none are available yet.",
                ),
                Ok(list) => this.present_restore_dialog(pc, list),
                Err(e) => this.show_error(
                    &format!("Could not load restore points of {}", pc.display_name),
                    &e.to_string(),
                ),
            }
        });
    }

    fn present_restore_dialog(self: &Rc<Self>, pc: CloudPc, snapshots: Vec<Snapshot>) {
        self.status_label.set_text("Ready");
        let labels: Vec<String> = snapshots.iter().map(snapshot_label).collect();
        let model = gtk4::StringList::new(&labels.iter().map(String::as_str).collect::<Vec<_>>());
        let combo = adw::ComboRow::builder()
            .title("Restore point")
            .model(&model)
            .build();
        let fields = ListBox::new();
        fields.add_css_class("boxed-list");
        fields.set_selection_mode(gtk4::SelectionMode::None);
        fields.append(&combo);

        let dialog = adw::AlertDialog::builder()
            .heading(format!("Restore {}?", pc.display_name))
            .body(
                "The Cloud PC returns to the state of the chosen restore point. Files, apps \
                 and settings changed since then are lost, and you are disconnected.",
            )
            .extra_child(&fields)
            .default_response("cancel")
            .close_response("cancel")
            .build();
        dialog.add_responses(&[("cancel", "Cancel"), ("restore", "Restore")]);
        dialog.set_response_appearance("restore", adw::ResponseAppearance::Destructive);
        let weak = Rc::downgrade(self);
        dialog.connect_response(Some("restore"), move |_, _| {
            let Some(this) = weak.upgrade() else { return };
            let Some(snapshot) = snapshots.get(combo.selected() as usize) else {
                return;
            };
            let service = Arc::clone(&this.cloud_service);
            let (id, snapshot_id) = (pc.id.clone(), snapshot.id.clone());
            this.run_pc_action(&pc, CloudPcAction::Restore, async move {
                service.restore_cloud_pc(&id, &snapshot_id).await
            });
        });
        dialog.present(Some(&self.window));
    }

    /// Rebuilds the Cloud PC from scratch after a typed confirmation.
    fn reset_cloud_pc(self: &Rc<Self>, pc: CloudPc) {
        let entry = adw::EntryRow::builder()
            .title(format!("Type “{}” to confirm", pc.display_name))
            .build();
        let fields = ListBox::new();
        fields.add_css_class("boxed-list");
        fields.set_selection_mode(gtk4::SelectionMode::None);
        fields.append(&entry);

        let dialog = adw::AlertDialog::builder()
            .heading(format!("Reset {}?", pc.display_name))
            .body(
                "Resetting reinstalls Windows. All your files, apps and settings on this \
                 Cloud PC are permanently removed. This cannot be undone.",
            )
            .extra_child(&fields)
            .default_response("cancel")
            .close_response("cancel")
            .build();
        dialog.add_responses(&[("cancel", "Cancel"), ("reset", "Reset")]);
        dialog.set_response_appearance("reset", adw::ResponseAppearance::Destructive);
        dialog.set_response_enabled("reset", false);
        let expected = pc.display_name.clone();
        let dlg = dialog.downgrade();
        entry.connect_changed(move |entry| {
            if let Some(dialog) = dlg.upgrade() {
                dialog.set_response_enabled("reset", entry.text().trim() == expected.trim());
            }
        });
        let weak = Rc::downgrade(self);
        dialog.connect_response(Some("reset"), move |_, _| {
            let Some(this) = weak.upgrade() else { return };
            let service = Arc::clone(&this.cloud_service);
            let id = pc.id.clone();
            this.run_pc_action(&pc, CloudPcAction::Reset, async move {
                service.reset_cloud_pc(&id).await
            });
        });
        dialog.present(Some(&self.window));
    }

    fn rename_cloud_pc(self: &Rc<Self>, pc: &CloudPc) {
        let entry = gtk4::Entry::builder()
            .text(&pc.display_name)
            .activates_default(true)
            .build();
        let dialog = adw::AlertDialog::builder()
            .heading("Rename Cloud PC")
            .body("Enter a new display name:")
            .extra_child(&entry)
            .default_response("rename")
            .close_response("cancel")
            .build();
        dialog.add_responses(&[("cancel", "Cancel"), ("rename", "Rename")]);
        dialog.set_response_appearance("rename", adw::ResponseAppearance::Suggested);

        let service = Arc::clone(&self.cloud_service);
        let weak = Rc::downgrade(self);
        let pc_id = pc.id.clone();
        dialog.connect_response(Some("rename"), move |_, _| {
            let new_name = entry.text().trim().to_string();
            if new_name.is_empty() {
                return;
            }
            let service = Arc::clone(&service);
            let pc_id = pc_id.clone();
            let weak = weak.clone();
            glib::MainContext::default().spawn_local(async move {
                let result = service.rename_cloud_pc(&pc_id, &new_name).await;
                let Some(this) = weak.upgrade() else { return };
                match result {
                    Ok(()) => this.load_cloud_pcs(),
                    Err(e) => this.status_label.set_text(&format!("Rename failed: {e}")),
                }
            });
        });
        dialog.present(Some(&self.window));
    }

    fn inspect_connection(&self, pc: &CloudPc) {
        let config = AppConfig::load();
        let scale = match platform::desktop_scale_percent() {
            Some(detected) => format!("Matches display ({detected}%); change it in the session"),
            None => "Matches display; change it in the session".to_string(),
        };
        let items = [
            ("Cloud PC Name", pc.display_name.clone()),
            ("Cloud PC ID", pc.id.clone()),
            (
                "Assigned User",
                or_not_reported(pc.user_principal_name.as_ref()),
            ),
            (
                "RDP Engine",
                format!("FreeRDP {} (embedded)", rdp::version()),
            ),
            ("Sign-in Profile", config.auth_profile().label().to_string()),
            ("Desktop Scale", scale),
            ("Host Pool", or_not_reported(pc.host_pool_id.as_ref())),
            ("Region", or_not_reported(pc.device_region_name.as_ref())),
        ];
        self.present_property_dialog(&format!("Connection - {}", pc.display_name), &items);
    }

    fn view_details(&self, pc: &CloudPc) {
        let os = pc.os_name.as_ref().or(pc.image_display_name.as_ref());
        let now = chrono::Utc::now();
        let last_action = pc.last_remote_action_result.as_ref().map(|r| {
            let mut text = format!(
                "{} — {}",
                r.action_name.as_deref().unwrap_or("Action"),
                r.action_state.as_deref().unwrap_or("unknown")
            );
            if let Some(when) = r
                .last_updated_date_time
                .as_deref()
                .or(r.start_date_time.as_deref())
            {
                text.push_str(&format!(" ({})", format_time(when)));
            }
            if let Some(message) = r.status_details.as_ref().and_then(|d| d.message.as_deref()) {
                text.push_str(&format!("\n{message}"));
            }
            text
        });
        let progress = pc
            .provisioning_progress
            .filter(|p| *p > 0 && *p < 100)
            .map(|p| format!(" ({p}%)"))
            .unwrap_or_default();
        let items = [
            ("Device Name", pc.display_name.clone()),
            ("Operating System", or_not_reported(os)),
            ("Hardware", pc.hardware_summary().to_string()),
            ("Status", pc.status_summary(now).0),
            ("Licence", pc.license.label().to_string()),
            ("Power State", or_not_reported(pc.power_state.as_ref())),
            (
                "Provisioning",
                format!(
                    "{}{progress}",
                    or_not_reported(pc.provisioning_status.as_ref())
                ),
            ),
            (
                "Grace Period Ends",
                pc.grace_period_end
                    .as_deref()
                    .map_or_else(|| "Not in a grace period".to_string(), format_time),
            ),
            (
                "Last Sign-in",
                match (pc.last_login.as_deref(), pc.last_login_result.as_deref()) {
                    (Some(when), Some(result)) => format!("{} — {result}", format_time(when)),
                    (Some(when), None) => format_time(when),
                    _ => "Not reported".to_string(),
                },
            ),
            (
                "Last Action",
                last_action.unwrap_or_else(|| "None".to_string()),
            ),
            (
                "Disk Encryption",
                or_not_reported(pc.disk_encryption_state.as_ref()),
            ),
            (
                "User Principal Name",
                or_not_reported(pc.user_principal_name.as_ref()),
            ),
            ("Device ID", pc.id.clone()),
        ];
        self.present_property_dialog(&format!("Details - {}", pc.display_name), &items);
    }

    /// Presents a read-only list of title/value rows.
    fn present_property_dialog(&self, title: &str, items: &[(&str, String)]) {
        let group = adw::PreferencesGroup::new();
        for (key, value) in items {
            let row = adw::ActionRow::builder()
                .title(*key)
                .subtitle(glib::markup_escape_text(value).as_str())
                .subtitle_selectable(true)
                .build();
            row.add_css_class("property");
            group.add(&row);
        }
        let page = adw::PreferencesPage::new();
        page.add(&group);
        let toolbar_view = adw::ToolbarView::new();
        toolbar_view.add_top_bar(&adw::HeaderBar::new());
        toolbar_view.set_content(Some(&page));
        adw::Dialog::builder()
            .title(title)
            .content_width(500)
            .content_height(540)
            .child(&toolbar_view)
            .build()
            .present(Some(&self.window));
    }
}

/// Vertical box holding the main window sections.
fn sections_box() -> Box {
    let sections = Box::new(Orientation::Vertical, 18);
    sections.set_valign(gtk4::Align::Start);
    sections.set_margin_start(16);
    sections.set_margin_end(16);
    sections.set_margin_top(12);
    sections.set_margin_bottom(16);
    sections
}

fn chevron_icon(expanded: bool) -> &'static str {
    if expanded {
        "pan-down-symbolic"
    } else {
        "pan-end-symbolic"
    }
}

fn hook_context(launch: &Launch) -> HookContext {
    HookContext {
        name: launch.name.clone(),
        address: launch.address.clone(),
        target: launch
            .target
            .as_ref()
            .map(ToString::to_string)
            .unwrap_or_default(),
    }
}

fn rdp_file_filter() -> gtk4::FileFilter {
    let filter = gtk4::FileFilter::new();
    filter.set_name(Some("Remote Desktop Files"));
    filter.add_pattern("*.rdp");
    filter.add_mime_type("application/x-rdp");
    filter
}
