//! Microsoft Entra ID authentication: PKCE authorization-code and device-code sign-in,
//! token refresh, and the persistent token store.

pub mod account;
pub mod jwt;
pub mod pkap;
pub mod pkce;
pub mod pkeyauth;
pub mod redirect;
pub mod token_store;

pub use account::Account;
pub use token_store::TokenStore;

use crate::config::{AuthProfile, CLOUD_PC_SCOPES, COMMON_AUTHORITY, UA_WINDOWS_11_WAM};
use crate::error::{AppError, Result};
use crate::models::{DeviceCodeResponse, TokenResponse};
use chrono::Utc;
use reqwest::Client;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;
use url::Url;
use uuid::Uuid;

/// Refresh this many seconds before the access token actually expires.
const EXPIRY_MARGIN_SECS: i64 = 120;
/// Client SKU reported to Entra ID (matches the desktop MSAL used by Windows App).
pub const MSAL_SKU: &str = "MSAL.Desktop";
pub const MSAL_VERSION: &str = "4.60.0.0";

/// OAuth client for one Entra application identity (client id + authority).
///
/// Tokens are persisted together with the client id and authority that issued them,
/// so refreshes always use the identity the user actually signed in with.
pub struct AuthManager {
    http: Client,
    authority: String,
    client_id: String,
    store: &'static TokenStore,
    /// Serialises refreshes so concurrent API calls don't burn the refresh token twice.
    refresh_lock: tokio::sync::Mutex<()>,
    /// Access tokens for secondary resources (e.g. the AVD gateway), keyed by scope.
    scoped_tokens: Mutex<HashMap<String, (String, i64)>>,
}

impl AuthManager {
    #[must_use]
    pub fn new(authority: &str, client_id: &str) -> Self {
        Self::with_store(authority, client_id, TokenStore::global())
    }

    #[must_use]
    pub fn with_store(authority: &str, client_id: &str, store: &'static TokenStore) -> Self {
        Self {
            http: Client::builder()
                .timeout(Duration::from_secs(30))
                .user_agent(UA_WINDOWS_11_WAM)
                .build()
                .unwrap_or_default(),
            authority: authority.trim_end_matches('/').to_string(),
            client_id: client_id.to_string(),
            store,
            refresh_lock: tokio::sync::Mutex::new(()),
            scoped_tokens: Mutex::new(HashMap::new()),
        }
    }

    #[must_use]
    pub fn for_profile(profile: AuthProfile) -> Self {
        Self::new(COMMON_AUTHORITY, profile.client_id())
    }

    #[must_use]
    pub fn client_id(&self) -> &str {
        &self.client_id
    }

    #[must_use]
    pub fn authority(&self) -> &str {
        &self.authority
    }

    #[must_use]
    pub fn is_signed_in(&self) -> bool {
        self.store.load().is_some()
    }

    /// Drops cached secondary-resource tokens (e.g. after switching accounts).
    pub fn forget_scoped_tokens(&self) {
        self.scoped_tokens
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
    }

    /// Signs the active account out (other remembered accounts stay available).
    pub fn sign_out(&self) -> std::io::Result<()> {
        self.forget_scoped_tokens();
        self.store.clear()
    }

    /// Remembered accounts, including the active one.
    #[must_use]
    pub fn accounts(&self) -> Vec<Account> {
        self.store.accounts()
    }

    /// Makes a remembered account active. Returns `false` if it is unknown.
    pub fn switch_account(&self, upn: &str) -> std::io::Result<bool> {
        let switched = self.store.switch_to(upn)?;
        if switched {
            self.forget_scoped_tokens();
        }
        Ok(switched)
    }

    /// The active account, if signed in.
    #[must_use]
    pub fn current_account(&self) -> Option<Account> {
        self.store.load().map(|t| Account::from_tokens(&t))
    }

