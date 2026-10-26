//! Preparing connections: launch details from the service, the connection's own
//! settings, the commands before and after a session, and the connection history.
//!
//! Frontends supply the display the session will open on ([`DisplayInfo`]) and run the
//! async functions on their event loop with the Tokio runtime entered.

use crate::api::{AvdResource, CloudPcService};
use crate::auth::account;
use crate::config::AppConfig;
use crate::connections::SavedConnection;
use crate::history::RecentStore;
use crate::launcher::LaunchTarget;
use crate::models::CloudPc;
use crate::overrides::{ConnectionOverrides, OverrideStore};
use crate::platform;
use crate::platform::hooks::{self, HookContext};
use crate::session::{self, Credentials, PreparedSession, SessionOptions};

/// Logical size of a new session window; the remote desktop follows the real size.
pub const SESSION_WIDTH: u32 = 1280;
pub const SESSION_HEIGHT: u32 = 800;

/// The display a session opens on.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DisplayInfo {
    /// Monitor size in device pixels, if known.
    pub device_size: Option<(u32, u32)>,
    /// Display scale in percent, if known (falls back to the OS setting, then 100%).
    pub scale_percent: Option<u32>,
}

impl DisplayInfo {
    /// Effective desktop scale for the session.
    #[must_use]
    pub fn scale(self) -> u32 {
        session::clamp_scale(
            self.scale_percent
                .or_else(platform::desktop_scale_percent)
                .unwrap_or(100),
        )
    }
}

/// A connection about to start, for hooks, history and the session window.
#[derive(Debug, Clone)]
pub struct Launch {
    pub name: String,
    /// `None` for connections that are not saved (quick connect, opened `.rdp` files).
    pub target: Option<LaunchTarget>,
    pub address: String,
    pub login_hint: Option<String>,
    pub overrides: ConnectionOverrides,
    pub multi_monitor: bool,
    pub dynamic_resolution: bool,
}

impl Launch {
    #[must_use]
    pub fn hook_context(&self) -> HookContext {
        HookContext {
            name: self.name.clone(),
            address: self.address.clone(),
            target: self
                .target
                .as_ref()
                .map(ToString::to_string)
                .unwrap_or_default(),
        }
    }
}

/// Session options from the preferences, sized for the session window (which opens
/// full screen) so the host starts at the final size.
#[must_use]
pub fn session_options(
    config: &AppConfig,
    display: DisplayInfo,
    username: Option<String>,
) -> SessionOptions {
    let scale = display.scale();
    let (width, height) = display
        .device_size
        .filter(|(w, h)| *w > 0 && *h > 0)
        .unwrap_or((SESSION_WIDTH * scale / 100, SESSION_HEIGHT * scale / 100));
    SessionOptions::from_config(config, width, height, scale, username)
}

/// Applies the connection's settings to the preferences: the configuration for the
/// session arguments and the session options.
#[must_use]
pub fn launch_options(
    overrides: &ConnectionOverrides,
    display: DisplayInfo,
    username: Option<String>,
) -> (AppConfig, SessionOptions) {
    let config = overrides.apply(&AppConfig::load());
    let options = session_options(&config, display, username).with_resolution(overrides.resolution);
    (config, options)
}

/// A prepared connection: what to show and the engine arguments.
pub type Prepared = (Launch, PreparedSession);

/// Fetches the connection details of a Cloud PC and prepares its session.
pub async fn prepare_cloud_pc(
    service: &CloudPcService,
    pc: &CloudPc,
    display: DisplayInfo,
) -> Result<Prepared, String> {
    let launch = service.fetch_rdp_launch(pc).await;
    let login_hint = pc.user_principal_name.clone().or_else(account::current_upn);
    let target = LaunchTarget::CloudPc(pc.id.clone());
    finish_cloud(launch, target, &pc.display_name, login_hint, display)
}

/// Downloads the signed `.rdp` file of an Azure Virtual Desktop resource and prepares
/// its session.
pub async fn prepare_avd(
    service: &CloudPcService,
    resource: &AvdResource,
    display: DisplayInfo,
) -> Result<Prepared, String> {
    let Some(rdp_url) = resource.rdp_url.as_deref() else {
        return Err(format!(
            "{} cannot be opened: the workspace did not publish a connection file.",
            resource.title
        ));
    };
    let launch = service.fetch_rdp_file(rdp_url).await;
    let target = LaunchTarget::Avd(resource.id.clone());
    finish_cloud(
        launch,
        target,
        &resource.title,
        account::current_upn(),
        display,
    )
}

