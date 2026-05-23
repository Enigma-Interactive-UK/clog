//! Slow-request detection, aggregation, and speed-grid builder.
//!
//! Parses `SLOW REQUEST` lines emitted by Play 1.x in either of two
//! observed formats, dedupes records that report the same hit twice, and
//! groups them by (optionally normalised) URL path. A separate helper
//! buckets parsed occurrences across a fixed-count grid so the UI can
//! paint a file-wide speed heatmap.

use serde::{Deserialize, Serialize};

/// How paths are grouped in `SlowRequestSummary.entries`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PathMode {
    /// Numeric / UUID / long-hex segments collapse to `{id}`, query
    /// strings are stripped, trailing slash preserved.
    Normalised,
    /// Each raw observed path is its own group.
    Raw,
}

/// Normalise a raw URL path for aggregation. See [`PathMode::Normalised`].
#[must_use]
pub fn normalise_path(raw: &str) -> String {
    // Strip the query string at the first `?`.
    let path = match raw.find('?') {
        Some(i) => &raw[..i],
        None => raw,
    };
    if path.is_empty() {
        return String::new();
    }
    let leading = path.starts_with('/');
    let trailing = path.ends_with('/') && path.len() > 1;
    let mut out = String::with_capacity(path.len());
    if leading {
        out.push('/');
    }
    let mut first = true;
    for seg in path.split('/').filter(|s| !s.is_empty()) {
        if !first {
            out.push('/');
        }
        first = false;
        out.push_str(&normalise_segment(seg));
    }
    if trailing {
        out.push('/');
    }
    out
}

fn normalise_segment(seg: &str) -> String {
    if seg.chars().all(|c| c.is_ascii_digit()) {
        return "{id}".to_string();
    }
    if is_uuid(seg) || is_long_hex(seg) {
        return "{id}".to_string();
    }
    seg.to_string()
}

fn is_uuid(seg: &str) -> bool {
    if seg.len() != 36 {
        return false;
    }
    let bytes = seg.as_bytes();
    let dash_positions = [8usize, 13, 18, 23];
    for (i, &b) in bytes.iter().enumerate() {
        if dash_positions.contains(&i) {
            if b != b'-' {
                return false;
            }
        } else if !b.is_ascii_hexdigit() {
            return false;
        }
    }
    true
}

fn is_long_hex(seg: &str) -> bool {
    seg.len() >= 12 && seg.bytes().all(|b| b.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalise_strips_query_string() {
        assert_eq!(normalise_path("/foo?bar=1&baz=2"), "/foo");
    }

    #[test]
    fn normalise_collapses_numeric_segments() {
        assert_eq!(normalise_path("/order/12345/edit"), "/order/{id}/edit");
        assert_eq!(normalise_path("/12/34/56"), "/{id}/{id}/{id}");
    }

    #[test]
    fn normalise_collapses_uuid_segments() {
        assert_eq!(
            normalise_path("/job/3f3c8b58-0d2d-4f12-9f8f-1a0bb6e5e1aa/status"),
            "/job/{id}/status"
        );
    }

    #[test]
    fn normalise_collapses_long_hex_runs() {
        assert_eq!(
            normalise_path("/asset/abcdef0123456789/preview"),
            "/asset/{id}/preview"
        );
    }

    #[test]
    fn normalise_preserves_trailing_slash() {
        assert_eq!(normalise_path("/foo/"), "/foo/");
        assert_eq!(normalise_path("/foo"), "/foo");
        assert_ne!(normalise_path("/foo/"), normalise_path("/foo"));
    }

    #[test]
    fn normalise_root_path_is_root() {
        assert_eq!(normalise_path("/"), "/");
    }

    #[test]
    fn normalise_leaves_human_segments_alone() {
        assert_eq!(
            normalise_path("/checkout/setdeliveryaddress.json"),
            "/checkout/setdeliveryaddress.json"
        );
    }
}
