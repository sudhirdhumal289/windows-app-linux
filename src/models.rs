//! Data models for Windows 365 Cloud PCs, AVD resources, and authentication tokens.

use serde::{Deserialize, Serialize};

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
    #[serde(default)]
    pub scope: Option<String>,
    pub expires_in: u64,
    #[serde(default)]
    pub ext_expires_in: Option<u64>,
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: Option<String>,
    #[serde(default)]
    pub id_token: Option<String>,
    /// Unix time the token was issued (set locally).
    #[serde(default)]
    pub acquired_at: Option<i64>,
    /// Client id that obtained the token; refreshes must use the same one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    /// Authority that issued the token.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authority: Option<String>,
}

/// Cloud PC representation from Microsoft Graph (`/beta/me/cloudPCs`) or Windows 365 End User API.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
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
    /// Operating system reported by the service (e.g. "Windows 11 Enterprise 24H2").
    #[serde(default, alias = "osVersion")]
    pub os_name: Option<String>,
    /// End of the grace period (RFC 3339) when `provisioning_status` is `inGracePeriod`.
    #[serde(default, alias = "gracePeriodEndDateTime")]
    pub grace_period_end: Option<String>,
    /// Provisioning progress in percent while provisioning.
    #[serde(default)]
    pub provisioning_progress: Option<u32>,
    /// Frontline shared access state (`active`, `activating`, `standbyMode`, …).
    #[serde(default)]
    pub access_state: Option<String>,
    #[serde(default)]
    pub license: LicenseCategory,
    #[serde(default, alias = "lastLoginTime")]
    pub last_login: Option<String>,
    #[serde(default)]
    pub last_login_result: Option<String>,
    #[serde(default)]
    pub is_default: bool,
    /// Which end-user actions the service allows; `None` when it did not say.
    #[serde(default)]
    pub remote_actions: Option<RemoteActions>,
    #[serde(default)]
    pub last_remote_action_result: Option<RemoteActionResult>,
}

/// Progress of the latest end-user action (restart, troubleshoot, restore, …).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct RemoteActionResult {
    pub action_name: Option<String>,
    /// `pending`, `active`, `done`, `failed`, `canceled`, `notSupported`.
    pub action_state: Option<String>,
    pub start_date_time: Option<String>,
    pub last_updated_date_time: Option<String>,
    pub status_details: Option<ActionStatusDetails>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct ActionStatusDetails {
    pub code: Option<String>,
    pub message: Option<String>,
}

impl RemoteActionResult {
    #[must_use]
    pub fn is_for(&self, action: &str) -> bool {
        self.action_name
            .as_deref()
            .is_some_and(|n| n.eq_ignore_ascii_case(action))
    }

    /// Whether the action has finished (successfully or not).
    #[must_use]
    pub fn is_finished(&self) -> bool {
        matches!(
            self.action_state.as_deref(),
            Some("done" | "failed" | "canceled" | "notSupported")
        )
    }

    #[must_use]
    pub fn succeeded(&self) -> bool {
        self.action_state.as_deref() == Some("done")
    }

    #[must_use]
    pub fn code(&self) -> Option<&str> {
        self.status_details.as_ref()?.code.as_deref()
    }
}

/// Windows 365 licence type of a Cloud PC.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum LicenseCategory {
    #[default]
    Dedicated,
    Flex,
    /// Windows 365 Frontline in shared mode: powered on per session.
    FrontlineShared,
}

impl LicenseCategory {
    /// Numeric value used by the Windows 365 end-user API.
    #[must_use]
    pub fn api_value(self) -> u32 {
        match self {
            Self::Flex => 1,
            Self::Dedicated => 2,
            Self::FrontlineShared => 3,
        }
    }

    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Dedicated => "Enterprise / Business",
            Self::Flex => "Frontline",
            Self::FrontlineShared => "Frontline (shared)",
        }
    }
}

/// End-user actions the Windows 365 service allows on a Cloud PC.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct RemoteActions {
    pub restart: bool,
    pub rename: bool,
    pub restore: bool,
    pub reset: bool,
    pub troubleshoot: bool,
}

/// End-user action on a Cloud PC.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloudPcAction {
    Restart,
    Rename,
    Restore,
    Reset,
    Troubleshoot,
    Start,
    Stop,
}

