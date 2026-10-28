//! Cloud PC discovery, lifecycle actions and RDP launch material.

use crate::api::client::GraphApiClient;
use crate::config::{AVD_RESOURCE_SCOPE, RDWEB_USER_AGENT};
use crate::error::{AppError, Result};
use crate::models::{
    CloudPc, CloudPcAction, ODataResponse, RemoteActionResult, W365CloudPcItem,
    Windows365V2Response,
};
use log::{debug, warn};
use reqwest::Method;
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::json;
use std::time::Duration;
use url::Url;

/// Interval between status polls while an action runs.
const POLL_INTERVAL: Duration = Duration::from_secs(10);
/// Troubleshooting can take several minutes.
const TROUBLESHOOT_TIMEOUT: Duration = Duration::from_secs(15 * 60);

/// A restore point of a Cloud PC.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub id: String,
    #[serde(default)]
    pub created_date_time: Option<String>,
    #[serde(default)]
    pub snapshot_type: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
}

#[derive(Deserialize)]
struct SnapshotList {
    #[serde(default)]
    snapshots: Vec<Snapshot>,
}

/// Result of a troubleshoot run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TroubleshootOutcome {
    NoIssues,
    /// Problems were found and fixed.
    Fixed,
    /// Problems remain; the message says what the user (or admin) should do.
    Failed(String),
    /// Still running when we stopped waiting.
    StillRunning,
}

/// Everything needed to start an RDP session to a Cloud PC through the AVD gateway.
#[derive(Clone)]
pub struct RdpLaunch {
    /// Signed `.rdp` file content from the RD Web feed.
    pub rdp_file: String,
    /// Bearer token for the AVD ARM gateway.
    pub gateway_token: String,
}

impl std::fmt::Debug for RdpLaunch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RdpLaunch")
            .field("rdp_file_len", &self.rdp_file.len())
            .finish_non_exhaustive()
    }
}

pub struct CloudPcService {
    client: GraphApiClient,
}

impl CloudPcService {
    #[must_use]
    pub fn new(client: GraphApiClient) -> Self {
        Self { client }
    }

    #[must_use]
    pub fn client(&self) -> &GraphApiClient {
        &self.client
    }

    /// Lists the user's Cloud PCs: Windows 365 end-user API v2, then v1, then Graph.
    /// Accounts without a Cloud PC license yield an empty list rather than an error.
    pub async fn list_user_cloud_pcs(&self) -> Result<Vec<CloudPc>> {
        let base_url = self.client.environment().windows_365_base_url();
        let v2 = format!(
            "{base_url}/u/api/v2/cloudPCs?extend=IsDefault&licenseCategories=Dedicated,Flex,FrontlineShared"
        );
        if let Some(body) = self
            .try_get::<Windows365V2Response>(&v2, "Windows 365 v2")
            .await
        {
            let pcs = body.into_cloud_pcs();
            if !pcs.is_empty() {
                return Ok(pcs);
            }
        }

        let v1 = format!("{base_url}/u/api/v1/cloudPCs?extend=IsDefault");
        if let Some(items) = self
            .try_get::<Vec<W365CloudPcItem>>(&v1, "Windows 365 v1")
            .await
        {
            if !items.is_empty() {
                return Ok(items.into_iter().map(CloudPc::from).collect());
            }
        }

        debug!("Falling back to Microsoft Graph /me/cloudPCs");
        match self
            .client
            .get_json::<ODataResponse<CloudPc>>("me/cloudPCs")
            .await
        {
            Ok(response) => Ok(response.value),
            Err(AppError::GraphApi { status, message })
                if matches!(status, 400 | 403 | 404)
                    || message.contains("ResourceNotFound")
                    || message.contains("License") =>
            {
                debug!("Account has no Cloud PC resources (HTTP {status})");
                Ok(Vec::new())
            }
            Err(e) => Err(e),
        }
    }

    /// GETs and decodes `url`, logging and swallowing failures (used for fallbacks).
    async fn try_get<T: DeserializeOwned>(&self, url: &str, label: &str) -> Option<T> {
        match self.client.send_request(Method::GET, url, None).await {
            Ok(res) if res.status().is_success() => match res.json::<T>().await {
                Ok(body) => Some(body),
                Err(e) => {
                    warn!("{label}: unexpected response: {e}");
                    None
                }
            },
            Ok(res) => {
                warn!("{label}: HTTP {}", res.status());
                None
            }
            Err(e) => {
                warn!("{label}: request failed: {e}");
                None
            }
        }
    }

