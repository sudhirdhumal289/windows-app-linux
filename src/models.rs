//! Data models for Windows 365 Cloud PCs, AVD resources, and authentication tokens.

use serde::{Deserialize, Serialize};

fn default_true() -> bool {
    true
}

/// Microsoft Entra ID Device Code authorization response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceCodeResponse {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub expires_in: u64,
    pub interval: u64,
    pub message: String,
}

/// OAuth 2.0 Access & Refresh Token response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenResponse {
    pub token_type: String,
    pub scope: Option<String>,
    pub expires_in: u64,
    pub ext_expires_in: Option<u64>,
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub id_token: Option<String>,
    #[serde(default)]
    pub acquired_at: Option<i64>,
}

/// Cloud PC representation from Microsoft Graph (`/beta/me/cloudPCs`) or Windows 365 End User API.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CloudPc {
    #[serde(alias = "workspaceId")]
    pub id: String,
    #[serde(alias = "workspaceName")]
    pub display_name: String,
    #[serde(default, alias = "deviceId")]
    pub managed_device_id: Option<String>,
    #[serde(default, alias = "systemName")]
    pub managed_device_name: Option<String>,
    #[serde(default, alias = "provisionStatus")]
    pub provisioning_status: Option<String>,
    #[serde(default)]
    pub service_plan_name: Option<String>,
    #[serde(default, alias = "deviceStatus")]
    pub status: Option<String>,
    #[serde(default)]
    pub power_state: Option<String>,
    #[serde(default, alias = "deviceType")]
    pub image_display_name: Option<String>,
    #[serde(default)]
    pub user_principal_name: Option<String>,
    #[serde(default)]
    pub disk_encryption_state: Option<String>,
    #[serde(default, alias = "directLaunchURL")]
    pub direct_launch_url: Option<String>,
    #[serde(default)]
    pub host_pool_id: Option<String>,
    #[serde(default)]
    pub device_region_name: Option<String>,
}

impl CloudPc {
    /// Returns human-readable status badge.
    #[must_use]
    pub fn display_status(&self) -> &str {
        if let Some(status) = &self.status {
            return status.as_str();
        }
        if let Some(prov) = &self.provisioning_status {
            return prov.as_str();
        }
        "Unknown"
    }

    /// Returns a friendly description of the hardware specs if available.
    #[must_use]
    pub fn hardware_summary(&self) -> &str {
        self.service_plan_name
            .as_deref()
            .unwrap_or("Standard Cloud PC")
    }
}

/// Windows 365 End User API Service Plan information.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct W365ServicePlan {
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default, rename = "numvCPUs")]
    pub num_vcpus: Option<u32>,
    #[serde(default, rename = "ramInGB")]
    pub ram_in_gb: Option<u32>,
    #[serde(default, rename = "osDiskSizeInGB")]
    pub os_disk_size_in_gb: Option<u32>,
}

/// Windows 365 End User API Cloud PC item structure.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct W365CloudPcItem {
    pub workspace_id: Option<String>,
    pub device_id: Option<String>,
    pub workspace_name: Option<String>,
    pub system_name: Option<String>,
    pub device_type: Option<String>,
    pub device_status: Option<String>,
    pub provision_status: Option<String>,
    #[serde(default, rename = "directLaunchURL")]
    pub direct_launch_url: Option<String>,
    pub service_plan: Option<W365ServicePlan>,
    pub host_pool_id: Option<String>,
    pub device_region_name: Option<String>,
    pub os_name: Option<String>,
    pub os_version: Option<String>,
    pub user_principal_name: Option<String>,
}

impl From<W365CloudPcItem> for CloudPc {
    fn from(item: W365CloudPcItem) -> Self {
        let id = item
            .workspace_id
            .clone()
            .or_else(|| item.device_id.clone())
            .unwrap_or_default();
        let display_name = item
            .workspace_name
            .clone()
            .or_else(|| item.system_name.clone())
            .unwrap_or_else(|| "Windows 365 Cloud PC".to_string());
        let service_plan_name = item
            .service_plan
            .as_ref()
            .and_then(|sp| sp.display_name.clone())
            .or_else(|| item.device_type.clone());

        CloudPc {
            id,
            display_name,
            managed_device_id: item.device_id,
            managed_device_name: item.system_name,
            provisioning_status: item.provision_status,
            service_plan_name,
            status: item.device_status,
            power_state: None,
            image_display_name: item.device_type,
            user_principal_name: item.user_principal_name,
            disk_encryption_state: None,
            direct_launch_url: item.direct_launch_url,
            host_pool_id: item.host_pool_id,
            device_region_name: item.device_region_name,
        }
    }
}

/// Windows 365 End User v2 API response payload (`Dedicated`, `Flex`, `FrontlineShared`).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Windows365V2Response {
    #[serde(default, rename = "Dedicated")]
    pub dedicated: Vec<W365CloudPcItem>,
    #[serde(default, rename = "Flex")]
    pub flex: Vec<W365CloudPcItem>,
    #[serde(default, rename = "FrontlineShared")]
    pub frontline_shared: Vec<W365CloudPcItem>,
}

