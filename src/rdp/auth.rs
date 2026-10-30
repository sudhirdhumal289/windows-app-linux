//! RDS AAD (Entra ID) authorization flow requested by FreeRDP during connect.
//!
//! FreeRDP asks for an access token for the session host.
//! Like Windows WAM / Microsoft.AAD.BrokerPlugin, we first attempt to silently fulfill
//! the request using the stored refresh token from the active session. If silent
//! acquisition is unavailable or fails, we fall back to driving the authorization-code flow
//! in the frontend web view.

use crate::auth::TokenStore;
const DEFAULT_AVD_CLIENT_ID: &str = "a85cf173-4192-42f8-81fa-777a763e6e2c";
const AAD_AUTHORIZE_ENDPOINT: &str =
    "https://login.microsoftonline.com/common/oauth2/v2.0/authorize";

/// Parameters of an in-flight AAD authorization-code request from FreeRDP.
///
/// `scope` and `req_cnf` arrive from FreeRDP already URL-encoded and are used verbatim,
/// matching FreeRDP's own CLI client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PendingAuth {
    client_id: String,
    scope: String,
    req_cnf: Option<String>,
}

impl PendingAuth {
    pub(crate) fn new(client_id: Option<String>, scope: String, req_cnf: Option<String>) -> Self {
        Self {
            client_id: client_id
                .filter(|c| !c.is_empty())
                .unwrap_or_else(|| DEFAULT_AVD_CLIENT_ID.to_string()),
            scope,
            req_cnf: req_cnf.filter(|r| !r.is_empty()),
        }
    }

    fn redirect_uri(&self) -> String {
        format!(
            "ms-appx-web%3a%2f%2fMicrosoft.AAD.BrokerPlugin%2f{}",
            self.client_id
        )
    }

    /// URL the user signs in at. `login_hint` pre-fills the account.
    pub(crate) fn authorize_url(&self, login_hint: Option<&str>) -> String {
        let mut url = format!(
            "{AAD_AUTHORIZE_ENDPOINT}?client_id={}&response_type=code&scope={}&redirect_uri={}",
            self.client_id,
            self.scope,
            self.redirect_uri()
        );
        if let Some(hint) = login_hint.map(str::trim).filter(|h| !h.is_empty()) {
            url.push_str("&login_hint=");
            url.extend(url::form_urlencoded::byte_serialize(hint.as_bytes()));
        }
        url
    }

    /// x-www-form-urlencoded token request body for an authorization code.
    pub(crate) fn token_request(&self, code: &str) -> String {
        let mut body = format!(
            "grant_type=authorization_code&code={code}&client_id={}&scope={}&redirect_uri={}",
            self.client_id,
            self.scope,
            self.redirect_uri()
        );
        if let Some(req_cnf) = &self.req_cnf {
            body.push_str("&req_cnf=");
            body.push_str(req_cnf);
        }
        body
    }

    /// Attempts to build a silent token request using the stored refresh token from the active session.
    ///
    /// Matches Windows WAM (BrokerPlugin) behavior: acquires the session host token in the background
    /// without interactive browser navigation, avoiding Conditional Access unmanaged browser blocks.
    pub(crate) fn try_silent_token_request(&self) -> Option<String> {
        let saved = TokenStore::global().load()?;
        let refresh_token = saved.refresh_token.as_deref().filter(|rt| !rt.is_empty())?;
        log::debug!(
            "Found stored refresh token; attempting silent broker token request for scope {}",
            self.scope
        );

        let client_id = saved
            .client_id
            .as_deref()
            .filter(|c| !c.is_empty())
            .unwrap_or(&self.client_id);

        let mut body = format!(
            "grant_type=refresh_token&client_id={client_id}&scope={}&refresh_token={}",
            self.scope,
            url::form_urlencoded::byte_serialize(refresh_token.as_bytes()).collect::<String>()
        );
        if let Some(req_cnf) = &self.req_cnf {
            body.push_str("&req_cnf=");
            body.push_str(req_cnf);
        }
        Some(body)
    }
}

