//! Microsoft sign-in: a sheet with an embedded browser (authorization code + PKCE), a
//! device-code fallback and token import; plus the session-host sign-in requested by
//! the RDP engine, which first runs hidden so existing SSO cookies can complete it.

use super::util::{self, ns, rect};
use crate::frontend::common::signin::{
    BrowserSignIn, DeviceCode, DeviceCodeSignIn, Navigation, SessionHostSignIn,
};
use block2::DynBlock;
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{define_class, msg_send, AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSAutoresizingMaskOptions, NSBackingStoreType, NSButton, NSFont, NSProgressIndicator,
    NSProgressIndicatorStyle, NSScrollView, NSStackView, NSTabView, NSTabViewItem, NSTextAlignment,
    NSTextField, NSTextView, NSUserInterfaceLayoutOrientation, NSView, NSWindow, NSWindowDelegate,
    NSWindowStyleMask,
};
use objc2_foundation::{
    NSMutableURLRequest, NSNotification, NSObject, NSObjectProtocol, NSSize, NSURL,
};
use objc2_web_kit::{
    WKNavigation, WKNavigationAction, WKNavigationActionPolicy, WKNavigationDelegate, WKUIDelegate,
    WKUserScript, WKUserScriptInjectionTime, WKWebView, WKWebViewConfiguration, WKWebsiteDataStore,
    WKWindowFeatures,
};
use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

/// Where Microsoft's device-code page lives.
const DEVICE_LOGIN_URL: &str = "https://microsoft.com/devicelogin";

// ----- Web view plumbing -----

type UriHandler = Box<dyn Fn(&WKWebView, &str) -> bool>;

pub(super) struct NavIvars {
    /// Returns `true` when the URL was consumed (the navigation is cancelled).
    handle: UriHandler,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements and the class has no Drop impl.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "WindowsAppSignInNavigation"]
    #[ivars = NavIvars]
    struct NavDelegate;

    // SAFETY: NSObjectProtocol has no safety requirements.
    unsafe impl NSObjectProtocol for NavDelegate {}

    // SAFETY: The method signatures match the protocol.
    unsafe impl WKNavigationDelegate for NavDelegate {
        #[unsafe(method(webView:decidePolicyForNavigationAction:decisionHandler:))]
        fn decide_policy(
            &self,
            view: &WKWebView,
            action: &WKNavigationAction,
            handler: &DynBlock<dyn Fn(WKNavigationActionPolicy)>,
        ) {
            // SAFETY: The action's request is valid during this callback.
            let request = unsafe { action.request() };
            let url = request.URL().and_then(|url| url.absoluteString());
            let consumed = url.is_some_and(|url| (self.ivars().handle)(view, &url.to_string()));
            let policy = if consumed {
                WKNavigationActionPolicy::Cancel
            } else {
                WKNavigationActionPolicy::Allow
            };
            handler.call((policy,));
        }

        /// Some redirects bypass the navigation policy; catch them as they happen.
        #[unsafe(method(webView:didReceiveServerRedirectForProvisionalNavigation:))]
        fn did_redirect(&self, view: &WKWebView, _navigation: Option<&WKNavigation>) {
            // SAFETY: Reading the current URL of a live web view.
            if let Some(url) = unsafe { view.URL() }.and_then(|url| url.absoluteString()) {
                if (self.ivars().handle)(view, &url.to_string()) {
                    // SAFETY: Stopping a live web view on the main thread.
                    unsafe { view.stopLoading() };
                }
            }
        }
    }

    // SAFETY: The method signatures match the protocol.
    unsafe impl WKUIDelegate for NavDelegate {
        /// Keeps pop-ups (e.g. federated identity providers) in the same view.
        #[unsafe(method_id(webView:createWebViewWithConfiguration:forNavigationAction:windowFeatures:))]
        fn create_web_view(
            &self,
            view: &WKWebView,
            _configuration: &WKWebViewConfiguration,
            action: &WKNavigationAction,
            _features: &WKWindowFeatures,
        ) -> Option<Retained<WKWebView>> {
            // SAFETY: The action's request is valid during this callback, and loading it
            // in a live web view is allowed.
            unsafe {
                let request = action.request();
                let consumed = request
                    .URL()
                    .and_then(|url| url.absoluteString())
                    .is_some_and(|url| (self.ivars().handle)(view, &url.to_string()));
                if !consumed {
                    view.loadRequest(&request);
                }
            }
            None
        }
    }
);

