//! Cloud PC service endpoints and lifecycle operations.

use crate::api::client::GraphApiClient;
use crate::error::Result;
use crate::models::{
    CloudPc, ODataResponse, RdpConnectionConfig, W365CloudPcItem, Windows365V2Response,
};
use log::{debug, warn};
use serde_json::json;

pub struct CloudPcService {
    client: GraphApiClient,
}

impl CloudPcService {
    #[must_use]
    pub fn new(client: GraphApiClient) -> Self {
        Self { client }
    }

    /// Fetches all Cloud PCs assigned to the authenticated user.
    /// Tries the official Windows 365 End User API (v2 & v1) first, falling back to Microsoft Graph.
    pub async fn list_user_cloud_pcs(&self) -> Result<Vec<CloudPc>> {
        debug!("Fetching user Cloud PCs from Windows 365 End User API (v2)...");

        // Try Windows 365 End User v2 API (official Windows App endpoint)
        let w365_v2_url = "https://windows365.microsoft.com/u/api/v2/cloudPCs?extend=IsDefault&licenseCategories=Dedicated,Flex,FrontlineShared";
        match self
            .client
            .send_request(reqwest::Method::GET, w365_v2_url, None)
            .await
        {
            Ok(res) if res.status().is_success() => {
                match res.json::<Windows365V2Response>().await {
                    Ok(v2_body) => {
                        let pcs = v2_body.into_cloud_pcs();
                        if !pcs.is_empty() {
                            debug!(
                                "Successfully retrieved {} Cloud PCs from Windows 365 v2 API",
                                pcs.len()
                            );
                            return Ok(pcs);
                        }
                        debug!("Windows 365 v2 API returned 0 Cloud PCs, checking v1...");
                    }
                    Err(e) => {
                        warn!("Failed to deserialize Windows 365 v2 response: {e}");
                    }
                }
            }
            Ok(res) => {
                warn!(
                    "Windows 365 v2 API returned HTTP {}, trying v1",
                    res.status()
                );
            }
            Err(e) => {
                warn!("Windows 365 v2 request failed: {e}");
            }
        }

        // Try Windows 365 End User v1 API
        debug!("Trying Windows 365 End User API (v1)...");
        let w365_v1_url = "https://windows365.microsoft.com/u/api/v1/cloudPCs?extend=IsDefault";
        match self
            .client
            .send_request(reqwest::Method::GET, w365_v1_url, None)
            .await
        {
            Ok(res) if res.status().is_success() => {
                match res.json::<Vec<W365CloudPcItem>>().await {
                    Ok(items) => {
                        let pcs: Vec<CloudPc> = items.into_iter().map(CloudPc::from).collect();
                        if !pcs.is_empty() {
                            debug!(
                                "Successfully retrieved {} Cloud PCs from Windows 365 v1 API",
                                pcs.len()
                            );
                            return Ok(pcs);
                        }
                    }
                    Err(e) => {
                        warn!("Failed to deserialize Windows 365 v1 response: {e}");
                    }
                }
            }
            Ok(res) => {
                warn!("Windows 365 v1 API returned HTTP {}", res.status());
            }
            Err(e) => {
                warn!("Windows 365 v1 request failed: {e}");
            }
        }

        // Fallback to Microsoft Graph only if Windows 365 API returned nothing
        debug!("Falling back to Microsoft Graph /me/cloudPCs...");
        match self.client.get_json::<ODataResponse<CloudPc>>("me/cloudPCs").await {
            Ok(response) => Ok(response.value),
            Err(crate::error::AppError::GraphApi { status, ref message }) => {
                // If 404 (ResourceNotFound), 403 (Forbidden/Unlicensed), or 400 (Personal Microsoft account),
                // the account simply has no Cloud PC license or assigned desktops. Return empty list cleanly.
                if status == 404
                    || status == 403
                    || status == 400
                    || message.contains("ResourceNotFound")
                    || message.contains("License")
                {
                    debug!("Account has no Cloud PC resources (HTTP {status}). Returning empty desktop list.");
                    Ok(Vec::new())
                } else {
                    Err(crate::error::AppError::GraphApi {
                        status,
                        message: message.clone(),
                    })
                }
            }
            Err(e) => Err(e),
        }
    }