    pub async fn restart_cloud_pc(&self, id: &str) -> Result<()> {
        let base_url = self.client.environment().windows_365_base_url();
        let w365 = format!("{base_url}/u/api/v1/cloudPCs/{id}/reboot");
        match self
            .client
            .send_request(Method::POST, &w365, Some(json!({})))
            .await
        {
            Ok(res) if res.status().is_success() => return Ok(()),
            Ok(res) if matches!(res.status().as_u16(), 400 | 409) => {
                let status = res.status().as_u16();
                let body = res.text().await.unwrap_or_default();
                return Err(action_error(CloudPcAction::Restart, status, &body));
            }
            Ok(res) => warn!("Windows 365 reboot: HTTP {}, trying Graph", res.status()),
            Err(e) => warn!("Windows 365 reboot failed: {e}, trying Graph"),
        }
        self.client
            .post_action(&format!("me/cloudPCs/{id}/restart"), None)
            .await
    }

    pub async fn rename_cloud_pc(&self, id: &str, new_name: &str) -> Result<()> {
        let base_url = self.client.environment().windows_365_base_url();
        let w365 = format!("{base_url}/u/api/v1/cloudPCs/{id}/rename");
        match self
            .client
            .send_request(
                Method::POST,
                &w365,
                Some(json!({ "workspaceName": new_name })),
            )
            .await
        {
            Ok(res) if res.status().is_success() => return Ok(()),
            Ok(res) if matches!(res.status().as_u16(), 400 | 409) => {
                let status = res.status().as_u16();
                let body = res.text().await.unwrap_or_default();
                return Err(action_error(CloudPcAction::Rename, status, &body));
            }
            Ok(res) => warn!("Windows 365 rename: HTTP {}, trying Graph", res.status()),
            Err(e) => warn!("Windows 365 rename failed: {e}, trying Graph"),
        }
        self.client
            .post_action(
                &format!("me/cloudPCs/{id}/rename"),
                Some(json!({ "displayName": new_name })),
            )
            .await
    }

    /// Current state of one Cloud PC (status, last action result, …).
    pub async fn get_cloud_pc(&self, id: &str) -> Result<CloudPc> {
        let base_url = self.client.environment().windows_365_base_url();
        let url = format!("{base_url}/u/api/v1/cloudPCs/{id}");
        let item: W365CloudPcItem = self.client.get_json(&url).await?;
        Ok(CloudPc::from(item))
    }

    /// Posts an end-user action (`reboot`, `troubleshoot`, `reset`, `restore`).
    async fn post_w365_action(
        &self,
        id: &str,
        action: CloudPcAction,
        verb: &str,
        body: serde_json::Value,
    ) -> Result<()> {
        let base_url = self.client.environment().windows_365_base_url();
        let url = format!("{base_url}/u/api/v1/cloudPCs/{id}/{verb}");
        let res = self
            .client
            .send_request(Method::POST, &url, Some(body))
            .await?;
        if res.status().is_success() {
            return Ok(());
        }
        let status = res.status().as_u16();
        let body = res.text().await.unwrap_or_default();
        Err(action_error(action, status, &body))
    }

    /// Rebuilds the Cloud PC from its image; all user data is lost.
    pub async fn reset_cloud_pc(&self, id: &str) -> Result<()> {
        self.post_w365_action(id, CloudPcAction::Reset, "reset", json!({}))
            .await
    }

    /// Restore points the user can return to, newest first.
    pub async fn list_snapshots(&self, id: &str) -> Result<Vec<Snapshot>> {
        let base_url = self.client.environment().windows_365_base_url();
        let url = format!("{base_url}/u/api/v1/cloudPCs/{id}/snapshots");
        let mut list: SnapshotList = self.client.get_json(&url).await?;
        list.snapshots
            .sort_by(|a, b| b.created_date_time.cmp(&a.created_date_time));
        Ok(list.snapshots)
    }