impl NavDelegate {
    fn new(
        mtm: MainThreadMarker,
        handle: impl Fn(&WKWebView, &str) -> bool + 'static,
    ) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(NavIvars {
            handle: Box::new(handle),
        });
        // SAFETY: NSObject's `init` signature is correct.
        unsafe { msg_send![super(this), init] }
    }
}

/// A web view using the persistent website data store (shared cookies) and `user_agent`.
fn web_view(
    mtm: MainThreadMarker,
    user_agent: &str,
    script: Option<&str>,
    handle: impl Fn(&WKWebView, &str) -> bool + 'static,
) -> Retained<WKWebView> {
    // SAFETY: Standard WebKit configuration on the main thread; the delegate is kept
    // alive by the web view through an associated object.
    unsafe {
        let configuration = WKWebViewConfiguration::new(mtm);
        configuration.setWebsiteDataStore(&WKWebsiteDataStore::defaultDataStore(mtm));
        if let Some(source) = script {
            let script = WKUserScript::initWithSource_injectionTime_forMainFrameOnly(
                WKUserScript::alloc(mtm),
                &ns(source),
                WKUserScriptInjectionTime::AtDocumentStart,
                false,
            );
            configuration.userContentController().addUserScript(&script);
        }
        let view = WKWebView::initWithFrame_configuration(
            WKWebView::alloc(mtm),
            rect(0.0, 0.0, 600.0, 700.0),
            &configuration,
        );
        view.setCustomUserAgent(Some(&ns(user_agent)));
        let delegate = NavDelegate::new(mtm, handle);
        view.setNavigationDelegate(Some(ProtocolObject::from_ref(&*delegate)));
        view.setUIDelegate(Some(ProtocolObject::from_ref(&*delegate)));
        util::retain_with(&view, &delegate);
        view
    }
}

fn load(view: &WKWebView, url: &str, headers: &[(&str, String)]) {
    let Some(url) = NSURL::URLWithString(&ns(url)) else {
        log::warn!("Invalid sign-in URL {url}");
        return;
    };
    let request = NSMutableURLRequest::requestWithURL(&url);
    for (name, value) in headers {
        request.setValue_forHTTPHeaderField(Some(&ns(value)), &ns(name));
    }
    // SAFETY: Loading a request in a live web view on the main thread.
    unsafe { view.loadRequest(&request) };
}

// ----- Window delegate (close button) -----

pub(super) struct CloseIvars {
    closed: Box<dyn Fn()>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements and the class has no Drop impl.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "WindowsAppSignInWindowDelegate"]
    #[ivars = CloseIvars]
    struct CloseDelegate;

    // SAFETY: NSObjectProtocol has no safety requirements.
    unsafe impl NSObjectProtocol for CloseDelegate {}

    // SAFETY: The method signatures match the protocol.
    unsafe impl NSWindowDelegate for CloseDelegate {
        #[unsafe(method(windowWillClose:))]
        fn window_will_close(&self, _notification: &NSNotification) {
            (self.ivars().closed)();
        }
    }
);

fn on_close(window: &NSWindow, closed: impl Fn() + 'static) {
    let mtm = window.mtm();
    let delegate = CloseDelegate::alloc(mtm).set_ivars(CloseIvars {
        closed: Box::new(closed),
    });
    // SAFETY: NSObject's `init` signature is correct.
    let delegate: Retained<CloseDelegate> = unsafe { msg_send![super(delegate), init] };
    window.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
    util::retain_with(window, &delegate);
}

fn new_window(mtm: MainThreadMarker, title: &str, width: f64, height: f64) -> Retained<NSWindow> {
    // SAFETY: Plain window creation; the window is owned by `Retained`.
    let window = unsafe {
        let window = NSWindow::initWithContentRect_styleMask_backing_defer(
            NSWindow::alloc(mtm),
            rect(0.0, 0.0, width, height),
            NSWindowStyleMask::Titled | NSWindowStyleMask::Closable | NSWindowStyleMask::Resizable,
            NSBackingStoreType::Buffered,
            false,
        );
        window.setReleasedWhenClosed(false);
        window
    };
    window.setTitle(&ns(title));
    window.setMinSize(NSSize::new(420.0, 480.0));
    window
}

/// Ends a sheet or closes a standalone window.
fn dismiss(window: &NSWindow) {
    match window.sheetParent() {
        Some(parent) => parent.endSheet(window),
        None => window.close(),
    }
}

