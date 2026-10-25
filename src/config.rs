//! Application configuration and persistent storage helpers.

use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Official Windows App Client ID from PublicClientApplicationConfiguration in MSIX package
pub const WINDOWS_APP_CLIENT_ID: &str = "4fb5cc57-dbbc-4cdc-9595-748adff5f414";
/// Legacy Azure Virtual Desktop Client ID
pub const AVD_CLIENT_ID: &str = "a85cf173-4192-42f8-81fa-777a763e6e2c";
/// macOS Native Microsoft Remote Desktop Client ID
pub const MAC_CLIENT_ID: &str = "a85cf173-4192-42f8-81fa-777a763e6e2c";

/// Default to the real Windows App Client ID
pub const OFFICIAL_CLIENT_ID: &str = WINDOWS_APP_CLIENT_ID;

/// Multi-tenant authority matching official Windows App (Work or school accounts only)
pub const ORGANIZATIONS_AUTHORITY: &str = "https://login.microsoftonline.com/organizations";
/// Common multi-tenant authority (allows both personal MSA and Work/School accounts)
pub const COMMON_AUTHORITY: &str = "https://login.microsoftonline.com/common";

/// Default authority for Windows App.
pub const DEFAULT_AUTHORITY: &str = COMMON_AUTHORITY;

/// Official MSIX WAM Broker redirect URI
pub const WINDOWS_APP_REDIRECT_URI: &str =
    "ms-appx-web://microsoft.aad.brokerplugin/4fb5cc57-dbbc-4cdc-9595-748adff5f414";
pub const REDIRECT_URI: &str = WINDOWS_APP_REDIRECT_URI;
pub const WVD_REDIRECT_URI: &str = "https://www.wvd.microsoft.com/webclient";
pub const MAC_REDIRECT_URI: &str = "https://login.microsoftonline.com/common/oauth2/nativeclient";
pub const NATIVE_REDIRECT_URI: &str =
    "https://login.microsoftonline.com/common/oauth2/nativeclient";

pub const GRAPH_BASE_URL: &str = "https://graph.microsoft.com/beta";
pub const WINDOWS_365_BASE_URL: &str = "https://windows365.microsoft.com";
pub const CLIENT_VERSION: &str = "2.0.1484.0";
pub const SOURCE_CLIENT: &str = "NxtClient";

/// User-Agent constants for different platform profiles
pub const UA_LINUX_CHROME: &str =
    "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";
pub const UA_WINDOWS_11_WAM: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64; WebView/3.0) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/70.0.3538.102 Safari/537.36 Edge/18.22631";
pub const UA_WINDOWS_11_CHROME: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";
pub const UA_WINDOWS_11_EDGE: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36 Edg/131.0.0.0 win365.nxt/2.0.1484.0";
pub const UA_IOS_REMOTE_DESKTOP: &str =
    "Mozilla/5.0 (iPhone; CPU iPhone OS 17_4 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Mobile/15E148";
pub const UA_MAC_SAFARI: &str =
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.4 Safari/605.1.15";

/// Official Windows 365 Information Worker (End User) Service Scope
pub const WINDOWS_365_IW_SCOPE: &str = "7c0a6aea-533c-458c-9f81-15568f10f6e4/EndUser.Access";

/// Official AVD (Azure Virtual Desktop) ARM Gateway scope
pub const AVD_RESOURCE_SCOPE: &str = "9cdead84-a844-4324-93f2-b2e6bb768d07/.default";

pub const CLOUD_PC_SCOPES: &[&str] = &["openid", "profile", "offline_access", WINDOWS_365_IW_SCOPE];

fn default_true() -> bool {
    true
}

fn default_false() -> bool {
    false
}

fn default_shared_folder() -> String {
    std::env::var("HOME").unwrap_or_else(|_| String::new())
}

/// Stored user credentials for direct RDP / NLA session connection.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SavedCredentials {
    pub username: String,
    pub password: Option<String>,
    #[serde(default)]
    pub remember: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub client_id: String,
    pub authority: String,
    pub data_boundary: String,
    pub auto_refresh: bool,
    pub default_scale_factor: u32,
    pub preferred_rdp_client: String, // "freerdp" (in-app engine) or "ironrdp"
    pub view_mode: String,            // "grid" or "list"
    pub auth_profile_index: u32,      // 0 = Windows 11 WAM Broker [Default]
    pub enable_audio: bool,
    pub enable_microphone: bool,
    #[serde(default = "default_true")]
    pub enable_camera: bool,
    #[serde(default = "default_true")]
    pub enable_clipboard: bool,
    pub multi_monitor: bool,
    #[serde(default = "default_false")]
    pub enable_drive_redirection: bool,
    #[serde(default = "default_shared_folder")]
    pub shared_folder_path: String,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            client_id: OFFICIAL_CLIENT_ID.to_string(),
            authority: DEFAULT_AUTHORITY.to_string(),
            data_boundary: "commercial".to_string(),
            auto_refresh: true,
            default_scale_factor: 0,
            preferred_rdp_client: "freerdp".to_string(),
            view_mode: "grid".to_string(),
            auth_profile_index: 0,
            enable_audio: true,
            enable_microphone: true,
            enable_camera: true,
            enable_clipboard: true,
            multi_monitor: false,
            enable_drive_redirection: false,
            shared_folder_path: default_shared_folder(),
        }
    }
}