    pub async fn restore_cloud_pc(&self, id: &str, snapshot_id: &str) -> Result<()> {
        self.post_w365_action(
            id,
            CloudPcAction::Restore,
            "restore",
            json!({ "snapshotId": snapshot_id }),
        )
        .await
    }

    /// Runs the service-side connectivity check (and repair) and waits for its result.
    pub async fn troubleshoot_cloud_pc(&self, id: &str) -> Result<TroubleshootOutcome> {
        let started = chrono::Utc::now();
        self.post_w365_action(id, CloudPcAction::Troubleshoot, "troubleshoot", json!({}))
            .await?;
        let deadline = tokio::time::Instant::now() + TROUBLESHOOT_TIMEOUT;
        while tokio::time::Instant::now() < deadline {
            tokio::time::sleep(POLL_INTERVAL).await;
            let pc = match self.get_cloud_pc(id).await {
                Ok(pc) => pc,
                Err(e) => {
                    warn!("Polling troubleshoot status failed: {e}");
                    continue;
                }
            };
            if let Some(outcome) = pc
                .last_remote_action_result
                .as_ref()
                .and_then(|r| troubleshoot_outcome(r, started))
            {
                return Ok(outcome);
            }
        }
        Ok(TroubleshootOutcome::StillRunning)
    }

    /// Powers on (`start`) or deallocates (`stop`) a Windows 365 Frontline Cloud PC.
    /// Needs the delegated Graph permission `CloudPC.ReadWrite.All`.
    pub async fn set_power(&self, id: &str, start: bool) -> Result<()> {
        let (verb, action) = if start {
            ("start", CloudPcAction::Start)
        } else {
            ("stop", CloudPcAction::Stop)
        };
        match self
            .client
            .post_action(&format!("me/cloudPCs/{id}/{verb}"), None)
            .await
        {
            Ok(()) => Ok(()),
            Err(AppError::Auth(detail)) => Err(AppError::Service(format!(
                "Your organization has not allowed this app to {verb} Cloud PCs \
                 (Microsoft Graph CloudPC.ReadWrite.All). Connecting also starts the Cloud PC. \
                 ({detail})"
            ))),
            Err(AppError::GraphApi { status, message }) => {
                Err(action_error(action, status, &message))
            }
            Err(e) => Err(e),
        }
    }

    /// Downloads the signed `.rdp` file for `pc` and obtains an AVD gateway token.
    pub async fn fetch_rdp_launch(&self, pc: &CloudPc) -> Result<RdpLaunch> {
        let (tenant_id, resource_id) = launch_ids(pc)?;
        let rdweb_url = self.client.environment().rdweb_feed_url();
        let rdp_url =
            format!("{rdweb_url}/tenants/{tenant_id}/rdps/{resource_id}.rdp?directLaunch=true");
        debug!("Fetching signed RDP file for resource {resource_id}");
        self.fetch_rdp_file(&rdp_url).await
    }

    /// Downloads a signed `.rdp` file from the RD Web feed with an AVD gateway token.
    pub async fn fetch_rdp_file(&self, rdp_url: &str) -> Result<RdpLaunch> {
        let gateway_token = self
            .client
            .auth()
            .token_for_scope(AVD_RESOURCE_SCOPE)
            .await?;
        let res = self
            .client
            .http()
            .get(rdp_url)
            .bearer_auth(&gateway_token)
            .header("User-Agent", RDWEB_USER_AGENT)
            .header("x-ms-user-agent", RDWEB_USER_AGENT)
            .send()
            .await?;
        if !res.status().is_success() {
            return Err(AppError::GraphApi {
                status: res.status().as_u16(),
                message: format!(
                    "failed to download RDP file: {}",
                    res.text().await.unwrap_or_default()
                ),
            });
        }
        let rdp_file = res.text().await?;
        if !rdp_file.contains("full address:s:") {
            return Err(AppError::Config(
                "RD Web returned an RDP file without a 'full address'".into(),
            ));
        }
        Ok(RdpLaunch {
            rdp_file,
            gateway_token,
        })
    }
}

/// RD Web resource id of a Cloud PC (used to hide it from the AVD resource list).
#[must_use]
pub fn launch_resource_id(pc: &CloudPc) -> Option<String> {
    launch_ids(pc).ok().map(|(_, resource)| resource)
}