fn fill(view: &NSView, parent: &NSView, inset_bottom: f64) {
    let bounds = parent.bounds();
    view.setFrame(rect(
        0.0,
        inset_bottom,
        bounds.size.width,
        bounds.size.height - inset_bottom,
    ));
    view.setAutoresizingMask(
        NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable,
    );
    parent.addSubview(view);
}

fn stack(mtm: MainThreadMarker, spacing: f64) -> Retained<NSStackView> {
    let stack = NSStackView::new(mtm);
    stack.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
    stack.setAlignment(objc2_app_kit::NSLayoutAttribute::CenterX);
    stack.setSpacing(spacing);
    stack
}

fn spinner(mtm: MainThreadMarker) -> Retained<NSProgressIndicator> {
    let spinner = NSProgressIndicator::new(mtm);
    spinner.setStyle(NSProgressIndicatorStyle::Spinning);
    spinner.setDisplayedWhenStopped(false);
    spinner
}

fn animate(spinner: &NSProgressIndicator, on: bool) {
    // SAFETY: Animating a spinner on the main thread.
    unsafe {
        if on {
            spinner.startAnimation(None);
        } else {
            spinner.stopAnimation(None);
        }
    }
}

fn tab(id: &str, label: &str, view: &NSView) -> Retained<NSTabViewItem> {
    // SAFETY: The identifier is a string object.
    let item = unsafe { NSTabViewItem::initWithIdentifier(NSTabViewItem::alloc(), Some(&ns(id))) };
    item.setLabel(&ns(label));
    item.setView(Some(view));
    item
}

// ----- Main sign-in -----

/// Called with whether the user signed in.
type OnFinished = Box<dyn FnOnce(bool)>;

struct SignIn {
    window: Retained<NSWindow>,
    web: Retained<WKWebView>,
    busy: Retained<NSStackView>,
    busy_spinner: Retained<NSProgressIndicator>,
    busy_label: Retained<NSTextField>,
    retry: Retained<NSButton>,
    flow: RefCell<Option<BrowserSignIn>>,
    exchanging: Cell<bool>,
    code_label: Retained<NSTextField>,
    device_spinner: Retained<NSProgressIndicator>,
    device_status: Retained<NSTextField>,
    user_code: RefCell<String>,
    device_attempt: Cell<u32>,
    token: Retained<NSTextView>,
    token_error: Retained<NSTextField>,
    open: Cell<bool>,
    on_finished: RefCell<Option<OnFinished>>,
}

thread_local! {
    /// The sign-in in progress (kept alive while its window is open).
    static ACTIVE: RefCell<Option<Rc<SignIn>>> = const { RefCell::new(None) };
}

/// Shows the sign-in sheet on `parent`; `on_finished` receives whether the user signed
/// in.
pub(super) fn sign_in(parent: &NSWindow, on_finished: impl FnOnce(bool) + 'static) {
    let mtm = parent.mtm();
    if let Some(active) = ACTIVE.with(|a| a.borrow().clone()) {
        active.window.makeKeyAndOrderFront(None);
        return;
    }
    let state = Rc::new_cyclic(|weak: &Weak<SignIn>| build(mtm, weak.clone(), on_finished));
    ACTIVE.with(|a| *a.borrow_mut() = Some(Rc::clone(&state)));
    let weak = Rc::downgrade(&state);
    on_close(&state.window, move || {
        if let Some(this) = weak.upgrade() {
            this.closed();
        }
    });
    state.start_browser();
    if parent.isVisible() {
        parent.beginSheet_completionHandler(&state.window, None);
    } else {
        state.window.center();
        state.window.makeKeyAndOrderFront(None);
    }
}

