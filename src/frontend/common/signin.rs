//! Entra ID sign-in driven from any embedded web view (WebKitGTK, WebView2, WKWebView).
//!
//! The web view reports every URL it is about to load to [`BrowserSignIn::on_navigation`]
//! (or [`SessionHostSignIn::on_navigation`]) and does what the returned [`Navigation`]
//! says: continue, load another request with extra headers (PKeyAuth device challenges),
//! or stop because the flow produced a result.

use crate::auth::pkap::DeviceIdentity;
use crate::auth::pkce::Pkce;
use crate::auth::pkeyauth::Challenge;
use crate::auth::redirect::{self, Redirect};
use crate::auth::{AuthManager, MSAL_SKU, MSAL_VERSION};
use crate::config::{AppConfig, AuthProfile, UA_IOS_REMOTE_DESKTOP};
use std::sync::Arc;

/// Makes the page's JavaScript view of the browser match the Windows WAM broker web view,
/// so Entra ID classifies the sign-in as a desktop client rather than a browser. Inject
/// it at document start in every frame when [`BrowserSignIn::injected_script`] says so.
pub const WAM_MASK_SCRIPT: &str = r"
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

/// What the web view should do with a URL it is about to load.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Navigation {
    /// Not ours: let the web view load it.
    Continue,
    /// Cancel this navigation and load `url` with `headers` instead.
    Load {
        url: String,
        headers: Vec<(&'static str, String)>,
    },
    /// Cancel: the flow finished with this authorization code (main sign-in) or redirect
    /// URL (session host sign-in).
    Done(String),
    /// Cancel: the service reported an error.
    Failed(String),
}

fn answer_challenge(
    challenge: &Challenge,
    device: Option<&DeviceIdentity>,
    user_agent: &str,
    client_os: Option<&str>,
    msal_headers: bool,
) -> Navigation {
    log::debug!("Answering PKeyAuth device challenge");
    let mut headers = vec![
        ("Authorization", challenge.authorization(device)),
        ("User-Agent", user_agent.to_string()),
    ];
    if msal_headers {
        headers.push(("x-client-SKU", MSAL_SKU.to_string()));
        headers.push(("x-client-Ver", MSAL_VERSION.to_string()));
        if let Some(os) = client_os {
            headers.push(("x-client-OS", os.to_string()));
        }
    }
    Navigation::Load {
        url: challenge.submit_url.clone(),
        headers,
    }
}

/// Authorization-code + PKCE sign-in with the identity of the configured profile.
pub struct BrowserSignIn {
    profile: AuthProfile,
    auth: Arc<AuthManager>,
    verifier: String,
    url: String,
    device: Option<DeviceIdentity>,
    finished: bool,
}

impl BrowserSignIn {
    /// Starts a new attempt with the profile from the preferences.
    #[must_use]
    pub fn start() -> Self {
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
        log::debug!("Starting browser sign-in with profile {profile:?}");
        Self {
            profile,
            auth,
            verifier: pkce.verifier,
            url,
            device: DeviceIdentity::try_load_default(),
            finished: false,
        }
    }

    /// Page to load first.
    #[must_use]
    pub fn url(&self) -> &str {
        &self.url
    }

    #[must_use]
    pub fn user_agent(&self) -> &'static str {
        self.profile.user_agent()
    }

    /// Script to inject at document start, if the profile needs one.
    #[must_use]
    pub fn injected_script(&self) -> Option<&'static str> {
        self.profile.masks_as_wam().then_some(WAM_MASK_SCRIPT)
    }

    /// Decides what to do with a URL the web view is about to load. After `Done` or
    /// `Failed`, later navigations are ignored (`Continue`).
    pub fn on_navigation(&mut self, uri: &str) -> Navigation {
        if self.finished {
            return Navigation::Continue;
        }
        if let Some(challenge) = Challenge::parse(uri) {
            return answer_challenge(
                &challenge,
                self.device.as_ref(),
                self.profile.user_agent(),
                self.profile.client_os(),
                true,
            );
        }
        match redirect::parse(uri, &[self.profile.redirect_uri()]) {
            Some(Redirect::Code(code)) => {
                self.finished = true;
                Navigation::Done(code)
            }
            Some(Redirect::Error(message)) => {
                log::warn!("Sign-in failed: {message}");
                self.finished = true;
                Navigation::Failed(format!("Sign-in failed: {message}"))
            }
            None => Navigation::Continue,
        }
    }

    /// Exchanges the authorization code for tokens and stores them. Run it on the Tokio
    /// runtime; the returned future does not borrow `self`.
    pub fn exchange(
        &self,
        code: String,
    ) -> impl std::future::Future<Output = Result<(), String>> + Send + 'static {
        let auth = Arc::clone(&self.auth);
        let verifier = self.verifier.clone();
        let profile = self.profile;
        async move {
            auth.exchange_authorization_code(
                &code,
                &verifier,
                profile.redirect_uri(),
                profile.client_os(),
            )
            .await
            .map(|_| log::info!("Signed in"))
            .map_err(|e| {
                log::error!("Token exchange failed: {e}");
                format!("Sign-in could not be completed: {e}")
            })
        }
    }
}