/// Maps a refused action to a user-facing error using the service's `actionResultCode`.
fn action_error(action: CloudPcAction, status: u16, body: &str) -> AppError {
    let json = serde_json::from_str::<serde_json::Value>(body).ok();
    let code = json.as_ref().and_then(|v| {
        v["actionResultCode"]
            .as_i64()
            .or_else(|| v["error"]["actionResultCode"].as_i64())
    });
    let message = match code {
        Some(2) => "This Cloud PC no longer matches the list; refresh and try again.".into(),
        Some(3) => "The Windows 365 service is unavailable. Try again later.".into(),
        Some(4 | 409) => "Another action is already running on this Cloud PC.".into(),
        Some(5) => "Your organization does not allow this action.".into(),
        Some(6) => "This action is not available for Frontline Cloud PCs.".into(),
        Some(2001) => "That name is not allowed. Use letters, numbers, spaces and hyphens.".into(),
        _ if status == 409 => "Another action is already running on this Cloud PC.".into(),
        _ if status == 403 => "You are not allowed to do this on this Cloud PC.".into(),
        _ => {
            let detail = json
                .as_ref()
                .and_then(|v| {
                    v["error"]["message"]
                        .as_str()
                        .or_else(|| v["message"].as_str())
                })
                .map(str::to_string)
                .unwrap_or_else(|| format!("HTTP {status}"));
            format!("{} failed: {detail}", action.progress_label())
        }
    };
    AppError::Service(message)
}

/// Interprets the last action result once it belongs to the troubleshoot started at
/// `started` and has finished.
fn troubleshoot_outcome(
    result: &RemoteActionResult,
    started: chrono::DateTime<chrono::Utc>,
) -> Option<TroubleshootOutcome> {
    if !(result.is_for("Troubleshoot") || result.is_for("Diagnose")) || !result.is_finished() {
        return None;
    }
    // An earlier troubleshoot's result is still reported until the new one registers.
    let when = result
        .last_updated_date_time
        .as_deref()
        .or(result.start_date_time.as_deref())
        .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok());
    if when.is_some_and(|t| t < started - chrono::Duration::minutes(1)) {
        return None;
    }
    let code = result.code().filter(|c| !c.is_empty());
    Some(if result.succeeded() {
        match code {
            Some(c) if !c.eq_ignore_ascii_case("Available") => TroubleshootOutcome::Fixed,
            _ => TroubleshootOutcome::NoIssues,
        }
    } else {
        TroubleshootOutcome::Failed(troubleshoot_failure_message(code.unwrap_or_default()))
    })
}

fn troubleshoot_failure_message(code: &str) -> String {
    let ask_admin = |what: &str| {
        format!("You can't connect to this Cloud PC right now. Ask your admin to {what}.")
    };
    match code {
        "ErrorDomainJoinedFailed" => ask_admin("join this Cloud PC to a domain"),
        "ErrorDomainReachableFailed" => ask_admin("resolve issues connecting to the domain"),
        "ErrorDomainTrustFailed" => ask_admin("resolve the domain trust issue"),
        "ErrorUrlsNotAccessible" => {
            ask_admin("unblock the URLs that your Cloud PC uses to connect")
        }
        "ErrorDenyTSConnections" => {
            ask_admin("update the group policy to allow remote connections")
        }
        "ErrorVMDNSIssue" => ask_admin("resolve DNS issues"),
        "ErrorVMNetworkIssue" => ask_admin("resolve network issues"),
        "ErrorNotEnoughDiskSpace" => {
            "Your Cloud PC is out of disk space. Free up space or contact your admin.".into()
        }
        "ErrorSxsListenerNotReady" | "ErrorRDAgentBootLoaderStopped" | "ErrorVMAgentUnavailable" => {
            "You can't connect to this Cloud PC right now. Try resetting your Cloud PC or contact your admin.".into()
        }
        _ => "Something's preventing you from connecting to this Cloud PC. Try again in a few minutes or contact your admin.".into(),
    }
}