impl Windows365V2Response {
    #[must_use]
    pub fn into_cloud_pcs(self) -> Vec<CloudPc> {
        let mut list = Vec::new();
        for item in self.dedicated {
            list.push(CloudPc::from(item));
        }
        for item in self.flex {
            list.push(CloudPc::from(item));
        }
        for item in self.frontline_shared {
            list.push(CloudPc::from(item));
        }
        list
    }
}

/// Graph API Collection wrapper (e.g. `{"@odata.context": "...", "value": [...]}`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ODataResponse<T> {
    pub value: Vec<T>,
    #[serde(rename = "@odata.nextLink")]
    pub next_link: Option<String>,
}

/// AVD / Remote Desktop connection launch parameters.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RdpConnectionConfig {
    pub full_address: String,
    pub gateway_hostname: Option<String>,
    pub gateway_usage_method: u32,
    pub gateway_access_token: Option<String>,
    pub username: Option<String>,
    pub display_width: u32,
    pub display_height: u32,
    pub scale_factor: u32,
    pub enable_audio: bool,
    pub enable_microphone: bool,
    #[serde(default = "default_true")]
    pub enable_camera: bool,
    pub enable_clipboard: bool,
    pub multi_monitor: bool,
    pub direct_launch_url: Option<String>,
}

impl Default for RdpConnectionConfig {
    fn default() -> Self {
        Self {
            full_address: String::new(),
            gateway_hostname: None,
            gateway_usage_method: 2, // RD Gateway if direct connection fails
            gateway_access_token: None,
            username: None,
            display_width: 1920,
            display_height: 1080,
            scale_factor: 125,
            enable_audio: true,
            enable_microphone: true,
            enable_camera: true,
            enable_clipboard: true,
            multi_monitor: false,
            direct_launch_url: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_windows365_v2_deserialization() {
        let json_data = r#"{
            "Dedicated": [
                {
                    "workspaceName": "W365 CPC India - Dhumal, Sudhir",
                    "workspaceId": "eec89646-ae6e-4129-9629-9e8ef18c1e48",
                    "deviceId": "5f347b36-90fc-4b03-be97-6abc1558cbcb",
                    "systemName": "CPC-sudhi-X3ZHD",
                    "deviceType": "Windows 11 Enterprise + Microsoft 365 Apps 25H2",
                    "deviceStatus": "running",
                    "provisionStatus": "provisioned",
                    "directLaunchURL": "https://rdweb.wvd.microsoft.com/api/arm/weblaunch/...",
                    "servicePlan": {
                        "displayName": "Cloud PC Enterprise 4vCPU/16GB/128GB",
                        "numvCPUs": 4,
                        "ramInGB": 16,
                        "osDiskSizeInGB": 128
                    }
                }
            ],
            "Flex": [],
            "FrontlineShared": []
        }"#;

        let response: Windows365V2Response = serde_json::from_str(json_data).unwrap();
        assert_eq!(response.dedicated.len(), 1);
        let pcs = response.into_cloud_pcs();
        assert_eq!(pcs.len(), 1);
        let pc = &pcs[0];
        assert_eq!(pc.id, "eec89646-ae6e-4129-9629-9e8ef18c1e48");
        assert_eq!(pc.display_name, "W365 CPC India - Dhumal, Sudhir");
        assert_eq!(pc.display_status(), "running");
        assert_eq!(
            pc.hardware_summary(),
            "Cloud PC Enterprise 4vCPU/16GB/128GB"
        );
        assert_eq!(pc.managed_device_name.as_deref(), Some("CPC-sudhi-X3ZHD"));
        assert!(pc.direct_launch_url.is_some());
    }

    #[test]
    fn test_windows365_v1_deserialization() {
        let json_data = r#"[
            {
                "workspaceName": "W365 CPC India - Dhumal, Sudhir",
                "workspaceId": "eec89646-ae6e-4129-9629-9e8ef18c1e48",
                "deviceId": "5f347b36-90fc-4b03-be97-6abc1558cbcb",
                "systemName": "CPC-sudhi-X3ZHD",
                "deviceType": "Windows 11 Enterprise",
                "deviceStatus": "running"
            }
        ]"#;

        let items: Vec<W365CloudPcItem> = serde_json::from_str(json_data).unwrap();
        assert_eq!(items.len(), 1);
        let pc = CloudPc::from(items.into_iter().next().unwrap());
        assert_eq!(pc.id, "eec89646-ae6e-4129-9629-9e8ef18c1e48");
        assert_eq!(pc.display_name, "W365 CPC India - Dhumal, Sudhir");
        assert_eq!(pc.display_status(), "running");
    }

    #[test]
    fn test_graph_api_deserialization() {
        let json_data = r#"{
            "value": [
                {
                    "id": "graph-cpc-123",
                    "displayName": "My Graph Cloud PC",
                    "status": "provisioned",
                    "servicePlanName": "Enterprise 2vCPU"
                }
            ]
        }"#;

        let res: ODataResponse<CloudPc> = serde_json::from_str(json_data).unwrap();
        assert_eq!(res.value.len(), 1);
        let pc = &res.value[0];
        assert_eq!(pc.id, "graph-cpc-123");
        assert_eq!(pc.display_name, "My Graph Cloud PC");
        assert_eq!(pc.display_status(), "provisioned");
    }
}