fn build(
    mtm: MainThreadMarker,
    weak: Weak<SignIn>,
    on_finished: impl FnOnce(bool) + 'static,
) -> SignIn {
    let window = new_window(mtm, "Sign In – Windows App", 620.0, 760.0);
    let Some(root) = window.contentView() else {
        unreachable!("a new window always has a content view")
    };
    let with = |f: fn(&Rc<SignIn>)| {
        let weak = weak.clone();
        move || {
            if let Some(this) = weak.upgrade() {
                f(&this);
            }
        }
    };

    // Browser page: the web view, or a status page while busy or after a failure.
    let browser = NSView::new(mtm);
    let handle_weak = weak.clone();
    let web = web_view(mtm, "", None, move |view, url| {
        handle_weak
            .upgrade()
            .is_some_and(|this| this.handle_uri(view, url))
    });
    let busy_spinner = spinner(mtm);
    let busy_label = util::wrapping_label(mtm, "", 380.0);
    busy_label.setAlignment(NSTextAlignment::Center);
    util::secondary(&busy_label);
    let retry = util::button(mtm, "Try Again", with(SignIn::start_browser));
    let busy = stack(mtm, 12.0);
    busy.addArrangedSubview(&busy_spinner);
    busy.addArrangedSubview(&busy_label);
    busy.addArrangedSubview(&retry);
    browser.setFrame(rect(0.0, 0.0, 600.0, 680.0));
    fill(&web, &browser, 0.0);
    fill(&busy, &browser, 0.0);
    busy.setHidden(true);

    // Device-code page.
    let device = stack(mtm, 16.0);
    let title = super::dialogs::heading(mtm, "Sign in from Another Browser");
    let description = util::wrapping_label(
        mtm,
        "If your organization requires a managed browser (for example a Microsoft Edge \
         work profile), open the sign-in page there and enter this code:",
        420.0,
    );
    description.setAlignment(NSTextAlignment::Center);
    util::secondary(&description);
    let code_label = util::label(mtm, "…");
    code_label.setFont(Some(&NSFont::monospacedSystemFontOfSize_weight(
        28.0,
        // SAFETY: Reading an immutable extern constant.
        unsafe { objc2_app_kit::NSFontWeightSemibold },
    )));
    code_label.setSelectable(true);
    let open = util::button(
        mtm,
        "Copy Code & Open Browser",
        with(|this| {
            this.copy_user_code();
            util::open_url(DEVICE_LOGIN_URL);
        }),
    );
    open.setKeyEquivalent(&ns("\r"));
    let new_code = util::button(mtm, "Get New Code", with(SignIn::start_device_flow));
    let buttons = NSStackView::new(mtm);
    buttons.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
    buttons.setSpacing(12.0);
    buttons.addArrangedSubview(&open);
    buttons.addArrangedSubview(&new_code);
    let device_spinner = spinner(mtm);
    device_spinner.setControlSize(objc2_app_kit::NSControlSize::Small);
    let device_status = util::wrapping_label(mtm, "Click “Get New Code” to begin.", 380.0);
    util::secondary(&device_status);
    let status_row = NSStackView::new(mtm);
    status_row.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
    status_row.setSpacing(8.0);
    status_row.addArrangedSubview(&device_spinner);
    status_row.addArrangedSubview(&device_status);
    for view in [
        &*title as &NSView,
        &description,
        &code_label,
        &buttons,
        &status_row,
    ] {
        device.addArrangedSubview(view);
    }

    // Token page.
    let token_page = stack(mtm, 12.0);
    token_page.setAlignment(objc2_app_kit::NSLayoutAttribute::Leading);
    token_page.setEdgeInsets(objc2_foundation::NSEdgeInsets {
        top: 20.0,
        left: 20.0,
        bottom: 20.0,
        right: 20.0,
    });
    let token_title = super::dialogs::heading(mtm, "Paste an Access Token");
    let token_description = util::wrapping_label(
        mtm,
        "If Conditional Access blocks the other methods, paste an access token or a token \
         JSON response from an authenticated session (e.g. windows365.microsoft.com).",
        520.0,
    );
    util::secondary(&token_description);
    let token_scroll: Retained<NSScrollView> = NSTextView::scrollableTextView(mtm);
    let token = token_scroll
        .documentView()
        .and_then(|view| view.downcast::<NSTextView>().ok())
        .unwrap_or_else(|| NSTextView::new(mtm));
    token.setFont(Some(&NSFont::monospacedSystemFontOfSize_weight(
        11.0,
        // SAFETY: Reading an immutable extern constant.
        unsafe { objc2_app_kit::NSFontWeightRegular },
    )));
    token.setRichText(false);
    token_scroll.setBorderType(objc2_app_kit::NSBorderType::BezelBorder);
    let token_error = util::wrapping_label(mtm, "", 520.0);
    token_error.setTextColor(Some(&objc2_app_kit::NSColor::systemRedColor()));
    let import = util::button(mtm, "Sign In with Token", with(SignIn::import_token));
    for view in [
        &*token_title as &NSView,
        &token_description,
        &token_scroll,
        &token_error,
        &import,
    ] {
        token_page.addArrangedSubview(view);
    }
    token_scroll
        .widthAnchor()
        .constraintEqualToAnchor_constant(&token_page.widthAnchor(), -40.0)
        .setActive(true);
    token_scroll
        .heightAnchor()
        .constraintGreaterThanOrEqualToConstant(200.0)
        .setActive(true);

    let tabs = NSTabView::new(mtm);
    tabs.addTabViewItem(&tab("browser", "Browser", &browser));
    tabs.addTabViewItem(&tab("device", "Device Code", &device));
    tabs.addTabViewItem(&tab("token", "Token", &token_page));
    fill(&tabs, &root, 52.0);
    let cancel_weak = weak;
    let cancel = util::button(mtm, "Cancel", move || {
        if let Some(this) = cancel_weak.upgrade() {
            dismiss(&this.window);
            this.closed();
        }
    });
    cancel.setKeyEquivalent(&ns("\u{1b}"));
    let size = cancel.frame().size;
    cancel.setFrame(rect(
        root.bounds().size.width - 20.0 - size.width.max(90.0),
        12.0,
        size.width.max(90.0),
        size.height,
    ));
    cancel.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMinXMargin);
    root.addSubview(&cancel);

    SignIn {
        window,
        web,
        busy,
        busy_spinner,
        busy_label,
        retry,
        flow: RefCell::new(None),
        exchanging: Cell::new(false),
        code_label,
        device_spinner,
        device_status,
        user_code: RefCell::new(String::new()),
        device_attempt: Cell::new(0),
        token,
        token_error,
        open: Cell::new(true),
        on_finished: RefCell::new(Some(Box::new(on_finished))),
    }
}

