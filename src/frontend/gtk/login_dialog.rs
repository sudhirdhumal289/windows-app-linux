//! Microsoft Entra ID sign-in window.
//!
//! Three methods are offered, switchable from the header:
//! * **Browser** – an embedded web view running the authorization-code + PKCE flow with the
//!   identity of the configured [`AuthProfile`].
//! * **Device code** – for tenants whose Conditional Access requires a managed browser
//!   (e.g. an Intune Edge work profile); the code is approved in that browser.
//! * **Token** – pasting a token obtained elsewhere, as a last resort.

use super::webkit;
use crate::auth::pkce::Pkce;
use crate::auth::pkeyauth::Challenge;
use crate::auth::redirect::{self, Redirect};
use crate::auth::{pkap::DeviceIdentity, AuthManager, MSAL_SKU, MSAL_VERSION};
use crate::config::{AppConfig, AuthProfile, DEVICE_LOGIN_URL};
use adw::prelude::*;
use gtk4::{
    glib, Align, Box, Button, Label, Orientation, ScrolledWindow, Spinner, Stack, TextView,
};
use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::sync::Arc;
use webkit6::prelude::*;
use webkit6::{
    LoadEvent, NavigationPolicyDecision, PolicyDecisionType, Settings, URIRequest,
    UserContentInjectedFrames, UserContentManager, UserScript, UserScriptInjectionTime, WebView,
};

/// Makes the page's JavaScript view of the browser match the Windows WAM broker web view,
/// so Entra ID classifies the sign-in as a desktop client rather than a browser.
const WAM_MASK_SCRIPT: &str = r"
(function() {
    try {
        const ua = 'Mozilla/5.0 (Windows NT 10.0; Win64; x64; WebView/3.0) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/70.0.3538.102 Safari/537.36 Edge/18.22631';
        const define = (name, value) =>
            Object.defineProperty(navigator, name, { get: () => value, configurable: true });
        define('platform', 'Win32');
        define('oscpu', 'Windows NT 10.0; Win64; x64');
        define('vendor', '');
        define('webdriver', false);
        define('appVersion', ua.substring('Mozilla/'.length));
        define('userAgent', ua);
        try { define('userAgentData', undefined); } catch (e) {}
        try { delete window.chrome; } catch (e) { window.chrome = undefined; }
        window.external = window.external || {};
    } catch (e) {}
})();
";

type Completion = std::boxed::Box<dyn FnOnce(bool)>;

/// Authorization-code flow currently loaded in the web view.
struct BrowserFlow {
    profile: AuthProfile,
    auth: Arc<AuthManager>,
    verifier: String,
    exchanging: bool,
}

pub struct LoginDialog {
    window: adw::Window,
    pages: adw::ViewStack,
    // Browser page.
    browser_stack: Stack,
    webview: WebView,
    content_manager: UserContentManager,
    busy_spinner: Spinner,
    busy_label: Label,
    retry_button: Button,
    flow: RefCell<Option<BrowserFlow>>,
    device: Option<DeviceIdentity>,
    // Device-code page.
    code_label: Label,
    device_spinner: Spinner,
    device_status: Label,
    user_code: RefCell<String>,
    /// Incremented for every new device code so stale polls stop.
    device_attempt: Cell<u32>,
    // Token page.
    token_view: TextView,
    import_error: Label,
    /// Token store used for pasted tokens.
    auth: Arc<AuthManager>,
    open: Cell<bool>,
    on_finished: RefCell<Option<Completion>>,
}

