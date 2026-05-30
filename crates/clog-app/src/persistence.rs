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

use clog_core::SlowRequestThresholds;
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
    /// Global default thresholds used when no per-file override is set.
    /// Absent / `None` means slow-request detection is disabled.
    #[serde(default)]
    pub slow_request_thresholds: Option<SlowRequestThresholds>,
    /// When true, swap the diagnostic colour tokens (speed rail, level
    /// palette, marker rail, user highlight palette) for the Wong
    /// colour-blind safe palette so red/green deficiencies can still tell
    /// the stops apart.
    #[serde(default)]
    pub colour_blind: bool,
    /// 0.0..=1.0 multiplier on the per-bucket alpha of the minimap's hot
    /// overlay (warn/error/fatal heatmap). 0 hides the heatmap entirely,
    /// 1 is the historic full-strength behaviour.
    #[serde(default = "default_minimap_heatmap_blend")]
    pub minimap_heatmap_blend: f32,
    /// 0.0..=1.0 CSS opacity applied to the minimap's heatmap canvas
    /// (wash + hot overlay). The scroll-position handle stays fully
    /// opaque regardless. 0 hides the visualisation while the handle
    /// remains usable; 1 is the historic full-opacity behaviour.
    #[serde(default = "default_minimap_background_opacity")]
    pub minimap_background_opacity: f32,
    /// When false, the per-tab slow-request speed rail beside the
    /// minimap is hidden globally even when slow-request data is
    /// available.
    #[serde(default = "default_true")]
    pub speed_rail_enabled: bool,
    /// Optional override for the UI's monospace font family. When `None`
    /// or empty, the default `--font-mono` stack from `style.css` applies.
    /// When set, the family name is prepended to that stack so the rest
    /// of the fallback chain still kicks in if the chosen face is gone.
    #[serde(default)]
    pub mono_font_family: Option<String>,
    /// Global default collapse mode for multi-line records.
    /// `"none"` | `"errors"` | `"all"`. Default `"none"`.
    #[serde(default = "default_collapse_records_default")]
    pub collapse_records_default: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            schema: SCHEMA_VERSION,
            theme: default_theme(),
            font_size: default_font_size(),
            recent_files: Vec::new(),
            follow_tail_default: true,
            slow_request_thresholds: None,
            colour_blind: false,
            minimap_heatmap_blend: default_minimap_heatmap_blend(),
            minimap_background_opacity: default_minimap_background_opacity(),
            speed_rail_enabled: true,
            mono_font_family: None,
            collapse_records_default: default_collapse_records_default(),
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
fn default_minimap_heatmap_blend() -> f32 {
    0.0
}
fn default_minimap_background_opacity() -> f32 {
    0.5
}
fn default_collapse_records_default() -> String {
    "none".to_string()
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
    #[serde(default = "default_full_thread_group_mask")]
    pub thread_group_mask: u32,
    #[serde(default)]
    pub filter_text: String,
    /// `"smart"` | `"regex"`.
    #[serde(default = "default_smart")]
    pub search_mode: String,
    #[serde(default)]
    pub search_case_sensitive: bool,
    #[serde(default)]
    pub filter_mode: bool,
    /// Sorted, deduped physical line indices the user has bookmarked. Lines
    /// that no longer exist on next open are silently dropped UI-side.
    #[serde(default)]
    pub bookmarks: Vec<u64>,
    /// Per-file collapse mode: `"inherit"` | `"none"` | `"errors"` | `"all"`.
    /// Default `"inherit"` so a v1 session restores to "follow global".
    #[serde(default = "default_collapse_mode")]
    pub collapse_mode: String,
    /// Sorted, deduped header-row physical line indices forced open against
    /// the mode. Out-of-range entries are dropped UI-side on restore.
    #[serde(default)]
    pub manually_expanded: Vec<u64>,
    /// Sorted, deduped header-row physical line indices forced closed against
    /// the mode.
    #[serde(default)]
    pub manually_collapsed: Vec<u64>,
    /// First visible physical line (inclusive) of the truncate window, or
    /// `None` for no "above" cut. Out-of-range values are dropped UI-side.
    #[serde(default)]
    pub truncate_before: Option<u64>,
    /// One past the last visible physical line of the truncate window, or
    /// `None` for no "below" cut.
    #[serde(default)]
    pub truncate_after: Option<u64>,
}

