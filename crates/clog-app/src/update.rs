//! Auto-update wiring: persisted cadence/snooze state and the small Rust
//! shim the UI talks to. The heavy lifting (HTTP fetch, signature check,
//! download, relaunch) is handled by `tauri-plugin-updater`; this module
//! only enforces the once-per-24h silent-check cadence and the 7-day
//! per-version snooze, plus the portable-mode branch that prevents the
//! installer flow from ever running against a portable copy.
//!
//! All times are stored as seconds since the Unix epoch so the on-disk
//! file is trivial to inspect or hand-edit during development.

use std::fs;
use std::io;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::paths;

const SCHEMA_VERSION: u32 = 1;

/// 24 hours between silent checks. Manual checks (Check for updates...) bypass
/// this guard via `force = true`.
pub const SILENT_CHECK_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

/// Per-version snooze duration when the user dismisses a banner via the
/// close button. `Later` only suppresses for the session and is handled
/// UI-side.
pub const SNOOZE_DURATION: Duration = Duration::from_secs(7 * 24 * 60 * 60);

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UpdateState {
    #[serde(default = "default_schema")]
    pub schema: u32,
    /// Seconds since the Unix epoch at which the last silent check ran.
    /// `None` means "never checked".
    #[serde(default)]
    pub last_check_unix: Option<u64>,
    /// Version string the user snoozed (e.g. `"1.2.3"`). When the live
    /// `latest.json` advertises a *different* version the snooze is
    /// ignored - newer releases get a fresh banner immediately.
    #[serde(default)]
    pub snoozed_version: Option<String>,
    /// Seconds since the Unix epoch at which the snooze expires.
    #[serde(default)]
    pub snoozed_until_unix: Option<u64>,
}

fn default_schema() -> u32 {
    SCHEMA_VERSION
}

impl UpdateState {
    pub fn load() -> Self {
        load_or_default(&paths::update_state_path())
    }

    /// # Errors
    /// Bubbles up filesystem errors from the atomic write.
    pub fn save(&self) -> io::Result<()> {
        write_atomic(&paths::update_state_path(), self)
    }

    /// True when a silent check is due (no record of a prior check, or the
    /// last one was at least `SILENT_CHECK_INTERVAL` ago).
    #[must_use]
    pub fn should_silent_check(&self, now: SystemTime) -> bool {
        let Some(last) = self.last_check_unix else {
            return true;
        };
        let Ok(now_secs) = now.duration_since(UNIX_EPOCH).map(|d| d.as_secs()) else {
            // Clock before epoch is absurd; treat as "never checked".
            return true;
        };
        now_secs.saturating_sub(last) >= SILENT_CHECK_INTERVAL.as_secs()
    }

    /// True when the user has snoozed *this exact* `version` and the
    /// snooze has not yet expired. A different version always escapes
    /// the snooze.
    #[must_use]
    pub fn is_snoozed(&self, version: &str, now: SystemTime) -> bool {
        let Some(snoozed) = self.snoozed_version.as_deref() else {
            return false;
        };
        if snoozed != version {
            return false;
        }
        let Some(until) = self.snoozed_until_unix else {
            return false;
        };
        let Ok(now_secs) = now.duration_since(UNIX_EPOCH).map(|d| d.as_secs()) else {
            return false;
        };
        now_secs < until
    }

    /// Record that a silent check just ran. Bumped on every silent check
    /// regardless of outcome so transient failures don't cause a busy
    /// retry loop.
    pub fn mark_checked(&mut self, now: SystemTime) {
        if let Ok(secs) = now.duration_since(UNIX_EPOCH).map(|d| d.as_secs()) {
            self.last_check_unix = Some(secs);
        }
    }

    pub fn snooze(&mut self, version: &str, now: SystemTime) {
        let until = now + SNOOZE_DURATION;
        if let Ok(secs) = until.duration_since(UNIX_EPOCH).map(|d| d.as_secs()) {
            self.snoozed_version = Some(version.to_string());
            self.snoozed_until_unix = Some(secs);
        }
    }
}

fn load_or_default<T: serde::de::DeserializeOwned + Default>(path: &Path) -> T {
    let Ok(bytes) = fs::read(path) else {
        return T::default();
    };
    serde_json::from_slice::<T>(&bytes).unwrap_or_default()
}

fn write_atomic<T: Serialize>(path: &Path, value: &T) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let body = serde_json::to_vec_pretty(value).map_err(io::Error::other)?;
    let mut tmp_os: std::ffi::OsString = path.as_os_str().into();
    tmp_os.push(".tmp");
    let tmp = std::path::PathBuf::from(tmp_os);
    fs::write(&tmp, &body)?;
    fs::rename(&tmp, path).or_else(|_| {
        let _ = fs::remove_file(path);
        fs::rename(&tmp, path)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn epoch_plus(secs: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(secs)
    }

    #[test]
    fn silent_check_due_when_never_checked() {
        let s = UpdateState::default();
        assert!(s.should_silent_check(epoch_plus(1_000_000)));
    }

    #[test]
    fn silent_check_suppressed_within_24h() {
        let mut s = UpdateState::default();
        s.mark_checked(epoch_plus(1_000_000));
        // 23h59m later: still within the window.
        assert!(!s.should_silent_check(epoch_plus(1_000_000 + 23 * 3600 + 59 * 60)));
    }

    #[test]
    fn silent_check_due_after_24h() {
        let mut s = UpdateState::default();
        s.mark_checked(epoch_plus(1_000_000));
        // Exactly 24h: the boundary counts as "due" so a daily-launched
        // app reliably checks each day even with sub-second drift.
        assert!(s.should_silent_check(epoch_plus(1_000_000 + 24 * 3600)));
    }

    #[test]
    fn snooze_blocks_same_version_inside_window() {
        let mut s = UpdateState::default();
        s.snooze("1.2.3", epoch_plus(1_000_000));
        assert!(s.is_snoozed("1.2.3", epoch_plus(1_000_000 + 6 * 24 * 3600)));
    }

    #[test]
    fn snooze_expires_after_seven_days() {
        let mut s = UpdateState::default();
        s.snooze("1.2.3", epoch_plus(1_000_000));
        assert!(!s.is_snoozed("1.2.3", epoch_plus(1_000_000 + 7 * 24 * 3600 + 1)));
    }

    #[test]
    fn snooze_ignores_different_version() {
        let mut s = UpdateState::default();
        s.snooze("1.2.3", epoch_plus(1_000_000));
        // A newer release escapes the snooze immediately.
        assert!(!s.is_snoozed("1.2.4", epoch_plus(1_000_000 + 1)));
    }

    #[test]
    fn unknown_fields_decode_to_default() {
        let raw = br#"{"last_check_unix":42,"unknown_future_field":true}"#;
        let s: UpdateState = serde_json::from_slice(raw).expect("v1 update.json decodes");
        assert_eq!(s.last_check_unix, Some(42));
        assert_eq!(s.schema, SCHEMA_VERSION);
    }
}
