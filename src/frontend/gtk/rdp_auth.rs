//! Entra ID sign-in requested by FreeRDP while connecting to a session host.
//!
//! The authorize URL is first loaded in a hidden web view that shares the cookie jar of
//! the main sign-in, so an existing SSO session usually completes without any UI. If no
//! redirect arrives within [`SILENT_TIMEOUT`], the same web view is shown in a window so
//! the user can finish interactively (MFA, account picker, consent).
//!
//! The iOS Remote Desktop user agent is used because Conditional Access commonly blocks
//! unmanaged desktop platforms for the session-host resource.

use super::webkit;
use crate::auth::pkap::DeviceIdentity;
use crate::auth::pkeyauth::Challenge;
use crate::auth::redirect::{self, Redirect};
use crate::config::UA_IOS_REMOTE_DESKTOP;
use adw::prelude::*;
use gtk4::glib;
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;
use webkit6::prelude::*;
use webkit6::{
    LoadEvent, NavigationPolicyDecision, PolicyDecisionType, Settings, URIRequest, WebView,
};

const SILENT_TIMEOUT: Duration = Duration::from_secs(6);

type Completion = Box<dyn FnOnce(Option<String>)>;

/// State of one authorization attempt. Holding the web view here keeps it alive while it
/// is headless; [`State::finish`] drops it, breaking the handler → state → view cycle.
struct State {
    webview: RefCell<Option<WebView>>,
    window: RefCell<Option<adw::Window>>,
    on_done: RefCell<Option<Completion>>,
    device: Option<DeviceIdentity>,
}

impl State {
    fn is_done(&self) -> bool {
        self.on_done.borrow().is_none()
    }

    /// Completes the attempt once; later calls are ignored.
    fn finish(&self, redirect_url: Option<String>) {
        let Some(on_done) = self.on_done.borrow_mut().take() else {
            return;
        };

        // Defer WebKit stop and window close to the next event-loop tick.
        // Destroying an active WebKitWebView or its parent window synchronously from within
        // a WebKit signal handler (such as `decide-policy` or `load-changed`) violates WebKitGTK
        // reentrancy constraints and causes native assertions / SIGABRT (`Aborted`).
        let view = self.webview.borrow_mut().take();
        let window = self.window.borrow_mut().take();
        glib::idle_add_local_once(move || {
            if let Some(view) = view {
                view.stop_loading();
            }
            if let Some(window) = window {
                window.close();
            }
        });

        if redirect_url.is_some() {
            webkit::persist_session_cookies();
        }
        on_done(redirect_url);
    }

    /// Handles a URI the web view is about to load. Returns `true` if it was consumed.
    fn handle_uri(&self, view: &WebView, uri: &str) -> bool {
        if let Some(challenge) = Challenge::parse(uri) {
            log::debug!("Answering PKeyAuth device challenge");
            let request = URIRequest::new(&challenge.submit_url);
            if let Some(headers) = request.http_headers() {
                headers.append(
                    "Authorization",
                    &challenge.authorization(self.device.as_ref()),
                );
                headers.append("User-Agent", UA_IOS_REMOTE_DESKTOP);
            }
            view.load_request(&request);
            return true;
        }
        match redirect::parse(uri, &[]) {
            Some(Redirect::Code(_)) => {
                log::debug!("Session host authorization redirect captured");
                self.finish(Some(uri.to_string()));
                true
            }
            Some(Redirect::Error(message)) => {
                log::warn!("Session host authorization failed: {message}");
                self.finish(None);
                true
            }
            None => false,
        }
    }
}

pub struct RdpAuthDialog;

impl RdpAuthDialog {
    /// Obtains the authorization redirect for `auth_url`. `on_done` receives the redirect
    /// URL, or `None` if the user cancelled or the service returned an error.
    pub fn run(
        parent: &impl IsA<gtk4::Window>,
        auth_url: &str,
        on_done: impl FnOnce(Option<String>) + 'static,
    ) {
        let settings = Settings::new();
        settings.set_enable_javascript(true);
        settings.set_user_agent(Some(UA_IOS_REMOTE_DESKTOP));
        let webview = WebView::builder()
            .network_session(&webkit::shared_network_session())
            .settings(&settings)
            .vexpand(true)
            .hexpand(true)
            .build();

        let state = Rc::new(State {
            webview: RefCell::new(Some(webview.clone())),
            window: RefCell::new(None),
            on_done: RefCell::new(Some(Box::new(on_done))),
            device: DeviceIdentity::try_load_default(),
        });

        let s = Rc::clone(&state);
        webview.connect_decide_policy(move |view, decision, decision_type| {
            if decision_type != PolicyDecisionType::NavigationAction {
                return false;
            }
            let uri = decision
                .downcast_ref::<NavigationPolicyDecision>()
                .and_then(NavigationPolicyDecision::navigation_action)
                .and_then(|action| action.request())
                .and_then(|request| request.uri());
            match uri {
                Some(uri) if s.handle_uri(view, &uri) => {
                    decision.ignore();
                    true
                }
                _ => false,
            }
        });

        // Redirects that bypass the navigation policy (e.g. server-side 302s on some
        // WebKit versions) are still observed when the load starts.
        let s = Rc::clone(&state);
        webview.connect_load_changed(move |view, event| {
            if matches!(event, LoadEvent::Started | LoadEvent::Redirected) {
                if let Some(uri) = view.uri() {
                    s.handle_uri(view, &uri);
                }
            }
        });

        log::debug!("Starting session host authorization");
        webview.load_uri(auth_url);

        let parent = parent.as_ref().downgrade();
        let s = Rc::clone(&state);
        glib::timeout_add_local_once(SILENT_TIMEOUT, move || {
            if s.is_done() {
                return;
            }
            log::info!("Silent sign-in did not complete; asking the user");
            Self::present(&s, parent.upgrade().as_ref());
        });
    }

    fn present(state: &Rc<State>, parent: Option<&gtk4::Window>) {
        let Some(webview) = state.webview.borrow().clone() else {
            return;
        };
        let header = adw::HeaderBar::new();
        let content = adw::ToolbarView::new();
        content.add_top_bar(&header);
        content.set_content(Some(&webview));

        let window = adw::Window::builder()
            .title("Sign in to Cloud PC")
            .default_width(520)
            .default_height(720)
            .modal(true)
            .content(&content)
            .build();
        window.set_transient_for(parent);

        let s = Rc::downgrade(state);
        window.connect_close_request(move |_| {
            if let Some(s) = s.upgrade() {
                // If closed by user, clear window so finish won't attempt redundant close.
                s.window.borrow_mut().take();
                s.finish(None);
            }
            glib::Propagation::Proceed
        });
        *state.window.borrow_mut() = Some(window.clone());
        window.present();
    }
}