impl SignIn {
    /// The window closed without success (or after `finish` already reported).
    fn closed(&self) {
        if !self.open.replace(false) {
            return;
        }
        // SAFETY: Stopping a live web view on the main thread.
        unsafe { self.web.stopLoading() };
        ACTIVE.with(|a| a.borrow_mut().take());
        if let Some(on_finished) = self.on_finished.take() {
            on_finished(false);
        }
    }

    /// Reports success, then closes the window.
    fn finish(&self) {
        let on_finished = self.on_finished.take();
        dismiss(&self.window);
        self.closed();
        if let Some(on_finished) = on_finished {
            on_finished(true);
        }
    }

    fn show_busy(&self, message: &str, failed: bool) {
        self.busy_label.setStringValue(&ns(message));
        animate(&self.busy_spinner, !failed);
        self.retry.setHidden(!failed);
        self.busy.setHidden(false);
        self.web.setHidden(true);
    }

    fn start_browser(self: &Rc<Self>) {
        let flow = BrowserSignIn::start();
        // SAFETY: Configuring a live web view on the main thread.
        unsafe {
            self.web.setCustomUserAgent(Some(&ns(flow.user_agent())));
            let controller = self.web.configuration().userContentController();
            controller.removeAllUserScripts();
            if let Some(source) = flow.injected_script() {
                let script = WKUserScript::initWithSource_injectionTime_forMainFrameOnly(
                    WKUserScript::alloc(self.window.mtm()),
                    &ns(source),
                    WKUserScriptInjectionTime::AtDocumentStart,
                    false,
                );
                controller.addUserScript(&script);
            }
        }
        let url = flow.url().to_string();
        *self.flow.borrow_mut() = Some(flow);
        self.exchanging.set(false);
        self.busy.setHidden(true);
        self.web.setHidden(false);
        load(&self.web, &url, &[]);
    }

    /// Handles a URL the web view is about to load; `true` when consumed.
    fn handle_uri(self: &Rc<Self>, view: &WKWebView, url: &str) -> bool {
        let navigation = match self.flow.borrow_mut().as_mut() {
            Some(flow) => flow.on_navigation(url),
            None => return false,
        };
        match navigation {
            Navigation::Continue => false,
            Navigation::Load { url, headers } => {
                load(view, &url, &headers);
                true
            }
            Navigation::Done(code) => {
                self.exchange(code);
                true
            }
            Navigation::Failed(message) => {
                self.show_busy(&message, true);
                true
            }
        }
    }

