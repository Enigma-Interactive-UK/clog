//! On-disk JSON state: `settings.json`, `session.json`, `patterns.json`.
//!
//! Each file is schema-versioned (`"schema": 1`); a missing field is treated
//! as v1 so existing v1 files keep loading after a non-breaking field
//! addition. Disk reads return defaults on any error so a corrupted state
//! file never blocks the app from launching.
//!
//! Writes go through `std::fs::write` against a tempfile + rename so a
//! crashed write never leaves a half-truncated file.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::paths;

const SCHEMA_VERSION: u32 = 1;
const RECENT_FILES_CAP: usize = 20;

// --- settings.json ---------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default = "default_schema")]
    pub schema: u32,
    /// `"system"` | `"light"` | `"dark"`. UI consumes this to drive
    /// `data-theme` and falls back to system on unknown values.
    #[serde(default = "default_theme")]
    pub theme: String,
    /// Base font size in CSS px. Bound 9..=24 in the UI.
    #[serde(default = "default_font_size")]
    pub font_size: u32,
    /// MRU list of absolute path strings. Most recent first. Cap 20.
    #[serde(default)]
    pub recent_files: Vec<String>,
    /// "follow tail" preference for newly-opened files.
    #[serde(default = "default_true")]
    pub follow_tail_default: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            schema: SCHEMA_VERSION,
            theme: default_theme(),
            font_size: default_font_size(),
            recent_files: Vec::new(),
            follow_tail_default: true,
        }
    }
}

fn default_schema() -> u32 {
    SCHEMA_VERSION
}
fn default_theme() -> String {
    "system".to_string()
}
fn default_font_size() -> u32 {
    13
}
fn default_true() -> bool {
    true
}

impl Settings {
    pub fn load() -> Self {
        load_or_default(&paths::settings_path())
    }

    /// # Errors
    /// Bubbles up filesystem errors.
    pub fn save(&self) -> io::Result<()> {
        write_atomic(&paths::settings_path(), self)
    }

    /// Push `path` to the front of `recent_files`, de-duping by absolute
    /// string and clamping to `RECENT_FILES_CAP`. No-op if the path is
    /// empty.
    pub fn touch_recent(&mut self, path: &str) {
        if path.is_empty() {
            return;
        }
        self.recent_files.retain(|p| p != path);
        self.recent_files.insert(0, path.to_string());
        if self.recent_files.len() > RECENT_FILES_CAP {
            self.recent_files.truncate(RECENT_FILES_CAP);
        }
    }

    /// Drop `path` from the recent list (e.g. when the file is gone).
    pub fn forget_recent(&mut self, path: &str) {
        self.recent_files.retain(|p| p != path);
    }
}

// --- session.json ----------------------------------------------------------

/// Restoration state for the previously-open files. P9 generalised this
/// from a single `last_file` to an ordered tab list. The `last_file` field
/// is retained so a v1 session file still loads (it becomes the sole tab).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Session {
    #[serde(default = "default_schema")]
    pub schema: u32,
    /// Legacy single-file slot. Written for forward-compat with older
    /// builds so a downgrade still reopens *something*. Mirrors
    /// `tabs[active_tab]` at save time.
    #[serde(default)]
    pub last_file: Option<RestoredFile>,
    /// Tabs in display order. Empty on first launch.
    #[serde(default)]
    pub tabs: Vec<RestoredFile>,
    /// Index into `tabs` of the active tab. Clamped at load time.
    #[serde(default)]
    pub active_tab: usize,
}