impl CloudPcAction {
    /// Present-tense verb for progress messages.
    #[must_use]
    pub fn progress_label(self) -> &'static str {
        match self {
            Self::Restart => "Restarting",
            Self::Rename => "Renaming",
            Self::Restore => "Restoring",
            Self::Reset => "Resetting",
            Self::Troubleshoot => "Troubleshooting",
            Self::Start => "Starting",
            Self::Stop => "Stopping",
        }
    }
}

/// Power state of a Cloud PC.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerState {
    Running,
    Stopped,
    Hibernated,
    Unknown,
}

/// Coarse provisioning state of a Cloud PC.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProvisioningState {
    Provisioned,
    /// Being created, upgraded, moved or restored; not connectable yet.
    InProgress,
    /// Licence removed; the PC is deleted when the grace period ends.
    GracePeriod,
    Failed,
    Unknown,
}

/// Severity used to colour a status badge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusTone {
    Good,
    Neutral,
    Warning,
    Error,
}

impl CloudPc {
    /// Returns the raw status reported by the service.
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

    #[must_use]
    pub fn power(&self) -> PowerState {
        let raw = self.power_state.as_deref().or(self.status.as_deref());
        match raw.map(str::to_ascii_lowercase).as_deref() {
            Some("running" | "poweredon" | "on" | "available") => PowerState::Running,
            Some("deallocated" | "poweredoff" | "stopped" | "off") => PowerState::Stopped,
            Some("hibernated") => PowerState::Hibernated,
            _ => PowerState::Unknown,
        }
    }

    #[must_use]
    pub fn provisioning(&self) -> ProvisioningState {
        // Graph reports the provisioning state in `status`.
        match self
            .provisioning_status
            .as_deref()
            .or(self.status.as_deref())
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            Some("provisioned" | "provisionedwithwarnings") => ProvisioningState::Provisioned,
            Some(
                "provisioning" | "pending" | "upgrading" | "moving" | "restoring" | "resizing"
                | "deprovisioning" | "notprovisioned",
            ) => ProvisioningState::InProgress,
            Some("ingraceperiod") => ProvisioningState::GracePeriod,
            Some("failed" | "provisionfailed" | "reprovisionfailed") => ProvisioningState::Failed,
            _ => ProvisioningState::Unknown,
        }
    }

    #[must_use]
    pub fn is_frontline_shared(&self) -> bool {
        self.license == LicenseCategory::FrontlineShared
    }

    /// Whether the service allows `action`. Unknown capabilities are allowed and left
    /// to the service to refuse.
    #[must_use]
    pub fn allows(&self, action: CloudPcAction) -> bool {
        let busy = self.provisioning() == ProvisioningState::InProgress;
        let ra = self.remote_actions;
        match action {
            CloudPcAction::Restart => !busy && ra.is_none_or(|a| a.restart),
            CloudPcAction::Rename => ra.is_none_or(|a| a.rename),
            CloudPcAction::Restore => {
                !busy && !self.is_frontline_shared() && ra.is_none_or(|a| a.restore)
            }
            CloudPcAction::Reset => {
                !busy && !self.is_frontline_shared() && ra.is_none_or(|a| a.reset)
            }
            CloudPcAction::Troubleshoot => ra.is_none_or(|a| a.troubleshoot),
            CloudPcAction::Start => {
                self.license != LicenseCategory::Dedicated && self.power() != PowerState::Running
            }
            CloudPcAction::Stop => {
                self.license != LicenseCategory::Dedicated && self.power() != PowerState::Stopped
            }
        }
    }

    /// Whole days left in the grace period at `now`, if one applies.
    #[must_use]
    pub fn grace_days_left(&self, now: chrono::DateTime<chrono::Utc>) -> Option<i64> {
        if self.provisioning() != ProvisioningState::GracePeriod {
            return None;
        }
        let end = chrono::DateTime::parse_from_rfc3339(self.grace_period_end.as_deref()?).ok()?;
        Some(((end.with_timezone(&chrono::Utc) - now).num_hours().max(0) + 23) / 24)
    }

    /// Short status for a card badge, e.g. "Running" or "Grace period · 3 days left".
    #[must_use]
    pub fn status_summary(&self, now: chrono::DateTime<chrono::Utc>) -> (String, StatusTone) {
        match self.provisioning() {
            ProvisioningState::InProgress => {
                let label = match self.provisioning_progress.filter(|p| *p > 0 && *p < 100) {
                    Some(p) => format!("Setting up · {p}%"),
                    None => "Setting up".to_string(),
                };
                return (label, StatusTone::Neutral);
            }
            ProvisioningState::Failed => return ("Setup failed".into(), StatusTone::Error),
            ProvisioningState::GracePeriod => {
                let label = match self.grace_days_left(now) {
                    Some(1) => "Grace period · 1 day left".to_string(),
                    Some(d) => format!("Grace period · {d} days left"),
                    None => "Grace period".to_string(),
                };
                return (label, StatusTone::Warning);
            }
            ProvisioningState::Provisioned | ProvisioningState::Unknown => {}
        }
        if self.is_frontline_shared() {
            match self.access_state.as_deref() {
                Some("activating") => return ("Activating".into(), StatusTone::Neutral),
                Some("noLicensesAvailable" | "waitlisted") => {
                    return ("No licence available".into(), StatusTone::Warning)
                }
                Some("activationFailed") => return ("Activation failed".into(), StatusTone::Error),
                _ => {}
            }
        }
        match self.power() {
            PowerState::Running => ("Running".into(), StatusTone::Good),
            PowerState::Stopped => ("Stopped".into(), StatusTone::Neutral),
            PowerState::Hibernated => ("Hibernated".into(), StatusTone::Neutral),
            PowerState::Unknown => match self.provisioning() {
                ProvisioningState::Provisioned => ("Ready".into(), StatusTone::Good),
                _ => (self.display_status().to_string(), StatusTone::Neutral),
            },
        }
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
    #[serde(default)]
    pub grace_period_end_date_time: Option<String>,
    #[serde(default, deserialize_with = "lenient_u32")]
    pub provisioning_progress: Option<u32>,
    #[serde(default)]
    pub access_state: Option<String>,
    /// Numeric (`1` Flex, `2` Dedicated, `3` FrontlineShared) or string.
    #[serde(default)]
    pub license_category: Option<serde_json::Value>,
    #[serde(default)]
    pub last_login_time: Option<String>,
    #[serde(default)]
    pub last_login_result: Option<serde_json::Value>,
    #[serde(default)]
    pub is_default: Option<bool>,
    #[serde(default)]
    pub remote_actions: Option<RemoteActions>,
    #[serde(default)]
    pub last_remote_action_result: Option<RemoteActionResult>,
}