/// Extracts the (URL-encoded) authorization code from an OAuth redirect URL.
pub(crate) fn extract_auth_code(redirect_url: &str) -> Option<String> {
    if let Ok(url) = url::Url::parse(redirect_url.trim()) {
        if let Some((_, code)) = url.query_pairs().find(|(k, _)| k == "code") {
            if !code.is_empty() {
                return Some(url::form_urlencoded::byte_serialize(code.as_bytes()).collect());
            }
        }
    }
    // Fallback for redirects that aren't valid URLs or carry an encoded query.
    let start = match redirect_url.find("code=") {
        Some(i) => i + "code=".len(),
        None => redirect_url.find("code%3D")? + "code%3D".len(),
    };
    let rest = &redirect_url[start..];
    let end = rest.find(['&', ' ', '\r', '\n']).unwrap_or(rest.len());
    let code = &rest[..end];
    (!code.is_empty()).then(|| code.to_string())
}

/// Returns the ARM gateway bearer token from FreeRDP-style `/gateway:...,bearer:<token>` args.
pub(crate) fn gateway_bearer_token(args: &[String]) -> Option<&str> {
    args.iter()
        .filter(|arg| arg.starts_with("/gateway:"))
        .find_map(|arg| arg.split_once("bearer:").map(|(_, rest)| rest))
        .map(|rest| rest.split(',').next().unwrap_or(rest))
        .filter(|token| !token.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_url_and_token_request_match_freerdp_format() {
        let auth = PendingAuth::new(None, "openid%20profile".into(), Some("abc".into()));
        let redirect =
            "ms-appx-web%3a%2f%2fMicrosoft.AAD.BrokerPlugin%2fa85cf173-4192-42f8-81fa-777a763e6e2c";
        assert_eq!(
            auth.authorize_url(None),
            format!(
                "{AAD_AUTHORIZE_ENDPOINT}?client_id={DEFAULT_AVD_CLIENT_ID}&response_type=code&scope=openid%20profile&redirect_uri={redirect}"
            )
        );
        assert_eq!(
            auth.token_request("C0de"),
            format!(
                "grant_type=authorization_code&code=C0de&client_id={DEFAULT_AVD_CLIENT_ID}&scope=openid%20profile&redirect_uri={redirect}&req_cnf=abc"
            )
        );

        let custom = PendingAuth::new(Some("my-client".into()), "s".into(), Some(String::new()));
        assert!(custom.authorize_url(None).contains("client_id=my-client&"));
        assert!(!custom.token_request("x").contains("req_cnf"));
    }

    #[test]
    fn authorize_url_carries_login_hint() {
        let auth = PendingAuth::new(None, "s".into(), None);
        assert!(auth
            .authorize_url(Some("a.b+c@contoso.com"))
            .ends_with("&login_hint=a.b%2Bc%40contoso.com"));
        assert!(!auth.authorize_url(Some("  ")).contains("login_hint"));
    }

    #[test]
    fn extracts_auth_code_from_redirects() {
        let base = "ms-appx-web://Microsoft.AAD.BrokerPlugin/a85cf173";
        assert_eq!(
            extract_auth_code(&format!("{base}?code=0.AbC-d_e&session_state=x")).as_deref(),
            Some("0.AbC-d_e")
        );
        assert_eq!(
            extract_auth_code(&format!("{base}?error_code=1&code=real")).as_deref(),
            Some("real")
        );
        assert_eq!(
            extract_auth_code("not a url code%3Dencoded&x").as_deref(),
            Some("encoded")
        );
        assert_eq!(
            extract_auth_code(&format!("{base}?error=access_denied")),
            None
        );
        assert_eq!(extract_auth_code(&format!("{base}?code=")), None);
    }

    #[test]
    fn parses_gateway_bearer_token() {
        let args = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(
            gateway_bearer_token(&args(&["/v:h", "/gateway:type:arm,bearer:tok.en-1"])),
            Some("tok.en-1")
        );
        assert_eq!(
            gateway_bearer_token(&args(&["/gateway:bearer:abc,type:arm"])),
            Some("abc")
        );
        assert_eq!(
            gateway_bearer_token(&args(&["/v:h", "/gateway:bearer:"])),
            None
        );
        assert_eq!(gateway_bearer_token(&args(&["/v:h", "/u:bearer:x"])), None);
        assert_eq!(gateway_bearer_token(&args(&["/v:h"])), None);
    }
}
