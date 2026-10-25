//! Native RDP session launcher for Linux using FreeRDP 3 (Wayland & X11).

pub mod bridge;
pub use bridge::NativeRdpBridge;

use crate::error::{AppError, Result};
use crate::models::RdpConnectionConfig;
use log::{debug, trace, warn};
use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::{FromRawFd, RawFd};
use std::path::PathBuf;
use std::process::{Child, Command};
use std::sync::mpsc::Receiver;

pub struct RdpSession {
    pub child: Child,
    pub pty_master: RawFd,
    pub auth_url_rx: Receiver<String>,
}

pub struct RdpLauncher;

impl RdpLauncher {
    /// Checks if the current desktop environment is a Wayland session.
    #[must_use]
    pub fn is_wayland_session() -> bool {
        if std::env::var("WAYLAND_DISPLAY")
            .map(|v| !v.trim().is_empty())
            .unwrap_or(false)
        {
            return true;
        }
        if let Ok(session_type) = std::env::var("XDG_SESSION_TYPE") {
            if session_type.eq_ignore_ascii_case("wayland") {
                return true;
            }
        }
        false
    }

    /// Checks if the current desktop environment is GNOME.
    #[must_use]
    pub fn is_gnome_desktop() -> bool {
        if let Ok(desktop) = std::env::var("XDG_CURRENT_DESKTOP") {
            if desktop.to_ascii_uppercase().contains("GNOME") {
                return true;
            }
        }
        if let Ok(desktop) = std::env::var("XDG_SESSION_DESKTOP") {
            if desktop.to_ascii_uppercase().contains("GNOME") {
                return true;
            }
        }
        false
    }

    /// Checks if a command binary is accessible in PATH.
    fn check_binary_exists(bin: &str) -> bool {
        if let Ok(path_var) = std::env::var("PATH") {
            for dir in std::env::split_paths(&path_var) {
                let full_path = dir.join(bin);
                if full_path.is_file() {
                    return true;
                }
            }
        }
        false
    }