fn lenient_u32<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<u32>, D::Error> {
    let v = Option::<serde_json::Value>::deserialize(d)?;
    Ok(v.and_then(|v| match v {
        serde_json::Value::Number(n) => n.as_f64().map(|f| f.clamp(0.0, 100.0) as u32),
        serde_json::Value::String(s) => s.trim().trim_end_matches('%').parse().ok(),
        _ => None,
    }))
}

fn license_from_value(v: &serde_json::Value) -> Option<LicenseCategory> {
    match v {
        serde_json::Value::Number(n) => match n.as_u64()? {
            1 => Some(LicenseCategory::Flex),
            2 => Some(LicenseCategory::Dedicated),
            3 => Some(LicenseCategory::FrontlineShared),
            _ => None,
        },
        serde_json::Value::String(s) => match s.to_ascii_lowercase().as_str() {
            "flex" | "1" => Some(LicenseCategory::Flex),
            "dedicated" | "2" => Some(LicenseCategory::Dedicated),
            "frontlineshared" | "shared" | "3" => Some(LicenseCategory::FrontlineShared),
            _ => None,
        },
        _ => None,
    }
}

fn value_to_text(v: serde_json::Value) -> Option<String> {
    match v {
        serde_json::Value::String(s) => Some(s),
        serde_json::Value::Null => None,
        other => Some(other.to_string()),
    }
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
            os_name: match (item.os_name, item.os_version) {
                (Some(name), Some(version)) if !version.is_empty() => {
                    Some(format!("{name} {version}"))
                }
                (name, _) => name,
            },
            grace_period_end: item.grace_period_end_date_time,
            provisioning_progress: item.provisioning_progress,
            access_state: item.access_state,
            license: item
                .license_category
                .as_ref()
                .and_then(license_from_value)
                .unwrap_or_default(),
            last_login: item.last_login_time,
            last_login_result: item.last_login_result.and_then(value_to_text),
            is_default: item.is_default.unwrap_or(false),
            remote_actions: item.remote_actions,
            last_remote_action_result: item.last_remote_action_result,
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
    /// Flattens the groups; the group decides the licence type unless an item says otherwise.
    #[must_use]
    pub fn into_cloud_pcs(self) -> Vec<CloudPc> {
        let tag = |items: Vec<W365CloudPcItem>, license: LicenseCategory| {
            items.into_iter().map(move |item| {
                let explicit = item.license_category.is_some();
                let mut pc = CloudPc::from(item);
                if !explicit {
                    pc.license = license;
                }
                pc
            })
        };
        tag(self.dedicated, LicenseCategory::Dedicated)
            .chain(tag(self.flex, LicenseCategory::Flex))
            .chain(tag(self.frontline_shared, LicenseCategory::FrontlineShared))
            .collect()
    }
}

