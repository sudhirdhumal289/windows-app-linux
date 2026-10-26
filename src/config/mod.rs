//! User preferences and on-disk locations.

pub mod constants;
pub mod storage;

pub use constants::*;

use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Identity profile used for the Microsoft sign-in (persisted as an index).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthProfile {
    /// Windows App WAM broker identity (default).
    WindowsApp,
    /// Microsoft Remote Desktop for macOS.
    Mac,
    /// Legacy multi-platform Azure Virtual Desktop client.
    LegacyAvd,
    /// Windows App client with a plain Linux browser identity.
    WindowsAppLinux,
}

impl AuthProfile {
    pub const ALL: [AuthProfile; 4] = [
        Self::WindowsApp,
        Self::Mac,
        Self::LegacyAvd,
        Self::WindowsAppLinux,
    ];

    #[must_use]
    pub fn from_index(index: u32) -> Self {
        Self::ALL
            .get(index as usize)
            .copied()
            .unwrap_or(Self::WindowsApp)
    }

    #[must_use]
    pub fn index(self) -> u32 {
        Self::ALL.iter().position(|p| *p == self).unwrap_or(0) as u32
    }

    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::WindowsApp => "Windows App (WAM broker) [Default]",
            Self::Mac => "Remote Desktop for macOS",
            Self::LegacyAvd => "Legacy Azure Virtual Desktop client",
            Self::WindowsAppLinux => "Windows App (Linux browser identity)",
        }
    }

    #[must_use]
    pub fn client_id(self) -> &'static str {
        match self {
            Self::WindowsApp | Self::WindowsAppLinux => WINDOWS_APP_CLIENT_ID,
            Self::Mac => MAC_CLIENT_ID,
            Self::LegacyAvd => AVD_CLIENT_ID,
        }
    }

    #[must_use]
    pub fn redirect_uri(self) -> &'static str {
        match self {
            Self::WindowsApp | Self::WindowsAppLinux => WINDOWS_APP_REDIRECT_URI,
            Self::Mac => NATIVE_CLIENT_REDIRECT_URI,
            Self::LegacyAvd => WVD_REDIRECT_URI,
        }
    }

    #[must_use]
    pub fn user_agent(self) -> &'static str {
        match self {
            Self::WindowsApp => UA_WINDOWS_11_WAM,
            Self::Mac => UA_MAC_SAFARI,
            Self::LegacyAvd | Self::WindowsAppLinux => UA_LINUX_CHROME,
        }
    }

    /// Value of the `x-client-OS` authorize parameter, if the profile sends one.
    #[must_use]
    pub fn client_os(self) -> Option<&'static str> {
        match self {
            Self::WindowsApp => Some("Microsoft Windows NT 10.0.22631.0"),
            Self::Mac => Some("macOS"),
            Self::LegacyAvd | Self::WindowsAppLinux => Some("Linux"),
        }
    }

    /// Whether the WebView should mask its JS navigator identity as the WAM broker.
    #[must_use]
    pub fn masks_as_wam(self) -> bool {
        self == Self::WindowsApp
    }
}

/// An extra local folder shared with sessions as a drive called `name`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SharedFolder {
    pub name: String,
    pub path: String,
}

impl SharedFolder {
    /// Longest drive name accepted.
    pub const MAX_NAME_LEN: usize = 32;

    /// Whether the engine can share this folder: a short name of letters, digits,
    /// spaces, `-` and `_`, and a path without commas.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        let name = self.name.trim();
        !name.is_empty()
            && name.chars().count() <= Self::MAX_NAME_LEN
            && name
                .chars()
                .all(|c| c.is_alphanumeric() || matches!(c, ' ' | '-' | '_'))
            && !self.path.is_empty()
            && !self.path.contains(',')
            && !self.path.chars().any(char::is_control)
    }
}