impl LoginDialog {
    /// Opens the sign-in window on the in-app browser page; the header switcher offers the
    /// device-code and token methods. `on_finished` runs once: `true` after tokens were stored,
    /// `false` if the window was closed without signing in.
    pub fn present(
        parent: &impl IsA<gtk4::Window>,
        auth: Arc<AuthManager>,
        on_finished: impl FnOnce(bool) + 'static,
    ) {
        let window = adw::Window::builder()
            .transient_for(parent)
            .modal(true)
            .title("Sign in - Windows App")
            .default_width(620)
            .default_height(800)
            .build();

        let pages = adw::ViewStack::new();

        // Browser page.
        let content_manager = UserContentManager::new();
        let settings = Settings::new();
        settings.set_enable_javascript(true);
        settings.set_javascript_can_open_windows_automatically(true);
        settings.set_enable_developer_extras(cfg!(debug_assertions));
        let webview = WebView::builder()
            .network_session(&webkit::shared_network_session())
            .user_content_manager(&content_manager)
            .settings(&settings)
            .vexpand(true)
            .hexpand(true)
            .build();

        let busy_spinner = Spinner::builder()
            .width_request(40)
            .height_request(40)
            .build();
        let busy_label = Label::builder()
            .wrap(true)
            .justify(gtk4::Justification::Center)
            .build();
        busy_label.add_css_class("dim-label");
        let retry_button = Button::builder()
            .label("Try Again")
            .halign(Align::Center)
            .build();
        retry_button.add_css_class("pill");
        let busy_box = centered_box(12);
        busy_box.append(&busy_spinner);
        busy_box.append(&busy_label);
        busy_box.append(&retry_button);

        let browser_stack = Stack::new();
        browser_stack.add_named(&webview, Some("web"));
        browser_stack.add_named(&busy_box, Some("busy"));
        pages.add_titled_with_icon(
            &browser_stack,
            Some("browser"),
            "Browser",
            "web-browser-symbolic",
        );

        // Device-code page.
        let device_page = centered_box(16);
        for set_margin in [Box::set_margin_start, Box::set_margin_end] {
            set_margin(&device_page, 32);
        }
        let title = Label::new(Some("Sign in from Another Browser"));
        title.add_css_class("title-2");
        let description = Label::new(Some(
            "If your organization requires a managed browser (for example a Microsoft Edge \
             work profile), open the sign-in page there and enter this code:",
        ));
        description.set_wrap(true);
        description.set_max_width_chars(52);
        description.set_justify(gtk4::Justification::Center);
        description.add_css_class("dim-label");
        let code_label = Label::new(Some("…"));
        code_label.add_css_class("title-1");
        code_label.add_css_class("monospace");
        code_label.set_selectable(true);
        let copy_button = Button::with_label("Copy Code");
        let code_row = Box::new(Orientation::Horizontal, 12);
        code_row.set_halign(Align::Center);
        code_row.append(&code_label);
        code_row.append(&copy_button);
        let open_button = Button::with_label("Open Sign-in Page");
        open_button.add_css_class("suggested-action");
        let new_code_button = Button::with_label("Get New Code");
        let button_row = Box::new(Orientation::Horizontal, 12);
        button_row.set_halign(Align::Center);
        button_row.append(&open_button);
        button_row.append(&new_code_button);
        let device_spinner = Spinner::builder()
            .width_request(24)
            .height_request(24)
            .build();
        let device_status = Label::new(None);
        device_status.add_css_class("dim-label");
        device_status.set_wrap(true);
        let status_row = Box::new(Orientation::Horizontal, 8);
        status_row.set_halign(Align::Center);
        status_row.append(&device_spinner);
        status_row.append(&device_status);
        for widget in [
            title.upcast_ref::<gtk4::Widget>(),
            description.upcast_ref(),
            code_row.upcast_ref(),
            button_row.upcast_ref(),
            status_row.upcast_ref(),
        ] {
            device_page.append(widget);
        }
        pages.add_titled_with_icon(
            &device_page,
            Some("device"),
            "Device Code",
            "phone-symbolic",
        );

        // Token page.
        let import_page = Box::new(Orientation::Vertical, 16);
        for set_margin in [
            Box::set_margin_start,
            Box::set_margin_end,
            Box::set_margin_top,
            Box::set_margin_bottom,
        ] {
            set_margin(&import_page, 24);
        }
        let import_title = Label::new(Some("Paste an Access Token"));
        import_title.add_css_class("title-2");
        import_title.set_halign(Align::Start);
        let import_description = Label::new(Some(
            "If Conditional Access blocks the other methods, paste an access token or a \
             token JSON response from an authenticated session (e.g. windows365.microsoft.com).",
        ));
        import_description.set_wrap(true);
        import_description.set_xalign(0.0);
        import_description.add_css_class("dim-label");
        let token_view = TextView::builder()
            .wrap_mode(gtk4::WrapMode::Char)
            .monospace(true)
            .build();
        let token_scroll = ScrolledWindow::builder()
            .hscrollbar_policy(gtk4::PolicyType::Never)
            .vexpand(true)
            .min_content_height(200)
            .child(&token_view)
            .build();
        token_scroll.add_css_class("card");
        let import_error = Label::new(None);
        import_error.add_css_class("error");
        import_error.set_wrap(true);
        let import_button = Button::with_label("Sign In with Token");
        import_button.add_css_class("suggested-action");
        import_button.add_css_class("pill");
        import_button.set_halign(Align::Center);
        for widget in [
            import_title.upcast_ref::<gtk4::Widget>(),
            import_description.upcast_ref(),
            token_scroll.upcast_ref(),
            import_error.upcast_ref(),
            import_button.upcast_ref(),
        ] {
            import_page.append(widget);
        }
        pages.add_titled_with_icon(
            &import_page,
            Some("import"),
            "Token",
            "dialog-password-symbolic",
        );

        let header = adw::HeaderBar::new();
        let switcher = adw::ViewSwitcher::builder()
            .stack(&pages)
            .policy(adw::ViewSwitcherPolicy::Wide)
            .build();
        header.set_title_widget(Some(&switcher));
        let content = adw::ToolbarView::new();
        content.add_top_bar(&header);
        content.set_content(Some(&pages));
        window.set_content(Some(&content));

        let this = Rc::new(Self {
            window: window.clone(),
            pages: pages.clone(),
            browser_stack,
            webview,
            content_manager,
            busy_spinner,
            busy_label,
            retry_button: retry_button.clone(),
            flow: RefCell::new(None),
            device: DeviceIdentity::try_load_default(),
            code_label,
            device_spinner,
            device_status,
            user_code: RefCell::new(String::new()),
            device_attempt: Cell::new(0),
            token_view,
            import_error,
            auth,
            open: Cell::new(true),
            on_finished: RefCell::new(Some(std::boxed::Box::new(on_finished))),
        });

        this.install_browser_handlers();

        let weak = Rc::downgrade(&this);
        retry_button.connect_clicked(move |_| with(&weak, |this| this.start_browser_flow()));
        let weak = Rc::downgrade(&this);
        copy_button.connect_clicked(move |_| with(&weak, |this| this.copy_user_code()));
        let weak = Rc::downgrade(&this);
        open_button.connect_clicked(move |_| {
            with(&weak, |this| {
                this.copy_user_code();
                super::open_uri(DEVICE_LOGIN_URL, Some(&this.window));
            });
        });
        let weak = Rc::downgrade(&this);
        new_code_button.connect_clicked(move |_| with(&weak, Self::start_device_flow));
        let weak = Rc::downgrade(&this);
        import_button.connect_clicked(move |_| with(&weak, |this| this.import_token()));

        // The device code is requested only when that page is first shown.
        let weak = Rc::downgrade(&this);
        pages.connect_visible_child_name_notify(move |pages| {
            if pages.visible_child_name().as_deref() == Some("device") {
                with(&weak, |this| {
                    if this.device_attempt.get() == 0 {
                        this.start_device_flow();
                    }
                });
            }
        });

        // All close paths (header button, Escape, `finish`) emit close-request; the dialog
        // state is released there.
        let keep_alive = RefCell::new(Some(Rc::clone(&this)));
        window.connect_close_request(move |_| {
            if let Some(this) = keep_alive.take() {
                this.open.set(false);
                this.webview.stop_loading();
                if let Some(on_finished) = this.on_finished.take() {
                    on_finished(false);
                }
            }
            glib::Propagation::Proceed
        });

        this.start_browser_flow();
        this.pages.set_visible_child_name("browser");
        window.present();
    }

