//! Thread-name classification into a fixed five-group taxonomy + Other.
//!
//! Rules are tried in order; first match wins. Patterns are hand-rolled
//! byte matchers (no regex dependency in the hot path) because the
//! shapes are simple and `classify` runs once per record on the
//! filter/search hot path.
//!
//! Group set is locked for v1; see
//! docs/superpowers/specs/2026-05-24-thread-insights-design.md.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThreadGroup {
    Requests,
    Jobs,
    Scheduler,
    System,
    Infra,
    Other,
}

#[must_use]
pub fn group_bit(group: ThreadGroup) -> u8 {
    match group {
        ThreadGroup::Requests => 1 << 0,
        ThreadGroup::Jobs => 1 << 1,
        ThreadGroup::Scheduler => 1 << 2,
        ThreadGroup::System => 1 << 3,
        ThreadGroup::Infra => 1 << 4,
        ThreadGroup::Other => 1 << 5,
    }
}

/// Bitmask of thread groups the filter is allowed to include. A bit set =
/// include. Layout matches `group_bit`. `ALL` = 0x3F.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ThreadGroupMask(pub u8);

impl ThreadGroupMask {
    pub const ALL: Self = Self(0x3F);

    #[must_use]
    pub fn allows(self, group: ThreadGroup) -> bool {
        self.0 & group_bit(group) != 0
    }

    #[must_use]
    pub fn with(self, group: ThreadGroup, allow: bool) -> Self {
        if allow {
            Self(self.0 | group_bit(group))
        } else {
            Self(self.0 & !group_bit(group))
        }
    }
}

impl Default for ThreadGroupMask {
    fn default() -> Self {
        Self::ALL
    }
}

/// Classify a thread byte slice into one of the five named groups, or
/// `Other` as the fallthrough.
#[must_use]
pub fn classify(thread: &[u8]) -> ThreadGroup {
    // 1. Requests: ^play-thread-\d+$
    if has_prefix_then_digits(thread, b"play-thread-") {
        return ThreadGroup::Requests;
    }
    // 2. Jobs: ^jobs-thread-\d+$
    if has_prefix_then_digits(thread, b"jobs-thread-") {
        return ThreadGroup::Jobs;
    }
    // 3. Scheduler: case-insensitive substring "quartz"
    if contains_ascii_ci(thread, b"quartz") {
        return ThreadGroup::Scheduler;
    }
    // 4. System: ^main$  |  ^Thread-\d+$
    if thread == b"main" {
        return ThreadGroup::System;
    }
    if has_prefix_then_digits(thread, b"Thread-") {
        return ThreadGroup::System;
    }
    // 5. Infra: well-known framework plumbing names.
    if matches_infra(thread) {
        return ThreadGroup::Infra;
    }
    ThreadGroup::Other
}

/// True iff `s` starts with `prefix` and the remainder is a non-empty
/// ASCII-digit-only tail.
fn has_prefix_then_digits(s: &[u8], prefix: &[u8]) -> bool {
    if !s.starts_with(prefix) {
        return false;
    }
    let tail = &s[prefix.len()..];
    !tail.is_empty() && tail.iter().all(u8::is_ascii_digit)
}

/// Case-insensitive ASCII substring search. Non-ASCII bytes in either
/// side compare exactly. Fine for our needs - "quartz" is pure ASCII.
fn contains_ascii_ci(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() {
        return true;
    }
    if haystack.len() < needle.len() {
        return false;
    }
    let nl = needle.len();
    let limit = haystack.len() - nl + 1;
    'outer: for i in 0..limit {
        for j in 0..nl {
            if !haystack[i + j].eq_ignore_ascii_case(&needle[j]) {
                continue 'outer;
            }
        }
        return true;
    }
    false
}

fn matches_infra(s: &[u8]) -> bool {
    // pool-\d+-thread-\d+
    if s.starts_with(b"pool-") {
        let rest = &s[5..];
        if let Some(dash) = rest.iter().position(|&b| b == b'-') {
            let pool_id = &rest[..dash];
            let after = &rest[dash + 1..];
            if !pool_id.is_empty()
                && pool_id.iter().all(u8::is_ascii_digit)
                && after.starts_with(b"thread-")
                && has_digits_only(&after[7..])
            {
                return true;
            }
        }
    }
    // New I/O worker #\d+   |   New I/O boss #\d+
    if let Some(rest) = strip_prefix(s, b"New I/O worker #") {
        return has_digits_only(rest);
    }
    if let Some(rest) = strip_prefix(s, b"New I/O boss #") {
        return has_digits_only(rest);
    }
    // I/O dispatcher \d+
    if let Some(rest) = strip_prefix(s, b"I/O dispatcher ") {
        return has_digits_only(rest);
    }
    // jgroups-...   (any tail; the suffix carries cluster/node and varies)
    if s.starts_with(b"jgroups-") {
        return true;
    }
    // Memcached IO over ...
    if s.starts_with(b"Memcached IO ") {
        return true;
    }
    false
}

