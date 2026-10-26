//! Where each connection's session window was last shown (monitor and workspace), so
//! the next session opens there. Stored in `window-placement.json`, keyed by launch
//! target. Platform-neutral: frontends map their monitors to [`MonitorId`].

use crate::config::{storage, AppConfig};
use crate::launcher::LaunchTarget;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

/// Identifies a monitor across sessions and reboots.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MonitorId {
    /// Output the monitor is plugged into (`DP-1`, `eDP-1`, ...).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connector: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manufacturer: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

impl MonitorId {
    /// Manufacturer and model, when both are known.
    fn identity(&self) -> Option<(&str, &str)> {
        Some((self.manufacturer.as_deref()?, self.model.as_deref()?))
    }

    /// How well `candidate` matches this saved monitor; `None` when it is a different
    /// monitor. The same monitor moved to another port still matches, and a different
    /// monitor on the same port does not. Without a known identity on both sides only
    /// the connector can be compared.
    fn match_score(&self, candidate: &MonitorId) -> Option<u8> {
        let same_connector = self.connector.is_some() && self.connector == candidate.connector;
        match (self.identity(), candidate.identity()) {
            (Some(saved), Some(other)) if saved == other => Some(1 + u8::from(same_connector)),
            (Some(_), Some(_)) => None,
            _ => same_connector.then_some(1),
        }
    }

    /// The candidate that best matches this saved monitor.
    pub fn best_match<T>(&self, candidates: impl IntoIterator<Item = (T, MonitorId)>) -> Option<T> {
        candidates
            .into_iter()
            .filter_map(|(item, id)| self.match_score(&id).map(|score| (score, item)))
            .fold(None, |best: Option<(u8, T)>, (score, item)| match best {
                Some((best_score, _)) if best_score >= score => best,
                _ => Some((score, item)),
            })
            .map(|(_, item)| item)
    }
}

/// Where a session window was shown.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowPlacement {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub monitor: Option<MonitorId>,
    /// Zero-based workspace (virtual desktop), where the platform exposes it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<u32>,
}

impl WindowPlacement {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.monitor.is_none() && self.workspace.is_none()
    }
}

/// Last session window placement per connection, on disk.
#[derive(Debug, Clone)]
pub struct PlacementStore {
    path: PathBuf,
}

impl Default for PlacementStore {
    fn default() -> Self {
        Self::at(AppConfig::config_dir().join("window-placement.json"))
    }
}

impl PlacementStore {
    #[must_use]
    pub fn at(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    fn load_all(&self) -> BTreeMap<String, WindowPlacement> {
        std::fs::read(&self.path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    fn save_all(&self, all: &BTreeMap<String, WindowPlacement>) -> std::io::Result<()> {
        storage::write_private(&self.path, &serde_json::to_vec_pretty(all)?)
    }

    #[must_use]
    pub fn get(&self, target: &LaunchTarget) -> WindowPlacement {
        self.load_all()
            .remove(&target.to_string())
            .unwrap_or_default()
    }

    /// Records where `target`'s session window is. Unchanged placements are not rewritten.
    pub fn set(&self, target: &LaunchTarget, placement: WindowPlacement) -> std::io::Result<()> {
        let mut all = self.load_all();
        let key = target.to_string();
        if placement.is_empty() {
            if all.remove(&key).is_none() {
                return Ok(());
            }
        } else if all.get(&key) == Some(&placement) {
            return Ok(());
        } else {
            all.insert(key, placement);
        }
        self.save_all(&all)
    }

    pub fn remove(&self, target: &LaunchTarget) -> std::io::Result<()> {
        self.set(target, WindowPlacement::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn monitor(connector: &str, manufacturer: Option<&str>, model: Option<&str>) -> MonitorId {
        MonitorId {
            connector: Some(connector.to_string()),
            manufacturer: manufacturer.map(str::to_string),
            model: model.map(str::to_string),
        }
    }

    #[test]
    fn matches_the_same_monitor_on_any_port() {
        let saved = monitor("DP-1", Some("DEL"), Some("U2720Q"));
        let candidates = [
            ("laptop", monitor("eDP-1", Some("BOE"), Some("0x0afc"))),
            ("moved", monitor("DP-2", Some("DEL"), Some("U2720Q"))),
        ];
        assert_eq!(saved.best_match(candidates), Some("moved"));
    }

    #[test]
    fn prefers_the_same_port_among_identical_monitors() {
        let saved = monitor("DP-2", Some("DEL"), Some("U2720Q"));
        let candidates = [
            ("left", monitor("DP-1", Some("DEL"), Some("U2720Q"))),
            ("right", monitor("DP-2", Some("DEL"), Some("U2720Q"))),
            ("third", monitor("DP-3", Some("DEL"), Some("U2720Q"))),
        ];
        assert_eq!(saved.best_match(candidates), Some("right"));
    }

    #[test]
    fn rejects_a_different_monitor_on_the_same_port() {
        let saved = monitor("HDMI-1", Some("DEL"), Some("U2720Q"));
        let candidates = [("tv", monitor("HDMI-1", Some("SAM"), Some("QN90")))];
        assert_eq!(saved.best_match(candidates), None);
    }

    #[test]
    fn falls_back_to_the_connector_without_an_identity() {
        // X11 reports no manufacturer.
        let saved = monitor("HDMI-1", None, Some("HDMI-1"));
        let candidates = [
            ("a", monitor("eDP-1", None, Some("eDP-1"))),
            ("b", monitor("HDMI-1", Some("DEL"), Some("U2720Q"))),
        ];
        assert_eq!(saved.best_match(candidates.clone()), Some("b"));
        assert_eq!(MonitorId::default().best_match(candidates), None);
    }

    #[test]
    fn stores_updates_and_removes() {
        let dir = std::env::temp_dir().join(format!("wal-placement-{}", uuid::Uuid::new_v4()));
        let store = PlacementStore::at(dir.join("window-placement.json"));
        let target = LaunchTarget::RemotePc("pc".into());
        assert_eq!(store.get(&target), WindowPlacement::default());

        let placement = WindowPlacement {
            monitor: Some(monitor("DP-1", Some("DEL"), Some("U2720Q"))),
            workspace: Some(2),
        };
        store.set(&target, placement.clone()).unwrap();
        assert_eq!(store.get(&target), placement);
        let json = std::fs::read_to_string(dir.join("window-placement.json")).unwrap();
        assert!(json.contains("remote:pc") && json.contains("\"workspace\": 2"));

        store.remove(&target).unwrap();
        assert_eq!(store.get(&target), WindowPlacement::default());
        let _ = std::fs::remove_dir_all(dir);
    }
}