/// Sign-in requested by the RDP engine for the session host
/// ([`crate::rdp::BridgeEvent::AuthRequired`]). Load [`Self::url`] in a hidden web view
/// sharing the main sign-in's cookies; show it after [`Self::SILENT_TIMEOUT`] if it has
/// not finished. Answer the engine with the `Done` URL (or `None` when cancelled/failed).
pub struct SessionHostSignIn {
    url: String,
    device: Option<DeviceIdentity>,
    finished: bool,
}

impl SessionHostSignIn {
    /// How long to try silently (SSO cookies) before showing the page to the user.
    pub const SILENT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(6);
    /// The iOS Remote Desktop user agent: Conditional Access commonly blocks unmanaged
    /// desktop platforms for the session-host resource.
    pub const USER_AGENT: &'static str = UA_IOS_REMOTE_DESKTOP;
    pub const TITLE: &'static str = "Sign in to Cloud PC";

    #[must_use]
    pub fn new(url: &str) -> Self {
        log::debug!("Starting session host authorization");
        Self {
            url: url.to_string(),
            device: DeviceIdentity::try_load_default(),
            finished: false,
        }
    }

    #[must_use]
    pub fn url(&self) -> &str {
        &self.url
    }

    #[must_use]
    pub fn is_finished(&self) -> bool {
        self.finished
    }

    pub fn on_navigation(&mut self, uri: &str) -> Navigation {
        if self.finished {
            return Navigation::Continue;
        }
        if let Some(challenge) = Challenge::parse(uri) {
            return answer_challenge(
                &challenge,
                self.device.as_ref(),
                UA_IOS_REMOTE_DESKTOP,
                None,
                false,
            );
        }
        match redirect::parse(uri, &[]) {
            Some(Redirect::Code(_)) => {
                log::debug!("Session host authorization redirect captured");
                self.finished = true;
                Navigation::Done(uri.to_string())
            }
            Some(Redirect::Error(message)) => {
                log::warn!("Session host authorization failed: {message}");
                self.finished = true;
                Navigation::Failed(message)
            }
            None => Navigation::Continue,
        }
    }

    /// The user closed the window or the attempt was abandoned.
    pub fn cancel(&mut self) {
        self.finished = true;
    }
}

/// Device-code sign-in (for tenants that require a managed browser).
pub struct DeviceCodeSignIn {
    auth: Arc<AuthManager>,
}

/// Code the user enters at the verification page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceCode {
    pub user_code: String,
    pub verification_uri: String,
    pub message: String,
    device_code: String,
    interval: u64,
}

impl Default for DeviceCodeSignIn {
    fn default() -> Self {
        Self::new()
    }
}

impl DeviceCodeSignIn {
    #[must_use]
    pub fn new() -> Self {
        Self {
            auth: Arc::new(AuthManager::for_profile(AppConfig::load().auth_profile())),
        }
    }

    /// Requests a new code.
    pub fn request(
        &self,
    ) -> impl std::future::Future<Output = Result<DeviceCode, String>> + Send + 'static {
        let auth = Arc::clone(&self.auth);
        async move {
            let response = auth
                .initiate_device_code()
                .await
                .map_err(|e| format!("Could not get a sign-in code: {e}"))?;
            Ok(DeviceCode {
                user_code: response.user_code,
                verification_uri: response.verification_uri,
                message: response.message,
                device_code: response.device_code,
                interval: response.interval,
            })
        }
    }

    /// Waits until the user approved `code` (or `keep_polling` returns false).
    pub fn wait(
        &self,
        code: &DeviceCode,
        keep_polling: impl Fn() -> bool + Send + 'static,
    ) -> impl std::future::Future<Output = Result<(), String>> + Send + 'static {
        let auth = Arc::clone(&self.auth);
        let device_code = code.device_code.clone();
        let interval = code.interval;
        async move {
            auth.poll_device_token(&device_code, interval, keep_polling)
                .await
                .map(|_| ())
                .map_err(|e| e.to_string())
        }
    }

    /// Stores a token pasted by the user.
    pub fn import_token(&self, raw: &str) -> Result<(), String> {
        self.auth
            .import_token(raw)
            .map(|_| ())
            .map_err(|e| format!("Could not use this token: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_host_captures_redirect_once() {
        let mut flow = SessionHostSignIn::new("https://login.example/authorize");
        assert_eq!(
            flow.on_navigation("https://login.example/page"),
            Navigation::Continue
        );
        let redirect = "ms-appx-web://microsoft.aad.brokerplugin/x?code=abc";
        assert_eq!(
            flow.on_navigation(redirect),
            Navigation::Done(redirect.to_string())
        );
        assert!(flow.is_finished());
        assert_eq!(flow.on_navigation(redirect), Navigation::Continue);
    }

    #[test]
    fn session_host_reports_errors() {
        let mut flow = SessionHostSignIn::new("https://login.example/authorize");
        let result =
            flow.on_navigation("ms-appx-web://microsoft.aad.brokerplugin/x?error=access_denied");
        assert!(matches!(result, Navigation::Failed(_)));
    }
}