    /// Reports success, then closes the window.
    fn finish(&self) {
        let on_finished = self.on_finished.take();
        self.window.close();
        if let Some(on_finished) = on_finished {
            on_finished(true);
        }
    }

    // ----- Browser -----

    fn start_browser_flow(&self) {
        let profile = AppConfig::load().auth_profile();
        let auth = Arc::new(AuthManager::for_profile(profile));
        let pkce = Pkce::new();
        let url = auth.build_authorization_url(
            &pkce,
            profile.redirect_uri(),
            profile.client_os(),
            None,
            None,
        );

        self.content_manager.remove_all_scripts();
        if profile.masks_as_wam() {
            self.content_manager.add_script(&UserScript::new(
                WAM_MASK_SCRIPT,
                UserContentInjectedFrames::AllFrames,
                UserScriptInjectionTime::Start,
                &[],
                &[],
            ));
        }
        if let Some(settings) = WebViewExt::settings(&self.webview) {
            settings.set_user_agent(Some(profile.user_agent()));
        }

        log::debug!("Starting browser sign-in with profile {profile:?}");
        *self.flow.borrow_mut() = Some(BrowserFlow {
            profile,
            auth,
            verifier: pkce.verifier.clone(),
            exchanging: false,
        });
        self.browser_stack.set_visible_child_name("web");
        self.webview.load_uri(&url);
    }