/// Graph API Collection wrapper (e.g. `{"@odata.context": "...", "value": [...]}`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ODataResponse<T> {
    pub value: Vec<T>,
    #[serde(rename = "@odata.nextLink")]
    pub next_link: Option<String>,
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

    fn ts(s: &str) -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::parse_from_rfc3339(s).unwrap().into()
    }

    #[test]
    fn status_fields_and_groups() {
        let json = r#"{
            "Dedicated": [{"workspaceId": "a", "workspaceName": "A", "deviceStatus": "running",
                "provisionStatus": "inGracePeriod",
                "gracePeriodEndDateTime": "2025-01-04T12:00:00Z",
                "remoteActions": {"restart": true, "rename": true, "restore": false,
                                  "reset": false, "troubleshoot": true}}],
            "Flex": [{"workspaceId": "b", "workspaceName": "B", "deviceStatus": "deallocated",
                "provisionStatus": "provisioned"}],
            "FrontlineShared": [{"workspaceId": "c", "workspaceName": "C",
                "provisionStatus": "provisioning", "provisioningProgress": 42,
                "licenseCategory": 3, "accessState": "active"}]
        }"#;
        let pcs = serde_json::from_str::<Windows365V2Response>(json)
            .unwrap()
            .into_cloud_pcs();
        let now = ts("2025-01-01T12:00:00Z");

        assert_eq!(pcs[0].license, LicenseCategory::Dedicated);
        assert_eq!(pcs[0].provisioning(), ProvisioningState::GracePeriod);
        assert_eq!(pcs[0].grace_days_left(now), Some(3));
        assert_eq!(
            pcs[0].status_summary(now),
            ("Grace period · 3 days left".into(), StatusTone::Warning)
        );
        assert!(pcs[0].allows(CloudPcAction::Restart));
        assert!(!pcs[0].allows(CloudPcAction::Restore));
        assert!(!pcs[0].allows(CloudPcAction::Reset));
        assert!(
            !pcs[0].allows(CloudPcAction::Start),
            "dedicated PCs are always on"
        );

        assert_eq!(pcs[1].license, LicenseCategory::Flex);
        assert_eq!(pcs[1].power(), PowerState::Stopped);
        assert_eq!(pcs[1].status_summary(now).0, "Stopped");
        assert!(pcs[1].allows(CloudPcAction::Start));
        assert!(!pcs[1].allows(CloudPcAction::Stop));

        assert_eq!(pcs[2].license, LicenseCategory::FrontlineShared);
        assert_eq!(pcs[2].status_summary(now).0, "Setting up · 42%");
        assert!(
            !pcs[2].allows(CloudPcAction::Restart),
            "busy while provisioning"
        );
        assert!(!pcs[2].allows(CloudPcAction::Reset));
    }

    #[test]
    fn license_category_accepts_strings_and_overrides_group() {
        let json = r#"{"Dedicated": [{"workspaceId": "x", "licenseCategory": "FrontlineShared",
            "deviceStatus": "running", "accessState": "activationFailed"}]}"#;
        let pc = serde_json::from_str::<Windows365V2Response>(json)
            .unwrap()
            .into_cloud_pcs()
            .remove(0);
        assert_eq!(pc.license, LicenseCategory::FrontlineShared);
        assert_eq!(
            pc.status_summary(ts("2025-01-01T00:00:00Z")).1,
            StatusTone::Error
        );
        assert_eq!(LicenseCategory::FrontlineShared.api_value(), 3);
    }
}