/// Extracts `(tenant id, resource id)` from the Cloud PC's direct-launch URL
/// (`…/tenants/{tenant}/resources/{resource}?…`).
fn launch_ids(pc: &CloudPc) -> Result<(String, String)> {
    let url = pc
        .direct_launch_url
        .as_deref()
        .and_then(|u| Url::parse(u).ok())
        .ok_or_else(|| {
            AppError::Config(format!(
                "'{}' has no direct-launch URL; it may still be provisioning",
                pc.display_name
            ))
        })?;
    let segments: Vec<&str> = url
        .path_segments()
        .map(Iterator::collect)
        .unwrap_or_default();
    let after = |name: &str| {
        segments
            .windows(2)
            .find(|w| w[0].eq_ignore_ascii_case(name) && !w[1].is_empty())
            .map(|w| w[1].to_string())
    };
    let tenant = after("tenants")
        .ok_or_else(|| AppError::Config("direct-launch URL does not identify the tenant".into()))?;
    let resource = after("resources").unwrap_or_else(|| pc.id.clone());
    Ok((tenant, resource))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pc(url: Option<&str>) -> CloudPc {
        serde_json::from_value(json!({
            "id": "pc-id",
            "displayName": "PC",
            "directLaunchURL": url,
        }))
        .unwrap()
    }

    #[test]
    fn parses_launch_ids() {
        let p = pc(Some(
            "https://rdweb.wvd.microsoft.com/api/arm/weblaunch/tenants/t-1/resources/r-2?x=1",
        ));
        assert_eq!(launch_ids(&p).unwrap(), ("t-1".into(), "r-2".into()));

        let p = pc(Some(
            "https://rdweb.wvd.microsoft.com/api/arm/weblaunch/tenants/t-1",
        ));
        assert_eq!(launch_ids(&p).unwrap(), ("t-1".into(), "pc-id".into()));
    }

    #[test]
    fn rejects_missing_tenant() {
        assert!(launch_ids(&pc(None)).is_err());
        assert!(launch_ids(&pc(Some("https://rdweb.wvd.microsoft.com/x/resources/r"))).is_err());
    }

    #[test]
    fn maps_action_result_codes() {
        let msg = |code: i64| {
            action_error(
                CloudPcAction::Reset,
                400,
                &json!({ "actionResultCode": code }).to_string(),
            )
            .to_string()
        };
        assert!(msg(4).contains("already running"));
        assert!(msg(6).contains("Frontline"));
        assert!(msg(2001).contains("name"));
        let other = action_error(CloudPcAction::Reset, 500, r#"{"error":{"message":"boom"}}"#);
        assert_eq!(other.to_string(), "Resetting failed: boom");
    }

    #[test]
    fn troubleshoot_results() {
        let started: chrono::DateTime<chrono::Utc> =
            chrono::DateTime::parse_from_rfc3339("2025-01-01T10:00:00Z")
                .unwrap()
                .into();
        let result = |name: &str, state: &str, code: Option<&str>, at: &str| RemoteActionResult {
            action_name: Some(name.into()),
            action_state: Some(state.into()),
            start_date_time: None,
            last_updated_date_time: Some(at.into()),
            status_details: Some(crate::models::ActionStatusDetails {
                code: code.map(str::to_string),
                message: None,
            }),
        };
        let now = "2025-01-01T10:05:00Z";
        assert_eq!(
            troubleshoot_outcome(&result("Troubleshoot", "active", None, now), started),
            None
        );
        assert_eq!(
            troubleshoot_outcome(&result("Restart", "done", None, now), started),
            None
        );
        assert_eq!(
            troubleshoot_outcome(
                &result("Troubleshoot", "done", None, "2024-12-01T00:00:00Z"),
                started
            ),
            None,
            "stale result"
        );
        assert_eq!(
            troubleshoot_outcome(
                &result("Troubleshoot", "done", Some("Available"), now),
                started
            ),
            Some(TroubleshootOutcome::NoIssues)
        );
        assert_eq!(
            troubleshoot_outcome(
                &result("Troubleshoot", "done", Some("Repaired"), now),
                started
            ),
            Some(TroubleshootOutcome::Fixed)
        );
        match troubleshoot_outcome(
            &result("Troubleshoot", "failed", Some("ErrorVMDNSIssue"), now),
            started,
        ) {
            Some(TroubleshootOutcome::Failed(m)) => assert!(m.contains("DNS")),
            other => panic!("{other:?}"),
        }
    }
}
