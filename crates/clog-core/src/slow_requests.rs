//! Slow-request detection, aggregation, and speed-grid builder.
//!
//! Parses `SLOW REQUEST` lines emitted by Play 1.x in either of two
//! observed formats, dedupes records that report the same hit twice, and
//! groups them by (optionally normalised) URL path. A separate helper
//! buckets parsed occurrences across a fixed-count grid so the UI can
//! paint a file-wide speed heatmap.

use std::sync::OnceLock;

use regex::bytes::Regex;
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

/// Configurable speed-rail gradient anchors. When `None` at every
/// persistence tier, the rail falls back to per-file auto-normalisation.
/// When `Some`, both fields are present and `fast_ms < slow_ms`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlowRequestThresholds {
    pub fast_ms: u32,
    pub slow_ms: u32,
}

impl SlowRequestThresholds {
    /// Maximum permitted anchor value (10 minutes in ms).
    pub const MAX_MS: u32 = 600_000;

    /// Build a validated pair. Returns `None` when `fast_ms >= slow_ms`
    /// or when either field exceeds `MAX_MS`. Stored on disk as
    /// `Option<Self>` so "unset" and "set but invalid" never collide.
    #[must_use]
    pub fn new(fast_ms: u32, slow_ms: u32) -> Option<Self> {
        if fast_ms >= slow_ms {
            return None;
        }
        if slow_ms > Self::MAX_MS {
            return None;
        }
        Some(Self { fast_ms, slow_ms })
    }
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

/// Per-occurrence record, pre-dedup, pre-aggregation. Internal to the
/// crate; the public output is `SlowRequestOccurrence`.
#[derive(Debug, Clone)]
pub struct RawSlowRequest {
    pub duration_ms: u32,
    pub raw_path: String,
    pub class_method: String,
}

fn slow_request_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        // Anchored to message head. Two duration delimiters (`:` or
        // `()`), two class.method delimiters (`()` or `[]`). The
        // double-alternation captures land in groups 1+3 (format A) or
        // 2+4 (format B). Path is whitespace-bounded between them.
        Regex::new(
            r"(?-u)^SLOW REQUEST\s*(?::\s*(\d+)ms|\((\d+)ms\))\s*-\s*(\S+)\s+(?:\(([^)]+)\)|\[([^\]]+)\])",
        )
        .expect("slow-request regex compiles")
    })
}

/// Try to parse a single message-byte slice into a `RawSlowRequest`.
/// `None` when the bytes do not start with `SLOW REQUEST` or do not
/// match either supported phrasing.
#[must_use]
pub fn extract_raw(message: &[u8]) -> Option<RawSlowRequest> {
    let caps = slow_request_re().captures(message)?;
    let duration_bytes = caps.get(1).or_else(|| caps.get(2))?.as_bytes();
    let duration_ms: u32 = std::str::from_utf8(duration_bytes).ok()?.parse().ok()?;
    let raw_path = std::str::from_utf8(caps.get(3)?.as_bytes())
        .ok()?
        .to_string();
    let class_method_bytes = caps.get(4).or_else(|| caps.get(5))?.as_bytes();
    let class_method = std::str::from_utf8(class_method_bytes).ok()?.to_string();
    Some(RawSlowRequest {
        duration_ms,
        raw_path,
        class_method,
    })
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

    #[test]
    fn thresholds_new_accepts_valid_range() {
        assert!(SlowRequestThresholds::new(1000, 5000).is_some());
        assert!(SlowRequestThresholds::new(0, 600_000).is_some());
    }

    #[test]
    fn thresholds_new_rejects_fast_ge_slow() {
        assert!(SlowRequestThresholds::new(5000, 5000).is_none());
        assert!(SlowRequestThresholds::new(5000, 1000).is_none());
    }

    #[test]
    fn thresholds_new_rejects_out_of_bounds() {
        assert!(SlowRequestThresholds::new(0, 600_001).is_none());
    }

    fn make_message(msg: &str) -> Vec<u8> {
        msg.as_bytes().to_vec()
    }

    #[test]
    fn extract_parses_format_a() {
        let msg = make_message(
            "SLOW REQUEST: 5064ms - /preflight/killpreflightrequest.json (SoloPreflightFront.killPreflightRequest_JSON)",
        );
        let r = extract_raw(&msg).expect("format A parses");
        assert_eq!(r.duration_ms, 5064);
        assert_eq!(r.raw_path, "/preflight/killpreflightrequest.json");
        assert_eq!(
            r.class_method,
            "SoloPreflightFront.killPreflightRequest_JSON"
        );
    }

    #[test]
    fn extract_parses_format_b_with_suggestion_tail() {
        let msg = make_message(
            "SLOW REQUEST (5064ms) - /preflight/killpreflightrequest.json [SoloPreflightFront.killPreflightRequest_JSON] - consider using an asynchronous call to ease the load on the threadpool.",
        );
        let r = extract_raw(&msg).expect("format B parses");
        assert_eq!(r.duration_ms, 5064);
        assert_eq!(r.raw_path, "/preflight/killpreflightrequest.json");
        assert_eq!(
            r.class_method,
            "SoloPreflightFront.killPreflightRequest_JSON"
        );
    }

    #[test]
    fn extract_rejects_unrelated_lines() {
        assert!(extract_raw(b"Google 360 identifier /event-tickets/").is_none());
        assert!(extract_raw(b"finalisePreflightRequest designId 14861895").is_none());
        assert!(extract_raw(b"").is_none());
    }

    #[test]
    fn extract_rejects_anchored_substring_only() {
        assert!(extract_raw(b"prefix SLOW REQUEST: 1000ms - /x (Y.Z)").is_none());
    }
}
