//! Per-connection settings that replace the global preferences for one Cloud PC, Azure
//! Virtual Desktop resource or Remote PC, plus commands run around its sessions. Stored
//! in `connection-settings.json`, keyed by launch target.

use crate::config::{storage, AppConfig};
use crate::launcher::LaunchTarget;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

/// A fixed remote desktop size in pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Resolution {
    pub width: u32,
    pub height: u32,
}

impl Resolution {
    pub const MIN_WIDTH: u32 = 640;
    pub const MIN_HEIGHT: u32 = 480;
    pub const MAX: u32 = 8192;

    /// Common sizes offered in the settings dialog.
    pub const PRESETS: [Resolution; 6] = [
        Self::new(1280, 720),
        Self::new(1366, 768),
        Self::new(1600, 900),
        Self::new(1920, 1080),
        Self::new(2560, 1440),
        Self::new(3840, 2160),
    ];

    #[must_use]
    pub const fn new(width: u32, height: u32) -> Self {
        Self { width, height }
    }

    /// Clamped to sizes the engine accepts.
    #[must_use]
    pub fn clamped(self) -> Self {
        Self {
            width: self.width.clamp(Self::MIN_WIDTH, Self::MAX),
            height: self.height.clamp(Self::MIN_HEIGHT, Self::MAX),
        }
    }

    /// Parses `1920x1080`.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let (w, h) = text.trim().split_once(['x', 'X', '×'])?;
        Some(Self::new(w.trim().parse().ok()?, h.trim().parse().ok()?).clamped())
    }
}

impl std::fmt::Display for Resolution {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}×{}", self.width, self.height)
    }
}

/// Settings of one connection; `None` means "use the preference".
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ConnectionOverrides {
    /// Fixed desktop size instead of following the window.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolution: Option<Resolution>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub graphics_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quality: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub clipboard: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub microphone: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub camera: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub drives: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub printers: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub smartcard: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub multi_monitor: Option<bool>,
    /// Shell command run (and awaited) before connecting.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub pre_command: String,
    /// Shell command started after the session ends.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub post_command: String,
    /// Starred connection for quick one-click access.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub is_favorite: bool,
    /// Custom tags for filtering (e.g. "Production", "Development", "Lab").
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// Folder heading or category (e.g. "Production", "Development", "Lab").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
}