/// Persisted user preferences. Unknown keys from older versions are ignored and missing
/// keys take their default, so configuration files stay forward and backward compatible.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct AppConfig {
    pub data_boundary: String,
    /// "grid" or "list".
    pub view_mode: String,
    /// Main window sections (e.g. "cloud-pcs", "remote-pcs") the user collapsed.
    pub collapsed_sections: Vec<String>,
    /// See [`AuthProfile`].
    pub auth_profile_index: u32,
    pub enable_audio: bool,
    pub enable_microphone: bool,
    pub enable_camera: bool,
    pub enable_clipboard: bool,
    pub enable_drive_redirection: bool,
    pub shared_folder_path: String,
    /// Folders shared in addition to `shared_folder_path`.
    pub extra_folders: Vec<SharedFolder>,
    /// Also share removable drives, including ones plugged in during the session.
    pub hotplug_drives: bool,
    /// Use every monitor when a session is full screen.
    pub multi_monitor: bool,
    /// "auto" (H.264/AVC444 graphics pipeline) or "rfx" (RemoteFX, lower latency on LAN).
    pub graphics_mode: String,
    /// Connection quality preset: "auto", "lan", "broadband" or "modem".
    pub quality: String,
    /// USB devices (`vid:pid`, hex) redirected to sessions.
    pub usb_devices: Vec<String>,
    pub enable_smartcard: bool,
    pub enable_printers: bool,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            data_boundary: "commercial".to_string(),
            view_mode: "grid".to_string(),
            collapsed_sections: Vec::new(),
            auth_profile_index: 0,
            enable_audio: true,
            enable_microphone: true,
            enable_camera: true,
            enable_clipboard: true,
            enable_drive_redirection: false,
            shared_folder_path: directories::BaseDirs::new()
                .map(|d| d.home_dir().display().to_string())
                .unwrap_or_default(),
            extra_folders: Vec::new(),
            hotplug_drives: false,
            multi_monitor: false,
            graphics_mode: "auto".to_string(),
            quality: "auto".to_string(),
            usb_devices: Vec::new(),
            enable_smartcard: false,
            enable_printers: false,
        }
    }
}

impl AppConfig {
    /// Per-user configuration directory (e.g. `~/.config/windows-app`).
    /// Transparently falls back to `windows-app-linux` if an existing installation exists.
    #[must_use]
    pub fn config_dir() -> PathBuf {
        let new_dir = ProjectDirs::from("com", "microsoft", "windows-app")
            .map(|dirs| dirs.config_dir().to_path_buf())
            .unwrap_or_else(|| PathBuf::from(".config/windows-app"));
        if new_dir.exists() {
            return new_dir;
        }
        let legacy_dir = ProjectDirs::from("com", "microsoft", "windows-app-linux")
            .map(|dirs| dirs.config_dir().to_path_buf())
            .unwrap_or_else(|| PathBuf::from(".config/windows-app-linux"));
        if legacy_dir.exists() {
            return legacy_dir;
        }
        new_dir
    }

    #[must_use]
    pub fn config_file_path() -> PathBuf {
        Self::config_dir().join("config.json")
    }

    #[must_use]
    pub fn load() -> Self {
        std::fs::read_to_string(Self::config_file_path())
            .ok()
            .and_then(|content| serde_json::from_str(&content).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> std::io::Result<()> {
        let json = serde_json::to_vec_pretty(self)?;
        storage::write_private(&Self::config_file_path(), &json)
    }

    #[must_use]
    pub fn auth_profile(&self) -> AuthProfile {
        AuthProfile::from_index(self.auth_profile_index)
    }

    /// Removes files written by older releases that are no longer used. `credentials.json`
    /// held a plaintext password and must not linger on disk.
    pub fn remove_legacy_files() {
        let path = Self::config_dir().join("credentials.json");
        if path.exists() {
            match storage::remove_if_exists(&path) {
                Ok(()) => log::info!("Removed legacy credentials file {}", path.display()),
                Err(e) => log::warn!("Could not remove {}: {e}", path.display()),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_legacy_config_with_unknown_and_missing_fields() {
        let json = r#"{
            "client_id": "old", "authority": "old", "auto_refresh": true,
            "preferred_rdp_client": "ironrdp", "multi_monitor": true,
            "view_mode": "list", "auth_profile_index": 2, "enable_audio": false
        }"#;
        let cfg: AppConfig = serde_json::from_str(json).unwrap();
        assert_eq!(cfg.view_mode, "list");
        assert_eq!(cfg.auth_profile(), AuthProfile::LegacyAvd);
        assert!(!cfg.enable_audio);
        assert!(cfg.enable_clipboard);
    }

    #[test]
    fn validates_shared_folders() {
        let folder = |name: &str, path: &str| SharedFolder {
            name: name.into(),
            path: path.into(),
        };
        assert!(folder("Music 2", "/home/u/Music").is_valid());
        for bad in [
            folder("", "/x"),
            folder("a,b", "/x"),
            folder("a", "/x,y"),
            folder("a", ""),
            folder(&"n".repeat(33), "/x"),
        ] {
            assert!(!bad.is_valid(), "{bad:?}");
        }
    }

    #[test]
    fn auth_profile_round_trips_and_clamps() {
        for profile in AuthProfile::ALL {
            assert_eq!(AuthProfile::from_index(profile.index()), profile);
        }
        assert_eq!(AuthProfile::from_index(42), AuthProfile::WindowsApp);
        assert_eq!(AuthProfile::Mac.client_id(), MAC_CLIENT_ID);
    }
}
