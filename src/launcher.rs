//! Launch targets: stable identifiers for "connect to this PC" used by the command line
//! (`--connect <target>`), app launcher entries, auto-connect at login and desktop
//! search. The search index is a small cache of the last listed connections so the OS
//! search can answer without signing in or touching the network.

use crate::api::AvdResource;
use crate::config::{storage, AppConfig};
use crate::connections::SavedConnection;
use crate::models::CloudPc;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::PathBuf;

/// Something the app can connect to directly.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub enum LaunchTarget {
    CloudPc(String),
    RemotePc(String),
    Avd(String),
}

impl LaunchTarget {
    /// Parses `cloudpc:<id>`, `remote:<id>` or `avd:<id>`.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim();
        let valid = |id: &str| {
            !id.is_empty()
                && id.len() <= 256
                && id
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "-_.{}".contains(c))
        };
        let (kind, id) = text.split_once(':')?;
        if !valid(id) {
            return None;
        }
        let id = id.to_string();
        match kind {
            "cloudpc" => Some(Self::CloudPc(id)),
            "remote" => Some(Self::RemotePc(id)),
            "avd" => Some(Self::Avd(id)),
            _ => None,
        }
    }

    #[must_use]
    pub fn id(&self) -> &str {
        match self {
            Self::CloudPc(id) | Self::RemotePc(id) | Self::Avd(id) => id,
        }
    }

    /// Short stable key for file names (hex of a hash of the target).
    #[must_use]
    pub fn file_key(&self) -> String {
        let digest = ring::digest::digest(&ring::digest::SHA256, self.to_string().as_bytes());
        digest.as_ref()[..8]
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }
}

impl fmt::Display for LaunchTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kind = match self {
            Self::CloudPc(_) => "cloudpc",
            Self::RemotePc(_) => "remote",
            Self::Avd(_) => "avd",
        };
        write!(f, "{kind}:{}", self.id())
    }
}

impl TryFrom<String> for LaunchTarget {
    type Error = String;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value).ok_or_else(|| format!("invalid launch target {value:?}"))
    }
}

impl From<LaunchTarget> for String {
    fn from(target: LaunchTarget) -> Self {
        target.to_string()
    }
}

/// A connection as shown by launchers and desktop search.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaunchEntry {
    pub target: LaunchTarget,
    pub name: String,
    /// Kind and detail, e.g. "Cloud PC · 4 vCPU, 16 GB".
    pub description: String,
}

impl LaunchEntry {
    #[must_use]
    pub fn cloud_pc(pc: &CloudPc) -> Self {
        Self {
            target: LaunchTarget::CloudPc(pc.id.clone()),
            name: pc.display_name.clone(),
            description: format!("Cloud PC · {}", pc.hardware_summary()),
        }
    }

    #[must_use]
    pub fn remote_pc(connection: &SavedConnection) -> Self {
        Self {
            target: LaunchTarget::RemotePc(connection.id.clone()),
            name: connection.display_name().to_string(),
            description: format!("Remote PC · {}", connection.address),
        }
    }

    #[must_use]
    pub fn avd(resource: &AvdResource) -> Self {
        Self {
            target: LaunchTarget::Avd(resource.id.clone()),
            name: resource.title.clone(),
            description: format!("{} · {}", resource.kind_label(), resource.workspace),
        }
    }

    /// Whether every term matches the name or description (case-insensitive).
    #[must_use]
    pub fn matches(&self, terms: &[impl AsRef<str>]) -> bool {
        let haystack = format!("{} {}", self.name, self.description).to_lowercase();
        !terms.is_empty()
            && terms
                .iter()
                .all(|t| haystack.contains(&t.as_ref().to_lowercase()))
    }
}

/// Last listed connections, for desktop search.
pub struct SearchIndex {
    path: PathBuf,
}

impl Default for SearchIndex {
    fn default() -> Self {
        Self::new(AppConfig::config_dir().join("search-index.json"))
    }
}

impl SearchIndex {
    #[must_use]
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    #[must_use]
    pub fn load(&self) -> Vec<LaunchEntry> {
        std::fs::read_to_string(&self.path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, entries: &[LaunchEntry]) -> std::io::Result<()> {
        if self.load() == entries {
            return Ok(());
        }
        storage::write_private(&self.path, &serde_json::to_vec_pretty(entries)?)
    }

    pub fn clear(&self) -> std::io::Result<()> {
        storage::remove_if_exists(&self.path)
    }

    /// Entries matching all `terms`, in index order.
    #[must_use]
    pub fn search(&self, terms: &[impl AsRef<str>]) -> Vec<LaunchEntry> {
        self.load()
            .into_iter()
            .filter(|e| e.matches(terms))
            .collect()
    }

    #[must_use]
    pub fn find(&self, target: &LaunchTarget) -> Option<LaunchEntry> {
        self.load().into_iter().find(|e| &e.target == target)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn targets_round_trip_and_reject_garbage() {
        for text in [
            "cloudpc:3f2a-11",
            "remote:5b1c7d2e-0000-4000-8000-000000000001",
            "avd:{ABC}.1",
        ] {
            let target = LaunchTarget::parse(text).unwrap();
            assert_eq!(target.to_string(), text);
        }
        for bad in [
            "",
            "cloudpc:",
            "ftp:x",
            "remote:a b",
            "avd:x;rm",
            "remote:$(id)",
        ] {
            assert_eq!(LaunchTarget::parse(bad), None, "{bad}");
        }
        assert_eq!(
            LaunchTarget::parse("remote:x").unwrap().file_key().len(),
            16
        );
    }

    #[test]
    fn search_matches_all_terms() {
        let dir = std::env::temp_dir().join(format!("wal-search-{}", uuid::Uuid::new_v4()));
        let index = SearchIndex::new(dir.join("search-index.json"));
        let entries = vec![
            LaunchEntry {
                target: LaunchTarget::CloudPc("1".into()),
                name: "Dev Box".into(),
                description: "Cloud PC · 8 vCPU".into(),
            },
            LaunchEntry {
                target: LaunchTarget::RemotePc("2".into()),
                name: "Office".into(),
                description: "Remote PC · office-pc".into(),
            },
        ];
        index.save(&entries).unwrap();
        let names = |terms: &[&str]| -> Vec<String> {
            index.search(terms).into_iter().map(|e| e.name).collect()
        };
        assert_eq!(names(&["dev"]), ["Dev Box"]);
        assert_eq!(names(&["pc"]), ["Dev Box", "Office"]);
        assert_eq!(names(&["remote", "OFFICE"]), ["Office"]);
        assert!(names(&[]).is_empty());
        assert_eq!(
            index
                .find(&LaunchTarget::RemotePc("2".into()))
                .unwrap()
                .name,
            "Office"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
}