    fn install_browser_handlers(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.webview
            .connect_decide_policy(move |view, decision, decision_type| {
                let Some(this) = weak.upgrade() else {
                    return false;
                };
                if !matches!(
                    decision_type,
                    PolicyDecisionType::NavigationAction | PolicyDecisionType::NewWindowAction
                ) {
                    return false;
                }
                let Some(uri) = decision
                    .downcast_ref::<NavigationPolicyDecision>()
                    .and_then(NavigationPolicyDecision::navigation_action)
                    .and_then(|action| action.request())
                    .and_then(|request| request.uri())
                else {
                    return false;
                };
                if this.handle_uri(&uri) {
                    decision.ignore();
                    return true;
                }
                // Keep pop-ups (e.g. federated IdP windows) in this view.
                if decision_type == PolicyDecisionType::NewWindowAction {
                    view.load_uri(&uri);
                    decision.ignore();
                    return true;
                }
                false
            });

        // Some redirects bypass the navigation policy; catch them as the load starts.
        let weak = Rc::downgrade(self);
        self.webview.connect_load_changed(move |view, event| {
            if matches!(
                event,
                LoadEvent::Started | LoadEvent::Redirected | LoadEvent::Committed
            ) {
                if let (Some(this), Some(uri)) = (weak.upgrade(), view.uri()) {
                    this.handle_uri(&uri);
                }
            }
        });
    }

    /// Handles a URI the web view is about to load. Returns `true` if it was consumed.
    fn handle_uri(self: &Rc<Self>, uri: &str) -> bool {
        let Some(profile) = self.flow.borrow().as_ref().map(|f| f.profile) else {
            return false;
        };
        if let Some(challenge) = Challenge::parse(uri) {
            log::debug!("Answering PKeyAuth device challenge");
            let request = URIRequest::new(&challenge.submit_url);
            if let Some(headers) = request.http_headers() {
                headers.append(
                    "Authorization",
                    &challenge.authorization(self.device.as_ref()),
                );
                headers.append("User-Agent", profile.user_agent());
                headers.append("x-client-SKU", MSAL_SKU);
                headers.append("x-client-Ver", MSAL_VERSION);
                if let Some(os) = profile.client_os() {
                    headers.append("x-client-OS", os);
                }
            }
            self.webview.load_request(&request);
            return true;
        }
        match redirect::parse(uri, &[profile.redirect_uri()]) {
            Some(Redirect::Code(code)) => {
                self.exchange_code(code);
                true
            }
            Some(Redirect::Error(message)) => {
                log::warn!("Sign-in failed: {message}");
                self.show_browser_error(&format!("Sign-in failed: {message}"));
                true
            }
            None => false,
        }
    }