impl ConnectionOverrides {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self == &Self::default()
    }

    /// The preferences with this connection's settings applied.
    #[must_use]
    pub fn apply(&self, config: &AppConfig) -> AppConfig {
        let mut config = config.clone();
        let set = |target: &mut bool, value: Option<bool>| {
            if let Some(value) = value {
                *target = value;
            }
        };
        if let Some(mode) = &self.graphics_mode {
            config.graphics_mode.clone_from(mode);
        }
        if let Some(quality) = &self.quality {
            config.quality.clone_from(quality);
        }
        set(&mut config.enable_clipboard, self.clipboard);
        set(&mut config.enable_audio, self.audio);
        set(&mut config.enable_microphone, self.microphone);
        set(&mut config.enable_camera, self.camera);
        set(&mut config.enable_drive_redirection, self.drives);
        set(&mut config.enable_printers, self.printers);
        set(&mut config.enable_smartcard, self.smartcard);
        set(&mut config.multi_monitor, self.multi_monitor);
        config
    }

    /// Trims the commands; a command must be a single line.
    pub fn normalize(&mut self) -> Result<(), OverrideError> {
        for command in [&mut self.pre_command, &mut self.post_command] {
            *command = command.trim().to_string();
            if command.contains(['\n', '\r', '\0']) {
                return Err(OverrideError::MultiLineCommand);
            }
        }
        self.resolution = self.resolution.map(Resolution::clamped);
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum OverrideError {
    #[error("Commands must fit on a single line")]
    MultiLineCommand,
}

/// All connections' settings on disk.
#[derive(Debug, Clone)]
pub struct OverrideStore {
    path: PathBuf,
}

impl Default for OverrideStore {
    fn default() -> Self {
        Self::at(AppConfig::config_dir().join("connection-settings.json"))
    }
}

impl OverrideStore {
    #[must_use]
    pub fn at(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn load_all(&self) -> BTreeMap<String, ConnectionOverrides> {
        std::fs::read(&self.path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    /// Settings of `target` (all `None` when it has none).
    #[must_use]
    pub fn get(&self, target: &LaunchTarget) -> ConnectionOverrides {
        self.load_all()
            .remove(&target.to_string())
            .unwrap_or_default()
    }

    /// Stores `overrides` for `target`, removing the entry when it is empty.
    pub fn set(
        &self,
        target: &LaunchTarget,
        overrides: ConnectionOverrides,
    ) -> std::io::Result<()> {
        let mut all = self.load_all();
        if overrides.is_empty() {
            if all.remove(&target.to_string()).is_none() {
                return Ok(());
            }
        } else {
            all.insert(target.to_string(), overrides);
        }
        storage::write_private(&self.path, &serde_json::to_vec_pretty(&all)?)
    }

    pub fn remove(&self, target: &LaunchTarget) -> std::io::Result<()> {
        self.set(target, ConnectionOverrides::default())
    }

    #[must_use]
    pub fn is_favorite(&self, target: &LaunchTarget) -> bool {
        self.get(target).is_favorite
    }

    pub fn set_favorite(&self, target: &LaunchTarget, favorite: bool) -> std::io::Result<()> {
        let mut ov = self.get(target);
        ov.is_favorite = favorite;
        self.set(target, ov)
    }

    #[must_use]
    pub fn tags(&self, target: &LaunchTarget) -> Vec<String> {
        self.get(target).tags
    }

    pub fn set_tags(&self, target: &LaunchTarget, tags: Vec<String>) -> std::io::Result<()> {
        let mut ov = self.get(target);
        ov.tags = tags;
        self.set(target, ov)
    }

    #[must_use]
    pub fn folder(&self, target: &LaunchTarget) -> Option<String> {
        self.get(target).folder
    }

    pub fn set_folder(&self, target: &LaunchTarget, folder: Option<String>) -> std::io::Result<()> {
        let mut ov = self.get(target);
        ov.folder = folder;
        self.set(target, ov)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn applies_only_set_fields() {
        let config = AppConfig::default();
        let overrides = ConnectionOverrides {
            audio: Some(!config.enable_audio),
            quality: Some("modem".into()),
            ..Default::default()
        };
        let applied = overrides.apply(&config);
        assert_eq!(applied.enable_audio, !config.enable_audio);
        assert_eq!(applied.quality, "modem");
        assert_eq!(applied.enable_clipboard, config.enable_clipboard);
        assert_eq!(ConnectionOverrides::default().apply(&config), config);
    }

    #[test]
    fn resolutions_parse_and_clamp() {
        assert_eq!(
            Resolution::parse("1920x1080"),
            Some(Resolution::new(1920, 1080))
        );
        assert_eq!(
            Resolution::parse("10 x 10"),
            Some(Resolution::new(640, 480))
        );
        assert_eq!(Resolution::parse("big"), None);
        assert_eq!(Resolution::new(1280, 720).to_string(), "1280×720");
    }

    #[test]
    fn normalizes_commands() {
        let mut o = ConnectionOverrides {
            pre_command: "  nmcli c up vpn  ".into(),
            ..Default::default()
        };
        o.normalize().unwrap();
        assert_eq!(o.pre_command, "nmcli c up vpn");
        o.post_command = "a\nb".into();
        assert_eq!(o.normalize(), Err(OverrideError::MultiLineCommand));
    }

    #[test]
    fn store_round_trip() {
        let dir = std::env::temp_dir().join(format!("wal-ovr-{}", uuid::Uuid::new_v4()));
        let store = OverrideStore::at(dir.join("s.json"));
        let target = LaunchTarget::RemotePc("abc".into());
        assert!(store.get(&target).is_empty());
        let o = ConnectionOverrides {
            resolution: Some(Resolution::new(1280, 720)),
            post_command: "true".into(),
            ..Default::default()
        };
        store.set(&target, o.clone()).unwrap();
        assert_eq!(store.get(&target), o);
        assert!(store.get(&LaunchTarget::Avd("abc".into())).is_empty());
        store.remove(&target).unwrap();
        assert!(store.get(&target).is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }
    #[test]
    fn overrides_favorites_folders_tags() {
        let dir = std::env::temp_dir().join(format!("wal-overrides-{}", uuid::Uuid::new_v4()));
        let store = OverrideStore::at(dir.join("connection-settings.json"));
        let target = LaunchTarget::CloudPc("pc-123".into());

        assert!(!store.is_favorite(&target));
        assert!(store.tags(&target).is_empty());
        assert!(store.folder(&target).is_none());

        store.set_favorite(&target, true).unwrap();
        store
            .set_tags(&target, vec!["Dev".into(), "Test".into()])
            .unwrap();
        store
            .set_folder(&target, Some("Workspaces".into()))
            .unwrap();

        assert!(store.is_favorite(&target));
        assert_eq!(store.tags(&target), vec!["Dev", "Test"]);
        assert_eq!(store.folder(&target).as_deref(), Some("Workspaces"));
    }
}
