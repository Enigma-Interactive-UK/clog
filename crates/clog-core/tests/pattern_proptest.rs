//! Property test for the `PatternLayout` compiler: generate well-formed
//! records from a chosen builtin pattern, parse them back, and assert that
//! every emitted record is classified as a header with the expected level.

use clog_core::{CompiledPattern, Level, BUILTIN_PATTERNS};
use proptest::prelude::*;

fn level_word(lv: Level) -> &'static str {
    match lv {
        Level::Trace => "TRACE",
        Level::Debug => "DEBUG",
        Level::Info | Level::Unknown => "INFO",
        Level::Warn => "WARN",
        Level::Error => "ERROR",
        Level::Fatal => "FATAL",
        Level::Off => "OFF",
        Level::All => "ALL",
    }
}

fn level_strategy() -> impl Strategy<Value = Level> {
    prop_oneof![
        Just(Level::Trace),
        Just(Level::Debug),
        Just(Level::Info),
        Just(Level::Warn),
        Just(Level::Error),
        Just(Level::Fatal),
    ]
}

/// Render a level keyword padded to width `w` on the right with spaces.
fn pad_right(word: &str, w: usize) -> String {
    if word.len() >= w {
        word.to_string()
    } else {
        let mut s = String::with_capacity(w);
        s.push_str(word);
        for _ in word.len()..w {
            s.push(' ');
        }
        s
    }
}

fn thread_name() -> impl Strategy<Value = String> {
    // Letters, digits, dashes - avoids the bracket characters used as
    // surrounding literals in every builtin pattern.
    "[a-zA-Z][a-zA-Z0-9_-]{0,15}".prop_map(String::from)
}

fn logger_name() -> impl Strategy<Value = String> {
    "[a-z][a-z0-9]{0,10}".prop_map(String::from)
}

fn message_body() -> impl Strategy<Value = String> {
    // Anything except newline. Avoid leading whitespace, otherwise the line
    // could look like a continuation of a previous record (it isn't, here,
    // but the body shape stays realistic).
    "[A-Za-z][A-Za-z0-9 .,:;_/()=-]{0,80}".prop_map(String::from)
}

proptest! {
    /// Render-and-reparse round trip for the wsl-oink pattern.
    #[test]
    fn wsl_oink_roundtrips(
        level in level_strategy(),
        thread in thread_name(),
        logger in logger_name(),
        msg in message_body(),
    ) {
        let pat = CompiledPattern::compile(BUILTIN_PATTERNS[0].1).expect("compile");
        let level_text = pad_right(level_word(level), 5);
        let line = format!(
            "[{level_text}] 2026-05-22 16:28:59.246 [{thread}] {logger} - {msg}"
        );
        let h = pat
            .try_parse_header(line.as_bytes())
            .expect("must parse as header");
        prop_assert_eq!(h.level, level);
    }

    /// Render-and-reparse round trip for the prod pattern.
    #[test]
    fn prod_roundtrips(
        level in level_strategy(),
        thread in thread_name(),
        msg in message_body(),
    ) {
        let pat = CompiledPattern::compile(BUILTIN_PATTERNS[1].1).expect("compile");
        let level_text = level_word(level);
        let line = format!(
            "2026-05-21 00:00:04.401 {level_text} [{thread}] - {msg}"
        );
        let h = pat
            .try_parse_header(line.as_bytes())
            .expect("must parse as header");
        prop_assert_eq!(h.level, level);
    }
}
