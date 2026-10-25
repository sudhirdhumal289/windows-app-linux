//! Embedded in-app WebKitGTK sign-in window for Microsoft Entra ID.

use crate::auth::pkap::DeviceIdentity;
use crate::auth::pkce::Pkce;
use crate::auth::AuthManager;
use crate::config::{
    AppConfig, AVD_CLIENT_ID, COMMON_AUTHORITY, DEFAULT_AUTHORITY,
    WINDOWS_APP_CLIENT_ID, WINDOWS_APP_REDIRECT_URI, WVD_REDIRECT_URI,
};
use crate::models::TokenResponse;
use chrono::Utc;
use gtk4::gdk::Display;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{
    Box, Button, HeaderBar, Label, Orientation, ScrolledWindow, Spinner, Stack,
    TextView, Window,
};
use log::{debug, error, warn};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use url::Url;
use webkit6::prelude::*;
use webkit6::{
    LoadEvent, NavigationPolicyDecision, PolicyDecisionType,
    Settings, URIRequest, UserContentInjectedFrames, UserContentManager, UserScript,
    UserScriptInjectionTime, WebView,
};

const MAC_CLIENT_ID: &str = "12a27547-9124-4f81-8064-a78c94625d97";
const MAC_REDIRECT_URI: &str = "https://login.microsoftonline.com/common/oauth2/nativeclient";

pub const UA_WINDOWS_11_WAM: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64; WebView/3.0) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/70.0.3538.102 Safari/537.36 Edge/18.22631";
const UA_LINUX_CHROME: &str =
    "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";
const _UA_WINDOWS_11_EDGE: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36 Edg/131.0.0.0 win365.nxt/2.0.1484.0";
const _UA_WINDOWS_11_CHROME: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";
const UA_MAC_SAFARI: &str =
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.4 Safari/605.1.15";

#[derive(Clone)]
struct ActiveAuthState {
    auth_manager: Arc<AuthManager>,
    verifier: String,
    expected_redirect: String,
    os_param: Option<String>,
    user_agent: String,
    completed: bool,
}

pub struct LoginDialog;

