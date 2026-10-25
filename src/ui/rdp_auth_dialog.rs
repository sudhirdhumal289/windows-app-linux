//! Completely headless background OAuth broker for FreeRDP Entra ID desktop authentication.
//!
//! Reuses the authenticated session from the main window and completes the authorization
//! redirect headlessly in the background with ZERO windows created or presented.
//! Uses the Mobile / iOS Remote Desktop profile by default to bypass Intune MDM
//! Conditional Access compliance policies enforced on unmanaged Linux devices.

use crate::auth::pkap::DeviceIdentity;
use crate::config::UA_IOS_REMOTE_DESKTOP;
use crate::ui::get_shared_network_session;
use gtk4::glib;
use gtk4::prelude::*;
use log::{debug, warn};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use url::Url;
use webkit6::prelude::*;
use webkit6::{
    LoadEvent, NavigationPolicyDecision, PolicyDecisionType, Settings, URIRequest, WebView,
};

pub struct RdpAuthDialog;

impl RdpAuthDialog {
    /// Acquires Entra ID authorization for FreeRDP completely silently in the background.
    ///
    /// Executes headlessly with ZERO windows. Reuses the active SSO session from the
    /// main window and completes in ~0.6s.
    pub fn show<P, FRedirect>(_parent: &P, auth_url: &str, on_redirect: FRedirect)
    where
        P: IsA<gtk4::Window>,
        FRedirect: Fn(String) + 'static,
    {
        let network_session = get_shared_network_session();

        // Default to Mobile / iOS Remote Desktop profile for session host auth (termsrv.wvd.microsoft.com)
        // to bypass the Intune MDM compliance requirement ("You can't get there from here").
        let webview = WebView::builder().network_session(&network_session).build();

        let settings = Settings::new();
        settings.set_enable_javascript(true);
        settings.set_javascript_can_open_windows_automatically(true);
        settings.set_user_agent(Some(UA_IOS_REMOTE_DESKTOP));
        webview.set_settings(&settings);

        let device_identity = Arc::new(DeviceIdentity::try_load_default());
        let completed = Rc::new(RefCell::new(false));
        let on_redirect = Rc::new(on_redirect);

        // Handle navigation and policy decisions
        let on_redirect_cb = Rc::clone(&on_redirect);
        let completed_cb = Rc::clone(&completed);
        let webview_cb = webview.clone();
        let dev_id_cb = Arc::clone(&device_identity);

        webview.connect_decide_policy(move |_, decision, decision_type| {
            if decision_type != PolicyDecisionType::NavigationAction {
                return false;
            }

            let Some(nav_decision) = decision.dynamic_cast_ref::<NavigationPolicyDecision>() else {
                return false;
            };

            let Some(nav_action) = nav_decision.navigation_action() else {
                return false;
            };

            let Some(req) = nav_action.request() else {
                return false;
            };

            let Some(uri) = req.uri() else {
                return false;
            };

            let uri_str = uri.as_str();

            // Handle MS-PKAP / PKeyAuth device certificate challenge
            if uri_str.starts_with("urn:http-auth:PKeyAuth") {
                debug!("RDP Auth: Intercepted PKeyAuth device challenge");
                if let Ok(parsed) = Url::parse(uri_str) {
                    let query_map: std::collections::HashMap<_, _> =
                        parsed.query_pairs().into_owned().collect();
                    if let Some(submit_url) = query_map.get("SubmitUrl") {
                        let context = query_map.get("Context").map(String::as_str).unwrap_or("");
                        let nonce = query_map.get("Nonce").map(String::as_str).unwrap_or("");

                        let auth_val = if let Some(ref dev_id) = dev_id_cb.as_ref() {
                            match dev_id.sign_challenge(submit_url, nonce, context) {
                                Ok(signed) => signed,
                                Err(e) => {
                                    warn!("Failed to sign PKeyAuth challenge: {e}");
                                    format!(r#"PKeyAuth Context="{}", Version="1.0""#, context)
                                }
                            }
                        } else {
                            format!(r#"PKeyAuth Context="{}", Version="1.0""#, context)
                        };

                        let req = URIRequest::new(submit_url);
                        if let Some(headers) = req.http_headers() {
                            headers.append("Authorization", &auth_val);
                            headers.append("User-Agent", UA_IOS_REMOTE_DESKTOP);
                        }
                        webview_cb.load_request(&req);
                        decision.ignore();
                        return true;
                    }
                }
                decision.ignore();
                return true;
            }

            // Redirect with authorization code - require genuine code parameter and no error
            let has_code = (uri_str.contains("?code=") || uri_str.contains("&code="))
                && !uri_str.contains("error=");

            if has_code
                && (uri_str.starts_with("ms-appx-web://")
                    || uri_str.contains("BrokerPlugin")
                    || uri_str.contains("nativeclient"))
            {
                if *completed_cb.borrow() {
                    decision.ignore();
                    return true;
                }
                *completed_cb.borrow_mut() = true;

                debug!(
                    "Successfully captured Entra ID authorization redirect for FreeRDP silently"
                );
                decision.ignore();

                crate::ui::persist_session_cookies();
                let redirect_captured = uri_str.to_string();
                on_redirect_cb(redirect_captured);
                return true;
            }

            false
        });

        // Safety net: capture redirect via load-changed
        let on_redirect_load = Rc::clone(&on_redirect);
        let completed_load = Rc::clone(&completed);
        webview.connect_load_changed(move |view, event| {
            if event != LoadEvent::Started && event != LoadEvent::Committed {
                return;
            }
            if let Some(uri) = view.uri() {
                let uri_str = uri.as_str();
                let has_code = (uri_str.contains("?code=") || uri_str.contains("&code="))
                    && !uri_str.contains("error=");

                if has_code
                    && (uri_str.starts_with("ms-appx-web://")
                        || uri_str.contains("BrokerPlugin")
                        || uri_str.contains("nativeclient"))
                {
                    if *completed_load.borrow() {
                        return;
                    }
                    *completed_load.borrow_mut() = true;

                    debug!("Successfully captured Entra ID redirect via load_changed");
                    crate::ui::persist_session_cookies();
                    let redirect_captured = uri_str.to_string();
                    on_redirect_load(redirect_captured);
                }
            }
        });

        // Load the authorization URL headlessly in background
        debug!("Initiating silent background FreeRDP authorization");
        webview.load_uri(auth_url);

        // Timeout check: if silent SSO doesn't complete within 5s, session may be expired
        let completed_timer = Rc::clone(&completed);
        glib::timeout_add_local_once(std::time::Duration::from_millis(5000), move || {
            if !*completed_timer.borrow() {
                warn!("Silent SSO did not complete within 5s; SSO session from main window may have expired");
            }
        });
    }
}