    fn exchange(self: &Rc<Self>, code: String) {
        if self.exchanging.replace(true) {
            return;
        }
        let Some(future) = self.flow.borrow().as_ref().map(|flow| flow.exchange(code)) else {
            return;
        };
        self.show_busy("Completing sign-in…", false);
        let weak = Rc::downgrade(self);
        util::spawn(future, move |result| {
            let Some(this) = weak.upgrade() else { return };
            match result {
                Ok(()) => this.finish(),
                Err(message) => {
                    this.exchanging.set(false);
                    this.show_busy(&message, true);
                }
            }
        });
    }

    fn start_device_flow(self: &Rc<Self>) {
        let attempt = self.device_attempt.get() + 1;
        self.device_attempt.set(attempt);
        self.user_code.borrow_mut().clear();
        self.code_label.setStringValue(&ns("…"));
        animate(&self.device_spinner, true);
        self.device_status
            .setStringValue(&ns("Requesting a code from Microsoft Entra ID…"));
        let flow = Rc::new(DeviceCodeSignIn::new());
        let weak = Rc::downgrade(self);
        let request = flow.request();
        util::spawn(request, move |result: Result<DeviceCode, String>| {
            let Some(this) = weak.upgrade().filter(|t| t.is_current(attempt)) else {
                return;
            };
            match result {
                Ok(code) => this.wait_for_approval(&flow, &code, attempt),
                Err(message) => {
                    log::error!("Device code request failed: {message}");
                    this.device_failed(&message);
                }
            }
        });
    }

    fn is_current(&self, attempt: u32) -> bool {
        self.open.get() && self.device_attempt.get() == attempt
    }

    fn wait_for_approval(
        self: &Rc<Self>,
        flow: &DeviceCodeSignIn,
        code: &DeviceCode,
        attempt: u32,
    ) {
        self.code_label.setStringValue(&ns(&code.user_code));
        code.user_code.clone_into(&mut self.user_code.borrow_mut());
        self.copy_user_code();
        self.device_status
            .setStringValue(&ns("Code copied. Waiting for approval in your browser…"));
        // Polling stops when the window closes or a new code is requested.
        let current = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
        let keep_polling = std::sync::Arc::clone(&current);
        let wait = flow.wait(code, move || {
            keep_polling.load(std::sync::atomic::Ordering::Relaxed)
        });
        let weak = Rc::downgrade(self);
        let watch = weak.clone();
        util::spawn(wait, move |result| {
            let Some(this) = weak.upgrade().filter(|t| t.is_current(attempt)) else {
                return;
            };
            match result {
                Ok(()) => {
                    log::info!("Signed in with device code");
                    this.finish();
                }
                Err(message) => {
                    log::warn!("Device sign-in stopped: {message}");
                    this.device_failed(&message);
                }
            }
        });
        poll_validity(watch, attempt, current);
    }

    fn device_failed(&self, message: &str) {
        animate(&self.device_spinner, false);
        self.device_status.setStringValue(&ns(message));
    }

    fn copy_user_code(&self) {
        let code = self.user_code.borrow();
        if !code.is_empty() {
            util::set_pasteboard_text(&code);
        }
    }

    fn import_token(self: &Rc<Self>) {
        let raw = self.token.string().to_string();
        match DeviceCodeSignIn::new().import_token(&raw) {
            Ok(()) => {
                log::info!("Signed in with an imported token");
                self.finish();
            }
            Err(message) => self.token_error.setStringValue(&ns(&message)),
        }
    }
}

/// Clears `flag` once the device-code attempt is no longer current, so the background
/// poll stops (the poll closure cannot reach main-thread state itself).
fn poll_validity(
    weak: Weak<SignIn>,
    attempt: u32,
    flag: std::sync::Arc<std::sync::atomic::AtomicBool>,
) {
    util::run_later(std::time::Duration::from_secs(1), move || {
        let current = weak.upgrade().is_some_and(|this| this.is_current(attempt));
        if current {
            poll_validity(weak, attempt, flag);
        } else {
            flag.store(false, std::sync::atomic::Ordering::Relaxed);
        }
    });
}

// ----- Session host sign-in -----

/// Called with the authorization code, or `None` when cancelled.
type OnCode = Box<dyn FnOnce(Option<String>)>;

struct HostSignIn {
    flow: RefCell<SessionHostSignIn>,
    web: RefCell<Option<Retained<WKWebView>>>,
    window: RefCell<Option<Retained<NSWindow>>>,
    on_done: RefCell<Option<OnCode>>,
}