    /// Authorization URL for the embedded browser (PKCE, query response mode).
    #[must_use]
    pub fn build_authorization_url(
        &self,
        pkce: &pkce::Pkce,
        redirect_uri: &str,
        os: Option<&str>,
        domain_hint: Option<&str>,
        login_hint: Option<&str>,
    ) -> String {
        let mut url = Url::parse(&format!("{}/oauth2/v2.0/authorize", self.authority))
            .expect("authority is a valid URL");
        {
            let mut q = url.query_pairs_mut();
            q.append_pair("client_id", &self.client_id)
                .append_pair("response_type", "code")
                .append_pair("redirect_uri", redirect_uri)
                .append_pair("response_mode", "query")
                .append_pair("scope", &CLOUD_PC_SCOPES.join(" "))
                .append_pair("code_challenge", &pkce.challenge)
                .append_pair("code_challenge_method", "S256")
                .append_pair("prompt", "select_account")
                .append_pair("instance_aware", "true")
                .append_pair("x-client-SKU", MSAL_SKU)
                .append_pair("x-client-Ver", MSAL_VERSION)
                .append_pair("x-client-CPU", "x64");
            if let Some(os) = os {
                q.append_pair("x-client-OS", os);
            }
            for (key, value) in [("domain_hint", domain_hint), ("login_hint", login_hint)] {
                if let Some(value) = value.map(str::trim).filter(|v| !v.is_empty()) {
                    q.append_pair(key, value);
                }
            }
            q.append_pair("client_info", "1")
                .append_pair("client-request-id", &Uuid::new_v4().to_string());
        }
        url.into()
    }

    /// Exchanges an authorization code from the embedded browser and stores the tokens.
    pub async fn exchange_authorization_code(
        &self,
        code: &str,
        verifier: &str,
        redirect_uri: &str,
        os: Option<&str>,
    ) -> Result<TokenResponse> {
        let scopes = CLOUD_PC_SCOPES.join(" ");
        let mut params = vec![
            ("client_id", self.client_id.as_str()),
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", redirect_uri),
            ("code_verifier", verifier),
            ("scope", scopes.as_str()),
            ("instance_aware", "true"),
            ("x-client-SKU", MSAL_SKU),
            ("x-client-Ver", MSAL_VERSION),
            ("x-client-CPU", "x64"),
        ];
        if let Some(os) = os {
            params.push(("x-client-OS", os));
        }
        let tokens = self
            .token_request(&self.authority, &params, "authorization code exchange")
            .await?;
        self.persist(tokens, &self.client_id, &self.authority)
    }

    /// Starts the OAuth device authorization flow.
    pub async fn initiate_device_code(&self) -> Result<DeviceCodeResponse> {
        let endpoint = format!("{}/oauth2/v2.0/devicecode", self.authority);
        let scopes = CLOUD_PC_SCOPES.join(" ");
        let res = self
            .http
            .post(&endpoint)
            .form(&[("client_id", self.client_id.as_str()), ("scope", &scopes)])
            .send()
            .await?;
        if !res.status().is_success() {
            return Err(auth_error("device code request", res).await);
        }
        Ok(res.json().await?)
    }

    /// Polls until the user approves the device code, it expires, or `keep_polling`
    /// returns false. Stores the tokens on success.
    pub async fn poll_device_token(
        &self,
        device_code: &str,
        interval_secs: u64,
        keep_polling: impl Fn() -> bool,
    ) -> Result<TokenResponse> {
        let endpoint = format!("{}/oauth2/v2.0/token", self.authority);
        let mut interval = Duration::from_secs(interval_secs.max(2));
        loop {
            tokio::time::sleep(interval).await;
            if !keep_polling() {
                return Err(AppError::Auth("device sign-in cancelled".into()));
            }
            let res = self
                .http
                .post(&endpoint)
                .form(&[
                    ("client_id", self.client_id.as_str()),
                    ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                    ("device_code", device_code),
                ])
                .send()
                .await?;
            if res.status().is_success() {
                let tokens = stamp(res.json().await?);
                return self.persist(tokens, &self.client_id, &self.authority);
            }
            let body: serde_json::Value = res.json().await.unwrap_or_default();
            match body["error"].as_str().unwrap_or_default() {
                "authorization_pending" => {}
                // RFC 8628 §3.5: increase the interval by 5 seconds.
                "slow_down" => interval += Duration::from_secs(5),
                "expired_token" => {
                    return Err(AppError::Auth(
                        "device code expired, please try again".into(),
                    ))
                }
                "access_denied" => {
                    return Err(AppError::Auth("authorization declined by user".into()))
                }
                other => {
                    let detail = body["error_description"].as_str().unwrap_or(other);
                    return Err(AppError::Auth(format!("device sign-in failed: {detail}")));
                }
            }
        }
    }

