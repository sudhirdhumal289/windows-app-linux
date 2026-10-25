//! Main GTK4 Application Window with Grid/List views, Hamburger user menu, and auxiliary actions.

use crate::api::CloudPcService;
use crate::auth::AuthManager;
use crate::config::AppConfig;
use crate::models::CloudPc;
use crate::rdp::RdpLauncher;
use crate::ui::login_dialog::LoginDialog;
use crate::ui::native_rdp_window::NativeRdpWindow;
use crate::ui::pc_card::{CardAction, CardLayoutMode, CloudPcCardWidget};
use crate::ui::preferences_dialog::PreferencesDialog;
use crate::ui::rdp_auth_dialog::RdpAuthDialog;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{
    Application, ApplicationWindow, Box, Button, Entry, FlowBox, HeaderBar, Label, MenuButton,
    Orientation, Popover, ScrolledWindow, Separator, Spinner, Stack,
};
use log::{debug, warn};
use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;
use std::sync::Arc;

pub struct AppMainWindow {
    pub window: ApplicationWindow,
    pub stack: Stack,
    pub content_stack: Stack,
    pub grid_flowbox: FlowBox,
    pub list_container: Box,
    pub spinner: Spinner,
    pub status_label: Label,
    pub auth: Arc<AuthManager>,
    pub cloud_service: Arc<CloudPcService>,
    pub cached_pcs: RefCell<Vec<CloudPc>>,
    pub current_mode: RefCell<CardLayoutMode>,
    pub account_container: Box,
    pub grid_btn: Button,
    pub list_btn: Button,
    pub refresh_btn: Button,
    pub layout_box: Box,
    pub connecting_pcs: RefCell<HashSet<String>>,
}

