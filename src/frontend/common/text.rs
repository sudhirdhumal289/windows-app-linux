//! User-facing text shared by all frontends, so every platform words things the same.

use crate::api::{Snapshot, TroubleshootOutcome};
use crate::config::AppConfig;
use crate::error::AppError;
use crate::models::CloudPc;
use crate::{platform, rdp, rdp_file};

/// Application name shown in windows, menus and the About box.
pub const APP_NAME: &str = "Windows App";
pub const REPOSITORY_URL: &str = "https://github.com/sudhirdhumal289/windows-app-linux";
pub const DEVELOPER: &str = "Sudhir Dhumal";
pub const COPYRIGHT: &str = "© 2026 Sudhir Dhumal";
pub const ABOUT_COMMENTS: &str = "Connect to Windows 365 Cloud PCs, Azure Virtual Desktop and \
     any Remote Desktop (RDP) computer.";
pub const TRADEMARKS: &str = "Windows, Windows 365 and Azure Virtual Desktop are trademarks of \
     the Microsoft group of companies. This app is not affiliated with or endorsed by Microsoft.";

/// Local, human-readable time for an RFC 3339 timestamp (unparsable values as-is).
#[must_use]
pub fn format_time(value: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(value).map_or_else(
        |_| value.to_string(),
        |t| {
            t.with_timezone(&chrono::Local)
                .format("%e %b %Y, %H:%M")
                .to_string()
                .trim()
                .to_string()
        },
    )
}

/// Label of a restore point: its time, marked when it was taken manually.
#[must_use]
pub fn snapshot_label(snapshot: &Snapshot) -> String {
    let when = snapshot
        .created_date_time
        .as_deref()
        .map_or_else(|| snapshot.id.clone(), format_time);
    match snapshot.snapshot_type.as_deref() {
        Some("manual") => format!("{when} (manual)"),
        _ => when,
    }
}

/// Status line after loading `pcs` Cloud PCs and `avd` Azure Virtual Desktop resources.
#[must_use]
pub fn load_summary(pcs: usize, avd: usize) -> String {
    let plural = |n: usize, one: &str, many: &str| {
        if n == 1 {
            format!("1 {one}")
        } else {
            format!("{n} {many}")
        }
    };
    match (pcs, avd) {
        (0, 0) => "No Cloud PCs or Azure Virtual Desktop resources are assigned to this account."
            .to_string(),
        (pcs, 0) => format!("Loaded {}", plural(pcs, "Cloud PC", "Cloud PCs")),
        (0, avd) => format!(
            "Loaded {}",
            plural(avd, "desktop or app", "desktops and apps")
        ),
        (pcs, avd) => format!(
            "Loaded {} and {}",
            plural(pcs, "Cloud PC", "Cloud PCs"),
            plural(avd, "desktop or app", "desktops and apps")
        ),
    }
}

#[must_use]
pub fn or_not_reported(value: Option<&String>) -> String {
    value
        .filter(|v| !v.trim().is_empty())
        .cloned()
        .unwrap_or_else(|| "Not reported".to_string())
}

/// Short, user-facing message for a failed Cloud PC listing.
#[must_use]
pub fn load_error_message(error: &AppError) -> String {
    match error {
        AppError::GraphApi { status, message } => {
            serde_json::from_str::<serde_json::Value>(message)
                .ok()
                .and_then(|v| v["error"]["message"].as_str().map(str::to_string))
                .map_or_else(
                    || format!("Unable to load Cloud PCs (HTTP {status})"),
                    |m| format!("Unable to load Cloud PCs: {m}"),
                )
        }
        _ => format!("Unable to load Cloud PCs: {error}"),
    }
}

/// "A", "A and B", "A, B and C".
#[must_use]
pub fn join_names(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [only] => only.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// `name` without characters that are not allowed in file names.
#[must_use]
pub fn sanitize_file_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c == '/' || c == '\\' || c.is_control() {
                '_'
            } else {
                c
            }
        })
        .collect();
    let cleaned = cleaned.trim().trim_start_matches('.').to_string();
    if cleaned.is_empty() {
        "Remote PC".to_string()
    } else {
        cleaned
    }
}