    /// Retrieves details for a specific Cloud PC.
    pub async fn get_cloud_pc_by_id(&self, id: &str) -> Result<CloudPc> {
        // Try Windows 365 End User API v1 first
        let w365_url = format!("https://windows365.microsoft.com/u/api/v1/cloudPCs/{id}");
        if let Ok(res) = self
            .client
            .send_request(reqwest::Method::GET, &w365_url, None)
            .await
        {
            if res.status().is_success() {
                if let Ok(item) = res.json::<W365CloudPcItem>().await {
                    return Ok(CloudPc::from(item));
                }
            }
        }

        let path = format!("me/cloudPCs/{id}");
        self.client.get_json(&path).await
    }

    /// Triggers a reboot of the specified Cloud PC.
    pub async fn restart_cloud_pc(&self, id: &str) -> Result<()> {
        debug!("Initiating restart for Cloud PC [{id}]...");

        // Try Windows 365 reboot endpoint
        let w365_url = format!("https://windows365.microsoft.com/u/api/v1/cloudPCs/{id}/reboot");
        if let Ok(res) = self
            .client
            .send_request(reqwest::Method::POST, &w365_url, Some(json!({})))
            .await
        {
            if res.status().is_success() {
                debug!("Windows 365 reboot command submitted successfully");
                return Ok(());
            }
        }

        let path = format!("me/cloudPCs/{id}/restart");
        self.client.post_action(&path, None).await
    }

    /// Triggers automated troubleshooting and health diagnostics.

    /// Renames the specified Cloud PC.
    pub async fn rename_cloud_pc(&self, id: &str, new_name: &str) -> Result<()> {
        debug!("Renaming Cloud PC [{id}] to [{new_name}]...");
        let path = format!("me/cloudPCs/{id}/rename");
        let body = serde_json::json!({ "displayName": new_name });
        self.client.post_action(&path, Some(body)).await
    }

    pub async fn troubleshoot_cloud_pc(&self, id: &str) -> Result<()> {
        debug!("Initiating diagnostic troubleshoot for Cloud PC [{id}]...");

        // Try Windows 365 troubleshoot endpoint
        let w365_url =
            format!("https://windows365.microsoft.com/u/api/v1/cloudPCs/{id}/troubleshoot");
        if let Ok(res) = self
            .client
            .send_request(reqwest::Method::POST, &w365_url, Some(json!({})))
            .await
        {
            if res.status().is_success() {
                debug!("Windows 365 troubleshoot command submitted successfully");
                return Ok(());
            }
        }

        let path = format!("me/cloudPCs/{id}/troubleshoot");
        self.client.post_action(&path, None).await
    }

    /// Fetches the official signed RDP configuration and AVD bearer token for connecting to the Cloud PC.
    pub async fn fetch_rdp_file_and_token(&self, pc: &CloudPc) -> Result<(String, String)> {
        debug!(
            "Retrieving AVD authorization token and official signed RDP configuration for [{}]...",
            pc.display_name
        );

        let tokens = crate::config::AppConfig::load_tokens().ok_or_else(|| {
            crate::error::AppError::Auth("No authentication tokens found".to_string())
        })?;
        let refresh_token = tokens.refresh_token.as_ref().ok_or_else(|| {
            crate::error::AppError::Auth(
                "No refresh token available to generate AVD session".to_string(),
            )
        })?;

        // 1. Exchange refresh token for AVD scope (9cdead84-a844-4324-93f2-b2e6bb768d07)
        let auth_mgr = crate::auth::AuthManager::new(
            crate::config::COMMON_AUTHORITY,
            crate::config::WINDOWS_APP_CLIENT_ID,
        );
        let avd_tokens = auth_mgr
            .refresh_access_token_for_scope(refresh_token, crate::config::AVD_RESOURCE_SCOPE)
            .await?;
        let avd_token = avd_tokens.access_token;

        // 2. Determine tenant_id and resource_id from direct_launch_url
        let mut tenant_id = None;
        let mut resource_id = None;
        if let Some(direct_url) = &pc.direct_launch_url {
            let parts: Vec<&str> = direct_url.split('/').collect();
            for i in 0..parts.len() {
                if parts[i] == "tenants" && i + 1 < parts.len() {
                    tenant_id = Some(parts[i + 1].to_string());
                }
                if parts[i] == "resources" && i + 1 < parts.len() {
                    let r_part = parts[i + 1].split('?').next().unwrap_or(parts[i + 1]);
                    resource_id = Some(r_part.to_string());
                }
            }
        }

        let tenant_id =
            tenant_id.unwrap_or_else(|| "f6339a5a-2844-4c22-b81b-a241d0801fb6".to_string());
        let resource_id = resource_id.unwrap_or_else(|| pc.id.clone());

        let rdp_url = format!(
            "https://rdweb.wvd.microsoft.com/api/arm/feeddiscovery/tenants/{tenant_id}/rdps/{resource_id}.rdp?directLaunch=true"
        );
        debug!("Fetching signed RDP file for Cloud PC [{resource_id}]...");

        let client = reqwest::Client::new();
        let res = client
            .get(&rdp_url)
            .header("Authorization", format!("Bearer {avd_token}"))
            .header("User-Agent", "WindowsDesktop/1.2.5105.0")
            .header("x-ms-user-agent", "WindowsDesktop/1.2.5105.0")
            .send()
            .await?;

        if !res.status().is_success() {
            let status = res.status();
            let body = res.text().await.unwrap_or_default();
            return Err(crate::error::AppError::GraphApi {
                status: status.as_u16(),
                message: format!("Failed to download AVD RDP file: {body}"),
            });
        }

        let rdp_content = res.text().await?;
        Ok((rdp_content, avd_token))
    }

