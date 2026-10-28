//! Authorized HTTP client for Microsoft Graph and the Windows 365 end-user API.

use crate::auth::AuthManager;
use crate::config::{
    CloudEnvironment, CLIENT_VERSION, GRAPH_DEFAULT_SCOPE, SOURCE_CLIENT, W365_USER_AGENT,
};
use crate::error::{AppError, Result};
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_TYPE};
use reqwest::{Client, Method, Response};
use serde::de::DeserializeOwned;
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

#[derive(Clone)]
pub struct GraphApiClient {
    http: Client,
    auth: Arc<AuthManager>,
    data_boundary: String,
    environment: CloudEnvironment,
}

impl GraphApiClient {
    #[must_use]
    pub fn new(auth: Arc<AuthManager>, data_boundary: &str, environment: CloudEnvironment) -> Self {
        Self {
            http: Client::builder()
                .timeout(Duration::from_secs(30))
                .build()
                .unwrap_or_default(),
            auth,
            data_boundary: data_boundary.to_string(),
            environment,
        }
    }

    #[must_use]
    pub fn auth(&self) -> &Arc<AuthManager> {
        &self.auth
    }

    #[must_use]
    pub fn environment(&self) -> CloudEnvironment {
        self.environment
    }

    /// Shared HTTP connection pool, for requests that use a different token.
    #[must_use]
    pub fn http(&self) -> &Client {
        &self.http
    }

    /// Headers the Windows App sends to the Windows 365 service.
    fn build_headers(&self, token: &str) -> Result<HeaderMap> {
        let header =
            |v: &str| HeaderValue::from_str(v).map_err(|e| AppError::Config(e.to_string()));
        let mut headers = HeaderMap::new();
        headers.insert(
            CONTENT_TYPE,
            HeaderValue::from_static("application/json;charset=UTF-8"),
        );
        let mut auth_value = header(&format!("Bearer {token}"))?;
        auth_value.set_sensitive(true);
        headers.insert(AUTHORIZATION, auth_value);
        headers.insert("client-request-id", header(&Uuid::new_v4().to_string())?);
        headers.insert("cpc-data-boundary", header(&self.data_boundary)?);
        headers.insert("source-client", HeaderValue::from_static(SOURCE_CLIENT));
        headers.insert("client-version", HeaderValue::from_static(CLIENT_VERSION));
        headers.insert("x-ms-user-agent", HeaderValue::from_static(W365_USER_AGENT));
        Ok(headers)
    }

    #[must_use]
    pub fn url_for(&self, path: &str) -> String {
        if path.starts_with("https://") {
            path.to_string()
        } else {
            format!(
                "{}/{}",
                self.environment.graph_base_url().trim_end_matches('/'),
                path.trim_start_matches('/')
            )
        }
    }

    #[must_use]
    pub fn is_graph_url(&self, url: &str) -> bool {
        url::Url::parse(url)
            .ok()
            .and_then(|u| {
                u.host_str().map(|h| {
                    h.eq_ignore_ascii_case("graph.microsoft.com")
                        || h.eq_ignore_ascii_case("graph.microsoft.us")
                        || h.contains("chinacloudapi.cn")
                })
            })
            .unwrap_or(false)
    }

    /// Sends an authorized request and returns the response whatever its status.
    pub async fn send_request(
        &self,
        method: Method,
        path: &str,
        body: Option<serde_json::Value>,
    ) -> Result<Response> {
        let url = self.url_for(path);
        // The sign-in token is for the Windows 365 service; Graph needs its own audience.
        let token = if self.is_graph_url(&url) {
            self.auth.token_for_scope(GRAPH_DEFAULT_SCOPE).await?
        } else {
            self.auth.get_valid_token().await?
        };
        let mut req = self
            .http
            .request(method, url)
            .headers(self.build_headers(&token)?);
        if let Some(body) = body {
            req = req.json(&body);
        }
        Ok(req.send().await?)
    }

    /// Sends an authorized request, mapping non-success statuses to [`AppError::GraphApi`].
    pub async fn execute_request(
        &self,
        method: Method,
        path: &str,
        body: Option<serde_json::Value>,
    ) -> Result<Response> {
        let res = self.send_request(method, path, body).await?;
        if res.status().is_success() {
            Ok(res)
        } else {
            Err(AppError::GraphApi {
                status: res.status().as_u16(),
                message: res.text().await.unwrap_or_default(),
            })
        }
    }

    pub async fn get_json<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        Ok(self
            .execute_request(Method::GET, path, None)
            .await?
            .json()
            .await?)
    }

    pub async fn post_action(&self, path: &str, body: Option<serde_json::Value>) -> Result<()> {
        self.execute_request(Method::POST, path, body).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_relative_and_absolute_urls() {
        let auth = Arc::new(AuthManager::new(
            "https://login.microsoftonline.com/common",
            "dummy-client",
        ));
        let client = GraphApiClient::new(auth, "commercial", CloudEnvironment::Commercial);
        assert_eq!(
            client.url_for("/me/cloudPCs"),
            "https://graph.microsoft.com/beta/me/cloudPCs"
        );
        assert_eq!(
            client.url_for("https://windows365.microsoft.com/u/api"),
            "https://windows365.microsoft.com/u/api"
        );
    }

    #[test]
    fn graph_urls_are_detected() {
        let auth = Arc::new(AuthManager::new(
            "https://login.microsoftonline.com/common",
            "dummy-client",
        ));
        let client = GraphApiClient::new(auth, "commercial", CloudEnvironment::Commercial);
        assert!(client.is_graph_url(&client.url_for("me/cloudPCs")));
        assert!(!client.is_graph_url("https://windows365.microsoft.com/u/api/v1/cloudPCs"));
        assert!(!client.is_graph_url("https://graph.microsoft.com.evil.example/x"));
    }
}
