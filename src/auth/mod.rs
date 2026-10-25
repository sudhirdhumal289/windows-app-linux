//! Microsoft Entra ID (Azure AD) authentication using OAuth 2.0 PKCE and Device Code flows.

pub mod pkap;
pub mod pkce;

use crate::config::{AppConfig, CLOUD_PC_SCOPES};
use crate::error::{AppError, Result};
use crate::models::{DeviceCodeResponse, TokenResponse};
use chrono::Utc;
use reqwest::Client;
use std::collections::HashMap;
use std::time::Duration;
use url::Url;
use uuid::Uuid;

const DEFAULT_HTTP_USER_AGENT: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64; WebView/3.0) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/70.0.3538.102 Safari/537.36 Edge/18.22631";

#[derive(Debug, Clone)]
pub struct AuthManager {
    client: Client,
    authority: String,
    client_id: String,
}

impl AuthManager {
    #[must_use]
    pub fn new(authority: &str, client_id: &str) -> Self {
        Self {
            client: Client::builder()
                .timeout(Duration::from_secs(30))
                .user_agent(DEFAULT_HTTP_USER_AGENT)
                .build()
                .unwrap_or_default(),
            authority: authority.trim_end_matches('/').to_string(),
            client_id: client_id.to_string(),
        }
    }

    #[must_use]
    pub fn client_id(&self) -> &str {
        &self.client_id
    }

    #[must_use]
    pub fn authority(&self) -> &str {
        &self.authority
    }

    /// Constructs the OAuth2 authorization URL with PKCE for embedded WebView sign-in.
    #[must_use]
    pub fn build_authorization_url(
        &self,
        pkce: &pkce::Pkce,
        redirect_uri: &str,
        os: Option<&str>,
        domain_hint: Option<&str>,
        login_hint: Option<&str>,
    ) -> String {
        let endpoint = format!("{}/oauth2/v2.0/authorize", self.authority);
        let scopes = CLOUD_PC_SCOPES.join(" ");

        let mut url = Url::parse(&endpoint).expect("valid auth endpoint");
        url.query_pairs_mut()
            .append_pair("client_id", &self.client_id)
            .append_pair("response_type", "code")
            .append_pair("redirect_uri", redirect_uri)
            .append_pair("response_mode", "query")
            .append_pair("scope", &scopes)
            .append_pair("code_challenge", &pkce.challenge)
            .append_pair("code_challenge_method", "S256")
            .append_pair("prompt", "select_account")
            .append_pair("instance_aware", "true")
            .append_pair("x-client-SKU", "MSAL.Desktop")
            .append_pair("x-client-Ver", "4.60.0.0")
            .append_pair("x-client-CPU", "x64");

        if let Some(os_name) = os {
            url.query_pairs_mut().append_pair("x-client-OS", os_name);
        }

        if let Some(dh) = domain_hint {
            let trimmed = dh.trim();
            if !trimmed.is_empty() {
                url.query_pairs_mut().append_pair("domain_hint", trimmed);
            }
        }

        if let Some(lh) = login_hint {
            let trimmed = lh.trim();
            if !trimmed.is_empty() {
                url.query_pairs_mut().append_pair("login_hint", trimmed);
            }
        }

        url.query_pairs_mut()
            .append_pair("client_info", "1")
            .append_pair("client-request-id", &Uuid::new_v4().to_string());

        url.to_string()
    }

    /// Exchanges the authorization code received by the WebView for OAuth tokens.
    pub async fn exchange_authorization_code(
        &self,
        code: &str,
        verifier: &str,
        redirect_uri: &str,
        os: Option<&str>,
    ) -> Result<TokenResponse> {
        let endpoint = format!("{}/oauth2/v2.0/token", self.authority);
        let scopes = CLOUD_PC_SCOPES.join(" ");

        let mut params = HashMap::new();
        params.insert("client_id", self.client_id.as_str());
        params.insert("grant_type", "authorization_code");
        params.insert("code", code);
        params.insert("redirect_uri", redirect_uri);
        params.insert("code_verifier", verifier);
        params.insert("scope", scopes.as_str());
        params.insert("instance_aware", "true");
        params.insert("x-client-SKU", "MSAL.Desktop");
        params.insert("x-client-Ver", "4.60.0.0");
        params.insert("x-client-CPU", "x64");

        if let Some(os_name) = os {
            params.insert("x-client-OS", os_name);
        }

        let res = self.client.post(&endpoint).form(&params).send().await?;

        if !res.status().is_success() {
            let status = res.status();
            let body = res.text().await.unwrap_or_default();
            return Err(AppError::Auth(format!(
                "authorization code exchange failed [{status}]: {body}"
            )));
        }

        let mut tokens: TokenResponse = res.json().await?;
        tokens.acquired_at = Some(Utc::now().timestamp());
        let _ = AppConfig::save_tokens(&tokens);
        Ok(tokens)
    }