    /// Retrieves launch parameters and connection gateway address for a Cloud PC session.
    pub async fn get_launch_info(&self, id: &str) -> Result<RdpConnectionConfig> {
        debug!("Fetching launch configuration for Cloud PC [{id}]...");

        // Try getting Cloud PC details first to extract directLaunchURL or systemName
        let pc_details = self.get_cloud_pc_by_id(id).await.ok();

        let host = pc_details
            .as_ref()
            .and_then(|p| p.managed_device_name.as_deref())
            .map(|name| format!("{name}.cloudpc.microsoft.com"))
            .unwrap_or_else(|| format!("{id}.cloudpc.microsoft.com"));

        let direct_launch_url = pc_details
            .as_ref()
            .and_then(|p| p.direct_launch_url.clone());

        // Try Windows 365 End User API first
        let w365_url =
            format!("https://windows365.microsoft.com/u/api/v1/cloudPCs/{id}/getLaunchInfo");
        if let Ok(res) = self
            .client
            .send_request(reqwest::Method::POST, &w365_url, Some(json!({})))
            .await
        {
            if res.status().is_success() {
                if let Ok(data) = res.json::<serde_json::Value>().await {
                    let full_address = data["cloudPcHost"]
                        .as_str()
                        .or_else(|| data["fullAddress"].as_str())
                        .map(ToString::to_string)
                        .unwrap_or(host.clone());
                    let gateway = data["gatewayUrl"]
                        .as_str()
                        .or_else(|| data["gatewayHostname"].as_str())
                        .map(ToString::to_string)
                        .or_else(|| Some("rdgateway.wvd.microsoft.com".to_string()));

                    return Ok(RdpConnectionConfig {
                        full_address,
                        gateway_hostname: gateway,
                        direct_launch_url,
                        ..Default::default()
                    });
                }
            }
        }

        let path = format!("me/cloudPCs/{id}/getCloudPcLaunchInfo");
        if let Ok(res) = self
            .client
            .execute_request(reqwest::Method::POST, &path, Some(json!({})))
            .await
        {
            if let Ok(data) = res.json::<serde_json::Value>().await {
                let full_address = data["cloudPcHost"]
                    .as_str()
                    .or_else(|| data["fullAddress"].as_str())
                    .map(ToString::to_string)
                    .unwrap_or(host.clone());
                let gateway = data["gatewayUrl"]
                    .as_str()
                    .or_else(|| data["gatewayHostname"].as_str())
                    .map(ToString::to_string)
                    .or_else(|| Some("rdgateway.wvd.microsoft.com".to_string()));

                return Ok(RdpConnectionConfig {
                    full_address,
                    gateway_hostname: gateway,
                    direct_launch_url,
                    ..Default::default()
                });
            }
        }

        // Standard AVD / Cloud PC RDP addressing fallback
        Ok(RdpConnectionConfig {
            full_address: host,
            gateway_hostname: Some("rdgateway.wvd.microsoft.com".to_string()),
            direct_launch_url,
            ..Default::default()
        })
    }
}
