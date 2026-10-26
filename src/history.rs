//! When each connection was last used, for "recent" ordering and card subtitles.
//! Stored in `recent.json`, keyed by launch target.

use crate::config::{storage, AppConfig};
use crate::launcher::LaunchTarget;
use chrono::{DateTime, Local, Utc};
use std::collections::BTreeMap;
use std::path::PathBuf;

/// Last-connected times on disk.
#[derive(Debug, Clone)]
pub struct RecentStore {
    path: PathBuf,
}

impl Default for RecentStore {
    fn default() -> Self {
        Self::at(AppConfig::config_dir().join("recent.json"))
    }
}

impl RecentStore {
    #[must_use]
    pub fn at(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// Last connection time per launch target (`remote:<id>`, ...).
    #[must_use]
    pub fn load(&self) -> BTreeMap<String, DateTime<Utc>> {
        std::fs::read(&self.path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    #[must_use]
    pub fn last_used(&self, target: &LaunchTarget) -> Option<DateTime<Utc>> {
        self.load().get(&target.to_string()).copied()
    }

    /// Records that `target` was connected to at `when`.
    pub fn record(&self, target: &LaunchTarget, when: DateTime<Utc>) -> std::io::Result<()> {
        let mut all = self.load();
        all.insert(target.to_string(), when);
        storage::write_private(&self.path, &serde_json::to_vec_pretty(&all)?)
    }

    pub fn remove(&self, target: &LaunchTarget) -> std::io::Result<()> {
        let mut all = self.load();
        if all.remove(&target.to_string()).is_some() {
            storage::write_private(&self.path, &serde_json::to_vec_pretty(&all)?)?;
        }
        Ok(())
    }
}

/// "Just now", "5 minutes ago", "Yesterday", "3 days ago" or a date.
#[must_use]
pub fn relative_time(when: DateTime<Utc>, now: DateTime<Utc>) -> String {
    let secs = (now - when).num_seconds().max(0);
    let plural = |n: i64, unit: &str| {
        if n == 1 {
            format!("1 {unit} ago")
        } else {
            format!("{n} {unit}s ago")
        }
    };
    match secs {
        0..=59 => "Just now".to_string(),
        60..=3599 => plural(secs / 60, "minute"),
        3600..=86_399 => plural(secs / 3600, "hour"),
        86_400..=172_799 => "Yesterday".to_string(),
        172_800..=604_799 => plural(secs / 86_400, "day"),
        _ => when.with_timezone(&Local).format("%e %b %Y").to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    #[test]
    fn formats_relative_times() {
        let now = Utc::now();
        assert_eq!(relative_time(now, now), "Just now");
        assert_eq!(
            relative_time(now - Duration::minutes(1), now),
            "1 minute ago"
        );
        assert_eq!(relative_time(now - Duration::hours(5), now), "5 hours ago");
        assert_eq!(relative_time(now - Duration::hours(30), now), "Yesterday");
        assert_eq!(relative_time(now - Duration::days(3), now), "3 days ago");
        assert!(relative_time(now - Duration::days(30), now).contains(' '));
        assert_eq!(relative_time(now + Duration::hours(1), now), "Just now");
    }

    #[test]
    fn records_and_removes() {
        let dir = std::env::temp_dir().join(format!("wal-recent-{}", uuid::Uuid::new_v4()));
        let store = RecentStore::at(dir.join("recent.json"));
        let target = LaunchTarget::CloudPc("pc".into());
        assert_eq!(store.last_used(&target), None);
        let when = Utc::now();
        store.record(&target, when).unwrap();
        assert_eq!(store.last_used(&target), Some(when));
        store.remove(&target).unwrap();
        assert_eq!(store.last_used(&target), None);
        let _ = std::fs::remove_dir_all(dir);
    }
}
