//! Embedded Cloud PC Remote Desktop Session Window.

use crate::auth::pkap::DeviceIdentity;
use crate::config::AppConfig;
use crate::models::CloudPc;
use crate::ui::login_dialog::UA_WINDOWS_11_WAM;
use gtk4::prelude::*;
use gtk4::{Box, Button, HeaderBar, Orientation, Window};
use log::debug;
use std::sync::Arc;
use webkit6::prelude::*;
use webkit6::{
    NavigationPolicyDecision, PolicyDecisionType, URIRequest, UserContentInjectedFrames,
    UserContentManager, UserScript, UserScriptInjectionTime, WebView,
};

pub struct SessionWindow;

impl SessionWindow {
    /// Opens an embedded Cloud PC session window rendering the official Microsoft Remote Desktop Web Client via Entra ID SSO.
    pub fn open<P: IsA<Window>>(parent: &P, pc: &CloudPc) {
        let user_upn = pc
            .user_principal_name
            .clone()
            .or_else(AppConfig::get_user_upn)
            .unwrap_or_else(|| "user@domain.com".to_string());

        let encoded_upn: String =
            url::form_urlencoded::byte_serialize(user_upn.as_bytes()).collect();

        // Official Cloud PC SSO desktop endpoint as extracted from Windows App MSIX bundle
        let url = format!(
            "https://windows.cloud.microsoft/webclient/ent/{}#login_hint={}",
            pc.id, encoded_upn
        );
        debug!(
            "Opening Cloud PC SSO session window for [{}]",
            pc.display_name
        );

        let window = Window::builder()
            .transient_for(parent)
            .title(&format!("Cloud PC - {}", pc.display_name))
            .default_width(1280)
            .default_height(800)
            .build();

        let header = HeaderBar::new();

        let fullscreen_btn = Button::from_icon_name("view-fullscreen-symbolic");
        fullscreen_btn.set_tooltip_text(Some("Toggle Fullscreen"));
        let win_fs = window.clone();
        fullscreen_btn.connect_clicked(move |_| {
            if win_fs.is_fullscreen() {
                win_fs.unfullscreen();
            } else {
                win_fs.fullscreen();
            }
        });
        header.pack_start(&fullscreen_btn);

        let refresh_btn = Button::from_icon_name("view-refresh-symbolic");
        refresh_btn.set_tooltip_text(Some("Reload Session"));
        header.pack_start(&refresh_btn);

        let browser_btn = Button::with_label("🌐 Open in Browser");
        browser_btn.set_tooltip_text(Some("Open this Cloud PC session in external browser"));
        let url_copy = url.clone();
        browser_btn.connect_clicked(move |_| {
            let _ = open::that(&url_copy);
        });
        header.pack_end(&browser_btn);

        window.set_titlebar(Some(&header));

        let main_box = Box::new(Orientation::Vertical, 0);

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
        content_manager.add_script(&wam_mask_script);

        // Persistent shared network session containing the user's login cookies from the main window
        let session = crate::ui::get_shared_network_session();

        let webview = WebView::builder()
            .network_session(&session)
            .user_content_manager(&content_manager)
            .build();

        if let Some(settings) = webkit6::prelude::WebViewExt::settings(&webview) {
            settings.set_enable_webaudio(true);
            settings.set_enable_media_stream(true);
            settings.set_enable_mediasource(true);
            settings.set_enable_javascript(true);
            settings.set_javascript_can_access_clipboard(true);
            settings.set_user_agent(Some(UA_WINDOWS_11_WAM));
        }

        // Auto-allow permissions (clipboard, audio, notifications)
        webview.connect_permission_request(|_, req| {
            req.allow();
            true
        });

        // Handle PKeyAuth device identity challenge if issued by Entra ID
        let device_identity = Arc::new(DeviceIdentity::try_load_default());
        let ua_str = UA_WINDOWS_11_WAM.to_string();
        webview.connect_decide_policy(move |wv, decision, decision_type| {
            if decision_type == PolicyDecisionType::NavigationAction
                || decision_type == PolicyDecisionType::NewWindowAction
            {
                if let Some(nav_decision) = decision.dynamic_cast_ref::<NavigationPolicyDecision>() {
                    if let Some(action) = nav_decision.navigation_action() {
                        if let Some(req) = action.request() {
                            if let Some(uri) = req.uri() {
                                if uri.starts_with("urn:http-auth:PKeyAuth") {
                                    if let Ok(parsed) = url::Url::parse(&uri) {
                                        let query_map: std::collections::HashMap<_, _> =
                                            parsed.query_pairs().into_owned().collect();
                                        if let Some(submit_url) = query_map.get("SubmitUrl") {
                                            let context =
                                                query_map.get("Context").map(String::as_str).unwrap_or("");
                                            let nonce =
                                                query_map.get("Nonce").map(String::as_str).unwrap_or("");

                                            let auth_val = if let Some(ref dev_id) = device_identity.as_ref() {
                                                match dev_id.sign_challenge(submit_url, nonce, context) {
                                                    Ok(signed_auth) => signed_auth,
                                                    Err(_) => {
                                                        format!(r#"PKeyAuth Context="{}", Version="1.0""#, context)
                                                    }
                                                }
                                            } else {
                                                format!(r#"PKeyAuth Context="{}", Version="1.0""#, context)
                                            };

                                            let req = URIRequest::new(submit_url);
                                            if let Some(headers) = req.http_headers() {
                                                headers.append("Authorization", &auth_val);
                                                headers.append("User-Agent", &ua_str);
                                                headers.append("x-client-SKU", "MSAL.Desktop");
                                                headers.append("x-client-Ver", "4.60.0.0");
                                                headers.append(
                                                    "x-client-OS",
                                                    "Microsoft Windows NT 10.0.22631.0",
                                                );
                                            }
                                            wv.load_request(&req);
                                            decision.ignore();
                                            return true;
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            false
        });

        let wv_refresh = webview.clone();
        refresh_btn.connect_clicked(move |_| {
            wv_refresh.reload();
        });

        webview.set_hexpand(true);
        webview.set_vexpand(true);

        main_box.append(&webview);
        window.set_child(Some(&main_box));

        let req = URIRequest::new(&url);
        if let Some(headers) = req.http_headers() {
            headers.append("User-Agent", UA_WINDOWS_11_WAM);
            headers.append("x-client-SKU", "MSAL.Desktop");
            headers.append("x-client-Ver", "4.60.0.0");
            headers.append("x-client-OS", "Microsoft Windows NT 10.0.22631.0");
        }
        webview.load_request(&req);

        window.present();
    }
}