    /// Detects Wayland-native FreeRDP client binary (wlfreerdp3 or wlfreerdp).
    #[must_use]
    pub fn detect_wayland_freerdp() -> Option<&'static str> {
        for &bin in &["wlfreerdp3", "wlfreerdp"] {
            if Self::check_binary_exists(bin) {
                return Some(bin);
            }
        }
        None
    }

    /// Detects X11 FreeRDP client binary (xfreerdp3 or xfreerdp).
    #[must_use]
    pub fn detect_x11_freerdp() -> Option<&'static str> {
        for &bin in &["xfreerdp3", "xfreerdp"] {
            if Self::check_binary_exists(bin) {
                return Some(bin);
            }
        }
        None
    }

    /// Detects the best available FreeRDP binary.
    /// Note on GNOME Wayland: GNOME's Mutter compositor intentionally does not support
    /// Server-Side Decorations (`xdg-decoration`) for native Wayland clients. Because `wlfreerdp`
    /// relies on SSD and does not draw Client-Side Decorations (CSD), it renders as a borderless canvas
    /// without a window title bar or window controls.
    /// On GNOME Wayland, `xfreerdp3` (via XWayland) provides the full native GNOME title bar,
    /// minimize/maximize/close buttons, window borders, and window dragging.
    #[must_use]
    pub fn detect_freerdp_client() -> Option<&'static str> {
        if Self::is_wayland_session() {
            if Self::is_gnome_desktop() {
                // On GNOME Wayland, prefer xfreerdp3 for full window decorations and title bar
                Self::detect_x11_freerdp().or_else(Self::detect_wayland_freerdp)
            } else {
                Self::detect_wayland_freerdp().or_else(Self::detect_x11_freerdp)
            }
        } else {
            Self::detect_x11_freerdp().or_else(Self::detect_wayland_freerdp)
        }
    }

    /// Checks if ANY supported RDP client (FreeRDP in-app engine or CLI binary) is available on the system.
    #[must_use]
    pub fn detect_any_rdp_client() -> Option<&'static str> {
        // Native FreeRDP 3 C-FFI engine is built into the application
        Some("freerdp")
    }

    /// Resolves the concrete executable client binary based on user preference and system availability.
    #[must_use]
    pub fn resolve_client(preferred: &str) -> Option<&'static str> {
        match preferred {
            "ironrdp" => None, // Pure Rust internal engine
            _ => Some("freerdp"), // Embedded in-app FreeRDP 3 engine
        }
    }

    /// Detects installed platform-provided RDP client binary in order of preference.
    #[must_use]
    pub fn detect_rdp_client() -> Option<&'static str> {
        let config = crate::config::AppConfig::load();
        Self::resolve_client(&config.preferred_rdp_client).or_else(Self::detect_any_rdp_client)
    }

    /// Sets up FreeRDP command execution environment using standard system paths.
    fn configure_client_env(_cmd: &mut Command) {
        // Relies strictly on platform-provided system PATH and dynamic linker paths.
    }

    /// Detects the host desktop scaling factor (percentage, e.g. 100, 125, 150, 200).
    /// Inspects GNOME monitors.xml, gsettings, or defaults to 125 for HiDPI displays.
    pub fn get_system_desktop_scale() -> u32 {
        if let Ok(home) = std::env::var("HOME") {
            let monitors_path = PathBuf::from(home).join(".config/monitors.xml");
            if let Ok(content) = std::fs::read_to_string(monitors_path) {
                if let Some(scale) = Self::parse_scale_from_monitors_xml(&content) {
                    debug!("Detected display desktop scale: {scale}% from ~/.config/monitors.xml");
                    return scale;
                }
            }
        }

        if let Ok(output) = Command::new("gsettings")
            .args(["get", "org.gnome.desktop.interface", "text-scaling-factor"])
            .output()
        {
            if output.status.success() {
                let s = String::from_utf8_lossy(&output.stdout);
                if let Ok(factor) = s.trim().parse::<f64>() {
                    if factor > 1.05 {
                        let scale = (factor * 100.0).round() as u32;
                        debug!("Detected text scaling factor: {scale}% from gsettings");
                        return scale;
                    }
                }
            }
        }

        if let Ok(output) = Command::new("gsettings")
            .args(["get", "org.gnome.desktop.interface", "scaling-factor"])
            .output()
        {
            if output.status.success() {
                let s = String::from_utf8_lossy(&output.stdout);
                let cleaned = s.trim().replace("uint32", "");
                if let Ok(factor) = cleaned.trim().parse::<u32>() {
                    if factor > 1 {
                        let scale = factor * 100;
                        debug!("Detected integer scaling factor: {scale}% from gsettings");
                        return scale;
                    }
                }
            }
        }

        debug!("Defaulting display desktop scale to 125%");
        125
    }

    /// Extracts scale percentage from GNOME ~/.config/monitors.xml.
    /// Prefers the primary logical monitor if designated.
    pub fn parse_scale_from_monitors_xml(xml: &str) -> Option<u32> {
        let mut first_scale = None;
        for lm in xml.split("<logicalmonitor>") {
            if let Some(end_lm) = lm.find("</logicalmonitor>") {
                let block = &lm[..end_lm];
                let is_primary = block.contains("<primary>yes</primary>");
                if let Some(start_scale) = block.find("<scale>") {
                    let after = &block[start_scale + "<scale>".len()..];
                    if let Some(end_scale) = after.find("</scale>") {
                        let scale_str = after[..end_scale].trim();
                        if let Ok(factor) = scale_str.parse::<f64>() {
                            let percent = (factor * 100.0).round() as u32;
                            if (100..=500).contains(&percent) {
                                if is_primary {
                                    return Some(percent);
                                }
                                if first_scale.is_none() {
                                    first_scale = Some(percent);
                                }
                            }
                        }
                    }
                }
            }
        }
        first_scale
    }

    /// Generates RDP file content for direct connection with gateway parameters.
    pub fn generate_rdp_content(config: &RdpConnectionConfig) -> String {
        let mut rdp = String::new();

        rdp.push_str(&format!("full address:s:{}\n", config.full_address));
        rdp.push_str(&format!("desktopwidth:i:{}\n", config.display_width));
        rdp.push_str(&format!("desktopheight:i:{}\n", config.display_height));
        rdp.push_str(&format!("desktopscalefactor:i:{}\n", config.scale_factor));

        if let Some(gw) = &config.gateway_hostname {
            rdp.push_str(&format!("gatewayhostname:s:{gw}\n"));
            rdp.push_str(&format!(
                "gatewayusagemethod:i:{}\n",
                config.gateway_usage_method
            ));
            rdp.push_str("gatewayprofileusagemethod:i:1\n");
        }

        if let Some(user) = &config.username {
            rdp.push_str(&format!("username:s:{user}\n"));
        }

        rdp
    }

    /// Creates a temporary .rdp file on the client system.
    pub fn create_temp_rdp_file(id: &str, config: &RdpConnectionConfig) -> Result<PathBuf> {
        let tmp_dir = std::env::temp_dir();
        let file_path = tmp_dir.join(format!("cloudpc-{id}.rdp"));
        let mut file = File::create(&file_path)?;
        let content = Self::generate_rdp_content(config);
        file.write_all(content.as_bytes())?;
        Ok(file_path)
    }

    /// Converts a signed AVD RDP file into an NLA direct-connect RDP file:
    /// - Disables `enablerdsaadauth` (set to 0) so the session host does not demand
    ///   Entra ID PoP device compliance / MDM tokens that personal Linux devices cannot satisfy.
    /// - Strips `signscope:` and `signature:` lines so FreeRDP parses the modified config without signature errors.
    #[must_use]
    pub fn prepare_nla_rdp_content(rdp_content: &str) -> String {
        let mut lines = Vec::new();
        let mut found_aad_auth = false;

        for line in rdp_content.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("signscope:") || trimmed.starts_with("signature:") {
                continue;
            }
            if trimmed.starts_with("enablerdsaadauth:") {
                lines.push("enablerdsaadauth:i:0".to_string());
                found_aad_auth = true;
                continue;
            }
            lines.push(line.to_string());
        }

        if !found_aad_auth {
            lines.push("enablerdsaadauth:i:0".to_string());
        }

        lines.join("\n") + "\n"
    }

    /// Launches a native RDP session with ARM Gateway bearer token and NLA host credentials using FreeRDP 3.
    pub fn launch_nla_session(
        id: &str,
        rdp_content: &str,
        avd_token: &str,
        username: &str,
        password: Option<&str>,
        preferred_client: Option<&str>,
    ) -> Result<Child> {
        let pref = preferred_client.unwrap_or("");
        let client = Self::resolve_client(pref).unwrap_or_else(|| {
            if Self::is_wayland_session() && !Self::is_gnome_desktop() {
                "wlfreerdp3"
            } else {
                "xfreerdp3"
            }
        });
        let scale_percent = Self::get_system_desktop_scale();
        debug!("Launching NLA session with client binary [{client}] (preferred='{pref}') at {scale_percent}% scale...");

        let modified_rdp = Self::prepare_nla_rdp_content(rdp_content);
        let tmp_dir = std::env::temp_dir();
        let rdp_path = tmp_dir.join(format!("cloudpc-{id}.rdp"));
        {
            let mut file = File::create(&rdp_path)?;
            file.write_all(modified_rdp.as_bytes())?;
        }
        debug!("Wrote NLA RDP configuration");

        let mut cmd = Command::new(client);
        Self::configure_client_env(&mut cmd);

        if client.contains("freerdp") {
            cmd.arg(&rdp_path);
            cmd.arg(format!("/gateway:type:arm,bearer:{avd_token}"));
            cmd.arg("/cert:ignore");
            cmd.arg(format!("/scale-desktop:{scale_percent}"));
            cmd.arg("/scale-device:100");
            cmd.arg("/size:90%");
            cmd.arg("/dynamic-resolution");
            cmd.arg("/f");
            cmd.arg("/rfx");
            let app_config = crate::config::AppConfig::load();
            if app_config.enable_clipboard {
                cmd.arg("+clipboard");
            } else {
                cmd.arg("-clipboard");
            }
            cmd.arg("/sound:sys:pulse");
            cmd.arg("/microphone:sys:pulse");
            cmd.arg("/dvc:rdpecam");
            if app_config.enable_drive_redirection && !app_config.shared_folder_path.is_empty() {
                let p = std::path::Path::new(&app_config.shared_folder_path);
                if p.exists() {
                                        cmd.arg(format!("/drive:Shared,{}", app_config.shared_folder_path));
                }
            }
            cmd.arg("+decorations");
            cmd.arg("/t:Windows 365 Cloud PC");
            cmd.arg("/wm-class:windows-app-linux");
            cmd.arg(format!("/u:{username}"));
            if let Some(pass) = password {
                if !pass.is_empty() {
                    cmd.arg(format!("/p:{pass}"));
                }
            }
        } else {
            return Err(AppError::RdpLauncher(format!(
                "unsupported RDP client: {client}"
            )));
        }

        match cmd.spawn() {
            Ok(child) => {
                debug!("RDP process launched successfully (PID: {})", child.id());
                Ok(child)
            }
            Err(e) => {
                warn!("Failed to spawn {client}: {e}");
                Err(AppError::RdpLauncher(format!(
                    "failed to execute '{client}': {e}. Please ensure FreeRDP is installed."
                )))
            }
        }
    }

    /// Launches an AVD session with ARM gateway and interactive AAD auth capture.
    /// Azure Virtual Desktop / Cloud PC sessions use Microsoft ARM WebSockets Gateways (`rdgateway*.wvd.microsoft.com`),
    /// which require FreeRDP 3 (`xfreerdp3` for GNOME window decorations or `wlfreerdp3` for Wayland native canvas).
    pub fn launch_avd_session(
        id: &str,
        rdp_content: &str,
        avd_token: &str,
        username: Option<&str>,
        _tenant_id: Option<&str>,
        preferred_client: Option<&str>,
    ) -> Result<RdpSession> {
        let pref = preferred_client.unwrap_or("");
        let client = Self::resolve_client(pref).unwrap_or_else(|| {
            if Self::is_wayland_session() && !Self::is_gnome_desktop() {
                "wlfreerdp3"
            } else {
                "xfreerdp3"
            }
        });

        let scale_percent = Self::get_system_desktop_scale();
        let is_wayland = Self::is_wayland_session();
        debug!(
            "Launching AVD session with client binary [{client}] (wayland={is_wayland}) at {scale_percent}% scale..."
        );

        // Write the original signed RDP configuration unmodified.
        // Modifying the file by appending 'smart sizing' causes FreeRDP CLI to reject '/dynamic-resolution' as mutually exclusive.
        let tmp_dir = std::env::temp_dir();
        let rdp_path = tmp_dir.join(format!("cloudpc-{id}.rdp"));
        {
            let mut file = File::create(&rdp_path)?;
            file.write_all(rdp_content.as_bytes())?;
        }
        debug!("Wrote official signed RDP configuration to {:?}", rdp_path);

        // FreeRDP 3: Native Wayland (wlfreerdp3) or X11/XWayland (xfreerdp3) with ARM WebSockets & PTY OAuth prompt capture
        extern "C" {
            fn openpty(
                amaster: *mut std::os::raw::c_int,
                aslave: *mut std::os::raw::c_int,
                name: *mut std::os::raw::c_char,
                termp: *const std::ffi::c_void,
                winp: *const std::ffi::c_void,
            ) -> std::os::raw::c_int;
            fn dup(fd: std::os::raw::c_int) -> std::os::raw::c_int;
            fn close(fd: std::os::raw::c_int) -> std::os::raw::c_int;
            fn setsid() -> std::os::raw::c_int;
            fn signal(sig: std::os::raw::c_int, handler: usize) -> usize;
        }

        let mut master_fd: std::os::raw::c_int = 0;
        let mut slave_fd: std::os::raw::c_int = 0;
        let res = unsafe {
            openpty(
                &mut master_fd,
                &mut slave_fd,
                std::ptr::null_mut(),
                std::ptr::null(),
                std::ptr::null(),
            )
        };
        if res != 0 {
            return Err(AppError::RdpLauncher(format!(
                "Failed to allocate PTY for RDP process: errno {}",
                std::io::Error::last_os_error()
            )));
        }

        let slave_in = unsafe { std::process::Stdio::from_raw_fd(slave_fd) };
        let slave_out = unsafe { std::process::Stdio::from_raw_fd(dup(slave_fd)) };
        let slave_err = unsafe { std::process::Stdio::from_raw_fd(dup(slave_fd)) };

        let mut cmd = Command::new(client);
        Self::configure_client_env(&mut cmd);
        cmd.arg(&rdp_path);

        if client.contains("freerdp") {
            cmd.arg(format!("/gateway:type:arm,bearer:{avd_token}"));
            cmd.arg("/cert:ignore");
            cmd.arg(format!("/scale-desktop:{scale_percent}"));
            cmd.arg("/scale-device:100");
            cmd.arg("/size:90%");
            cmd.arg("/dynamic-resolution");
            cmd.arg("/f");
            cmd.arg("/rfx");
            let app_config = crate::config::AppConfig::load();
            if app_config.enable_clipboard {
                cmd.arg("+clipboard");
            } else {
                cmd.arg("-clipboard");
            }
            cmd.arg("/sound:sys:pulse");
            cmd.arg("/microphone:sys:pulse");
            cmd.arg("/dvc:rdpecam");
            if app_config.enable_drive_redirection && !app_config.shared_folder_path.is_empty() {
                let p = std::path::Path::new(&app_config.shared_folder_path);
                if p.exists() {
                                        cmd.arg(format!("/drive:Shared,{}", app_config.shared_folder_path));
                }
            }
            cmd.arg("+decorations");
            cmd.arg("/t:Windows 365 Cloud PC");
            cmd.arg("/wm-class:windows-app-linux");
            if let Some(user) = username {
                cmd.arg(format!("/u:{user}"));
            }
        }

        cmd.stdin(slave_in);
        cmd.stdout(slave_out);
        cmd.stderr(slave_err);

        // In Linux/Unix, background PTY processes executing tcsetattr() or write() can receive SIGTTOU / SIGTTIN (signal 22/21).
        // FreeRDP lists SIGTTOU in fatal_signals and terminates if not ignored.
        // We set a new session and ignore SIGTTOU/SIGTTIN in the spawned child.
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            unsafe {
                cmd.pre_exec(|| {
                    setsid();
                    signal(21, 1); // SIGTTIN = 21, SIG_IGN = 1
                    signal(22, 1); // SIGTTOU = 22, SIG_IGN = 1
                    Ok(())
                });
            }
        }

        let child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                unsafe {
                    close(master_fd);
                }
                return Err(AppError::RdpLauncher(format!(
                    "Failed to spawn {client}: {e}. Ensure FreeRDP 3 (freerdp3-x11 or freerdp3-wayland) is installed."
                )));
            }
        };

        debug!(
            "FreeRDP process launched (PID: {}) using client [{client}]",
            child.id()
        );

        let (auth_tx, auth_rx) = std::sync::mpsc::channel();
        let master_read_fd = master_fd;

        std::thread::spawn(move || {
            let mut file = unsafe { File::from_raw_fd(dup(master_read_fd)) };
            let mut buffer = [0u8; 4096];
            let mut line_buf = String::new();

            loop {
                match file.read(&mut buffer) {
                    Ok(0) => break, // EOF
                    Ok(n) => {
                        let text = String::from_utf8_lossy(&buffer[..n]);
                        for ch in text.chars() {
                            line_buf.push(ch);
                            if ch == '\n' || line_buf.contains("Paste redirect URL here:") {
                                let line = line_buf.trim();
                                if line.contains("Browse to:")
                                    || line.contains("Paste redirect")
                                    || line.contains("http")
                                {
                                    trace!("[FreeRDP] (authentication challenge prompt)");
                                } else {
                                    trace!("[FreeRDP] {line}");
                                }
                                if let Some(start) = line.find("Browse to:") {
                                    let after = &line[start + "Browse to:".len()..];
                                    let url_end = after
                                        .find("Paste redirect URL")
                                        .or_else(|| after.find('\n'))
                                        .unwrap_or(after.len());
                                    let trimmed_url = after[..url_end].trim().to_string();
                                    if !trimmed_url.is_empty() && trimmed_url.starts_with("http") {
                                        debug!("Extracted FreeRDP authentication challenge");
                                        let _ = auth_tx.send(trimmed_url);
                                    }
                                }
                                if ch == '\n' {
                                    line_buf.clear();
                                }
                            }
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        Ok(RdpSession {
            child,
            pty_master: master_fd,
            auth_url_rx: auth_rx,
        })
    }

    /// Sends the OAuth redirect URL response into the FreeRDP PTY.
    pub fn send_redirect_url(master_fd: RawFd, redirect_url: &str) -> std::io::Result<()> {
        if master_fd < 0 {
            return Ok(());
        }
        extern "C" {
            fn dup(fd: std::os::raw::c_int) -> std::os::raw::c_int;
        }
        let mut file = unsafe { File::from_raw_fd(dup(master_fd)) };
        let clean_url = redirect_url.trim();
        write!(file, "{clean_url}\n")?;
        file.flush()?;
        debug!("Sent authentication redirect response to FreeRDP session");
        Ok(())
    }

    /// Launches the RDP connection process using a fallback .rdp configuration file.
    pub fn launch_session(
        id: &str,
        config: &RdpConnectionConfig,
        preferred_client: Option<&str>,
    ) -> Result<Child> {
        let pref = preferred_client.unwrap_or("");
        let client = Self::resolve_client(pref).unwrap_or_else(|| {
            if Self::is_wayland_session() && !Self::is_gnome_desktop() {
                "wlfreerdp3"
            } else {
                "xfreerdp3"
            }
        });
        let scale_percent = Self::get_system_desktop_scale();
        debug!("Launching RDP session with client binary [{client}] (preferred='{pref}') at {scale_percent}% scale...");

        let rdp_path = Self::create_temp_rdp_file(id, config)?;
        debug!("Generated temporary RDP file");

        let mut cmd = Command::new(client);
        Self::configure_client_env(&mut cmd);

        if client.contains("freerdp") {
            cmd.arg(&rdp_path);
            cmd.arg("/gateway:type:arm");
            cmd.arg("/sec:aad");
            cmd.arg("/cert:ignore");
            cmd.arg(format!("/scale-desktop:{scale_percent}"));
            cmd.arg("/scale-device:100");
            cmd.arg("/size:90%");
            cmd.arg("/dynamic-resolution");
            cmd.arg("/f");
            cmd.arg("/rfx");
            let app_config = crate::config::AppConfig::load();
            if app_config.enable_clipboard {
                cmd.arg("+clipboard");
            } else {
                cmd.arg("-clipboard");
            }
            cmd.arg("/sound:sys:pulse");
            cmd.arg("/microphone:sys:pulse");
            cmd.arg("/dvc:rdpecam");
            if app_config.enable_drive_redirection && !app_config.shared_folder_path.is_empty() {
                let p = std::path::Path::new(&app_config.shared_folder_path);
                if p.exists() {
                                        cmd.arg(format!("/drive:Shared,{}", app_config.shared_folder_path));
                }
            }
            cmd.arg("+decorations");
            cmd.arg("/t:Windows 365 Cloud PC");
            cmd.arg("/wm-class:windows-app-linux");
            if let Some(user) = &config.username {
                cmd.arg(format!("/u:{user}"));
            }
        } else {
            return Err(AppError::RdpLauncher(format!(
                "unsupported RDP client: {client}"
            )));
        }

        match cmd.spawn() {
            Ok(child) => {
                debug!("RDP process launched successfully (PID: {})", child.id());
                Ok(child)
            }
            Err(e) => {
                warn!("Failed to spawn {client}: {e}");
                Err(AppError::RdpLauncher(format!(
                    "failed to execute '{client}': {e}. Please ensure FreeRDP is installed."
                )))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_scale_from_monitors_xml() {
        let sample_xml = r#"<monitors version="2">
  <configuration>
    <layoutmode>logical</layoutmode>
    <logicalmonitor>
      <x>0</x>
      <y>0</y>
      <scale>1.5</scale>
      <primary>yes</primary>
    </logicalmonitor>
  </configuration>
</monitors>"#;

        assert_eq!(
            RdpLauncher::parse_scale_from_monitors_xml(sample_xml),
            Some(150)
        );
    }

    #[test]
    fn test_prepare_nla_rdp_content() {
        let input = "full address:s:test\nsignscope:1\nsignature:abc\nenablerdsaadauth:i:1\n";
        let output = RdpLauncher::prepare_nla_rdp_content(input);
        assert!(!output.contains("signscope:"));
        assert!(!output.contains("signature:"));
        assert!(output.contains("enablerdsaadauth:i:0"));
    }

    #[test]
    fn test_wayland_freerdp_prioritization() {
        if RdpLauncher::is_wayland_session() {
            let freerdp = RdpLauncher::detect_freerdp_client();
            if RdpLauncher::detect_wayland_freerdp().is_some() {
                if RdpLauncher::is_gnome_desktop() {
                    assert!(
                        freerdp.unwrap().contains("xfreerdp")
                            || freerdp.unwrap().contains("wlfreerdp"),
                        "Expected FreeRDP client on GNOME Wayland session"
                    );
                } else {
                    assert!(
                        freerdp.unwrap().contains("wlfreerdp"),
                        "Expected Wayland FreeRDP client on non-GNOME Wayland session"
                    );
                }
            }
        }
    }

    #[test]
    fn test_resolve_freerdp_variants() {
        assert_eq!(RdpLauncher::resolve_client("freerdp"), Some("freerdp"));
        assert_eq!(RdpLauncher::resolve_client("ironrdp"), None);
    }

    #[test]
    fn test_is_wayland_session() {
        let _ = RdpLauncher::is_wayland_session();
    }
}