impl Session {
    /// After load, fold the legacy `last_file` field into `tabs` if `tabs`
    /// is empty so the rest of the app sees a single shape.
    pub fn normalise(mut self) -> Self {
        if self.tabs.is_empty() {
            if let Some(f) = self.last_file.take() {
                self.tabs.push(f);
                self.active_tab = 0;
            }
        }
        if !self.tabs.is_empty() && self.active_tab >= self.tabs.len() {
            self.active_tab = self.tabs.len() - 1;
        }
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RestoredFile {
    pub path: String,
    #[serde(default)]
    pub scroll_top: f64,
    #[serde(default = "default_true")]
    pub follow_tail: bool,
    #[serde(default = "default_full_mask")]
    pub level_mask: u32,
    #[serde(default)]
    pub filter_text: String,
    /// `"smart"` | `"regex"`.
    #[serde(default = "default_smart")]
    pub search_mode: String,
    #[serde(default)]
    pub search_case_sensitive: bool,
    #[serde(default)]
    pub filter_mode: bool,
}

fn default_full_mask() -> u32 {
    0xFFFF_FFFF
}
fn default_smart() -> String {
    "smart".to_string()
}

impl Session {
    pub fn load() -> Self {
        load_or_default::<Self>(&paths::session_path()).normalise()
    }

    /// # Errors
    /// Bubbles up filesystem errors.
    pub fn save(&self) -> io::Result<()> {
        // Mirror the active tab into `last_file` so an older binary can still
        // open something. We clone rather than move so callers' state is
        // unchanged.
        let mut to_write = self.clone();
        to_write.last_file = to_write.tabs.get(to_write.active_tab).cloned();
        write_atomic(&paths::session_path(), &to_write)
    }
}

// --- patterns.json ---------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PatternsFile {
    #[serde(default = "default_schema")]
    pub schema: u32,
    /// Path -> per-file override. Stored as the absolute file path so a
    /// rename of the data folder doesn't lose all of them.
    #[serde(default)]
    pub overrides: BTreeMap<String, PatternOverride>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PatternOverride {
    /// Either `"pattern"` (`PatternLayout` source) or `"regex"`.
    pub kind: String,
    pub source: String,
}

impl PatternsFile {
    pub fn load() -> Self {
        load_or_default(&paths::patterns_path())
    }

    /// # Errors
    /// Bubbles up filesystem errors.
    pub fn save(&self) -> io::Result<()> {
        write_atomic(&paths::patterns_path(), self)
    }
}

// --- shared I/O helpers ----------------------------------------------------

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
    let tmp = tmp_path(path);
    fs::write(&tmp, &body)?;
    fs::rename(&tmp, path).or_else(|_| {
        let _ = fs::remove_file(path);
        fs::rename(&tmp, path)
    })
}

fn tmp_path(path: &Path) -> PathBuf {
    let mut s: std::ffi::OsString = path.as_os_str().into();
    s.push(".tmp");
    PathBuf::from(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recent_files_dedupes_and_caps() {
        let mut s = Settings::default();
        for i in 0..30 {
            s.touch_recent(&format!("/tmp/f{i}.log"));
        }
        assert_eq!(s.recent_files.len(), RECENT_FILES_CAP);
        // Most-recent first.
        assert_eq!(s.recent_files[0], "/tmp/f29.log");

        // Re-touch a middle entry: it moves to the front, length unchanged.
        s.touch_recent("/tmp/f25.log");
        assert_eq!(s.recent_files[0], "/tmp/f25.log");
        assert_eq!(s.recent_files.len(), RECENT_FILES_CAP);
    }

    #[test]
    fn settings_defaults_round_trip() {
        let s = Settings::default();
        let json = serde_json::to_string(&s).unwrap();
        let back: Settings = serde_json::from_str(&json).unwrap();
        assert_eq!(s.theme, back.theme);
        assert_eq!(s.font_size, back.font_size);
    }

    #[test]
    fn unknown_fields_decode_to_default() {
        let json = br#"{"theme":"light"}"#;
        let s: Settings = serde_json::from_slice(json).unwrap();
        assert_eq!(s.theme, "light");
        assert_eq!(s.font_size, default_font_size());
        assert_eq!(s.schema, SCHEMA_VERSION);
    }
}