/// Longest `.rdp` file accepted.
pub const MAX_RDP_FILE_BYTES: u64 = 1024 * 1024;

/// Reads and imports a `.rdp` file, named after the file; errors are user-facing.
pub fn read_rdp_file(path: &std::path::Path) -> Result<rdp_file::ImportedRdp, String> {
    let name = path
        .file_stem()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let too_big = std::fs::metadata(path).is_ok_and(|m| m.len() > MAX_RDP_FILE_BYTES);
    if too_big {
        return Err(format!("{} is too large for an RDP file.", path.display()));
    }
    let bytes =
        std::fs::read(path).map_err(|e| format!("Could not read {}: {e}", path.display()))?;
    rdp_file::decode(&bytes)
        .and_then(|text| rdp_file::import(&text, &name))
        .map_err(|e| format!("Could not import {}: {e}", path.display()))
}

/// Heading and body reporting the outcome of a troubleshooting run on `name`.
#[must_use]
pub fn troubleshoot_report(
    name: &str,
    outcome: &crate::Result<TroubleshootOutcome>,
) -> (String, String) {
    match outcome {
        Ok(TroubleshootOutcome::NoIssues) => (
            format!("No issues found on {name}"),
            "The Cloud PC passed all checks. If you still cannot connect, restart it.".to_string(),
        ),
        Ok(TroubleshootOutcome::Fixed) => (
            format!("Issues fixed on {name}"),
            "Problems were found and fixed. Try connecting again.".to_string(),
        ),
        Ok(TroubleshootOutcome::Failed(detail)) => {
            (format!("Issues remain on {name}"), detail.clone())
        }
        Ok(TroubleshootOutcome::StillRunning) => (
            format!("Still troubleshooting {name}"),
            "The check is taking longer than usual. Look at the status again later.".to_string(),
        ),
        Err(e) => (format!("Could not troubleshoot {name}"), e.to_string()),
    }
}

/// Bodies of the confirmations for destructive Cloud PC actions.
pub const RESTART_BODY: &str =
    "Unsaved work on the Cloud PC is lost and open sessions are disconnected.";
pub const RESTORE_BODY: &str = "The Cloud PC returns to the state of the chosen restore point. \
     Files, apps and settings changed since then are lost, and you are disconnected.";
pub const RESET_BODY: &str = "Resetting reinstalls Windows. All your files, apps and settings \
     on this Cloud PC are permanently removed. This cannot be undone.";

/// Heading and body asking whether quitting should disconnect the open sessions `names`.
#[must_use]
pub fn quit_prompt(names: &[String]) -> (String, String) {
    match names {
        [name] => (
            "Disconnect Session?".to_string(),
            format!(
                "{name} is still connected. Quitting disconnects it; keeping it open closes \
                 only this window."
            ),
        ),
        _ => (
            format!("Disconnect {} Sessions?", names.len()),
            format!(
                "{} are still connected. Quitting disconnects them; keeping them open closes \
                 only this window.",
                join_names(names)
            ),
        ),
    }
}

/// Text of the empty main window.
#[must_use]
pub fn empty_description(signed_in: bool) -> &'static str {
    if signed_in {
        "No Cloud PCs are assigned to your account. Use the + button to add a Remote PC."
    } else {
        "Use the + button to add a Microsoft Cloud PC or a Remote PC."
    }
}

pub const EMPTY_TITLE: &str = "No Connections Yet";

/// Rows of the "Inspect Connection" sheet of a Cloud PC.
#[must_use]
pub fn inspect_rows(pc: &CloudPc) -> Vec<(&'static str, String)> {
    let config = AppConfig::load();
    let scale = match platform::desktop_scale_percent() {
        Some(detected) => format!("Matches display ({detected}%); change it in the session"),
        None => "Matches display; change it in the session".to_string(),
    };
    vec![
        ("Cloud PC Name", pc.display_name.clone()),
        ("Cloud PC ID", pc.id.clone()),
        (
            "Assigned User",
            or_not_reported(pc.user_principal_name.as_ref()),
        ),
        (
            "RDP Engine",
            format!("FreeRDP {} (embedded)", rdp::version()),
        ),
        ("Sign-in Profile", config.auth_profile().label().to_string()),
        ("Desktop Scale", scale),
        ("Host Pool", or_not_reported(pc.host_pool_id.as_ref())),
        ("Region", or_not_reported(pc.device_region_name.as_ref())),
    ]
}

