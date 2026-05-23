//! log4j2 `PatternLayout` compiler.
//!
//! Compiles a pattern string (e.g. `[%-5level] %d{yyyy-MM-dd HH:mm:ss.SSS}
//! [%t] %c{1} - %msg%n`) into a `CompiledPattern` that can classify a line
//! as a record header, extract field byte-spans for axis-1 styling, and
//! score a sample of lines to drive auto-detect.
//!
//! Supported subset (matches design.md s5):
//! - `%d{...}` timestamps (digit/literal classification of the format)
//! - `%level`, `%-Nlevel`, `%p`, `%-Np`
//! - `%t`, `%c`, `%c{N}`, `%C`, `%C{N}` (class name; aliased to logger for
//!   structural styling - same field-role as logger from the parser's POV)
//! - `%F` (source filename, variable-length up to next literal)
//! - `%L` (source line number, digits up to next literal)
//! - `%msg`, `%m`
//! - `%n`
//! - literal text
//!
//! Unknown specifiers (`%X{}`, `%mdc`, `%throwable{}` etc.) compile to a
//! literal placeholder so the rest of the line still parses; they show up
//! as `PatternWarning`s in the returned `CompiledPattern`.

use serde::Serialize;
use thiserror::Error;

use crate::record::Level;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Token {
    Literal(Vec<u8>),
    Level {
        width: Option<usize>,
    },
    /// Timestamp matcher derived from the date-format string.
    Date(DateFormat),
    /// Thread name. Greedy until the next literal token (or end of header).
    Thread,
    /// Logger name. `precision` is the trailing-segments hint (`%c{N}`); it
    /// only affects rendering - the matcher itself reads until the next
    /// literal.
    Logger {
        precision: Option<u32>,
    },
    /// Message body. Consumes the rest of the line.
    Message,
    /// Record terminator (`%n`). Must be the last token if present.
    Newline,
    /// Source filename (`%F`). Variable-length, bounded by the next literal.
    /// Recorded into `HeaderFields::logger` for axis-1 styling since it
    /// occupies the same structural lane visually.
    SourceFile,
    /// Source line number (`%L`). Variable-length digit run, bounded by the
    /// next literal. Not styled separately.
    SourceLine,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DateFormat {
    pub raw: String,
    pub atoms: Vec<DateAtom>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DateAtom {
    /// One ASCII digit. Comes from y/M/d/H/m/s/S/N format chars.
    Digit,
    /// Literal byte that must match exactly.
    Literal(u8),
}

#[derive(Debug, Error, Clone)]
pub enum PatternError {
    #[error("pattern is empty")]
    Empty,
    #[error("unterminated `%d{{` (missing `}}`) at column {col}")]
    UnterminatedDateBrace { col: usize },
    #[error("unterminated `%c{{` (missing `}}`) at column {col}")]
    UnterminatedLoggerBrace { col: usize },
    #[error("logger precision `%c{{{found}}}` is not a positive integer at column {col}")]
    BadLoggerPrecision { col: usize, found: String },
    #[error("level width `%-{found}level` is not a positive integer at column {col}")]
    BadLevelWidth { col: usize, found: String },
    #[error("dangling `%` at end of pattern")]
    DanglingPercent,
    #[error("variable-length token `%{kind}` at column {col} must be followed by a literal or be the last header token")]
    AmbiguousVariableToken { kind: &'static str, col: usize },
}

#[derive(Debug, Clone, Serialize)]
pub struct PatternWarning {
    pub message: String,
    pub col: usize,
}

#[derive(Debug, Clone)]
pub struct CompiledPattern {
    pub source: String,
    pub tokens: Vec<Token>,
    pub warnings: Vec<PatternWarning>,
}

/// Byte ranges *within the first line of a record* (relative to the line's
/// first byte, not the file). Axis-1 styling references these.
#[derive(Debug, Clone, Default, Serialize)]
pub struct HeaderFields {
    pub level: Option<(u32, u32)>,
    pub timestamp: Option<(u32, u32)>,
    pub thread: Option<(u32, u32)>,
    pub logger: Option<(u32, u32)>,
    pub message: Option<(u32, u32)>,
}

#[derive(Debug, Clone)]
pub struct ParsedHeader {
    pub level: Level,
    pub fields: HeaderFields,
}

impl CompiledPattern {
    /// Compile `src` into a token sequence.
    ///
    /// # Errors
    ///
    /// Returns `PatternError` if the pattern is syntactically invalid (bad
    /// braces, dangling `%`, unparseable level width, or a variable-length
    /// specifier with no following literal terminator).
    #[allow(clippy::too_many_lines)]
    pub fn compile(src: &str) -> Result<Self, PatternError> {
        if src.is_empty() {
            return Err(PatternError::Empty);
        }
        let bytes = src.as_bytes();
        let mut tokens: Vec<Token> = Vec::new();
        let mut warnings: Vec<PatternWarning> = Vec::new();
        let mut lit: Vec<u8> = Vec::new();
        let mut i = 0;

        let flush_lit = |lit: &mut Vec<u8>, tokens: &mut Vec<Token>| {
            if !lit.is_empty() {
                tokens.push(Token::Literal(std::mem::take(lit)));
            }
        };

        while i < bytes.len() {
            let b = bytes[i];
            if b != b'%' {
                lit.push(b);
                i += 1;
                continue;
            }
            // We're at a `%`. Parse a specifier.
            let spec_col = i;
            i += 1;
            if i >= bytes.len() {
                return Err(PatternError::DanglingPercent);
            }
            // Optional width: `-N`
            let mut width: Option<usize> = None;
            if bytes[i] == b'-' {
                let mut j = i + 1;
                let start = j;
                while j < bytes.len() && bytes[j].is_ascii_digit() {
                    j += 1;
                }
                if j == start {
                    return Err(PatternError::BadLevelWidth {
                        col: spec_col,
                        found: String::new(),
                    });
                }
                let n: usize = src[start..j]
                    .parse()
                    .map_err(|_| PatternError::BadLevelWidth {
                        col: spec_col,
                        found: src[start..j].to_string(),
                    })?;
                width = Some(n);
                i = j;
            }

            // Parse the specifier name.
            let name_start = i;
            while i < bytes.len() && bytes[i].is_ascii_alphabetic() {
                i += 1;
            }
            let name = &src[name_start..i];
            match name {
                "d" => {
                    flush_lit(&mut lit, &mut tokens);
                    let fmt = parse_braced(bytes, &mut i, spec_col, BraceFor::Date)?
                        .unwrap_or_else(|| "yyyy-MM-dd HH:mm:ss,SSS".to_string());
                    let expanded = expand_named_date(&fmt);
                    tokens.push(Token::Date(DateFormat::compile(&expanded)));
                }
                "level" | "p" => {
                    flush_lit(&mut lit, &mut tokens);
                    tokens.push(Token::Level { width });
                }
                "t" => {
                    flush_lit(&mut lit, &mut tokens);
                    tokens.push(Token::Thread);
                }
                "c" | "C" => {
                    flush_lit(&mut lit, &mut tokens);
                    let precision = match parse_braced(bytes, &mut i, spec_col, BraceFor::Logger)? {
                        Some(s) => Some(s.parse::<u32>().map_err(|_| {
                            PatternError::BadLoggerPrecision {
                                col: spec_col,
                                found: s.clone(),
                            }
                        })?),
                        None => None,
                    };
                    tokens.push(Token::Logger { precision });
                }
                "F" => {
                    flush_lit(&mut lit, &mut tokens);
                    tokens.push(Token::SourceFile);
                }
                "L" => {
                    flush_lit(&mut lit, &mut tokens);
                    tokens.push(Token::SourceLine);
                }
                "msg" | "m" => {
                    flush_lit(&mut lit, &mut tokens);
                    tokens.push(Token::Message);
                }
                "n" => {
                    flush_lit(&mut lit, &mut tokens);
                    tokens.push(Token::Newline);
                }
                "" => {
                    // `%%` is a literal percent.
                    if i < bytes.len() && bytes[i] == b'%' {
                        lit.push(b'%');
                        i += 1;
                    } else {
                        return Err(PatternError::DanglingPercent);
                    }
                }
                other => {
                    // Unknown specifier. Emit a warning and absorb any
                    // trailing `{...}` so it does not poison literal parsing.
                    let _ = parse_braced(bytes, &mut i, spec_col, BraceFor::Unknown)?;
                    warnings.push(PatternWarning {
                        message: format!("unknown specifier `%{other}` ignored"),
                        col: spec_col,
                    });
                }
            }
        }
        flush_lit(&mut lit, &mut tokens);

        validate_variable_terminators(&tokens)?;

        Ok(Self {
            source: src.to_string(),
            tokens,
            warnings,
        })
    }

    /// Attempt to parse `line` (one physical line, without trailing newline)
    /// as a record header. Returns `Some(ParsedHeader)` on success,
    /// otherwise `None` - meaning `line` is a continuation of the previous
    /// record.
    #[must_use]
    pub fn try_parse_header(&self, line: &[u8]) -> Option<ParsedHeader> {
        let mut cur: usize = 0;
        let mut fields = HeaderFields::default();
        let mut level: Option<Level> = None;
        let toks = &self.tokens;

        for (idx, token) in toks.iter().enumerate() {
            match token {
                Token::Literal(bytes) => {
                    if line.len() < cur + bytes.len() {
                        return None;
                    }
                    if &line[cur..cur + bytes.len()] != bytes.as_slice() {
                        return None;
                    }
                    cur += bytes.len();
                }
                Token::Level { width } => {
                    let start = cur;
                    let (lv, consumed) = parse_level(&line[cur..], *width)?;
                    cur += consumed;
                    level = Some(lv);
                    // Field span: trim trailing padding spaces for highlight
                    // colouring but keep the consumed range whole.
                    let end = cur;
                    fields.level = Some((u32_of(start), u32_of(end)));
                }
                Token::Date(fmt) => {
                    let start = cur;
                    let consumed = fmt.match_at(&line[cur..])?;
                    cur += consumed;
                    fields.timestamp = Some((u32_of(start), u32_of(cur)));
                }
                Token::Thread => {
                    let next_lit = next_literal_bytes(toks, idx + 1);
                    let (consumed, _ok) = read_until_literal(&line[cur..], next_lit)?;
                    let start = cur;
                    cur += consumed;
                    fields.thread = Some((u32_of(start), u32_of(cur)));
                }
                Token::Logger { .. } => {
                    let next_lit = next_literal_bytes(toks, idx + 1);
                    let (consumed, _ok) = read_until_literal(&line[cur..], next_lit)?;
                    let start = cur;
                    cur += consumed;
                    fields.logger = Some((u32_of(start), u32_of(cur)));
                }
                Token::SourceFile => {
                    let next_lit = next_literal_bytes(toks, idx + 1);
                    let (consumed, _ok) = read_until_literal(&line[cur..], next_lit)?;
                    let start = cur;
                    cur += consumed;
                    // Reuse the logger field-span so axis-1 styling treats the
                    // source filename as the same structural lane visually.
                    // If the pattern already had a logger token, the previous
                    // span wins; we only overwrite when nothing was recorded.
                    if fields.logger.is_none() {
                        fields.logger = Some((u32_of(start), u32_of(cur)));
                    }
                }
                Token::SourceLine => {
                    // Read a run of ASCII digits. Length is bounded by the
                    // next literal (parens/space) but we don't need to
                    // forward-search: digits stop on the first non-digit.
                    let start = cur;
                    let mut j = cur;
                    while j < line.len() && line[j].is_ascii_digit() {
                        j += 1;
                    }
                    if j == start {
                        return None;
                    }
                    cur = j;
                }
                Token::Message => {
                    let start = cur;
                    cur = line.len();
                    fields.message = Some((u32_of(start), u32_of(cur)));
                }
                Token::Newline => {
                    // `%n` is the record terminator. A physical line never
                    // carries the trailing newline (caller stripped EOL),
                    // so this token simply asserts we have consumed everything.
                    // We accept extra trailing whitespace as a courtesy.
                }
            }
        }

        // After all tokens, allow leftover only when a Message token already
        // ate the remainder; otherwise reject.
        if !ends_with_message(toks) && cur != line.len() {
            return None;
        }

        Some(ParsedHeader {
            level: level.unwrap_or(Level::Unknown),
            fields,
        })
    }

    /// Score this pattern against a sample of physical lines. Returns the
    /// fraction (0.0..=1.0) that parsed as headers.
    #[must_use]
    pub fn match_score<'a, I>(&self, lines: I) -> f32
    where
        I: IntoIterator<Item = &'a [u8]>,
    {
        let mut total: u32 = 0;
        let mut hit: u32 = 0;
        for line in lines {
            total += 1;
            if self.try_parse_header(line).is_some() {
                hit += 1;
            }
        }
        if total == 0 {
            0.0
        } else {
            f32::from(u16::try_from(hit).unwrap_or(u16::MAX))
                / f32::from(u16::try_from(total).unwrap_or(u16::MAX))
        }
    }
}

fn u32_of(x: usize) -> u32 {
    u32::try_from(x).unwrap_or(u32::MAX)
}

fn ends_with_message(tokens: &[Token]) -> bool {
    tokens
        .iter()
        .rev()
        .find(|t| !matches!(t, Token::Newline))
        .is_some_and(|t| matches!(t, Token::Message))
}

fn next_literal_bytes(tokens: &[Token], from: usize) -> Option<&[u8]> {
    for t in &tokens[from..] {
        match t {
            Token::Literal(b) => return Some(b),
            Token::Newline => return None,
            _ => {}
        }
    }
    None
}

fn read_until_literal(buf: &[u8], needle: Option<&[u8]>) -> Option<(usize, bool)> {
    match needle {
        None => Some((buf.len(), true)),
        Some([]) => Some((0, true)),
        Some(n) => {
            // Reject empty captures: a thread/logger must have at least one
            // byte. This stops "[]" from matching `[%t]` and lets the next
            // literal terminator be picked up cleanly.
            let mut i = 1;
            while i + n.len() <= buf.len() {
                if &buf[i..i + n.len()] == n {
                    return Some((i, true));
                }
                i += 1;
            }
            None
        }
    }
}

fn parse_level(buf: &[u8], width: Option<usize>) -> Option<(Level, usize)> {
    if let Some(w) = width {
        if buf.len() < w {
            return None;
        }
        let raw = &buf[..w];
        // The level keyword is left-aligned; trailing bytes are space-pad.
        let mut end = raw.len();
        while end > 0 && raw[end - 1] == b' ' {
            end -= 1;
        }
        let lv = classify_level_word(&raw[..end])?;
        return Some((lv, w));
    }
    // No width: read greedy alphabetic.
    let mut j = 0;
    while j < buf.len() && buf[j].is_ascii_alphabetic() {
        j += 1;
    }
    if j == 0 {
        return None;
    }
    let lv = classify_level_word(&buf[..j])?;
    Some((lv, j))
}

fn classify_level_word(word: &[u8]) -> Option<Level> {
    match word {
        b"TRACE" => Some(Level::Trace),
        b"DEBUG" => Some(Level::Debug),
        b"INFO" => Some(Level::Info),
        b"WARN" => Some(Level::Warn),
        b"ERROR" => Some(Level::Error),
        b"FATAL" => Some(Level::Fatal),
        b"OFF" => Some(Level::Off),
        b"ALL" => Some(Level::All),
        _ => None,
    }
}

#[derive(Copy, Clone)]
enum BraceFor {
    Date,
    Logger,
    Unknown,
}

/// Parse an optional `{...}` immediately after a specifier name. Returns
/// `Ok(Some(inner))` if present, `Ok(None)` otherwise. Bumps `i` past the
/// closing brace on success.
fn parse_braced(
    bytes: &[u8],
    i: &mut usize,
    spec_col: usize,
    purpose: BraceFor,
) -> Result<Option<String>, PatternError> {
    if *i >= bytes.len() || bytes[*i] != b'{' {
        return Ok(None);
    }
    let start = *i + 1;
    let mut j = start;
    while j < bytes.len() && bytes[j] != b'}' {
        j += 1;
    }
    if j >= bytes.len() {
        return Err(match purpose {
            BraceFor::Date | BraceFor::Unknown => {
                PatternError::UnterminatedDateBrace { col: spec_col }
            }
            BraceFor::Logger => PatternError::UnterminatedLoggerBrace { col: spec_col },
        });
    }
    let inner = std::str::from_utf8(&bytes[start..j])
        .map(str::to_string)
        .unwrap_or_default();
    *i = j + 1;
    Ok(Some(inner))
}

/// Expand log4j2 named date formats like `ISO8601` and `DEFAULT`.
fn expand_named_date(fmt: &str) -> String {
    match fmt {
        "ISO8601" | "DEFAULT" => "yyyy-MM-dd HH:mm:ss,SSS".to_string(),
        "ISO8601_BASIC" => "yyyyMMdd HHmmss,SSS".to_string(),
        "ABSOLUTE" => "HH:mm:ss,SSS".to_string(),
        "DATE" => "dd MMM yyyy HH:mm:ss,SSS".to_string(),
        other => other.to_string(),
    }
}

impl DateFormat {
    fn compile(raw: &str) -> Self {
        let mut atoms: Vec<DateAtom> = Vec::new();
        let mut iter = raw.bytes();
        while let Some(b) = iter.next() {
            // Single-quoted literal section per SimpleDateFormat: 'X' or ''.
            if b == b'\'' {
                let mut closed = false;
                let mut next_byte = iter.next();
                while let Some(c) = next_byte {
                    if c == b'\'' {
                        closed = true;
                        break;
                    }
                    atoms.push(DateAtom::Literal(c));
                    next_byte = iter.next();
                }
                let _ = closed;
                continue;
            }
            let atom = if is_date_digit_char(b) {
                DateAtom::Digit
            } else {
                DateAtom::Literal(b)
            };
            atoms.push(atom);
        }
        Self {
            raw: raw.to_string(),
            atoms,
        }
    }

    fn match_at(&self, buf: &[u8]) -> Option<usize> {
        if buf.len() < self.atoms.len() {
            return None;
        }
        for (i, atom) in self.atoms.iter().enumerate() {
            let b = buf[i];
            match *atom {
                DateAtom::Digit => {
                    if !b.is_ascii_digit() {
                        return None;
                    }
                }
                DateAtom::Literal(want) => {
                    if b != want {
                        return None;
                    }
                }
            }
        }
        Some(self.atoms.len())
    }
}

fn is_date_digit_char(b: u8) -> bool {
    // SimpleDateFormat letters that expand to digits in the timestamps clog
    // sees. Day-of-week / month names (EEE, MMM) are not supported in v1.
    matches!(b, b'y' | b'M' | b'd' | b'H' | b'm' | b's' | b'S' | b'N')
}

fn validate_variable_terminators(tokens: &[Token]) -> Result<(), PatternError> {
    // Variable-length tokens must be DIRECTLY followed by a Literal anchor,
    // SourceLine (digits self-terminate), or the header tail.
    for (idx, tok) in tokens.iter().enumerate() {
        let kind = match tok {
            Token::Thread => "t",
            Token::Logger { .. } => "c",
            Token::SourceFile => "F",
            _ => continue,
        };
        let Some(next) = tokens.get(idx + 1) else {
            // Trailing variable token: greedy to end of header line, ok.
            continue;
        };
        match next {
            Token::Literal(_) | Token::Message | Token::Newline => {}
            _ => return Err(PatternError::AmbiguousVariableToken { kind, col: 0 }),
        }
    }
    Ok(())
}

/// Built-in patterns auto-detect runs against, in priority order. Order
/// matters as a tie-breaker: when two patterns score the same against a
/// sample, the earlier-listed wins. More specific patterns (more literal
/// anchors / extra tokens) come first so they edge out looser supersets.
pub const BUILTIN_PATTERNS: &[(&str, &str)] = &[
    (
        "wsl-oink",
        "[%-5level] %d{yyyy-MM-dd HH:mm:ss.SSS} [%t] %c{1} - %msg%n",
    ),
    ("play-class-site", "%d %-5p [%t] %C{2} (%F:%L) - %m%n"),
    ("play-absolute-site", "%d{ABSOLUTE} %-5p ~ (%F:%L) - %m%n"),
    ("prod", "%d{yyyy-MM-dd HH:mm:ss.SSS} %level [%t] - %msg%n"),
    (
        "log4j2-default",
        "%d{ISO8601} [%t] %-5level %c{36} - %msg%n",
    ),
    ("play-short-dash", "%d{HH:mm:ss,SSS} %level ~ - %msg%n"),
    ("play-absolute", "%d{ABSOLUTE} %-5p ~ %m%n"),
    ("play-short", "%d{HH:mm:ss,SSS} %level ~ %msg%n"),
    (
        "prod-no-thread",
        "%d{yyyy-MM-dd HH:mm:ss.SSS} %level - %msg%n",
    ),
];

/// Look up a built-in pattern source by name. Returns `None` if no such
/// pattern is registered. Tests prefer this over `BUILTIN_PATTERNS[i]` so
/// reordering or extending the list doesn't break call sites.
#[must_use]
pub fn builtin_pattern(name: &str) -> Option<&'static str> {
    BUILTIN_PATTERNS
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, src)| *src)
}