impl AppMainWindow {
    pub fn new(
        app: &Application,
        auth: Arc<AuthManager>,
        cloud_service: Arc<CloudPcService>,
    ) -> Rc<Self> {
        Self::load_custom_css();

        let window = ApplicationWindow::builder()
            .application(app)
            .title("Windows App")
            .default_width(980)
            .default_height(680)
            .icon_name("com.microsoft.windowsapp.linux")
            .build();

        let config = AppConfig::load();
        let initial_mode = if config.view_mode == "list" {
            CardLayoutMode::List
        } else {
            CardLayoutMode::Grid
        };

        // --- Custom HeaderBar ---
        let header = HeaderBar::new();

        // Left: Refresh button (Visible post-login only)
        let refresh_btn = Button::from_icon_name("view-refresh-symbolic");
        refresh_btn.set_tooltip_text(Some("Refresh Cloud PCs"));
        refresh_btn.set_visible(false);
        header.pack_start(&refresh_btn);

        // Left: Layout Switcher (Grid / List) (Visible post-login with Cloud PCs)
        let layout_box = Box::new(Orientation::Horizontal, 0);
        layout_box.add_css_class("linked");
        layout_box.set_visible(false);

        let grid_btn = Button::from_icon_name("view-grid-symbolic");
        grid_btn.set_tooltip_text(Some("Grid View"));

        let list_btn = Button::from_icon_name("view-list-symbolic");
        list_btn.set_tooltip_text(Some("List View"));

        if initial_mode == CardLayoutMode::Grid {
            grid_btn.add_css_class("suggested-action");
            list_btn.add_css_class("flat");
        } else {
            list_btn.add_css_class("suggested-action");
            grid_btn.add_css_class("flat");
        }

        layout_box.append(&grid_btn);
        layout_box.append(&list_btn);
        header.pack_start(&layout_box);

        // Right: Account Container (Sign-in button OR Hamburger user menu)
        let account_container = Box::new(Orientation::Horizontal, 6);
        header.pack_end(&account_container);

        window.set_titlebar(Some(&header));

        // --- Main Vertical Layout ---
        let main_box = Box::new(Orientation::Vertical, 0);

        // Stack for primary states: Loading, Empty, Content
        let stack = Stack::new();
        stack.set_vexpand(true);

        // State 1: Loading
        let loading_box = Box::new(Orientation::Vertical, 12);
        loading_box.set_valign(gtk4::Align::Center);
        loading_box.set_halign(gtk4::Align::Center);
        let spinner = Spinner::new();
        spinner.set_size_request(48, 48);
        let loading_label = Label::new(Some("Connecting to Microsoft 365..."));
        loading_box.append(&spinner);
        loading_box.append(&loading_label);
        stack.add_named(&loading_box, Some("loading"));

        // State 2: Empty / Sign-in required
        let empty_box = Box::new(Orientation::Vertical, 16);
        empty_box.set_valign(gtk4::Align::Center);
        empty_box.set_halign(gtk4::Align::Center);
        let empty_icon = Label::new(Some("☁️"));
        empty_icon.add_css_class("title-1");
        let empty_title = Label::new(Some("No Cloud PCs Available"));
        empty_title.add_css_class("title-2");
        let empty_desc = Label::new(Some(
            "Sign in with your Microsoft Work or School account to access your Cloud PCs.",
        ));
        empty_desc.add_css_class("dim-label");

        let empty_signin_btn = Button::with_label("Sign In with Microsoft");
        empty_signin_btn.add_css_class("suggested-action");

        empty_box.append(&empty_icon);
        empty_box.append(&empty_title);
        empty_box.append(&empty_desc);
        empty_box.append(&empty_signin_btn);
        stack.add_named(&empty_box, Some("empty"));

        // State 3: Content Stack (Grid vs List)
        let content_stack = Stack::new();
        content_stack.set_vexpand(true);

        // Grid View
        let grid_scrolled = ScrolledWindow::builder()
            .hscrollbar_policy(gtk4::PolicyType::Never)
            .vscrollbar_policy(gtk4::PolicyType::Automatic)
            .build();

        let grid_flowbox = FlowBox::builder()
            .valign(gtk4::Align::Start)
            .max_children_per_line(6)
            .min_children_per_line(1)
            .selection_mode(gtk4::SelectionMode::None)
            .column_spacing(16)
            .row_spacing(16)
            .homogeneous(true)
            .margin_start(16)
            .margin_end(16)
            .margin_top(16)
            .margin_bottom(16)
            .build();
        grid_scrolled.set_child(Some(&grid_flowbox));
        content_stack.add_named(&grid_scrolled, Some("grid"));

        // List View (valign: Start so items stay compact and don't stretch vertically)
        let list_scrolled = ScrolledWindow::builder()
            .hscrollbar_policy(gtk4::PolicyType::Never)
            .vscrollbar_policy(gtk4::PolicyType::Automatic)
            .build();

        let list_container = Box::new(Orientation::Vertical, 8);
        list_container.set_valign(gtk4::Align::Start);
        list_container.set_margin_start(16);
        list_container.set_margin_end(16);
        list_container.set_margin_top(16);
        list_container.set_margin_bottom(16);
        list_scrolled.set_child(Some(&list_container));
        content_stack.add_named(&list_scrolled, Some("list"));

        if initial_mode == CardLayoutMode::Grid {
            content_stack.set_visible_child_name("grid");
        } else {
            content_stack.set_visible_child_name("list");
        }

        stack.add_named(&content_stack, Some("content"));
        main_box.append(&stack);

        // Bottom Status Bar
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
        main_box.append(&status_bar);

        window.set_child(Some(&main_box));

        let app_window = Rc::new(Self {
            window,
            stack,
            content_stack,
            grid_flowbox,
            list_container,
            spinner,
            status_label,
            auth,
            cloud_service,
            cached_pcs: RefCell::new(Vec::new()),
            current_mode: RefCell::new(initial_mode),
            account_container,
            grid_btn,
            list_btn,
            refresh_btn: refresh_btn.clone(),
            layout_box: layout_box.clone(),
            connecting_pcs: RefCell::new(HashSet::new()),
        });

        // Check if a supported platform RDP client (FreeRDP) is installed
        if RdpLauncher::detect_any_rdp_client().is_none() {
            let win_dep = app_window.window.clone();
            glib::idle_add_local_once(move || {
                crate::ui::MissingDependencyDialog::show(Some(&win_dep));
            });
        }

        // Wire Header Controls
        let win_clone = Rc::clone(&app_window);
        refresh_btn.connect_clicked(move |_| {
            win_clone.load_cloud_pcs();
        });

        let win_clone = Rc::clone(&app_window);
        empty_signin_btn.connect_clicked(move |_| {
            win_clone.initiate_login();
        });

        // Wire Grid / List Switchers
        let win_clone = Rc::clone(&app_window);
        app_window.grid_btn.connect_clicked(move |_| {
            win_clone.set_layout_mode(CardLayoutMode::Grid);
        });

        let win_clone = Rc::clone(&app_window);
        app_window.list_btn.connect_clicked(move |_| {
            win_clone.set_layout_mode(CardLayoutMode::List);
        });

        // Update Account Header UI
        app_window.update_account_header();

        // Initial Load
        app_window.load_cloud_pcs();

        app_window
    }