    /// Stores a token pasted by the user: either a token-endpoint JSON response or a
    /// bare (optionally `Bearer`-prefixed) access token.
    pub fn import_token(&self, raw: &str) -> Result<TokenResponse> {
        let raw = raw.trim();
        if raw.is_empty() {
            return Err(AppError::Auth("no token provided".into()));
        }
        let tokens = if raw.starts_with('{') {
            let mut tokens: TokenResponse = serde_json::from_str(raw)?;
            tokens
                .acquired_at
                .get_or_insert_with(|| Utc::now().timestamp());
            tokens
        } else {
            let token = raw
                .strip_prefix("Bearer ")
                .or_else(|| raw.strip_prefix("bearer "))
                .unwrap_or(raw)
                .trim();
            let expires_in = jwt::payload(token)
                .and_then(|p| p["exp"].as_i64())
                .map_or(3600, |exp| (exp - Utc::now().timestamp()).max(0) as u64);
            stamp(TokenResponse {
                token_type: "Bearer".into(),
                scope: None,
                expires_in,
                ext_expires_in: None,
                access_token: token.to_string(),
                refresh_token: None,
                id_token: None,
                acquired_at: None,
                client_id: None,
                authority: None,
            })
        };
        self.store.save(&tokens)?;
        Ok(tokens)
    }

    /// Returns a valid Windows 365 access token, refreshing it when close to expiry.
    pub async fn get_valid_token(&self) -> Result<String> {
        let saved = self.store.load().ok_or(AppError::TokenExpired)?;
        if !is_expiring(&saved) {
            return Ok(saved.access_token);
        }
        let _guard = self.refresh_lock.lock().await;
        // Another task may have refreshed while we waited.
        let saved = self.store.load().ok_or(AppError::TokenExpired)?;
        if !is_expiring(&saved) {
            return Ok(saved.access_token);
        }
        let refresh_token = saved.refresh_token.clone().ok_or(AppError::TokenExpired)?;
        let (client_id, authority) = self.identity_of(&saved);
        let scopes = CLOUD_PC_SCOPES.join(" ");
        let refreshed = self
            .token_request(
                &authority,
                &[
                    ("client_id", client_id.as_str()),
                    ("grant_type", "refresh_token"),
                    ("refresh_token", refresh_token.as_str()),
                    ("scope", scopes.as_str()),
                ],
                "token refresh",
            )
            .await?;
        let merged = TokenResponse {
            refresh_token: refreshed.refresh_token.clone().or(saved.refresh_token),
            id_token: refreshed.id_token.clone().or(saved.id_token),
            ..refreshed
        };
        Ok(self.persist(merged, &client_id, &authority)?.access_token)
    }

    /// Access token for another resource (e.g. [`crate::config::AVD_RESOURCE_SCOPE`]),
    /// obtained from the stored refresh token and cached in memory until near expiry.
    pub async fn token_for_scope(&self, scope: &str) -> Result<String> {
        let now = Utc::now().timestamp();
        if let Some((token, expires_at)) = self
            .scoped_tokens
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(scope)
        {
            if now < expires_at - EXPIRY_MARGIN_SECS {
                return Ok(token.clone());
            }
        }

        let saved = self.store.load().ok_or(AppError::TokenExpired)?;
        let refresh_token = saved.refresh_token.as_deref().ok_or_else(|| {
            AppError::Auth("signed in without a refresh token; please sign in again".into())
        })?;
        let (client_id, authority) = self.identity_of(&saved);
        let tokens = self
            .token_request(
                &authority,
                &[
                    ("client_id", client_id.as_str()),
                    ("grant_type", "refresh_token"),
                    ("refresh_token", refresh_token),
                    ("scope", scope),
                ],
                "scoped token request",
            )
            .await?;
        let expires_at = tokens.acquired_at.unwrap_or(now) + tokens.expires_in as i64;
        self.scoped_tokens
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(scope.to_string(), (tokens.access_token.clone(), expires_at));
        Ok(tokens.access_token)
    }

    fn identity_of(&self, tokens: &TokenResponse) -> (String, String) {
        (
            tokens
                .client_id
                .clone()
                .unwrap_or_else(|| self.client_id.clone()),
            tokens
                .authority
                .clone()
                .unwrap_or_else(|| self.authority.clone()),
        )
    }

    fn persist(
        &self,
        mut tokens: TokenResponse,
        client_id: &str,
        authority: &str,
    ) -> Result<TokenResponse> {
        tokens.client_id = Some(client_id.to_string());
        tokens.authority = Some(authority.to_string());
        self.store.save(&tokens)?;
        Ok(tokens)
    }

    async fn token_request(
        &self,
        authority: &str,
        params: &[(&str, &str)],
        what: &str,
    ) -> Result<TokenResponse> {
        let endpoint = format!("{authority}/oauth2/v2.0/token");
        let res = self.http.post(&endpoint).form(params).send().await?;
        if !res.status().is_success() {
            return Err(auth_error(what, res).await);
        }
        Ok(stamp(res.json().await?))
    }
}