    /// Initiates OAuth 2.0 Device Authorization flow (fallback).
    pub async fn initiate_device_code(&self) -> Result<DeviceCodeResponse> {
        let endpoint = format!("{}/oauth2/v2.0/devicecode", self.authority);
        let scopes = CLOUD_PC_SCOPES.join(" ");

        let mut params = HashMap::new();
        params.insert("client_id", self.client_id.as_str());
        params.insert("scope", scopes.as_str());

        let res = self.client.post(&endpoint).form(&params).send().await?;

        if !res.status().is_success() {
            let status = res.status();
            let body = res.text().await.unwrap_or_default();
            return Err(AppError::Auth(format!(
                "device code initiation failed [{status}]: {body}"
            )));
        }

        let device_code: DeviceCodeResponse = res.json().await?;
        Ok(device_code)
    }

    /// Polls the token endpoint until the user completes the device authorization.
    pub async fn poll_device_token(
        &self,
        device_code: &str,
        interval_secs: u64,
    ) -> Result<TokenResponse> {
        let endpoint = format!("{}/oauth2/v2.0/token", self.authority);
        let interval = Duration::from_secs(interval_secs.max(2));

        loop {
            tokio::time::sleep(interval).await;

            let mut params = HashMap::new();
            params.insert("client_id", self.client_id.as_str());
            params.insert("grant_type", "urn:ietf:params:oauth:grant-type:device_code");
            params.insert("device_code", device_code);

            let res = self.client.post(&endpoint).form(&params).send().await?;

            if res.status().is_success() {
                let mut tokens: TokenResponse = res.json().await?;
                tokens.acquired_at = Some(Utc::now().timestamp());
                let _ = AppConfig::save_tokens(&tokens);
                return Ok(tokens);
            }

            let body: serde_json::Value = res.json().await.unwrap_or_default();
            let error_code = body["error"].as_str().unwrap_or_default();

            match error_code {
                "authorization_pending" => {
                    continue;
                }
                "slow_down" => {
                    tokio::time::sleep(Duration::from_secs(5)).await;
                    continue;
                }
                "expired_token" => {
                    return Err(AppError::Auth(
                        "device code expired, please try again".to_string(),
                    ));
                }
                "access_denied" => {
                    return Err(AppError::Auth("authorization declined by user".to_string()));
                }
                other => {
                    return Err(AppError::Auth(format!("polling failed: {other}")));
                }
            }
        }
    }

    /// Refreshes expired access tokens using the refresh token.
    pub async fn refresh_access_token(&self, refresh_token: &str) -> Result<TokenResponse> {
        let endpoint = format!("{}/oauth2/v2.0/token", self.authority);
        let scopes = CLOUD_PC_SCOPES.join(" ");

        let mut params = HashMap::new();
        params.insert("client_id", self.client_id.as_str());
        params.insert("grant_type", "refresh_token");
        params.insert("refresh_token", refresh_token);
        params.insert("scope", scopes.as_str());

        let res = self.client.post(&endpoint).form(&params).send().await?;

        if !res.status().is_success() {
            let status = res.status();
            let body = res.text().await.unwrap_or_default();
            return Err(AppError::Auth(format!(
                "token refresh failed [{status}]: {body}"
            )));
        }

        let mut tokens: TokenResponse = res.json().await?;
        tokens.acquired_at = Some(Utc::now().timestamp());
        let _ = AppConfig::save_tokens(&tokens);
        Ok(tokens)
    }