    /// Loads custom CSS styling for modern Windows 11 cards, banners, and borders.
    fn load_custom_css() {
        let provider = gtk4::CssProvider::new();
        let css = r#"
            .cpc-grid-card {
                border: 1px solid rgba(255, 255, 255, 0.22);
                border-radius: 14px;
                background: linear-gradient(160deg, #09477e 0%, #06284e 45%, #0d121c 100%);
                box-shadow: 0 4px 20px rgba(0, 0, 0, 0.45);
                overflow: hidden;
                transition: all 180ms ease-in-out;
            }
            .cpc-grid-card:hover {
                border-color: rgba(0, 150, 255, 0.85);
                box-shadow: 0 6px 24px rgba(0, 120, 212, 0.40);
            }
            .cpc-grid-banner {
                background: transparent;
                transition: background 150ms ease-in-out;
            }
            .cpc-grid-banner:hover {
                background: rgba(255, 255, 255, 0.06);
            }
            .cpc-banner-os-logo {
                font-size: 34px;
                font-weight: bold;
                color: #ffffff;
            }
            .cpc-banner-os-title {
                font-size: 13px;
                font-weight: 600;
                color: rgba(255, 255, 255, 0.9);
            }
            .cpc-grid-details {
                background: rgba(16, 20, 30, 0.76);
                border-top: 1px solid rgba(255, 255, 255, 0.12);
                padding: 10px 14px 10px 14px;
            }
            .cpc-status-pill {
                background: rgba(0, 0, 0, 0.45);
                color: #f0f0f0;
                font-size: 11px;
                font-weight: 600;
                padding: 3px 8px;
                border-radius: 10px;
            }
            .cpc-status-pill.success {
                background: rgba(46, 125, 50, 0.85);
                color: #ffffff;
            }
            .cpc-list-card {
                border: 1px solid rgba(255, 255, 255, 0.14);
                border-radius: 12px;
                background: #1c1c22;
                transition: all 180ms ease-in-out;
            }
            .cpc-list-card:hover {
                border-color: rgba(0, 120, 212, 0.75);
                box-shadow: 0 4px 14px rgba(0, 120, 212, 0.25);
            }
            .cpc-list-icon-badge {
                background: linear-gradient(135deg, #0078d4 0%, #002050 100%);
                border-radius: 8px;
            }
            .cpc-list-os-logo {
                font-size: 22px;
                color: #ffffff;
            }
        "#;
        provider.load_from_string(css);
        if let Some(display) = gtk4::gdk::Display::default() {
            gtk4::style_context_add_provider_for_display(
                &display,
                &provider,
                gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
            );
        }
    }

    /// Presents the application window.
    pub fn show(&self) {
        self.window.present();
    }

    /// Sets Grid or List layout mode and re-renders cards.
    pub fn set_layout_mode(self: &Rc<Self>, mode: CardLayoutMode) {
        *self.current_mode.borrow_mut() = mode;

        let mut config = AppConfig::load();
        if mode == CardLayoutMode::Grid {
            self.grid_btn.add_css_class("suggested-action");
            self.grid_btn.remove_css_class("flat");
            self.list_btn.remove_css_class("suggested-action");
            self.list_btn.add_css_class("flat");
            self.content_stack.set_visible_child_name("grid");
            config.view_mode = "grid".to_string();
        } else {
            self.list_btn.add_css_class("suggested-action");
            self.list_btn.remove_css_class("flat");
            self.grid_btn.remove_css_class("suggested-action");
            self.grid_btn.add_css_class("flat");
            self.content_stack.set_visible_child_name("list");
            config.view_mode = "list".to_string();
        }
        let _ = config.save();
        self.render_pcs();
    }

    /// Updates the Title Bar account button / Hamburger menu.
    pub fn update_account_header(self: &Rc<Self>) {
        while let Some(child) = self.account_container.first_child() {
            self.account_container.remove(&child);
        }

        let user_upn = AppConfig::get_user_upn();
        let user_name = AppConfig::get_user_display_name();
        let is_signed_in = user_upn.is_some();
        let has_pcs = !self.cached_pcs.borrow().is_empty();

        // Only show refresh and grid/list layout controls post-login when viewing Cloud PCs
        self.refresh_btn.set_visible(is_signed_in);
        self.layout_box.set_visible(is_signed_in && has_pcs);

        if let Some(upn) = user_upn {
            // Signed In: Hamburger MenuButton
            let menu_btn = MenuButton::new();
            menu_btn.set_icon_name("open-menu-symbolic");
            menu_btn.add_css_class("flat");
            menu_btn.set_tooltip_text(Some("Account & Options"));

            let popover = Popover::new();
            let menu_box = Box::new(Orientation::Vertical, 4);
            menu_box.set_margin_start(10);
            menu_box.set_margin_end(10);
            menu_box.set_margin_top(8);
            menu_box.set_margin_bottom(8);

            // User Info Header (Left-aligned)
            let name_label = Label::new(Some(&user_name.unwrap_or_else(|| upn.clone())));
            name_label.set_halign(gtk4::Align::Start);
            name_label.add_css_class("title-4");

            let upn_label = Label::new(Some(&upn));
            upn_label.set_halign(gtk4::Align::Start);
            upn_label.add_css_class("dim-label");

            menu_box.append(&name_label);
            menu_box.append(&upn_label);
            menu_box.append(&Separator::new(Orientation::Horizontal));

            // Preferences Item (Clean text, Left-aligned, no emojis)
            let pref_btn = Button::with_label("Preferences...");
            pref_btn.add_css_class("flat");
            pref_btn.set_halign(gtk4::Align::Fill);
            if let Some(lbl) = pref_btn.child().and_downcast::<Label>() {
                lbl.set_halign(gtk4::Align::Start);
                lbl.set_xalign(0.0);
            }
            let win_p = self.window.clone();
            let pop_p = popover.clone();
            let this_p = Rc::clone(self);
            pref_btn.connect_clicked(move |_| {
                pop_p.popdown();
                let this_cb = Rc::clone(&this_p);
                PreferencesDialog::show(&win_p, move |_| {
                    this_cb.render_pcs();
                });
            });
            menu_box.append(&pref_btn);

            // Sign In with Another Profile Item (Clean text, Left-aligned)
            let another_btn = Button::with_label("Sign In with Another Profile...");
            another_btn.add_css_class("flat");
            another_btn.set_halign(gtk4::Align::Fill);
            if let Some(lbl) = another_btn.child().and_downcast::<Label>() {
                lbl.set_halign(gtk4::Align::Start);
                lbl.set_xalign(0.0);
            }
            let this_l = Rc::clone(self);
            let pop_l = popover.clone();
            another_btn.connect_clicked(move |_| {
                pop_l.popdown();
                this_l.initiate_login();
            });
            menu_box.append(&another_btn);

            menu_box.append(&Separator::new(Orientation::Horizontal));

            // Sign Out Item (Clean text, Left-aligned)
            let signout_btn = Button::with_label("Sign Out");
            signout_btn.add_css_class("flat");
            signout_btn.set_halign(gtk4::Align::Fill);
            if let Some(lbl) = signout_btn.child().and_downcast::<Label>() {
                lbl.set_halign(gtk4::Align::Start);
                lbl.set_xalign(0.0);
            }
            let this_s = Rc::clone(self);
            let pop_s = popover.clone();
            signout_btn.connect_clicked(move |_| {
                pop_s.popdown();
                let _ = AppConfig::clear_tokens();
                let _ = AppConfig::clear_credentials();
                *this_s.cached_pcs.borrow_mut() = Vec::new();
                this_s.update_account_header();
                this_s.stack.set_visible_child_name("empty");
                this_s.status_label.set_text("Signed out successfully.");
            });
            menu_box.append(&signout_btn);

            popover.set_child(Some(&menu_box));
            menu_btn.set_popover(Some(&popover));
            self.account_container.append(&menu_btn);
        } else {
            // Not Signed In: Sign In Button + Aux Actions (In-App Web Default, Edge/Code & Paste Token in Aux)
            let signin_group = Box::new(Orientation::Horizontal, 0);
            signin_group.add_css_class("linked");

            // Sign In Button (Normal styling without suggested-action blue background)
            let signin_btn = Button::with_label("Sign In");
            let this_i = Rc::clone(self);
            signin_btn.connect_clicked(move |_| {
                this_i.initiate_login_with_view("browser");
            });
            signin_group.append(&signin_btn);

            // Aux Action Dropdown Menu Button
            let aux_btn = MenuButton::new();
            aux_btn.set_icon_name("pan-down-symbolic");
            aux_btn.set_tooltip_text(Some("Sign-in options"));

            let popover = Popover::new();
            let menu_box = Box::new(Orientation::Vertical, 4);
            menu_box.set_margin_start(8);
            menu_box.set_margin_end(8);
            menu_box.set_margin_top(8);
            menu_box.set_margin_bottom(8);

            // 1. In-App Web Browser (Default)
            let web_item = Button::with_label("In-App Web Browser (Default)");
            web_item.add_css_class("flat");
            web_item.set_halign(gtk4::Align::Fill);
            if let Some(lbl) = web_item.child().and_downcast::<Label>() {
                lbl.set_halign(gtk4::Align::Start);
                lbl.set_xalign(0.0);
            }
            let this_w = Rc::clone(self);
            let pop_w = popover.clone();
            web_item.connect_clicked(move |_| {
                pop_w.popdown();
                this_w.initiate_login_with_view("browser");
            });
            menu_box.append(&web_item);

            // 2. Edge / Browser Code
            let edge_item = Button::with_label("Edge / Browser Code...");
            edge_item.add_css_class("flat");
            edge_item.set_halign(gtk4::Align::Fill);
            if let Some(lbl) = edge_item.child().and_downcast::<Label>() {
                lbl.set_halign(gtk4::Align::Start);
                lbl.set_xalign(0.0);
            }
            let this_e = Rc::clone(self);
            let pop_e = popover.clone();
            edge_item.connect_clicked(move |_| {
                pop_e.popdown();
                this_e.initiate_login_with_view("device");
            });
            menu_box.append(&edge_item);

            // 3. Paste Token
            let paste_item = Button::with_label("Paste Token...");
            paste_item.add_css_class("flat");
            paste_item.set_halign(gtk4::Align::Fill);
            if let Some(lbl) = paste_item.child().and_downcast::<Label>() {
                lbl.set_halign(gtk4::Align::Start);
                lbl.set_xalign(0.0);
            }
            let this_p = Rc::clone(self);
            let pop_p = popover.clone();
            paste_item.connect_clicked(move |_| {
                pop_p.popdown();
                this_p.initiate_login_with_view("import");
            });
            menu_box.append(&paste_item);

            popover.set_child(Some(&menu_box));
            aux_btn.set_popover(Some(&popover));
            signin_group.append(&aux_btn);

            self.account_container.append(&signin_group);
        }
    }

    /// Triggers in-app WebKitGTK authentication flow (defaults to "browser").
    pub fn initiate_login(self: &Rc<Self>) {
        self.initiate_login_with_view("browser");
    }

    /// Triggers authentication flow with a specific initial view ("browser", "device", or "import").
    pub fn initiate_login_with_view(self: &Rc<Self>, view: &'static str) {
        let auth = Arc::clone(&self.auth);
        let parent = self.window.clone();
        let status = self.status_label.clone();
        let this = Rc::clone(self);

        status.set_text("Opening Microsoft sign-in...");

        let status_close = status.clone();
        LoginDialog::show_embedded(
            &parent,
            auth,
            Some(view),
            move || {
                status.set_text("Sign-in successful. Loading Cloud PCs...");
                this.update_account_header();
                this.load_cloud_pcs();
            },
            move || {
                // If sign-in dialog closed without login, restore status to Ready
                if AppConfig::get_user_upn().is_none() {
                    status_close.set_text("Ready");
                }
            },
        );
    }

    /// Fetches Cloud PCs from Windows 365 / Microsoft Graph.
    pub fn load_cloud_pcs(self: &Rc<Self>) {
        // If user is not yet signed in, present clean empty login screen with Ready status without trying network fetch
        if AppConfig::get_user_upn().is_none() {
            self.spinner.stop();
            self.update_account_header();
            self.stack.set_visible_child_name("empty");
            self.status_label.set_text("Ready");
            return;
        }

        let service = Arc::clone(&self.cloud_service);
        let stack = self.stack.clone();
        let spinner = self.spinner.clone();
        let status = self.status_label.clone();
        let this = Rc::clone(self);

        stack.set_visible_child_name("loading");
        spinner.start();
        status.set_text("Fetching Cloud PCs from Windows 365...");

        glib::MainContext::default().spawn_local(async move {
            match service.list_user_cloud_pcs().await {
                Ok(pcs) if !pcs.is_empty() => {
                    debug!("Successfully loaded {} Cloud PCs", pcs.len());
                    status.set_text(&format!("Loaded {} Cloud PCs", pcs.len()));
                    *this.cached_pcs.borrow_mut() = pcs;
                    this.update_account_header();
                    this.render_pcs();
                    spinner.stop();
                    stack.set_visible_child_name("content");
                }
                Ok(_) => {
                    spinner.stop();
                    *this.cached_pcs.borrow_mut() = Vec::new();
                    this.update_account_header();
                    stack.set_visible_child_name("empty");
                    status.set_text("No Cloud PCs assigned to this account.");
                }
                Err(e) => {
                    spinner.stop();
                    this.update_account_header();
                    stack.set_visible_child_name("empty");
                    let clean_msg = match &e {
                        crate::error::AppError::GraphApi { status: s, message: m } => {
                            if let Ok(val) = serde_json::from_str::<serde_json::Value>(m) {
                                if let Some(msg) = val.get("error").and_then(|err| err.get("message")).and_then(|s| s.as_str()) {
                                    format!("Unable to load Cloud PCs: {msg}")
                                } else {
                                    format!("Unable to load Cloud PCs (HTTP {s})")
                                }
                            } else {
                                format!("Unable to load Cloud PCs (HTTP {s})")
                            }
                        }
                        _ => format!("Unable to load Cloud PCs: {e}"),
                    };
                    status.set_text(&clean_msg);
                }
            }
        });
    }

    /// Renders cached Cloud PCs in either Grid or List mode.

    /// Clears connecting state for a Cloud PC and refreshes UI cards.
    pub fn finish_connecting(self: &Rc<Self>, pc_id: &str) {
        if self.connecting_pcs.borrow_mut().remove(pc_id) {
            self.render_pcs();
        }
    }
    pub fn render_pcs(self: &Rc<Self>) {
        while let Some(child) = self.grid_flowbox.first_child() {
            self.grid_flowbox.remove(&child);
        }
        while let Some(child) = self.list_container.first_child() {
            self.list_container.remove(&child);
        }

        let pcs = self.cached_pcs.borrow();
        let mode = *self.current_mode.borrow();
        let connecting_ids = self.connecting_pcs.borrow();

        for pc in pcs.iter() {
            let this = Rc::clone(self);
            let is_connecting = connecting_ids.contains(&pc.id);
            let widget = CloudPcCardWidget::build(pc, mode, is_connecting, move |action, spinner, card_box| {
                this.handle_card_action(action, spinner, card_box);
            });

            match mode {
                CardLayoutMode::Grid => self.grid_flowbox.append(&widget),
                CardLayoutMode::List => self.list_container.append(&widget),
            }
        }
    }

    /// Handles actions triggered from the card or its 3-dots auxiliary menu.
    fn handle_card_action(self: &Rc<Self>, action: CardAction, spinner: Spinner, card_box: Box) {
        match action {
            CardAction::Connect(pc) => self.connect_cloud_pc(pc, spinner, card_box),
            CardAction::Restart(id) => self.restart_cloud_pc(id),
            CardAction::Rename(pc) => self.rename_cloud_pc(pc),
            CardAction::Inspect(pc) => self.inspect_connection(pc),
            CardAction::Details(pc) => self.view_details(pc),
            CardAction::Settings => {
                let win = self.window.clone();
                let this = Rc::clone(self);
                PreferencesDialog::show(&win, move |_| {
                    this.render_pcs();
                });
            }
        }
    }

    /// Initiates connection to the selected Cloud PC, honoring engine and avoiding premature enable delay.
    fn connect_cloud_pc(self: &Rc<Self>, pc: CloudPc, _spinner: Spinner, _card_box: Box) {
        if RdpLauncher::detect_any_rdp_client().is_none() {
            crate::ui::MissingDependencyDialog::show(Some(&self.window));
            self.status_label.set_text("Please install FreeRDP to connect to your Cloud PC.");
            return;
        }

        self.connecting_pcs.borrow_mut().insert(pc.id.clone());
        self.render_pcs();

        let service = Arc::clone(&self.cloud_service);
        let status = self.status_label.clone();
        let parent_win = self.window.clone();
        let config = AppConfig::load();
        let engine_name = match config.preferred_rdp_client.as_str() {
            "ironrdp" => "IronRDP".to_string(),
            _ => "In-App Native FreeRDP 3".to_string(),
        };

        status.set_text(&format!(
            "Acquiring authorization for {} ({engine_name})...",
            pc.display_name
        ));

        let this_launch = Rc::clone(self);
        let pc_id_launch = pc.id.clone();
        let preferred_client = config.preferred_rdp_client.clone();

        glib::MainContext::default().spawn_local(async move {
            match service.fetch_rdp_file_and_token(&pc).await {
                Ok((rdp_content, avd_token)) => {
                    let is_ironrdp = preferred_client == "ironrdp";
                    let is_arm = rdp_content.contains("resourceprovider:s:arm")
                        || rdp_content.contains("wvd.microsoft.com")
                        || rdp_content.contains("armpath:");

                    let active_engine = if is_ironrdp && is_arm {
                        "In-App Native FreeRDP 3 (Fallback)"
                    } else {
                        &engine_name
                    };

                    status.set_text(&format!(
                        "Starting session for {} ({active_engine})...",
                        pc.display_name
                    ));

                    let tenant_id = rdp_content.lines().find_map(|l| {
                        let t = l.trim();
                        if t.starts_with("aadtenantid:s:") {
                            Some(t.trim_start_matches("aadtenantid:s:").to_string())
                        } else {
                            None
                        }
                    });

                    let user_upn = pc
                        .user_principal_name
                        .clone()
                        .or_else(AppConfig::get_user_upn)
                        .unwrap_or_else(|| "user@domain.com".to_string());

                    let is_in_app = preferred_client == "freerdp" || preferred_client.is_empty();

                    if is_in_app {
                        status.set_text(&format!(
                            "Starting in-app session for {}...",
                            pc.display_name
                        ));

                        let tmp_dir = std::env::temp_dir();
                        let rdp_path = tmp_dir.join(format!("cloudpc-{}.rdp", pc.id));
                        if let Ok(mut f) = std::fs::File::create(&rdp_path) {
                            use std::io::Write;
                            let _ = f.write_all(rdp_content.as_bytes());
                        }

                        let scale_percent = match config.default_scale_factor {
                            0 => RdpLauncher::get_system_desktop_scale(),
                            s => s,
                        };

                        let mut cli_args = vec![
                            "freerdp".to_string(),
                            rdp_path.to_string_lossy().to_string(),
                            format!("/gateway:type:arm,bearer:{avd_token}"),
                            "/cert:ignore".to_string(),
                            "/size:1280x752".to_string(),
                            "/dynamic-resolution".to_string(),
                            "/rfx".to_string(),
                        ];
                        if config.enable_clipboard {
                            cli_args.push("+clipboard".to_string());
                        } else {
                            cli_args.push("-clipboard".to_string());
                        }
                        if config.enable_audio {
                            cli_args.push("/sound:sys:pulse".to_string());
                        }
                        if config.enable_microphone {
                            cli_args.push("/microphone:sys:pulse".to_string());
                        }
                        if config.enable_camera {
                            cli_args.push("/dvc:rdpecam".to_string());
                        }
                        if config.enable_drive_redirection && !config.shared_folder_path.is_empty() {
                            let p = std::path::Path::new(&config.shared_folder_path);
                            if p.exists() {
                                                                cli_args.push(format!("/drive:Shared,{}", config.shared_folder_path));
                            }
                        }
                        cli_args.push(format!("/u:{user_upn}"));

                        let this_win = Rc::clone(&this_launch);
                        let pc_id_win = pc_id_launch.clone();
                        let status_win = status.clone();

                        NativeRdpWindow::open(
                            Some(&parent_win),
                            &pc,
                            cli_args,
                            scale_percent,
                            move || {
                                this_win.finish_connecting(&pc_id_win);
                                status_win.set_text("Session ended.");
                            },
                        );
                        return;
                    }

                    // Launch AVD session externally
                    match RdpLauncher::launch_avd_session(
                        &pc.id,
                        &rdp_content,
                        &avd_token,
                        Some(&user_upn),
                        tenant_id.as_deref(),
                        Some(&preferred_client),
                    ) {
                        Ok(session) => {
                            let mut child = session.child;
                            let pty_master = session.pty_master;
                            let rx = session.auth_url_rx;
                            let parent = parent_win.clone();
                            let status_auth = status.clone();
                            let user_upn_auth = user_upn.clone();
                            let this_auth = Rc::clone(&this_launch);
                            let pc_id_auth = pc_id_launch.clone();

                            // Watch process lifetime: if FreeRDP terminates/closes, re-enable card immediately
                            let this_proc = Rc::clone(&this_launch);
                            let pc_id_proc = pc_id_launch.clone();
                            let status_proc = status.clone();
                            let (exit_tx, mut exit_rx) = tokio::sync::mpsc::unbounded_channel::<()>();
                            std::thread::spawn(move || {
                                let _ = child.wait();
                                let _ = exit_tx.send(());
                            });
                            glib::MainContext::default().spawn_local(async move {
                                if exit_rx.recv().await.is_some() {
                                    this_proc.finish_connecting(&pc_id_proc);
                                    status_proc.set_text("Session ended.");
                                }
                            });

                            let (tx, mut rx_async) =
                                tokio::sync::mpsc::unbounded_channel::<String>();
                            std::thread::spawn(move || {
                                while let Ok(url) = rx.recv() {
                                    if tx.send(url).is_err() {
                                        break;
                                    }
                                }
                            });

                            // Watchdog: If no auth URL is required (immediate connection), re-enable after window loads
                            let this_wd = Rc::clone(&this_launch);
                            let pc_id_wd = pc_id_launch.clone();
                            let status_wd = status.clone();
                            glib::timeout_add_local_once(
                                std::time::Duration::from_secs(5),
                                move || {
                                    this_wd.finish_connecting(&pc_id_wd);
                                    if status_wd.text().contains("Starting") || status_wd.text().contains("Connecting") {
                                        status_wd.set_text("Connected to Windows desktop.");
                                    }
                                },
                            );

                            glib::MainContext::default().spawn_local(async move {
                                while let Some(mut auth_url) = rx_async.recv().await {
                                    debug!("Received FreeRDP authentication challenge");
                                    if !auth_url.contains("login_hint=") {
                                        let encoded_upn: String =
                                            url::form_urlencoded::byte_serialize(
                                                user_upn_auth.as_bytes(),
                                            )
                                            .collect();
                                        let sep = if auth_url.contains("?") { "&" } else { "?" };
                                        auth_url =
                                            format!("{auth_url}{sep}login_hint={encoded_upn}");
                                    }

                                    status_auth.set_text("Authenticating Cloud PC session...");
                                    let status_finish = status_auth.clone();
                                    let this_to = Rc::clone(&this_auth);
                                    let pc_id_to = pc_id_auth.clone();

                                    RdpAuthDialog::show(&parent, &auth_url, move |redirect_url| {
                                        debug!("Submitting authentication redirect callback to RDP session");
                                        status_finish
                                            .set_text("Authentication verified. Window opening...");

                                        if let Err(e) = RdpLauncher::send_redirect_url(
                                            pty_master,
                                            &redirect_url,
                                        ) {
                                            warn!("Failed to send redirect URL to RDP: {e}");
                                        }

                                        // Keep disabled with spinner active until desktop window is fully rendered on screen
                                        let this_timeout = Rc::clone(&this_to);
                                        let pc_id_timeout = pc_id_to.clone();
                                        let status_to = status_finish.clone();
                                        glib::timeout_add_local_once(
                                            std::time::Duration::from_secs(4),
                                            move || {
                                                this_timeout.finish_connecting(&pc_id_timeout);
                                                status_to.set_text("Connected to Windows desktop.");
                                            },
                                        );
                                    });
                                }
                            });
                        }
                        Err(e) => {
                            status.set_text(&format!("Failed to launch RDP session: {e}"));
                            this_launch.finish_connecting(&pc_id_launch);
                        }
                    }
                }
                Err(e) => {
                    status.set_text(&format!("Failed to prepare connection: {e}"));
                    this_launch.finish_connecting(&pc_id_launch);
                }
            }
        });
    }

    /// Sends a restart command to the specified Cloud PC.
    fn restart_cloud_pc(self: &Rc<Self>, pc_id: String) {
        let service = Arc::clone(&self.cloud_service);
        let status = self.status_label.clone();

        glib::MainContext::default().spawn_local(async move {
            status.set_text("Sending restart command...");
            match service.restart_cloud_pc(&pc_id).await {
                Ok(()) => status.set_text("Restart command accepted."),
                Err(e) => status.set_text(&format!("Restart failed: {e}")),
            }
        });
    }

    /// Displays the Rename modal dialog.
    fn rename_cloud_pc(self: &Rc<Self>, pc: CloudPc) {
        let parent = &self.window;
        let service = Arc::clone(&self.cloud_service);
        let this = Rc::clone(self);

        let dialog = gtk4::Window::builder()
            .transient_for(parent)
            .modal(true)
            .title("Rename Cloud PC")
            .default_width(380)
            .default_height(160)
            .build();

        let vbox = Box::new(Orientation::Vertical, 12);
        vbox.set_margin_start(20);
        vbox.set_margin_end(20);
        vbox.set_margin_top(16);
        vbox.set_margin_bottom(16);

        let label = Label::new(Some("Enter a new display name:"));
        label.set_halign(gtk4::Align::Start);

        let entry = Entry::new();
        entry.set_text(&pc.display_name);

        let btn_box = Box::new(Orientation::Horizontal, 8);
        btn_box.set_halign(gtk4::Align::End);
        let cancel_btn = Button::with_label("Cancel");
        let ok_btn = Button::with_label("Rename");
        ok_btn.add_css_class("suggested-action");

        let d_close = dialog.clone();
        cancel_btn.connect_clicked(move |_| d_close.close());

        let pc_id = pc.id.clone();
        let d_ok = dialog.clone();
        let entry_c = entry.clone();
        ok_btn.connect_clicked(move |_| {
            let new_name = entry_c.text().to_string();
            let service = Arc::clone(&service);
            let pc_id = pc_id.clone();
            let d = d_ok.clone();
            let this_l = Rc::clone(&this);
            glib::MainContext::default().spawn_local(async move {
                let _ = service.rename_cloud_pc(&pc_id, &new_name).await;
                this_l.load_cloud_pcs();
                d.close();
            });
        });

        btn_box.append(&cancel_btn);
        btn_box.append(&ok_btn);
        vbox.append(&label);
        vbox.append(&entry);
        vbox.append(&btn_box);

        dialog.set_child(Some(&vbox));
        dialog.present();
    }

    /// Displays the Inspect Connection diagnostics modal dialog.
    fn inspect_connection(&self, pc: CloudPc) {
        let config = AppConfig::load();
        let engine_str = match config.preferred_rdp_client.as_str() {
            "ironrdp" => "IronRDP (Pure Rust - Experimental Direct RDP Engine)",
            _ => "In-App Native FreeRDP 3 (Embedded GTK 4 C-FFI Engine)",
        };

        let dialog = gtk4::Window::builder()
            .transient_for(&self.window)
            .modal(true)
            .title(&format!("Connection Diagnostics - {}", pc.display_name))
            .default_width(520)
            .default_height(440)
            .build();

        let vbox = Box::new(Orientation::Vertical, 10);
        vbox.set_margin_start(24);
        vbox.set_margin_end(24);
        vbox.set_margin_top(18);
        vbox.set_margin_bottom(18);

        let items = [
            ("Cloud PC Name", pc.display_name.clone()),
            ("Cloud PC ID", pc.id.clone()),
            (
                "Assigned User",
                pc.user_principal_name
                    .clone()
                    .unwrap_or_else(|| "N/A".to_string()),
            ),
            (
                "Gateway Host",
                "afdfp-rdgateway-r1.wvd.microsoft.com:443".to_string(),
            ),
            (
                "Gateway Protocol",
                "Azure Virtual Desktop ARM WebSocket Bearer (/gateway:type:arm)".to_string(),
            ),
            (
                "Broker Authentication",
                "Entra ID Headless WAM Broker (0.6s Silent SSO)".to_string(),
            ),
            ("Configured Engine", engine_str.to_string()),
            (
                "Active RDP Binary",
                "FreeRDP 3.32 (with /gfx AVC420 & 0x0019 Surface Protection)".to_string(),
            ),
            (
                "Desktop Resolution Scaling",
                "125% DPI (120 DPI) Auto-negotiated via MS-RDPEDISP".to_string(),
            ),
            (
                "Host Pool ID",
                pc.host_pool_id
                    .clone()
                    .unwrap_or_else(|| "Dynamic WVD Pool".to_string()),
            ),
            (
                "Device Region",
                pc.device_region_name
                    .clone()
                    .unwrap_or_else(|| "India Central / APAC".to_string()),
            ),
        ];

        for (k, v) in items {
            let row = Box::new(Orientation::Vertical, 2);
            let kl = Label::new(Some(k));
            kl.set_halign(gtk4::Align::Start);
            kl.add_css_class("dim-label");

            let vl = Label::new(Some(&v));
            vl.set_halign(gtk4::Align::Start);
            vl.set_wrap(true);
            vl.set_selectable(true);

            row.append(&kl);
            row.append(&vl);
            vbox.append(&row);
            vbox.append(&Separator::new(Orientation::Horizontal));
        }

        let close_btn = Button::with_label("Close");
        close_btn.set_halign(gtk4::Align::End);
        let d = dialog.clone();
        close_btn.connect_clicked(move |_| d.close());
        vbox.append(&close_btn);

        let scrolled = ScrolledWindow::builder().child(&vbox).build();
        dialog.set_child(Some(&scrolled));
        dialog.present();
    }

    /// Displays the View Details modal dialog.
    fn view_details(&self, pc: CloudPc) {
        let dialog = gtk4::Window::builder()
            .transient_for(&self.window)
            .modal(true)
            .title(&format!("Cloud PC Details - {}", pc.display_name))
            .default_width(480)
            .default_height(400)
            .build();

        let vbox = Box::new(Orientation::Vertical, 10);
        vbox.set_margin_start(24);
        vbox.set_margin_end(24);
        vbox.set_margin_top(18);
        vbox.set_margin_bottom(18);

        let items = [
            ("Device Name", pc.display_name.clone()),
            (
                "Operating System",
                pc.image_display_name
                    .clone()
                    .unwrap_or_else(|| "Windows 11 Enterprise".to_string()),
            ),
            ("Hardware Summary", pc.hardware_summary().to_string()),
            ("Provisioning Status", pc.display_status().to_string()),
            (
                "Disk Encryption",
                pc.disk_encryption_state
                    .clone()
                    .unwrap_or_else(|| "Encrypted (BitLocker)".to_string()),
            ),
            (
                "User Principal Name",
                pc.user_principal_name
                    .clone()
                    .unwrap_or_else(|| "N/A".to_string()),
            ),
            ("Device ID", pc.id.clone()),
        ];

        for (k, v) in items {
            let row = Box::new(Orientation::Horizontal, 12);
            let kl = Label::new(Some(k));
            kl.set_halign(gtk4::Align::Start);
            kl.add_css_class("dim-label");

            let vl = Label::new(Some(&v));
            vl.set_halign(gtk4::Align::End);
            vl.set_hexpand(true);
            vl.set_selectable(true);

            row.append(&kl);
            row.append(&vl);
            vbox.append(&row);
            vbox.append(&Separator::new(Orientation::Horizontal));
        }

        let close_btn = Button::with_label("Close");
        close_btn.set_halign(gtk4::Align::End);
        let d = dialog.clone();
        close_btn.connect_clicked(move |_| d.close());
        vbox.append(&close_btn);

        dialog.set_child(Some(&vbox));
        dialog.present();
    }
}