fn stamp(mut tokens: TokenResponse) -> TokenResponse {
    tokens.acquired_at = Some(Utc::now().timestamp());
    tokens
}

fn is_expiring(tokens: &TokenResponse) -> bool {
    let expires_at = tokens.acquired_at.unwrap_or(0) + tokens.expires_in as i64;
    Utc::now().timestamp() >= expires_at - EXPIRY_MARGIN_SECS
}

/// Builds an error from an Entra error response, preferring `error_description`.
async fn auth_error(what: &str, res: reqwest::Response) -> AppError {
    let status = res.status();
    let body = res.text().await.unwrap_or_default();
    let detail = serde_json::from_str::<serde_json::Value>(&body)
        .ok()
        .and_then(|v| v["error_description"].as_str().map(str::to_string))
        .unwrap_or(body);
    AppError::Auth(format!("{what} failed [{status}]: {detail}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{WINDOWS_APP_CLIENT_ID, WINDOWS_APP_REDIRECT_URI};

    fn leaked_store() -> &'static TokenStore {
        let path = std::env::temp_dir()
            .join(format!("wal-auth-{}", Uuid::new_v4()))
            .join("tokens.json");
        Box::leak(Box::new(TokenStore::new(path)))
    }

    fn query(url: &str) -> HashMap<String, String> {
        Url::parse(url)
            .unwrap()
            .query_pairs()
            .into_owned()
            .collect()
    }

    #[test]
    fn authorization_url_contains_pkce_and_client_metadata() {
        let auth = AuthManager::with_store(
            "https://login.microsoftonline.com/organizations/",
            WINDOWS_APP_CLIENT_ID,
            leaked_store(),
        );
        let pkce = pkce::Pkce::new();
        let url = auth.build_authorization_url(
            &pkce,
            WINDOWS_APP_REDIRECT_URI,
            Some("Microsoft Windows NT 10.0.22631.0"),
            Some("organizations"),
            Some("  "),
        );
        let parsed = Url::parse(&url).unwrap();
        assert_eq!(parsed.path(), "/organizations/oauth2/v2.0/authorize");
        let q = query(&url);
        assert_eq!(q["client_id"], WINDOWS_APP_CLIENT_ID);
        assert_eq!(q["redirect_uri"], WINDOWS_APP_REDIRECT_URI);
        assert_eq!(q["code_challenge"], pkce.challenge);
        assert_eq!(q["code_challenge_method"], "S256");
        assert_eq!(q["domain_hint"], "organizations");
        assert_eq!(q["x-client-OS"], "Microsoft Windows NT 10.0.22631.0");
        assert!(!q.contains_key("login_hint"));
    }

    #[test]
    fn cloud_pc_scopes_exclude_admin_scopes() {
        for scope in CLOUD_PC_SCOPES {
            assert!(!scope.contains("CloudPC.Read.All"));
            assert!(!scope.contains("CloudPC.ReadWrite"));
        }
    }

    #[test]
    fn imports_bare_and_json_tokens() {
        let auth = AuthManager::with_store(COMMON_AUTHORITY, WINDOWS_APP_CLIENT_ID, leaked_store());
        let exp = Utc::now().timestamp() + 600;
        let jwt = jwt::make_unsigned(&serde_json::json!({ "exp": exp }));
        let t = auth.import_token(&format!("Bearer {jwt}")).unwrap();
        assert_eq!(t.access_token, jwt);
        assert!((598..=600).contains(&t.expires_in));
        assert!(auth.is_signed_in());

        let t = auth
            .import_token(r#"{"token_type":"Bearer","expires_in":10,"access_token":"x"}"#)
            .unwrap();
        assert_eq!(t.access_token, "x");
        assert!(t.acquired_at.is_some());
        assert!(auth.import_token("  ").is_err());

        auth.sign_out().unwrap();
        assert!(!auth.is_signed_in());
    }

    #[test]
    fn expiry_margin() {
        let mut t = TokenResponse {
            token_type: "Bearer".into(),
            scope: None,
            expires_in: 3600,
            ext_expires_in: None,
            access_token: String::new(),
            refresh_token: None,
            id_token: None,
            acquired_at: Some(Utc::now().timestamp()),
            client_id: None,
            authority: None,
        };
        assert!(!is_expiring(&t));
        t.expires_in = 60;
        assert!(is_expiring(&t));
    }
}