fn finish_cloud(
    launch: crate::Result<crate::api::RdpLaunch>,
    target: LaunchTarget,
    name: &str,
    login_hint: Option<String>,
    display: DisplayInfo,
) -> Result<Prepared, String> {
    let overrides = OverrideStore::default().get(&target);
    let (config, options) = launch_options(&overrides, display, login_hint.clone());
    let prepared = launch.map_err(|e| e.to_string()).and_then(|launch| {
        session::prepare(&launch, &options)
            .map_err(|e| format!("could not write the session file: {e}"))
    })?;
    Ok((
        Launch {
            name: name.to_string(),
            target: Some(target),
            address: String::new(),
            login_hint,
            overrides,
            multi_monitor: config.multi_monitor,
            dynamic_resolution: options.dynamic_resolution,
        },
        prepared,
    ))
}

/// Prepares a direct session to `connection` with the credentials the user entered.
/// `saved`: the connection is in the library (it has history and its own settings).
pub fn prepare_remote(
    connection: &SavedConnection,
    credentials: &Credentials,
    saved: bool,
    display: DisplayInfo,
) -> Result<Prepared, String> {
    let target = LaunchTarget::RemotePc(connection.id.clone());
    let overrides = if saved {
        OverrideStore::default().get(&target)
    } else {
        ConnectionOverrides::default()
    };
    let username = Some(connection.username.clone()).filter(|u| !u.is_empty());
    let (config, options) = launch_options(&overrides, display, username);
    let prepared =
        session::prepare_direct(connection, credentials, &options).map_err(|e| e.to_string())?;
    Ok((
        Launch {
            name: connection.display_name().to_string(),
            target: saved.then_some(target),
            address: connection.address.clone(),
            login_hint: None,
            overrides,
            multi_monitor: config.multi_monitor,
            dynamic_resolution: options.dynamic_resolution,
        },
        prepared,
    ))
}

/// Runs the connection's command before connecting, if any (off the UI thread).
pub async fn run_pre_command(launch: &Launch) -> Result<(), String> {
    let command = launch.overrides.pre_command.clone();
    if command.is_empty() {
        return Ok(());
    }
    let context = launch.hook_context();
    let name = launch.name.clone();
    tokio::task::spawn_blocking(move || {
        hooks::run_pre_connect(&command, &context, hooks::PRE_COMMAND_TIMEOUT)
    })
    .await
    .map_err(|_| "The command before connecting failed.".to_string())?
    .map_err(|e| {
        log::warn!("Command before connecting to {name} failed: {e}");
        format!("Not connecting to {name}: {e}")
    })
}

/// Records that the session started (for "Last used").
pub fn record_start(launch: &Launch) {
    if let Some(target) = &launch.target {
        if let Err(e) = RecentStore::default().record(target, chrono::Utc::now()) {
            log::warn!("Could not record the connection time: {e}");
        }
    }
}

/// Runs the connection's command after disconnecting, if any.
pub fn run_post_command(launch: &Launch) {
    let command = &launch.overrides.post_command;
    if command.is_empty() {
        return;
    }
    if let Err(e) = hooks::spawn_post_disconnect(command, &launch.hook_context()) {
        log::warn!(
            "Command after disconnecting from {} failed: {e}",
            launch.name
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_from_display() {
        let config = AppConfig::default();
        let display = DisplayInfo {
            device_size: Some((2560, 1440)),
            scale_percent: Some(150),
        };
        let options = session_options(&config, display, None);
        assert_eq!((options.width, options.height), (2560, 1440));
        assert_eq!(options.scale_percent, 150);

        let display = DisplayInfo {
            device_size: None,
            scale_percent: Some(200),
        };
        let options = session_options(&config, display, None);
        assert_eq!((options.width, options.height), (2560, 1600));
    }

    #[test]
    fn hook_context_uses_target() {
        let launch = Launch {
            name: "PC".into(),
            target: Some(LaunchTarget::RemotePc("abc".into())),
            address: "host".into(),
            login_hint: None,
            overrides: ConnectionOverrides::default(),
            multi_monitor: false,
            dynamic_resolution: true,
        };
        let context = launch.hook_context();
        assert_eq!(context.target, "remote:abc");
    }
}
