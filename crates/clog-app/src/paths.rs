//! Filesystem layout for clog's persistent data.
//!
//! Two modes:
//! - **Portable**: if a `clog-data\` directory sits next to the running
//!   executable, that's the data root. Makes the portable zip genuinely
//!   portable across machines.
//! - **Per-user**: otherwise the root is `%LOCALAPPDATA%\clog\` (or
//!   `$HOME/.local/share/clog` on non-Windows for dev convenience).
//!
//! `data_dir()` is the single entry point. Sub-helpers below pin per-concern
//! paths so callers don't have to reach for `.join("settings.json")` literals.

use std::path::PathBuf;

/// Resolve clog's data root. Portable-mode detection runs once per call and
/// is cheap (just a `Path::exists` next to `current_exe`). Creates the
/// directory if missing.
pub fn data_dir() -> PathBuf {
    let dir = if let Some(portable) = portable_root() {
        portable
    } else {
        per_user_root()
    };
    let _ = std::fs::create_dir_all(&dir);
    dir
}

fn portable_root() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let parent = exe.parent()?;
    let candidate = parent.join("clog-data");
    if candidate.is_dir() {
        Some(candidate)
    } else {
        None
    }
}

fn per_user_root() -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        if let Ok(local) = std::env::var("LOCALAPPDATA") {
            if !local.is_empty() {
                return PathBuf::from(local).join("clog");
            }
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        if let Ok(xdg) = std::env::var("XDG_DATA_HOME") {
            if !xdg.is_empty() {
                return PathBuf::from(xdg).join("clog");
            }
        }
        if let Ok(home) = std::env::var("HOME") {
            if !home.is_empty() {
                return PathBuf::from(home).join(".local/share/clog");
            }
        }
    }
    std::env::temp_dir().join("clog")
}

pub fn settings_path() -> PathBuf {
    data_dir().join("settings.json")
}

pub fn session_path() -> PathBuf {
    data_dir().join("session.json")
}

pub fn patterns_path() -> PathBuf {
    data_dir().join("patterns.json")
}

pub fn logs_dir() -> PathBuf {
    let dir = data_dir().join("logs");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

pub fn index_dir() -> PathBuf {
    let dir = data_dir().join("index");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// Hex-encoded blake3 of the absolute, case-folded path. Stable across runs,
/// collision-free in practice, keeps potentially-sensitive paths out of
/// filenames.
pub fn path_hash(path: &std::path::Path) -> String {
    use std::fmt::Write as _;
    let abs = path
        .canonicalize()
        .unwrap_or_else(|_| path.to_path_buf())
        .to_string_lossy()
        .to_lowercase();
    let hash = blake3::hash(abs.as_bytes());
    let mut hex = String::with_capacity(32);
    for b in hash.as_bytes().iter().take(16) {
        let _ = write!(&mut hex, "{b:02x}");
    }
    hex
}

pub fn index_cache_path(source_path: &std::path::Path) -> PathBuf {
    index_dir().join(format!("{}.idx", path_hash(source_path)))
}