    fn exchange_code(self: &Rc<Self>, code: String) {
        let (auth, verifier, profile) = {
            let mut flow = self.flow.borrow_mut();
            let Some(flow) = flow.as_mut().filter(|f| !f.exchanging) else {
                return;
            };
            flow.exchanging = true;
            (Arc::clone(&flow.auth), flow.verifier.clone(), flow.profile)
        };
        self.webview.stop_loading();
        self.busy_label.set_text("Completing sign-in…");
        self.busy_spinner.set_visible(true);
        self.busy_spinner.start();
        self.retry_button.set_visible(false);
        self.browser_stack.set_visible_child_name("busy");

        let weak = Rc::downgrade(self);
        glib::MainContext::default().spawn_local(async move {
            let result = auth
                .exchange_authorization_code(
                    &code,
                    &verifier,
                    profile.redirect_uri(),
                    profile.client_os(),
                )
                .await;
            let Some(this) = weak.upgrade() else { return };
            match result {
                Ok(_) => {
                    log::info!("Signed in");
                    webkit::persist_session_cookies();
                    this.finish();
                }
                Err(e) => {
                    log::error!("Token exchange failed: {e}");
                    this.show_browser_error(&format!("Sign-in could not be completed: {e}"));
                }
            }
        });
    }

    fn show_browser_error(&self, message: &str) {
        self.busy_spinner.stop();
        self.busy_spinner.set_visible(false);
        self.busy_label.set_text(message);
        self.retry_button.set_visible(true);
        self.browser_stack.set_visible_child_name("busy");
    }

    // ----- Device code -----

    fn start_device_flow(self: &Rc<Self>) {
        let attempt = self.device_attempt.get() + 1;
        self.device_attempt.set(attempt);
        self.user_code.borrow_mut().clear();
        self.code_label.set_text("…");
        self.device_spinner.set_visible(true);
        self.device_spinner.start();
        self.device_status
            .set_text("Requesting a code from Microsoft Entra ID…");

        // The device flow always uses the Windows App identity.
        let auth = AuthManager::for_profile(AuthProfile::WindowsApp);
        let weak = Rc::downgrade(self);
        glib::MainContext::default().spawn_local(async move {
            let is_current = |weak: &Weak<Self>| {
                weak.upgrade()
                    .is_some_and(|this| this.open.get() && this.device_attempt.get() == attempt)
            };
            let response = auth.initiate_device_code().await;
            if !is_current(&weak) {
                return;
            }
            let response = match response {
                Ok(response) => response,
                Err(e) => {
                    log::error!("Device code request failed: {e}");
                    with(&weak, |this| {
                        this.device_failed(&format!("Could not get a code: {e}"))
                    });
                    return;
                }
            };
            with(&weak, |this| {
                this.code_label.set_text(&response.user_code);
                *this.user_code.borrow_mut() = response.user_code.clone();
                this.copy_user_code();
                this.device_status
                    .set_text("Code copied. Waiting for approval in your browser…");
            });

            let result = auth
                .poll_device_token(&response.device_code, response.interval, || {
                    is_current(&weak)
                })
                .await;
            if !is_current(&weak) {
                return;
            }
            match result {
                Ok(_) => {
                    log::info!("Signed in with device code");
                    with(&weak, |this| this.finish());
                }
                Err(e) => {
                    log::warn!("Device sign-in stopped: {e}");
                    with(&weak, |this| this.device_failed(&e.to_string()));
                }
            }
        });
    }

    fn device_failed(&self, message: &str) {
        self.device_spinner.stop();
        self.device_spinner.set_visible(false);
        self.device_status.set_text(message);
    }

    fn copy_user_code(&self) {
        let code = self.user_code.borrow();
        if !code.is_empty() {
            self.window.clipboard().set_text(&code);
        }
    }

    // ----- Token import -----

    fn import_token(&self) {
        let buffer = self.token_view.buffer();
        let raw = buffer.text(&buffer.start_iter(), &buffer.end_iter(), false);
        match self.auth.import_token(&raw) {
            Ok(_) => {
                log::info!("Signed in with an imported token");
                self.finish();
            }
            Err(e) => self
                .import_error
                .set_text(&format!("Could not use this token: {e}")),
        }
    }
}

/// Runs `f` if the dialog still exists.
fn with<T>(weak: &Weak<T>, f: impl FnOnce(&Rc<T>)) {
    if let Some(this) = weak.upgrade() {
        f(&this);
    }
}

fn centered_box(spacing: i32) -> Box {
    let b = Box::new(Orientation::Vertical, spacing);
    b.set_valign(Align::Center);
    b.set_halign(Align::Center);
    b.set_vexpand(true);
    b
}
