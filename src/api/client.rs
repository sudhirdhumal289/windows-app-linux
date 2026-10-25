//! Microsoft Graph and Windows 365 API client for Cloud PCs.

use crate::auth::AuthManager;
use crate::config::{CLIENT_VERSION, GRAPH_BASE_URL, SOURCE_CLIENT};
use crate::error::{AppError, Result};
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderValue};
use reqwest::{Client, Method, Response};
use serde::de::DeserializeOwned;
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

#[derive(Clone)]
pub struct GraphApiClient {
    http: Client,
    auth: Arc<AuthManager>,
    base_url: String,
    data_boundary: String,
}

impl GraphApiClient {
    #[must_use]
    pub fn new(auth: Arc<AuthManager>, data_boundary: &str) -> Self {
        Self {
            http: Client::builder()
                .timeout(Duration::from_secs(30))
                .build()
                .unwrap_or_default(),
            auth,
            base_url: GRAPH_BASE_URL.to_string(),
            data_boundary: data_boundary.to_string(),
        }
    }

    /// Builds the standard header set extracted from the Windows App MSIX reverse-engineering
    fn build_headers(&self, token: &str) -> Result<HeaderMap> {
        let mut headers = HeaderMap::new();
        headers.insert(
            CONTENT_TYPE,
            HeaderValue::from_static("application/json;charset=UTF-8"),
        );
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {token}"))
                .map_err(|e| AppError::Config(e.to_string()))?,
        );
        headers.insert(
            "client-request-id",
            HeaderValue::from_str(&Uuid::new_v4().to_string())
                .map_err(|e| AppError::Config(e.to_string()))?,
        );
        headers.insert(
            "cpc-data-boundary",
            HeaderValue::from_str(&self.data_boundary)
                .map_err(|e| AppError::Config(e.to_string()))?,
        );
        headers.insert("source-client", HeaderValue::from_static(SOURCE_CLIENT));
        headers.insert("client-version", HeaderValue::from_static(CLIENT_VERSION));
        headers.insert("x-ms-user-agent", HeaderValue::from_static("win365.nxt/2.0.1484.0"));
        Ok(headers)
    }

    /// Sends an authorized request returning the Response regardless of HTTP status code
    pub async fn send_request(
        &self,
        method: Method,
        path: &str,
        body: Option<serde_json::Value>,
    ) -> Result<Response> {
        let token = self.auth.get_valid_token().await?;
        let headers = self.build_headers(&token)?;
        let url = if path.starts_with("http://") || path.starts_with("https://") {
            path.to_string()
        } else {
            format!(
                "{}/{}",
                self.base_url.trim_end_matches('/'),
                path.trim_start_matches('/')
            )
        };

        let mut req = self.http.request(method, &url).headers(headers);
        if let Some(b) = body {
            req = req.json(&b);
        }

        let res = req.send().await?;
        Ok(res)
    }

    /// Executes an authorized request to Microsoft Graph or Windows 365, checking for success
    pub async fn execute_request(
        &self,
        method: Method,
        path: &str,
        body: Option<serde_json::Value>,
    ) -> Result<Response> {
        let res = self.send_request(method, path, body).await?;
        if !res.status().is_success() {
            let status = res.status().as_u16();
            let msg = res.text().await.unwrap_or_default();
            return Err(AppError::GraphApi {
                status,
                message: msg,
            });
        }

        Ok(res)
    }

    /// Fetches and deserializes a JSON payload from Graph API or Windows 365
    pub async fn get_json<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        let res = self.execute_request(Method::GET, path, None).await?;
        let data = res.json::<T>().await?;
        Ok(data)
    }

    /// Posts a JSON payload and returns the response
    pub async fn post_action(&self, path: &str, body: Option<serde_json::Value>) -> Result<()> {
        self.execute_request(Method::POST, path, body).await?;
        Ok(())
    }
}