    /// Exchanges a refresh token for an access token with a specific target scope (e.g. AVD scope).
    pub async fn refresh_access_token_for_scope(
        &self,
        refresh_token: &str,
        scope: &str,
    ) -> Result<TokenResponse> {
        let endpoint = format!("{}/oauth2/v2.0/token", self.authority);
        let mut params = HashMap::new();
        params.insert("client_id", self.client_id.as_str());
        params.insert("grant_type", "refresh_token");
        params.insert("refresh_token", refresh_token);
        params.insert("scope", scope);

        let res = self.client.post(&endpoint).form(&params).send().await?;

        if !res.status().is_success() {
            let status = res.status();
            let body = res.text().await.unwrap_or_default();
            return Err(AppError::Auth(format!(
                "token exchange for scope '{scope}' failed [{status}]: {body}"
            )));
        }

        let mut tokens: TokenResponse = res.json().await?;
        tokens.acquired_at = Some(Utc::now().timestamp());
        Ok(tokens)
    }

    /// Retrieves an active access token, refreshing if necessary.
    pub async fn get_valid_token(&self) -> Result<String> {
        let saved = AppConfig::load_tokens().ok_or(AppError::TokenExpired)?;
        let now = Utc::now().timestamp();
        let acquired = saved.acquired_at.unwrap_or(0);
        let expires_in = saved.expires_in as i64;

        if now >= (acquired + expires_in - 120) {
            if let Some(refresh_token) = &saved.refresh_token {
                let refreshed = self.refresh_access_token(refresh_token).await?;
                return Ok(refreshed.access_token);
            }
            return Err(AppError::TokenExpired);
        }

        Ok(saved.access_token)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ORGANIZATIONS_AUTHORITY, WINDOWS_APP_CLIENT_ID, WINDOWS_APP_REDIRECT_URI};

    #[test]
    fn test_build_authorization_url_organizations() {
        let auth = AuthManager::new(ORGANIZATIONS_AUTHORITY, WINDOWS_APP_CLIENT_ID);
        let pkce = pkce::Pkce::new();
        let url_str = auth.build_authorization_url(
            &pkce,
            WINDOWS_APP_REDIRECT_URI,
            Some("Microsoft Windows NT 10.0.22631.0"),
            Some("organizations"),
            None,
        );

        let parsed = Url::parse(&url_str).expect("valid url");
        assert_eq!(parsed.scheme(), "https");
        assert_eq!(parsed.host_str(), Some("login.microsoftonline.com"));
        assert_eq!(parsed.path(), "/organizations/oauth2/v2.0/authorize");

        let q: HashMap<_, _> = parsed.query_pairs().into_owned().collect();
        assert_eq!(q.get("client_id"), Some(&WINDOWS_APP_CLIENT_ID.to_string()));
        assert_eq!(
            q.get("redirect_uri"),
            Some(&WINDOWS_APP_REDIRECT_URI.to_string())
        );
        assert_eq!(q.get("domain_hint"), Some(&"organizations".to_string()));
        assert_eq!(q.get("instance_aware"), Some(&"true".to_string()));
        assert_eq!(q.get("code_challenge_method"), Some(&"S256".to_string()));
        assert_eq!(
            q.get("x-client-OS"),
            Some(&"Microsoft Windows NT 10.0.22631.0".to_string())
        );
    }

    #[test]
    fn test_build_authorization_url_custom_tenant() {
        let tenant_authority = "https://login.microsoftonline.com/contoso.com";
        let auth = AuthManager::new(tenant_authority, WINDOWS_APP_CLIENT_ID);
        let pkce = pkce::Pkce::new();
        let url_str = auth.build_authorization_url(
            &pkce,
            WINDOWS_APP_REDIRECT_URI,
            None,
            Some("contoso.com"),
            Some("user@contoso.com"),
        );

        let parsed = Url::parse(&url_str).expect("valid url");
        assert_eq!(parsed.path(), "/contoso.com/oauth2/v2.0/authorize");

        let q: HashMap<_, _> = parsed.query_pairs().into_owned().collect();
        assert_eq!(q.get("domain_hint"), Some(&"contoso.com".to_string()));
        assert_eq!(q.get("login_hint"), Some(&"user@contoso.com".to_string()));
        assert_eq!(q.get("instance_aware"), Some(&"true".to_string()));
    }

    #[test]
    fn test_cloud_pc_scopes_no_admin_scope() {
        for scope in CLOUD_PC_SCOPES {
            assert!(
                !scope.contains("CloudPC.Read.All"),
                "CloudPC.Read.All is an admin scope that triggers Intune compliance block"
            );
            assert!(
                !scope.contains("CloudPC.ReadWrite"),
                "CloudPC.ReadWrite must not be present in default scopes"
            );
        }
    }
}