/// Compile every built-in pattern, score each against `lines`, and return
/// the best match together with its score. Returns `None` if no pattern
/// scores above zero.
#[must_use]
pub fn auto_detect<'a, I>(lines: I) -> Option<(&'static str, CompiledPattern, f32)>
where
    I: IntoIterator<Item = &'a [u8]>,
    I::IntoIter: Clone,
{
    let lines: Vec<&[u8]> = lines.into_iter().collect();
    let mut best: Option<(&'static str, CompiledPattern, f32)> = None;
    for (name, src) in BUILTIN_PATTERNS {
        let Ok(pat) = CompiledPattern::compile(src) else {
            continue;
        };
        let score = pat.match_score(lines.iter().copied());
        let take = match &best {
            None => true,
            Some((_, _, s)) => score > *s,
        };
        if take {
            best = Some((*name, pat, score));
        }
    }
    best.filter(|(_, _, s)| *s > 0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compile_wsl_oink_pattern() {
        let p = CompiledPattern::compile(BUILTIN_PATTERNS[0].1).expect("compile");
        assert!(p
            .tokens
            .iter()
            .any(|t| matches!(t, Token::Level { width: Some(5) })));
        assert!(p
            .tokens
            .iter()
            .any(|t| matches!(t, Token::Logger { precision: Some(1) })));
    }

    #[test]
    fn parse_wsl_oink_header_yields_field_spans() {
        let p = CompiledPattern::compile(BUILTIN_PATTERNS[0].1).expect("compile");
        let line = b"[INFO ] 2026-05-22 16:28:59.246 [main] play - Starting /var/play";
        let h = p.try_parse_header(line).expect("parses as header");
        assert_eq!(h.level, Level::Info);
        let (ls, le) = h.fields.level.unwrap();
        assert_eq!(&line[ls as usize..le as usize], b"INFO ");
        let (ts, te) = h.fields.timestamp.unwrap();
        assert_eq!(&line[ts as usize..te as usize], b"2026-05-22 16:28:59.246");
        let (ths, the) = h.fields.thread.unwrap();
        assert_eq!(&line[ths as usize..the as usize], b"main");
        let (lgs, lge) = h.fields.logger.unwrap();
        assert_eq!(&line[lgs as usize..lge as usize], b"play");
        let (ms, me) = h.fields.message.unwrap();
        assert_eq!(&line[ms as usize..me as usize], b"Starting /var/play");
    }

    #[test]
    fn parse_prod_header_no_logger() {
        let p = CompiledPattern::compile(builtin_pattern("prod").unwrap()).expect("compile");
        let line = b"2026-05-21 00:00:04.401 INFO [play-thread-1] - SLOW REQUEST: 2826ms";
        let h = p.try_parse_header(line).expect("parses");
        assert_eq!(h.level, Level::Info);
        let (ts, te) = h.fields.timestamp.unwrap();
        assert_eq!(&line[ts as usize..te as usize], b"2026-05-21 00:00:04.401");
        let (ths, the) = h.fields.thread.unwrap();
        assert_eq!(&line[ths as usize..the as usize], b"play-thread-1");
        assert!(h.fields.logger.is_none());
        let (ms, me) = h.fields.message.unwrap();
        assert!(line[ms as usize..me as usize].starts_with(b"SLOW REQUEST"));
    }

    #[test]
    fn parse_log4j2_default_header() {
        let p =
            CompiledPattern::compile(builtin_pattern("log4j2-default").unwrap()).expect("compile");
        let line = b"2026-05-22 16:28:59,246 [main] INFO  com.example.Foo - hi";
        let h = p.try_parse_header(line).expect("parses");
        assert_eq!(h.level, Level::Info);
    }

    #[test]
    fn rejects_continuation_lines() {
        let p = CompiledPattern::compile(BUILTIN_PATTERNS[0].1).expect("compile");
        assert!(p
            .try_parse_header(b"  at com.example.Foo.bar(Foo.java:42)")
            .is_none());
        assert!(p
            .try_parse_header(b"Caused by: java.lang.RuntimeException")
            .is_none());
    }

    #[test]
    fn match_score_runs() {
        let p = CompiledPattern::compile(BUILTIN_PATTERNS[0].1).expect("compile");
        let lines: Vec<&[u8]> = vec![
            b"[INFO ] 2026-05-22 16:28:59.246 [main] play - Starting" as &[u8],
            b"[WARN ] 2026-05-22 16:28:59.247 [main] play - hmm" as &[u8],
            b"  at com.example.Foo.bar(Foo.java:42)" as &[u8],
        ];
        let score = p.match_score(lines);
        assert!((score - 2.0 / 3.0).abs() < 1e-6);
    }

    #[test]
    fn auto_detect_picks_wsl_oink() {
        let lines: Vec<&[u8]> = vec![
            b"[INFO ] 2026-05-22 16:28:59.246 [main] play - Starting" as &[u8],
            b"[INFO ] 2026-05-22 16:28:59.247 [main] play - Module x" as &[u8],
        ];
        let (name, _, score) = auto_detect(lines).expect("detects");
        assert_eq!(name, "wsl-oink");
        assert!(score > 0.9);
    }

    #[test]
    fn auto_detect_picks_prod() {
        let lines: Vec<&[u8]> = vec![
            b"2026-05-21 00:00:04.401 INFO [play-thread-1] - hi" as &[u8],
            b"2026-05-21 00:00:30.409 WARN [play-thread-20] - oh" as &[u8],
        ];
        let (name, _, _) = auto_detect(lines).expect("detects");
        assert_eq!(name, "prod");
    }

    #[test]
    fn unknown_specifier_warns() {
        let p = CompiledPattern::compile("%X{foo} %level %msg%n").expect("compile");
        assert_eq!(p.warnings.len(), 1);
    }

    #[test]
    fn errors_on_dangling_percent() {
        assert!(matches!(
            CompiledPattern::compile("foo %"),
            Err(PatternError::DanglingPercent)
        ));
    }

    #[test]
    fn parse_class_site_header() {
        let p =
            CompiledPattern::compile(builtin_pattern("play-class-site").unwrap()).expect("compile");
        let line = b"2026-05-22 16:28:59,246 INFO  [main] com.example.Foo (Foo.java:42) - boot";
        let h = p.try_parse_header(line).expect("parses");
        assert_eq!(h.level, Level::Info);
        let (ts, te) = h.fields.timestamp.unwrap();
        assert_eq!(&line[ts as usize..te as usize], b"2026-05-22 16:28:59,246");
        let (ths, the) = h.fields.thread.unwrap();
        assert_eq!(&line[ths as usize..the as usize], b"main");
        // %C records into the logger field; the later %F does not overwrite it.
        let (lgs, lge) = h.fields.logger.unwrap();
        assert_eq!(&line[lgs as usize..lge as usize], b"com.example.Foo");
        let (ms, me) = h.fields.message.unwrap();
        assert_eq!(&line[ms as usize..me as usize], b"boot");
    }

    #[test]
    fn parse_absolute_site_header() {
        let p = CompiledPattern::compile(builtin_pattern("play-absolute-site").unwrap())
            .expect("compile");
        let line = b"12:30:45,123 INFO  ~ (App.java:7) - hello";
        let h = p.try_parse_header(line).expect("parses");
        assert_eq!(h.level, Level::Info);
        // %F lands in fields.logger because no prior %C/%c claimed it.
        let (lgs, lge) = h.fields.logger.unwrap();
        assert_eq!(&line[lgs as usize..lge as usize], b"App.java");
    }

    #[test]
    fn parse_short_dash_distinguishes_from_short() {
        let dash =
            CompiledPattern::compile(builtin_pattern("play-short-dash").unwrap()).expect("compile");
        let plain =
            CompiledPattern::compile(builtin_pattern("play-short").unwrap()).expect("compile");
        let with_dash = b"12:30:45,123 INFO ~ - greetings";
        let no_dash = b"12:30:45,123 INFO ~ greetings";
        assert!(dash.try_parse_header(with_dash).is_some());
        // play-short still matches the dashed line because %msg eats anything.
        assert!(plain.try_parse_header(with_dash).is_some());
        // But the dashed pattern must NOT match a line without the dash.
        assert!(dash.try_parse_header(no_dash).is_none());
        assert!(plain.try_parse_header(no_dash).is_some());
    }

    #[test]
    fn parse_absolute_pattern() {
        let p =
            CompiledPattern::compile(builtin_pattern("play-absolute").unwrap()).expect("compile");
        let line = b"12:30:45,123 INFO  ~ application booted";
        let h = p.try_parse_header(line).expect("parses");
        assert_eq!(h.level, Level::Info);
        let (ts, te) = h.fields.timestamp.unwrap();
        assert_eq!(&line[ts as usize..te as usize], b"12:30:45,123");
    }

    #[test]
    fn parse_prod_no_thread_header() {
        let p =
            CompiledPattern::compile(builtin_pattern("prod-no-thread").unwrap()).expect("compile");
        let line = b"2026-05-22 16:28:59.246 WARN - cache miss";
        let h = p.try_parse_header(line).expect("parses");
        assert_eq!(h.level, Level::Warn);
    }

    #[test]
    fn auto_detect_prefers_prod_over_prod_no_thread() {
        // A prod-shaped line (with thread) must keep auto-detecting as "prod"
        // even now that "prod-no-thread" exists. The shape with `[%t]` is
        // strictly more specific, so prod scores 1.0 and prod-no-thread 0.0.
        let lines: Vec<&[u8]> = vec![
            b"2026-05-21 00:00:04.401 INFO [play-thread-1] - hi" as &[u8],
            b"2026-05-21 00:00:30.409 WARN [play-thread-20] - oh" as &[u8],
        ];
        let (name, _, _) = auto_detect(lines).expect("detects");
        assert_eq!(name, "prod");
    }

    #[test]
    fn auto_detect_picks_class_site() {
        let lines: Vec<&[u8]> = vec![
            b"2026-05-22 16:28:59,246 INFO  [main] com.example.Foo (Foo.java:42) - hi" as &[u8],
            b"2026-05-22 16:28:59,247 WARN  [main] com.example.Bar (Bar.java:7) - oh" as &[u8],
        ];
        let (name, _, score) = auto_detect(lines).expect("detects");
        assert_eq!(name, "play-class-site");
        assert!(score > 0.9);
    }

    #[test]
    fn errors_on_unterminated_brace() {
        assert!(matches!(
            CompiledPattern::compile("%d{yyyy"),
            Err(PatternError::UnterminatedDateBrace { .. })
        ));
    }
}