fn strip_prefix<'a>(s: &'a [u8], prefix: &[u8]) -> Option<&'a [u8]> {
    if s.starts_with(prefix) {
        Some(&s[prefix.len()..])
    } else {
        None
    }
}

fn has_digits_only(s: &[u8]) -> bool {
    !s.is_empty() && s.iter().all(u8::is_ascii_digit)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_play_request_workers() {
        assert_eq!(classify(b"play-thread-1"), ThreadGroup::Requests);
        assert_eq!(classify(b"play-thread-20"), ThreadGroup::Requests);
    }

    #[test]
    fn classifies_play_job_workers() {
        assert_eq!(classify(b"jobs-thread-1"), ThreadGroup::Jobs);
        assert_eq!(classify(b"jobs-thread-8"), ThreadGroup::Jobs);
    }

    #[test]
    fn classifies_quartz_workers() {
        assert_eq!(
            classify(b"DefaultQuartzScheduler_Worker-10"),
            ThreadGroup::Scheduler
        );
        assert_eq!(classify(b"quartz-scheduler-1"), ThreadGroup::Scheduler);
        assert_eq!(
            classify(b"MyQuartzScheduler_Worker-3"),
            ThreadGroup::Scheduler
        );
    }

    #[test]
    fn classifies_system_threads() {
        assert_eq!(classify(b"main"), ThreadGroup::System);
        assert_eq!(classify(b"Thread-16"), ThreadGroup::System);
        assert_eq!(classify(b"Thread-1"), ThreadGroup::System);
    }

    #[test]
    fn classifies_infra_threads() {
        assert_eq!(classify(b"pool-3-thread-1"), ThreadGroup::Infra);
        assert_eq!(classify(b"pool-6-thread-1"), ThreadGroup::Infra);
        assert_eq!(classify(b"New I/O worker #1"), ThreadGroup::Infra);
        assert_eq!(classify(b"New I/O worker #63"), ThreadGroup::Infra);
        assert_eq!(classify(b"New I/O boss #132"), ThreadGroup::Infra);
        assert_eq!(classify(b"I/O dispatcher 1"), ThreadGroup::Infra);
        assert_eq!(
            classify(b"jgroups-12,solo.prod,solo-webapp-001-27322"),
            ThreadGroup::Infra
        );
        assert_eq!(
            classify(
                b"Memcached IO over {MemcachedConnection to /127.0.0.1:11211} - SHUTTING DOWN"
            ),
            ThreadGroup::Infra
        );
    }

    #[test]
    fn classifies_unknown_as_other() {
        assert_eq!(classify(b""), ThreadGroup::Other);
        assert_eq!(classify(b"some-other-thread"), ThreadGroup::Other);
        assert_eq!(classify(b"play-thread-"), ThreadGroup::Other); // empty digit tail
        assert_eq!(classify(b"play-thread-abc"), ThreadGroup::Other); // non-digit tail
        assert_eq!(classify(b"jobs-thread-1a"), ThreadGroup::Other);
        assert_eq!(classify(b"\xff\xfe\x00"), ThreadGroup::Other); // garbage bytes
    }

    #[test]
    fn first_match_wins_does_not_mis_route_main_substring() {
        // A thread that contains "main" but isn't exactly "main" must not
        // be classified as System.
        assert_eq!(classify(b"main-pool-worker-2"), ThreadGroup::Other);
    }

    #[test]
    fn mask_round_trip() {
        let m = ThreadGroupMask::ALL.with(ThreadGroup::Requests, false);
        assert!(!m.allows(ThreadGroup::Requests));
        assert!(m.allows(ThreadGroup::Jobs));
        let m2 = m.with(ThreadGroup::Requests, true);
        assert_eq!(m2, ThreadGroupMask::ALL);
    }

    #[test]
    fn mask_all_is_0x3f() {
        assert_eq!(ThreadGroupMask::ALL.0, 0x3F);
    }
}