impl AppConfig {
    #[must_use]
    pub fn config_dir() -> PathBuf {
        ProjectDirs::from("com", "microsoft", "windows-app-linux")
            .map(|dirs| dirs.config_dir().to_path_buf())
            .unwrap_or_else(|| PathBuf::from(".config/windows-app-linux"))
    }

    #[must_use]
    pub fn config_file_path() -> PathBuf {
        Self::config_dir().join("config.json")
    }

    pub fn load() -> Self {
        let path = Self::config_file_path();
        if let Ok(content) = std::fs::read_to_string(&path) {
            if let Ok(cfg) = serde_json::from_str(&content) {
                return cfg;
            }
        }
        Self::default()
    }

    pub fn save(&self) -> std::io::Result<()> {
        let path = Self::config_file_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(path, json)
    }

    #[must_use]
    pub fn token_file_path() -> PathBuf {
        Self::config_dir().join("tokens.json")
    }

    pub fn save_tokens(tokens: &crate::models::TokenResponse) -> std::io::Result<()> {
        let path = Self::token_file_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(tokens)?;
        std::fs::write(path, json)
    }

    pub fn load_tokens() -> Option<crate::models::TokenResponse> {
        let path = Self::token_file_path();
        let content = std::fs::read_to_string(path).ok()?;
        serde_json::from_str(&content).ok()
    }

    pub fn clear_tokens() -> std::io::Result<()> {
        let path = Self::token_file_path();
        if path.exists() {
            std::fs::remove_file(path)?;
        }
        Ok(())
    }

    #[must_use]
    pub fn credentials_path() -> PathBuf {
        Self::config_dir().join("credentials.json")
    }

    pub fn save_credentials(creds: &SavedCredentials) -> std::io::Result<()> {
        let path = Self::credentials_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(creds)?;
        std::fs::write(path, json)
    }

    pub fn load_credentials() -> Option<SavedCredentials> {
        let path = Self::credentials_path();
        let content = std::fs::read_to_string(path).ok()?;
        serde_json::from_str(&content).ok()
    }

    pub fn clear_credentials() -> std::io::Result<()> {
        let path = Self::credentials_path();
        if path.exists() {
            std::fs::remove_file(path)?;
        }
        Ok(())
    }

    /// Extracts the authenticated user's UPN / preferred username from stored token claims.
    #[must_use]
    pub fn get_user_upn() -> Option<String> {
        let tokens = Self::load_tokens()?;
        if let Some(id_token) = &tokens.id_token {
            if let Some(upn) = Self::extract_claim_from_jwt(id_token, "preferred_username") {
                return Some(upn);
            }
            if let Some(upn) = Self::extract_claim_from_jwt(id_token, "upn") {
                return Some(upn);
            }
        }
        if let Some(upn) = Self::extract_claim_from_jwt(&tokens.access_token, "preferred_username")
        {
            return Some(upn);
        }
        if let Some(upn) = Self::extract_claim_from_jwt(&tokens.access_token, "upn") {
            return Some(upn);
        }
        None
    }

    /// Extracts the authenticated user's display name from stored token claims.
    #[must_use]
    pub fn get_user_display_name() -> Option<String> {
        let tokens = Self::load_tokens()?;
        if let Some(id_token) = &tokens.id_token {
            if let Some(name) = Self::extract_claim_from_jwt(id_token, "name") {
                return Some(name);
            }
        }
        if let Some(name) = Self::extract_claim_from_jwt(&tokens.access_token, "name") {
            return Some(name);
        }
        Self::get_user_upn()
    }

    pub fn extract_claim_from_jwt(jwt: &str, claim_key: &str) -> Option<String> {
        let parts: Vec<&str> = jwt.split('.').collect();
        if parts.len() < 2 {
            return None;
        }
        use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
        use base64::Engine;

        let mut b64 = parts[1].to_string();
        while b64.len() % 4 != 0 {
            b64.push('=');
        }
        let decoded = STANDARD
            .decode(b64.as_bytes())
            .or_else(|_| URL_SAFE_NO_PAD.decode(parts[1].as_bytes()))
            .ok()?;
        let json: serde_json::Value = serde_json::from_slice(&decoded).ok()?;
        json.get(claim_key)
            .and_then(|v| v.as_str())
            .map(ToString::to_string)
    }

    pub fn extract_upn_from_jwt(jwt: &str) -> Option<String> {
        Self::extract_claim_from_jwt(jwt, "preferred_username")
            .or_else(|| Self::extract_claim_from_jwt(jwt, "upn"))
            .or_else(|| Self::extract_claim_from_jwt(jwt, "email"))
            .or_else(|| Self::extract_claim_from_jwt(jwt, "unique_name"))
    }
}
