//! Azure Virtual Desktop (AVD) Feed Discovery client.

use crate::auth::AuthManager;
use crate::error::{AppError, Result};
use reqwest::Client;
use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderValue};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

pub const AVD_FEED_DISCOVERY_URL: &str = "https://client.wvd.microsoft.com/api/arm/feeddiscovery";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AvdWorkspace {
    #[serde(rename = "id")]
    pub id: String,
    #[serde(rename = "name")]
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
}

pub struct AvdService {
    http: Client,
    auth: Arc<AuthManager>,
}

impl AvdService {
    #[must_use]
    pub fn new(auth: Arc<AuthManager>) -> Self {
        Self {
            http: Client::new(),
            auth,
        }
    }

    /// Discovers user workspaces from Azure Virtual Desktop feed
    pub async fn discover_workspaces(&self) -> Result<Vec<AvdWorkspace>> {
        let token = self.auth.get_valid_token().await?;
        let mut headers = HeaderMap::new();
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {token}"))
                .map_err(|e| AppError::Config(e.to_string()))?,
        );

        let res = self
            .http
            .get(AVD_FEED_DISCOVERY_URL)
            .headers(headers)
            .send()
            .await?;

        if !res.status().is_success() {
            let status = res.status();
            let body = res.text().await.unwrap_or_default();
            return Err(AppError::GraphApi {
                status: status.as_u16(),
                message: format!("AVD discovery failed: {body}"),
            });
        }

        let workspaces: Vec<AvdWorkspace> = res.json().await.unwrap_or_default();
        Ok(workspaces)
    }
}