fn default_full_mask() -> u32 {
    0xFFFF_FFFF
}
fn default_full_thread_group_mask() -> u32 {
    0x3F
}
fn default_smart() -> String {
    "smart".to_string()
}
fn default_collapse_mode() -> String {
    "inherit".to_string()
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

// --- highlight-rules.json (global) ----------------------------------------
//
// The UI carries every user-editable knob (colour, bold, italic, underline,
// priority, enabled, scope-name). The persistence layer is intentionally
// permissive: every field is `#[serde(default)]` so a future addition stays
// load-compatible, and unknown fields are dropped on the floor.

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[allow(clippy::struct_excessive_bools)] // four bools is the user-facing knob set
pub struct UserHighlightRule {
    pub name: String,
    pub pattern: String,
    #[serde(default)]
    pub flags: String,
    #[serde(default = "default_priority")]
    pub priority: i32,
    /// Palette key (`"red"`, `"blue"`, ...) or empty for "use class only".
    #[serde(default)]
    pub colour: String,
    /// Background palette key (same alphabet as `colour`) or empty for no
    /// background. Layered on top of axis-1 row backgrounds via !important.
    #[serde(default)]
    pub background: String,
    #[serde(default)]
    pub bold: bool,
    #[serde(default)]
    pub italic: bool,
    #[serde(default)]
    pub underline: bool,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_priority() -> i32 {
    100
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HighlightRulesFile {
    #[serde(default = "default_schema")]
    pub schema: u32,
    #[serde(default)]
    pub rules: Vec<UserHighlightRule>,
}

impl HighlightRulesFile {
    pub fn load() -> Self {
        load_or_default(&paths::highlight_rules_path())
    }

    /// # Errors
    /// Bubbles up filesystem errors.
    pub fn save(&self) -> io::Result<()> {
        write_atomic(&paths::highlight_rules_path(), self)
    }
}

// --- per-file-rules/<hash>.json -------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PerFileRulesFile {
    #[serde(default = "default_schema")]
    pub schema: u32,
    /// The absolute source path the rules apply to. Recorded inside the file
    /// so a hash collision (vanishingly rare in practice) can be detected by
    /// the loader and treated as a miss.
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub rules: Vec<UserHighlightRule>,
    /// Per-file slow-request thresholds. `None` means "inherit the global
    /// default from `Settings`". An explicit `Some` overrides it.
    #[serde(default)]
    pub slow_request_thresholds: Option<SlowRequestThresholds>,
}

impl PerFileRulesFile {
    pub fn load(source_path: &Path) -> Self {
        let file_path = paths::per_file_rules_path(source_path);
        let mut f: Self = load_or_default(&file_path);
        // If a hash collision parked someone else's rules here, treat the
        // miss as empty rather than handing the wrong file's rules over.
        let key = source_path.to_string_lossy().to_string();
        if !f.path.is_empty() && f.path != key {
            f = Self::default();
        }
        f
    }

    /// # Errors
    /// Bubbles up filesystem errors.
    pub fn save(&self, source_path: &Path) -> io::Result<()> {
        let mut to_write = self.clone();
        to_write.path = source_path.to_string_lossy().to_string();
        write_atomic(&paths::per_file_rules_path(source_path), &to_write)
    }

    /// True when this file holds no highlight rules and no slow-request
    /// thresholds. Callers use this to decide whether `save` should
    /// instead delete the file - leaving an empty stub on disk is wasted
    /// I/O and confuses readers grepping the per-file-rules folder.
    #[must_use]
    pub fn is_effectively_empty(&self) -> bool {
        self.rules.is_empty() && self.slow_request_thresholds.is_none()
    }

    /// Delete the per-file rules file for `source_path`. Idempotent.
    pub fn forget(source_path: &Path) -> io::Result<()> {
        let p = paths::per_file_rules_path(source_path);
        match fs::remove_file(&p) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        }
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

#[cfg(test)]
mod thresholds_tests {
    use super::*;
    use clog_core::SlowRequestThresholds;

    #[test]
    fn restored_file_loads_old_payload_without_thread_group_mask() {
        let raw = r#"{"path":"/x","scroll_top":0,"follow_tail":true,"level_mask":63,"filter_text":"","search_mode":"smart","search_case_sensitive":false,"filter_mode":false}"#;
        let r: RestoredFile = serde_json::from_str(raw).expect("v1 RestoredFile decodes");
        assert_eq!(r.thread_group_mask, 0x3F);
    }

    #[test]
    fn restored_file_round_trips_thread_group_mask() {
        let r = RestoredFile {
            path: "/x".into(),
            scroll_top: 0.0,
            follow_tail: true,
            level_mask: 63,
            thread_group_mask: 0x0B,
            filter_text: String::new(),
            search_mode: "smart".into(),
            search_case_sensitive: false,
            filter_mode: false,
            bookmarks: vec![],
            collapse_mode: "inherit".into(),
            manually_expanded: vec![],
            manually_collapsed: vec![],
            truncate_before: None,
            truncate_after: None,
        };
        let json = serde_json::to_string(&r).expect("serialises");
        let back: RestoredFile = serde_json::from_str(&json).expect("round-trips");
        assert_eq!(back.thread_group_mask, 0x0B);
    }

    #[test]
    fn restored_file_defaults_collapse_fields_when_absent() {
        let raw = r#"{"path":"/x","scroll_top":0,"follow_tail":true,"level_mask":63,"thread_group_mask":63,"filter_text":"","search_mode":"smart","search_case_sensitive":false,"filter_mode":false,"bookmarks":[]}"#;
        let r: RestoredFile = serde_json::from_str(raw).expect("v1 RestoredFile decodes");
        assert_eq!(r.collapse_mode, "inherit");
        assert!(r.manually_expanded.is_empty());
        assert!(r.manually_collapsed.is_empty());
        assert_eq!(r.truncate_before, None);
        assert_eq!(r.truncate_after, None);
    }

    #[test]
    fn restored_file_round_trips_collapse_fields() {
        let r = RestoredFile {
            path: "/x".into(),
            scroll_top: 0.0,
            follow_tail: true,
            level_mask: 63,
            thread_group_mask: 0x3F,
            filter_text: String::new(),
            search_mode: "smart".into(),
            search_case_sensitive: false,
            filter_mode: false,
            bookmarks: vec![],
            collapse_mode: "errors".into(),
            manually_expanded: vec![3, 9, 12],
            manually_collapsed: vec![7],
            truncate_before: Some(12),
            truncate_after: Some(900),
        };
        let json = serde_json::to_string(&r).expect("serialises");
        let back: RestoredFile = serde_json::from_str(&json).expect("round-trips");
        assert_eq!(back.collapse_mode, "errors");
        assert_eq!(back.manually_expanded, vec![3, 9, 12]);
        assert_eq!(back.manually_collapsed, vec![7]);
        assert_eq!(back.truncate_before, Some(12));
        assert_eq!(back.truncate_after, Some(900));
    }

    #[test]
    fn settings_defaults_collapse_records_default_to_none() {
        let raw = r#"{"schema":1,"theme":"dark","font_size":13,"recent_files":[],"follow_tail_default":true}"#;
        let s: Settings = serde_json::from_str(raw).expect("v1 settings decodes");
        assert_eq!(s.collapse_records_default, "none");
    }

    #[test]
    fn settings_loads_old_file_without_threshold_field() {
        let raw = r#"{"schema":1,"theme":"dark","font_size":13,"recent_files":[],"follow_tail_default":true}"#;
        let s: Settings = serde_json::from_str(raw).expect("v1 settings decodes");
        assert!(s.slow_request_thresholds.is_none());
    }

    #[test]
    fn settings_round_trips_thresholds() {
        let s = Settings {
            slow_request_thresholds: SlowRequestThresholds::new(1000, 5000),
            ..Settings::default()
        };
        let json = serde_json::to_string(&s).expect("serialises");
        let back: Settings = serde_json::from_str(&json).expect("round-trips");
        assert_eq!(back.slow_request_thresholds, s.slow_request_thresholds);
    }

    #[test]
    fn per_file_rules_loads_old_file_without_threshold_field() {
        let raw = r#"{"schema":1,"path":"/x","rules":[]}"#;
        let f: PerFileRulesFile = serde_json::from_str(raw).expect("v1 per-file decodes");
        assert!(f.slow_request_thresholds.is_none());
    }

    #[test]
    fn per_file_rules_is_empty_when_no_rules_and_no_thresholds() {
        let f = PerFileRulesFile::default();
        assert!(f.is_effectively_empty());
        let f2 = PerFileRulesFile {
            slow_request_thresholds: SlowRequestThresholds::new(100, 200),
            ..PerFileRulesFile::default()
        };
        assert!(!f2.is_effectively_empty());
    }
}