/// Rows of the "View Details" sheet of a Cloud PC (status, licence, provisioning, ...).
#[must_use]
pub fn detail_rows(pc: &CloudPc) -> Vec<(&'static str, String)> {
    let os = pc.os_name.as_ref().or(pc.image_display_name.as_ref());
    let now = chrono::Utc::now();
    let last_action = pc.last_remote_action_result.as_ref().map(|r| {
        let mut text = format!(
            "{} — {}",
            r.action_name.as_deref().unwrap_or("Action"),
            r.action_state.as_deref().unwrap_or("unknown")
        );
        if let Some(when) = r
            .last_updated_date_time
            .as_deref()
            .or(r.start_date_time.as_deref())
        {
            text.push_str(&format!(" ({})", format_time(when)));
        }
        if let Some(message) = r.status_details.as_ref().and_then(|d| d.message.as_deref()) {
            text.push_str(&format!("\n{message}"));
        }
        text
    });
    let progress = pc
        .provisioning_progress
        .filter(|p| *p > 0 && *p < 100)
        .map(|p| format!(" ({p}%)"))
        .unwrap_or_default();
    vec![
        ("Device Name", pc.display_name.clone()),
        ("Operating System", or_not_reported(os)),
        ("Hardware", pc.hardware_summary().to_string()),
        ("Status", pc.status_summary(now).0),
        ("Licence", pc.license.label().to_string()),
        ("Power State", or_not_reported(pc.power_state.as_ref())),
        (
            "Provisioning",
            format!(
                "{}{progress}",
                or_not_reported(pc.provisioning_status.as_ref())
            ),
        ),
        (
            "Grace Period Ends",
            pc.grace_period_end
                .as_deref()
                .map_or_else(|| "Not in a grace period".to_string(), format_time),
        ),
        (
            "Last Sign-in",
            match (pc.last_login.as_deref(), pc.last_login_result.as_deref()) {
                (Some(when), Some(result)) => format!("{} — {result}", format_time(when)),
                (Some(when), None) => format_time(when),
                _ => "Not reported".to_string(),
            },
        ),
        (
            "Last Action",
            last_action.unwrap_or_else(|| "None".to_string()),
        ),
        (
            "Disk Encryption",
            or_not_reported(pc.disk_encryption_state.as_ref()),
        ),
        (
            "User Principal Name",
            or_not_reported(pc.user_principal_name.as_ref()),
        ),
        ("Device ID", pc.id.clone()),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn joins_names() {
        assert_eq!(join_names(&[]), "");
        assert_eq!(join_names(&["A".into()]), "A");
        assert_eq!(join_names(&["A".into(), "B".into()]), "A and B");
        assert_eq!(
            join_names(&["A".into(), "B".into(), "C".into()]),
            "A, B and C"
        );
    }

    #[test]
    fn sanitizes_file_names() {
        assert_eq!(sanitize_file_name("a/b\\c"), "a_b_c");
        assert_eq!(sanitize_file_name("  .hidden "), "hidden");
        assert_eq!(sanitize_file_name(" .. "), "Remote PC");
    }

    #[test]
    fn summarizes_loads() {
        assert_eq!(load_summary(1, 0), "Loaded 1 Cloud PC");
        assert_eq!(
            load_summary(2, 1),
            "Loaded 2 Cloud PCs and 1 desktop or app"
        );
    }

    #[test]
    fn words_quit_prompt() {
        let (heading, _) = quit_prompt(&["A".into()]);
        assert_eq!(heading, "Disconnect Session?");
        let (heading, body) = quit_prompt(&["A".into(), "B".into()]);
        assert_eq!(heading, "Disconnect 2 Sessions?");
        assert!(body.starts_with("A and B are"));
    }
}