impl HostSignIn {
    fn is_done(&self) -> bool {
        self.on_done.borrow().is_none()
    }

    /// Completes the attempt once; later calls are ignored.
    fn finish(&self, redirect: Option<String>) {
        let Some(on_done) = self.on_done.borrow_mut().take() else {
            return;
        };
        if redirect.is_none() {
            self.flow.borrow_mut().cancel();
        }
        // Take the values first: closing the window re-enters `finish` via its delegate.
        let view = self.web.borrow_mut().take();
        if let Some(view) = view {
            // SAFETY: Stopping a live web view on the main thread.
            unsafe { view.stopLoading() };
        }
        let window = self.window.borrow_mut().take();
        if let Some(window) = window {
            dismiss(&window);
        }
        on_done(redirect);
    }

    fn handle_uri(&self, view: &WKWebView, url: &str) -> bool {
        let navigation = self.flow.borrow_mut().on_navigation(url);
        match navigation {
            Navigation::Continue => false,
            Navigation::Load { url, headers } => {
                load(view, &url, &headers);
                true
            }
            Navigation::Done(redirect) => {
                self.finish(Some(redirect));
                true
            }
            Navigation::Failed(_) => {
                self.finish(None);
                true
            }
        }
    }

    /// Shows the web view so the user can finish (MFA, account picker, consent).
    fn present(self: &Rc<Self>, parent: Option<&NSWindow>) {
        let Some(view) = self.web.borrow().clone() else {
            return;
        };
        let mtm = view.mtm();
        let window = new_window(mtm, SessionHostSignIn::TITLE, 520.0, 720.0);
        let Some(root) = window.contentView() else {
            return;
        };
        fill(&view, &root, 52.0);
        let weak = Rc::downgrade(self);
        let cancel = util::button(mtm, "Cancel", move || {
            if let Some(this) = weak.upgrade() {
                this.finish(None);
            }
        });
        cancel.setKeyEquivalent(&ns("\u{1b}"));
        let width = cancel.frame().size.width.max(90.0);
        cancel.setFrame(rect(
            root.bounds().size.width - 20.0 - width,
            12.0,
            width,
            cancel.frame().size.height,
        ));
        cancel.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMinXMargin);
        root.addSubview(&cancel);
        let weak = Rc::downgrade(self);
        on_close(&window, move || {
            if let Some(this) = weak.upgrade() {
                // Already closing: don't end the sheet again.
                this.window.borrow_mut().take();
                this.finish(None);
            }
        });
        *self.window.borrow_mut() = Some(window.clone());
        match parent.filter(|p| p.isVisible()) {
            Some(parent) => parent.beginSheet_completionHandler(&window, None),
            None => {
                window.center();
                window.makeKeyAndOrderFront(None);
            }
        }
    }
}

/// Obtains the session-host authorization redirect for `url` (requested by the RDP
/// engine). `on_done` receives the redirect URL, or `None` when cancelled or failed.
pub(super) fn session_host(
    mtm: MainThreadMarker,
    parent: Option<Retained<NSWindow>>,
    url: &str,
    on_done: impl FnOnce(Option<String>) + 'static,
) {
    let flow = SessionHostSignIn::new(url);
    let start = flow.url().to_string();
    let state = Rc::new(HostSignIn {
        flow: RefCell::new(flow),
        web: RefCell::new(None),
        window: RefCell::new(None),
        on_done: RefCell::new(Some(Box::new(on_done))),
    });
    let weak = Rc::downgrade(&state);
    let view = web_view(
        mtm,
        SessionHostSignIn::USER_AGENT,
        None,
        move |view, url| {
            weak.upgrade()
                .is_some_and(|this| this.handle_uri(view, url))
        },
    );
    *state.web.borrow_mut() = Some(view.clone());
    load(&view, &start, &[]);
    // The state stays alive through this timer until it presents or completes; a
    // presented window keeps it alive through its buttons and delegate.
    util::run_later(SessionHostSignIn::SILENT_TIMEOUT, move || {
        if state.is_done() {
            return;
        }
        log::info!("Silent sign-in did not complete; asking the user");
        state.present(parent.as_deref());
        keep_until_done(state);
    });
}

/// Holds `state` until it completed (the window's closures only keep weak references).
fn keep_until_done(state: Rc<HostSignIn>) {
    util::run_later(std::time::Duration::from_secs(1), move || {
        if !state.is_done() {
            keep_until_done(state);
        }
    });
}