impl LoginDialog {
    /// Launches an embedded in-app WebKit browser window for Microsoft Entra authentication.
    pub fn show_embedded<P, FSuccess, FClose>(
        parent: &P,
        _auth: Arc<AuthManager>,
        initial_view: Option<&str>,
        on_success: FSuccess,
        on_close: FClose,
    ) where
        P: IsA<Window>,
        FSuccess: Fn() + 'static,
        FClose: Fn() + 'static,
    {
        let completed = Arc::new(AtomicBool::new(false));
        let window = Window::builder()
            .transient_for(parent)
            .modal(true)
            .title("Sign in - Windows App")
            .default_width(620)
            .default_height(800)
            .build();

        let header = HeaderBar::new();
        window.set_titlebar(Some(&header));

        let view_stack = Stack::new();
        view_stack.set_transition_type(gtk4::StackTransitionType::Crossfade);

        // =========================================================================
        // VIEW 1: In-App Web Browser View
        // =========================================================================
        let web_box = Box::new(Orientation::Vertical, 0);

        // Spinner / Progress indicator shown during token exchange
        let spinner_box = Box::new(Orientation::Vertical, 12);
        spinner_box.set_valign(gtk4::Align::Center);
        spinner_box.set_halign(gtk4::Align::Center);
        spinner_box.set_vexpand(true);
        let spinner = Spinner::new();
        spinner.set_size_request(40, 40);
        let status_label = Label::new(Some("Completing authentication..."));
        status_label.add_css_class("dim-label");
        spinner_box.append(&spinner);
        spinner_box.append(&status_label);
        spinner_box.set_visible(false);

        // Persistent network session for cookies and storage shared with Cloud PC session
        let network_session = crate::ui::get_shared_network_session();

        let content_manager = UserContentManager::new();

        // Native Windows 11 WAM Broker identity matching Microsoft.AAD.BrokerPlugin EdgeHTML runtime
        let wam_mask_script = UserScript::new(
            r#"
            (function() {
                try {
                    Object.defineProperty(navigator, 'platform', { get: () => 'Win32', configurable: true });
                    Object.defineProperty(navigator, 'oscpu', { get: () => 'Windows NT 10.0; Win64; x64', configurable: true });
                    Object.defineProperty(navigator, 'vendor', { get: () => '', configurable: true });
                    Object.defineProperty(navigator, 'webdriver', { get: () => false, configurable: true });
                    Object.defineProperty(navigator, 'appVersion', {
                        get: () => '5.0 (Windows NT 10.0; Win64; x64; WebView/3.0) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/70.0.3538.102 Safari/537.36 Edge/18.22631',
                        configurable: true
                    });
                    Object.defineProperty(navigator, 'userAgent', {
                        get: () => 'Mozilla/5.0 (Windows NT 10.0; Win64; x64; WebView/3.0) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/70.0.3538.102 Safari/537.36 Edge/18.22631',
                        configurable: true
                    });
                    try { delete window.chrome; } catch(e) { window.chrome = undefined; }
                    try {
                        Object.defineProperty(navigator, 'userAgentData', { get: () => undefined, configurable: true });
                    } catch(e) {}
                    window.external = window.external || {};
                } catch(e) {}
            })();
            "#,
            UserContentInjectedFrames::AllFrames,
            UserScriptInjectionTime::Start,
            &[],
            &[],
        );

        let win11_mask_script = UserScript::new(
            r#"
            (function() {
                try {
                    Object.defineProperty(navigator, 'platform', { get: () => 'Win32', configurable: true });
                    Object.defineProperty(navigator, 'oscpu', { get: () => 'Windows NT 10.0; Win64; x64', configurable: true });
                    Object.defineProperty(navigator, 'vendor', { get: () => 'Google Inc.', configurable: true });
                    Object.defineProperty(navigator, 'appVersion', {
                        get: () => '5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36 Edg/131.0.0.0 win365.nxt/2.0.1484.0',
                        configurable: true
                    });
                    if (!window.chrome) {
                        window.chrome = { runtime: {}, loadTimes: function() {}, csi: function() {}, app: {} };
                    }
                    const uaData = {
                        brands: [
                            { brand: 'Microsoft Edge', version: '131' },
                            { brand: 'Chromium', version: '131' },
                            { brand: 'Not_A Brand', version: '24' }
                        ],
                        mobile: false,
                        platform: 'Windows',
                        getHighEntropyValues: function() {
                            return Promise.resolve({
                                architecture: 'x86',
                                bitness: '64',
                                brands: [
                                    { brand: 'Microsoft Edge', version: '131.0.2903.86' },
                                    { brand: 'Chromium', version: '131.0.6778.86' },
                                    { brand: 'Not_A Brand', version: '24.0.0.0' }
                                ],
                                fullVersionList: [
                                    { brand: 'Microsoft Edge', version: '131.0.2903.86' },
                                    { brand: 'Chromium', version: '131.0.6778.86' },
                                    { brand: 'Not_A Brand', version: '24.0.0.0' }
                                ],
                                mobile: false, model: '', platform: 'Windows',
                                platformVersion: '15.0.0', uaFullVersion: '131.0.2903.86', wow64: false
                            });
                        },
                        toJSON: function() { return { brands: this.brands, mobile: this.mobile, platform: this.platform }; }
                    };
                    Object.defineProperty(navigator, 'userAgentData', { get: () => uaData, configurable: true });
                } catch(e) {}
            })();
            "#,
            UserContentInjectedFrames::AllFrames,
            UserScriptInjectionTime::Start,
            &[],
            &[],
        );

        let win11_chrome_mask_script = UserScript::new(
            r#"
            (function() {
                try {
                    Object.defineProperty(navigator, 'platform', { get: () => 'Win32', configurable: true });
                    Object.defineProperty(navigator, 'oscpu', { get: () => 'Windows NT 10.0; Win64; x64', configurable: true });
                    Object.defineProperty(navigator, 'vendor', { get: () => 'Google Inc.', configurable: true });
                    Object.defineProperty(navigator, 'appVersion', {
                        get: () => '5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36',
                        configurable: true
                    });
                    if (!window.chrome) {
                        window.chrome = { runtime: {}, loadTimes: function() {}, csi: function() {}, app: {} };
                    }
                    const uaData = {
                        brands: [
                            { brand: 'Chromium', version: '131' },
                            { brand: 'Google Chrome', version: '131' },
                            { brand: 'Not_A Brand', version: '24' }
                        ],
                        mobile: false,
                        platform: 'Windows',
                        getHighEntropyValues: function() {
                            return Promise.resolve({
                                architecture: 'x86',
                                bitness: '64',
                                brands: [
                                    { brand: 'Chromium', version: '131.0.6778.86' },
                                    { brand: 'Google Chrome', version: '131.0.6778.86' },
                                    { brand: 'Not_A Brand', version: '24.0.0.0' }
                                ],
                                fullVersionList: [
                                    { brand: 'Chromium', version: '131.0.6778.86' },
                                    { brand: 'Google Chrome', version: '131.0.6778.86' },
                                    { brand: 'Not_A Brand', version: '24.0.0.0' }
                                ],
                                mobile: false, model: '', platform: 'Windows',
                                platformVersion: '15.0.0', uaFullVersion: '131.0.6778.86', wow64: false
                            });
                        },
                        toJSON: function() { return { brands: this.brands, mobile: this.mobile, platform: this.platform }; }
                    };
                    Object.defineProperty(navigator, 'userAgentData', { get: () => uaData, configurable: true });
                } catch(e) {}
            })();
            "#,
            UserContentInjectedFrames::AllFrames,
            UserScriptInjectionTime::Start,
            &[],
            &[],
        );

        // Embedded WebKitGTK Web View
        let mut webview_builder = WebView::builder()
            .user_content_manager(&content_manager)
            .vexpand(true)
            .hexpand(true);

        webview_builder = webview_builder.network_session(&network_session);

        let webview = webview_builder.build();

        let settings = Settings::new();
        settings.set_enable_javascript(true);
        settings.set_javascript_can_open_windows_automatically(true);
        settings.set_enable_developer_extras(true);
        settings.set_enable_back_forward_navigation_gestures(true);
        settings.set_user_agent(Some(UA_WINDOWS_11_WAM));
        webview.set_settings(&settings);

        web_box.append(&webview);
        web_box.append(&spinner_box);
        view_stack.add_named(&web_box, Some("browser"));

        // =========================================================================
        // VIEW 2: Edge Browser / Device Code View
        // =========================================================================
        let device_box = Box::new(Orientation::Vertical, 16);
        device_box.set_margin_start(32);
        device_box.set_margin_end(32);
        device_box.set_margin_top(28);
        device_box.set_margin_bottom(28);
        device_box.set_valign(gtk4::Align::Center);
        device_box.set_halign(gtk4::Align::Center);

        let dev_title = Label::new(Some("Sign in with Microsoft Edge / Work Profile"));
        dev_title.add_css_class("title-2");
        device_box.append(&dev_title);

        let dev_desc = Label::new(Some(
            "If your organization's Conditional Access policy requires an Intune-managed Microsoft Edge work profile, \
            click 'Open Microsoft Edge' below and enter this authorization code:",
        ));
        dev_desc.set_wrap(true);
        dev_desc.set_max_width_chars(52);
        dev_desc.set_justify(gtk4::Justification::Center);
        dev_desc.add_css_class("dim-label");
        device_box.append(&dev_desc);

        let code_display_box = Box::new(Orientation::Horizontal, 12);
        code_display_box.set_halign(gtk4::Align::Center);
        let code_label = Label::new(Some("Click 'Generate Code'"));
        code_label.add_css_class("title-1");
        code_label.add_css_class("monospace");
        code_label.set_selectable(true);
        code_display_box.append(&code_label);

        let copy_code_btn = Button::with_label("Copy Code");
        copy_code_btn.add_css_class("suggested-action");
        code_display_box.append(&copy_code_btn);
        device_box.append(&code_display_box);

        let action_btn_box = Box::new(Orientation::Horizontal, 12);
        action_btn_box.set_halign(gtk4::Align::Center);

        let open_browser_btn = Button::with_label("Open Microsoft Edge / Browser");
        open_browser_btn.add_css_class("suggested-action");
        action_btn_box.append(&open_browser_btn);

        let refresh_code_btn = Button::with_label("Get New Code");
        action_btn_box.append(&refresh_code_btn);
        device_box.append(&action_btn_box);

        let dev_spinner_box = Box::new(Orientation::Horizontal, 8);
        dev_spinner_box.set_halign(gtk4::Align::Center);
        let dev_spinner = Spinner::new();
        dev_spinner.set_size_request(24, 24);
        let dev_poll_status = Label::new(Some("Waiting for code generation..."));
        dev_poll_status.add_css_class("dim-label");
        dev_spinner_box.append(&dev_spinner);
        dev_spinner_box.append(&dev_poll_status);
        device_box.append(&dev_spinner_box);

        let switch_to_web_btn = Button::with_label("Switch to In-App Web Browser");
        switch_to_web_btn.add_css_class("flat");
        let vs_dev_to_web = view_stack.clone();
        let win_dev = window.clone();
        switch_to_web_btn.connect_clicked(move |_| {
            vs_dev_to_web.set_visible_child_name("browser");
            win_dev.set_title(Some("Sign in - Windows App"));
        });
        device_box.append(&switch_to_web_btn);

        view_stack.add_named(&device_box, Some("device"));

        // =========================================================================
        // VIEW 3: Manual Token Import View
        // =========================================================================
        let import_box = Box::new(Orientation::Vertical, 16);
        import_box.set_margin_start(24);
        import_box.set_margin_end(24);
        import_box.set_margin_top(24);
        import_box.set_margin_bottom(24);

        let import_title = Label::new(Some("Import Session / OAuth Token"));
        import_title.add_css_class("title-2");
        import_title.set_halign(gtk4::Align::Start);
        import_box.append(&import_title);

        let import_desc = Label::new(Some(
            "If Conditional Access blocks direct authentication, you can paste an Access Token \
            or JSON token response from your authenticated session (e.g. windows365.microsoft.com).",
        ));
        import_desc.set_wrap(true);
        import_desc.set_halign(gtk4::Align::Start);
        import_desc.add_css_class("dim-label");
        import_box.append(&import_desc);

        let token_scrolled = ScrolledWindow::builder()
            .hscrollbar_policy(gtk4::PolicyType::Never)
            .vscrollbar_policy(gtk4::PolicyType::Automatic)
            .vexpand(true)
            .build();
        let token_text_view = TextView::new();
        token_text_view.set_wrap_mode(gtk4::WrapMode::Char);
        token_text_view.set_monospace(true);
        token_scrolled.set_child(Some(&token_text_view));
        token_scrolled.set_size_request(-1, 200);
        import_box.append(&token_scrolled);

        let import_error_lbl = Label::new(None);
        import_error_lbl.add_css_class("error");
        import_box.append(&import_error_lbl);

        let save_token_btn = Button::with_label("Connect with Token");
        save_token_btn.add_css_class("suggested-action");
        save_token_btn.set_size_request(-1, 40);
        import_box.append(&save_token_btn);

        let switch_to_web_btn2 = Button::with_label("Switch to In-App Web Browser");
        switch_to_web_btn2.add_css_class("flat");
        let vs_imp_to_web = view_stack.clone();
        let win_imp = window.clone();
        switch_to_web_btn2.connect_clicked(move |_| {
            vs_imp_to_web.set_visible_child_name("browser");
            win_imp.set_title(Some("Sign in - Windows App"));
        });
        import_box.append(&switch_to_web_btn2);

        view_stack.add_named(&import_box, Some("import"));
        window.set_child(Some(&view_stack));

        let on_success = Arc::new(on_success);

        // Wire Save Token Button
        let tv_clone = token_text_view.clone();
        let err_clone = import_error_lbl.clone();
        let win_token_clone = window.clone();
        let on_succ_token = Arc::clone(&on_success);
        let comp_token = Arc::clone(&completed);
        save_token_btn.connect_clicked(move |_| {
            let buffer = tv_clone.buffer();
            let start = buffer.start_iter();
            let end = buffer.end_iter();
            let raw_text = buffer.text(&start, &end, false).trim().to_string();

            if raw_text.is_empty() {
                err_clone.set_text("Please paste an Access Token or JSON response.");
                return;
            }

            let token_resp = if raw_text.starts_with('{') {
                match serde_json::from_str::<TokenResponse>(&raw_text) {
                    Ok(mut t) => {
                        if t.acquired_at.is_none() {
                            t.acquired_at = Some(Utc::now().timestamp());
                        }
                        t
                    }
                    Err(e) => {
                        err_clone.set_text(&format!("Invalid JSON token: {e}"));
                        return;
                    }
                }
            } else {
                let clean_token = if let Some(stripped) = raw_text.strip_prefix("Bearer ") {
                    stripped.trim().to_string()
                } else if let Some(stripped) = raw_text.strip_prefix("bearer ") {
                    stripped.trim().to_string()
                } else {
                    raw_text.clone()
                };

                TokenResponse {
                    token_type: "Bearer".to_string(),
                    scope: Some("https://graph.microsoft.com/CloudPC.Read.All https://graph.microsoft.com/User.Read".to_string()),
                    expires_in: 3600,
                    ext_expires_in: Some(3600),
                    access_token: clean_token,
                    refresh_token: None,
                    id_token: None,
                    acquired_at: Some(Utc::now().timestamp()),
                }
            };

            match AppConfig::save_tokens(&token_resp) {
                Ok(_) => {
                    debug!("Successfully saved imported token");
                    comp_token.store(true, Ordering::SeqCst);
                    win_token_clone.close();
                    on_succ_token();
                }
                Err(e) => {
                    err_clone.set_text(&format!("Failed to save token: {e}"));
                }
            }
        });

        // =========================================================================
        // Device Code Flow Implementation (For Edge Work Profile / MAM bypass)
        // =========================================================================
        let device_poll_active = Arc::new(AtomicBool::new(true));
        let poll_flag_close = Arc::clone(&device_poll_active);
        let comp_close = Arc::clone(&completed);
        let on_close_rc = Arc::new(on_close);
        window.connect_close_request(move |_| {
            poll_flag_close.store(false, Ordering::SeqCst);
            if !comp_close.load(Ordering::SeqCst) {
                on_close_rc();
            }
            glib::Propagation::Proceed
        });

        let current_user_code = Rc::new(RefCell::new(String::new()));
        let comp_dev_flow = Arc::clone(&completed);
        let initiate_device_flow = {
            let code_label = code_label.clone();
            let dev_spinner = dev_spinner.clone();
            let dev_poll_status = dev_poll_status.clone();
            let on_success = Arc::clone(&on_success);
            let window = window.clone();
            let device_poll_active = Arc::clone(&device_poll_active);
            let current_user_code = Rc::clone(&current_user_code);
            let comp_dev = Arc::clone(&comp_dev_flow);

            move || {
                let authority = COMMON_AUTHORITY.to_string();

                let auth = Arc::new(AuthManager::new(&authority, WINDOWS_APP_CLIENT_ID));
                let c_lbl = code_label.clone();
                let d_spin = dev_spinner.clone();
                let d_stat = dev_poll_status.clone();
                let on_succ = Arc::clone(&on_success);
                let win = window.clone();
                let poll_active = Arc::clone(&device_poll_active);
                let u_code_ref = Rc::clone(&current_user_code);
                let comp_dev_inner = Arc::clone(&comp_dev);

                c_lbl.set_text("Generating...");
                d_spin.start();
                d_spin.set_visible(true);
                d_stat.set_text("Connecting to Microsoft Entra ID...");

                glib::MainContext::default().spawn_local(async move {
                    match auth.initiate_device_code().await {
                        Ok(resp) => {
                            debug!("Device Code initiated successfully");
                            c_lbl.set_text(&resp.user_code);
                            *u_code_ref.borrow_mut() = resp.user_code.clone();
                            d_stat.set_text("Waiting for approval in your Edge work profile...");

                            // Automatically copy code to clipboard for convenience
                            if let Some(display) = Display::default() {
                                display.clipboard().set_text(&resp.user_code);
                            }

                            // Begin polling for user approval
                            poll_active.store(true, Ordering::SeqCst);
                            let device_code = resp.device_code.clone();
                            let interval = resp.interval;
                            match auth.poll_device_token(&device_code, interval).await {
                                Ok(_) => {
                                    debug!("Device token acquisition completed successfully");
                                    d_stat.set_text("Authenticated! Loading Cloud PCs...");
                                    comp_dev_inner.store(true, Ordering::SeqCst);
                                    win.close();
                                    on_succ();
                                }
                                Err(e) => {
                                    if poll_active.load(Ordering::SeqCst) {
                                        warn!("Device token polling stopped: {e}");
                                        d_stat
                                            .set_text(&format!("Approval expired or failed: {e}"));
                                        d_spin.stop();
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            error!("Failed to initiate device code flow: {e}");
                            c_lbl.set_text("Error");
                            d_stat.set_text(&format!("Device code request failed: {e}"));
                            d_spin.stop();
                        }
                    }
                });
            }
        };

        // Wire Copy Code Button
        let u_code_copy = Rc::clone(&current_user_code);
        let d_stat_copy = dev_poll_status.clone();
        copy_code_btn.connect_clicked(move |_| {
            let code = u_code_copy.borrow().clone();
            if !code.is_empty() && code != "Click 'Generate Code'" && code != "Generating..." {
                if let Some(display) = Display::default() {
                    display.clipboard().set_text(&code);
                    d_stat_copy.set_text("Code copied to clipboard! Paste it into Edge.");
                }
            }
        });

        // Wire Open Edge / Browser Button
        let u_code_open = Rc::clone(&current_user_code);
        open_browser_btn.connect_clicked(move |_| {
            let code = u_code_open.borrow().clone();
            if !code.is_empty() {
                if let Some(display) = Display::default() {
                    display.clipboard().set_text(&code);
                }
            }
            let _ = open::that("https://login.microsoft.com/device");
        });

        // Wire Get New Code Button
        let init_dev_for_btn = initiate_device_flow.clone();
        refresh_code_btn.connect_clicked(move |_| {
            init_dev_for_btn();
        });



        // =========================================================================
        // In-App Web View State & Callbacks
        // =========================================================================
        let device_identity = Arc::new(DeviceIdentity::try_load_default());
        let initial_auth = Arc::new(AuthManager::new(COMMON_AUTHORITY, WINDOWS_APP_CLIENT_ID));
        let active_state = Rc::new(RefCell::new(ActiveAuthState {
            auth_manager: initial_auth,
            verifier: String::new(),
            expected_redirect: WINDOWS_APP_REDIRECT_URI.to_string(),
            os_param: Some("Microsoft Windows NT 10.0.22631.0".to_string()),
            user_agent: UA_WINDOWS_11_WAM.to_string(),
            completed: false,
        }));

        // Helper closure to handle intercepted OAuth callback
        let comp_web_flow = Arc::clone(&completed);
        let handle_callback = {
            let device_identity = Arc::clone(&device_identity);
            let active_state = Rc::clone(&active_state);
            let spinner_box = spinner_box.clone();
            let status_label = status_label.clone();
            let spinner = spinner.clone();
            let webview = webview.clone();
            let window = window.clone();
            let on_success = Arc::clone(&on_success);
            let comp_web = Arc::clone(&comp_web_flow);

            move |uri_str: &str| -> bool {
                let mut state = active_state.borrow_mut();
                if state.completed {
                    return false;
                }

                let uri = uri_str;

                if uri.starts_with("urn:http-auth:PKeyAuth") {
                    debug!("Received Entra ID PKeyAuth device challenge");
                    if let Ok(parsed) = Url::parse(uri) {
                        let query_map: std::collections::HashMap<_, _> =
                            parsed.query_pairs().into_owned().collect();
                        if let Some(submit_url) = query_map.get("SubmitUrl") {
                            let context =
                                query_map.get("Context").map(String::as_str).unwrap_or("");
                            let nonce = query_map.get("Nonce").map(String::as_str).unwrap_or("");

                            let auth_val = if let Some(ref dev_id) = device_identity.as_ref() {
                                match dev_id.sign_challenge(submit_url, nonce, context) {
                                    Ok(signed_auth) => {
                                        debug!("Successfully signed MS-PKAP PKeyAuth challenge with device RSA certificate");
                                        status_label
                                            .set_text("Device identity verified via PKeyAuth...");
                                        signed_auth
                                    }
                                    Err(e) => {
                                        warn!("Failed to sign PKeyAuth challenge: {e}");
                                        format!(r#"PKeyAuth Context="{}", Version="1.0""#, context)
                                    }
                                }
                            } else {
                                debug!("No device certificate found in config directory; submitting Context only");
                                status_label.set_text("Processing device verification...");
                                format!(r#"PKeyAuth Context="{}", Version="1.0""#, context)
                            };

                            let req = URIRequest::new(submit_url);
                            if let Some(headers) = req.http_headers() {
                                headers.append("Authorization", &auth_val);
                                headers.append("User-Agent", &state.user_agent);
                                headers.append("x-client-SKU", "MSAL.Desktop");
                                headers.append("x-client-Ver", "4.60.0.0");
                                if let Some(ref os) = state.os_param {
                                    headers.append("x-client-OS", os);
                                }
                            }
                            webview.load_request(&req);
                            return true;
                        }
                    }
                    return false;
                }

                let is_target = uri.starts_with("ms-appx-web://")
                    || uri.starts_with(&state.expected_redirect)
                    || uri.starts_with(
                        "https://login.microsoftonline.com/common/oauth2/nativeclient",
                    )
                    || (uri.contains("code=")
                        && (uri.contains("brokerplugin")
                            || uri.contains("webclient")
                            || uri.contains("nativeclient")));

                if !is_target {
                    return false;
                }

                state.completed = true;
                debug!("Intercepted OAuth redirect callback");

                if let Ok(parsed_url) = Url::parse(uri) {
                    let query_map: std::collections::HashMap<_, _> =
                        parsed_url.query_pairs().into_owned().collect();

                    if let Some(code) = query_map.get("code") {
                        let code = code.clone();
                        let verifier = state.verifier.clone();
                        let auth = Arc::clone(&state.auth_manager);
                        let redir = state.expected_redirect.clone();
                        let os_param = state.os_param.clone();
                        let win = window.clone();
                        let on_succ = Arc::clone(&on_success);
                        let st_lbl = status_label.clone();
                        let comp_web_inner = Arc::clone(&comp_web);

                        webview.set_visible(false);
                        spinner_box.set_visible(true);
                        spinner.start();
                        st_lbl.set_text("Exchanging authorization code for tokens...");

                        glib::MainContext::default().spawn_local(async move {
                            match auth
                                .exchange_authorization_code(
                                    &code,
                                    &verifier,
                                    &redir,
                                    os_param.as_deref(),
                                )
                                .await
                            {
                                Ok(_) => {
                                    debug!("Authentication successfully verified");
                                    crate::ui::persist_session_cookies();
                                    comp_web_inner.store(true, Ordering::SeqCst);
                                    win.close();
                                    on_succ();
                                }
                                Err(e) => {
                                    error!("Token exchange error: {e}");
                                    st_lbl.set_text(&format!("Sign-in error: {e}"));
                                }
                            }
                        });
                        return true;
                    } else if let Some(err_desc) = query_map.get("error_description") {
                        error!("OAuth error: {err_desc}");
                        status_label.set_text(&format!("Authentication error: {err_desc}"));
                        return true;
                    }
                }
                false
            }
        };

        // Connect policy decision handler to intercept custom schemes (ms-appx-web://) cleanly
        let handle_cb_policy = handle_callback.clone();
        let wv_policy = webview.clone();
        webview.connect_decide_policy(move |_view, decision, decision_type| {
            if decision_type == PolicyDecisionType::NavigationAction
                || decision_type == PolicyDecisionType::NewWindowAction
            {
                if let Some(nav_decision) = decision.dynamic_cast_ref::<NavigationPolicyDecision>()
                {
                    if let Some(nav_action) = nav_decision.navigation_action() {
                        if let Some(req) = nav_action.request() {
                            if let Some(uri) = req.uri() {
                                if (uri.starts_with("ms-appx-web://")
                                    || uri.contains("code=")
                                    || uri.starts_with("urn:http-auth:PKeyAuth"))
                                    && handle_cb_policy(uri.as_str())
                                {
                                    decision.ignore();
                                    return true;
                                }

                                if decision_type == PolicyDecisionType::NewWindowAction {
                                    wv_policy.load_uri(uri.as_str());
                                    decision.ignore();
                                    return true;
                                }
                            }
                        }
                    }
                }
            }
            false
        });

        // Connect load changed as safety net for standard web redirects
        let handle_cb_load = handle_callback.clone();
        webview.connect_load_changed(move |view, event| {
            if event != LoadEvent::Started && event != LoadEvent::Committed {
                return;
            }
            if let Some(uri) = view.uri() {
                handle_cb_load(uri.as_str());
            }
        });

        // Function to construct authority and load URL for in-app browser
        let load_auth = {
            let webview = webview.clone();
            let spinner_box = spinner_box.clone();
            let active_state = Rc::clone(&active_state);
            let content_manager = content_manager.clone();
            let wam_mask_script = wam_mask_script.clone();
            let _win11_mask_script = win11_mask_script.clone();
            let _win11_chrome_mask_script = win11_chrome_mask_script.clone();

            move |profile_index: u32| {
                spinner_box.set_visible(false);
                webview.set_visible(true);

                let custom_tenant: Option<String> = None;

                // Clear any prior scripts to prevent cross-profile leak
                content_manager.remove_all_scripts();

                let (
                    authority,
                    client_id,
                    redirect_uri,
                    os_param,
                    user_agent,
                    domain_hint,
                    script_to_inject,
                ) = match profile_index {
                    1 => (
                        DEFAULT_AUTHORITY.to_string(),
                        MAC_CLIENT_ID,
                        MAC_REDIRECT_URI,
                        Some("macOS"),
                        UA_MAC_SAFARI,
                        None,
                        None,
                    ),
                    2 => (
                        DEFAULT_AUTHORITY.to_string(),
                        AVD_CLIENT_ID,
                        WVD_REDIRECT_URI,
                        Some("Linux"),
                        UA_LINUX_CHROME,
                        None,
                        None,
                    ),
                    3 => (
                        COMMON_AUTHORITY.to_string(),
                        WINDOWS_APP_CLIENT_ID,
                        WINDOWS_APP_REDIRECT_URI,
                        Some("Linux"),
                        UA_LINUX_CHROME,
                        None,
                        None,
                    ),
                    _ => {
                        // Profile 0: Windows 11 Windows App (WAM Broker) (DEFAULT)
                        // Uses official Windows App MSIX client_id (4fb5cc57) with Microsoft.AAD.BrokerPlugin EdgeHTML WebView identity.
                        // Entra ID classifies this as "Mobile Apps and Desktop Clients" (NOT a Browser!).
                        // Completely eliminates the Edge MAM "Sign in to Microsoft Edge browser profile" rule.
                        // Authority is https://login.microsoftonline.com/common (clean email prompt).
                        // Domain hint is None so Entra ID federates to HPE SSO upon typing email.
                        let auth = if let Some(ref t) = custom_tenant {
                            format!("https://login.microsoftonline.com/{t}")
                        } else {
                            COMMON_AUTHORITY.to_string()
                        };
                        let dh = custom_tenant.as_deref();
                        (
                            auth,
                            WINDOWS_APP_CLIENT_ID,
                            WINDOWS_APP_REDIRECT_URI,
                            Some("Microsoft Windows NT 10.0.22631.0"),
                            UA_WINDOWS_11_WAM,
                            dh,
                            Some(&wam_mask_script),
                        )
                    }
                };

                if let Some(script) = script_to_inject {
                    content_manager.add_script(script);
                }

                let active_auth = Arc::new(AuthManager::new(&authority, client_id));
                let pkce = Pkce::new();
                let auth_url = active_auth.build_authorization_url(
                    &pkce,
                    redirect_uri,
                    os_param,
                    domain_hint,
                    None,
                );

                                debug!(
                    "Loading auth profile [{profile_index}] with client ID [{client_id}]"
                );

                if let Some(s) = webkit6::prelude::WebViewExt::settings(&webview) {
                    s.set_user_agent(Some(user_agent));
                }

                // Update session state
                {
                    let mut state = active_state.borrow_mut();
                    state.auth_manager = Arc::clone(&active_auth);
                    state.verifier = pkce.verifier.clone();
                    state.expected_redirect = redirect_uri.to_string();
                    state.os_param = os_param.map(ToString::to_string);
                    state.user_agent = user_agent.to_string();
                    state.completed = false;
                }

                webview.load_uri(&auth_url);
            }
        };

        let active_profile = AppConfig::load().auth_profile_index;

        // Set initial view stack
        let view_to_show = initial_view.unwrap_or("browser");
        view_stack.set_visible_child_name(view_to_show);
        match view_to_show {
            "device" => {
                window.set_title(Some("Sign in with Edge / Browser Code - Windows App"));
                initiate_device_flow();
            }
            "import" => {
                window.set_title(Some("Paste Token - Windows App"));
            }
            _ => {
                window.set_title(Some("Sign in - Windows App"));
            }
        }

        // Initial load with configured profile (defaults to Windows 11 Windows App WAM Broker)
        load_auth(active_profile);

        window.present();
    }
}
