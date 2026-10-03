//! A small, dependency-free JSON implementation.
//!
//! The engine's asset formats are all JSON, so this module is the front door of
//! the pipeline. It is deliberately small but not sloppy:
//!
//! * **Insertion-ordered objects.** [`JsonValue::Object`] is a `Vec` of pairs,
//!   not a map. A parse → write round trip therefore reproduces the author's key
//!   order exactly, which keeps generated assets (`noxel-gen`) diff-stable.
//! * **Precise errors.** [`JsonError`] carries a 1-based line and column so a
//!   broken hand-written asset file points at the offending byte.
//! * **Surrogate pairs.** `\uD83D\uDE00` decodes to a single `char`, because
//!   sprite and dialogue text is full of emoji.
//! * **Depth limited.** The parser refuses nesting deeper than [`MAX_DEPTH`]
//!   instead of overflowing the stack on a hostile file.
//!
//! ## Example
//!
//! ```
//! use noxel_asset::json::{parse_str, JsonValue};
//!
//! let v = parse_str(r#"{"name": "grass", "tile_size": 16}"#).unwrap();
//! assert_eq!(v.get_str("name"), Some("grass"));
//! assert_eq!(v.get_u32("tile_size", 0), 16);
//! assert_eq!(v.to_string(), r#"{"name":"grass","tile_size":16}"#);
//! ```

use std::fmt;

/// Maximum nesting depth accepted by the parser.
///
/// The parser is recursive descent, so this bound is what keeps a hostile file
/// from exhausting the stack. 256 levels is far beyond anything the engine's
/// formats need.
pub const MAX_DEPTH: usize = 256;

/// A parsed JSON value.
///
/// Numbers are always stored as `f64` (JSON has one number type); use the
/// accessors to convert, e.g. [`JsonValue::as_u32`].
#[derive(Clone, Debug, PartialEq)]
pub enum JsonValue {
    /// JSON `null`.
    Null,
    /// JSON `true` or `false`.
    Bool(bool),
    /// A JSON number. Never `NaN` or infinite: the parser only accepts finite
    /// literals, and the writer emits `null` for non-finite values it is handed.
    Number(f64),
    /// A JSON string, already unescaped.
    String(String),
    /// A JSON array.
    Array(Vec<JsonValue>),
    /// A JSON object, **in insertion order**. Duplicate keys are preserved as
    /// authored; lookups return the first match.
    Object(Vec<(String, JsonValue)>),
}

impl JsonValue {
    /// Builds an object value from `(key, value)` pairs.
    pub fn object<K, I>(entries: I) -> Self
    where
        K: Into<String>,
        I: IntoIterator<Item = (K, JsonValue)>,
    {
        Self::Object(entries.into_iter().map(|(k, v)| (k.into(), v)).collect())
    }

    /// Builds an array value.
    pub fn array<I>(items: I) -> Self
    where
        I: IntoIterator<Item = JsonValue>,
    {
        Self::Array(items.into_iter().collect())
    }

    /// The name of this value's type, for error messages: `"null"`, `"bool"`,
    /// `"number"`, `"string"`, `"array"` or `"object"`.
    #[must_use]
    pub fn type_name(&self) -> &'static str {
        match self {
            Self::Null => "null",
            Self::Bool(_) => "bool",
            Self::Number(_) => "number",
            Self::String(_) => "string",
            Self::Array(_) => "array",
            Self::Object(_) => "object",
        }
    }

    /// True for [`JsonValue::Null`].
    #[must_use]
    pub fn is_null(&self) -> bool {
        matches!(self, Self::Null)
    }

    /// True for [`JsonValue::Bool`].
    #[must_use]
    pub fn is_bool(&self) -> bool {
        matches!(self, Self::Bool(_))
    }

    /// True for [`JsonValue::Number`].
    #[must_use]
    pub fn is_number(&self) -> bool {
        matches!(self, Self::Number(_))
    }

    /// True for [`JsonValue::String`].
    #[must_use]
    pub fn is_string(&self) -> bool {
        matches!(self, Self::String(_))
    }

    /// True for [`JsonValue::Array`].
    #[must_use]
    pub fn is_array(&self) -> bool {
        matches!(self, Self::Array(_))
    }

    /// True for [`JsonValue::Object`].
    #[must_use]
    pub fn is_object(&self) -> bool {
        matches!(self, Self::Object(_))
    }

    /// Looks a key up in an object, returning the first match.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&Self> {
        match self {
            Self::Object(entries) => entries.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    /// Mutable variant of [`JsonValue::get`].
    pub fn get_mut(&mut self, key: &str) -> Option<&mut Self> {
        match self {
            Self::Object(entries) => entries.iter_mut().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    /// True when an object contains `key`. Arrays and scalars are always false.
    #[must_use]
    pub fn has(&self, key: &str) -> bool {
        self.get(key).is_some()
    }

    /// Indexes an array.
    #[must_use]
    pub fn index(&self, i: usize) -> Option<&Self> {
        match self {
            Self::Array(items) => items.get(i),
            _ => None,
        }
    }

    /// Number of elements in an array or object; 0 for scalars.
    #[must_use]
    pub fn len(&self) -> usize {
        match self {
            Self::Array(items) => items.len(),
            Self::Object(entries) => entries.len(),
            _ => 0,
        }
    }

    /// True when an array or object is empty. Scalars are always empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The boolean payload, or `None` for any other type.
    #[must_use]
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(b) => Some(*b),
            _ => None,
        }
    }

    /// The numeric payload as `f64`, or `None` for any other type.
    #[must_use]
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Self::Number(n) => Some(*n),
            _ => None,
        }
    }

    /// The numeric payload as `i64`, or `None` when it is not an integral value
    /// that fits.
    #[must_use]
    pub fn as_i64(&self) -> Option<i64> {
        let n = self.as_f64()?;
        if n.fract() != 0.0 || n < i64::MIN as f64 || n > i64::MAX as f64 {
            return None;
        }
        Some(n as i64)
    }

    /// The numeric payload as `i32`, or `None` when it does not fit.
    #[must_use]
    pub fn as_i32(&self) -> Option<i32> {
        i32::try_from(self.as_i64()?).ok()
    }

    /// The numeric payload as `u32`, or `None` when it is negative, fractional
    /// or too large.
    #[must_use]
    pub fn as_u32(&self) -> Option<u32> {
        u32::try_from(self.as_i64()?).ok()
    }

    /// The numeric payload as `usize`, or `None` when it does not fit.
    #[must_use]
    pub fn as_usize(&self) -> Option<usize> {
        usize::try_from(self.as_i64()?).ok()
    }

    /// The numeric payload as `f32`, or `None` for any other type. Values
    /// outside the `f32` range become infinities, exactly like an `as` cast.
    #[must_use]
    pub fn as_f32(&self) -> Option<f32> {
        self.as_f64().map(|n| n as f32)
    }

    /// The string payload, or `None` for any other type.
    #[must_use]
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(s) => Some(s),
            _ => None,
        }
    }

    /// The array payload, or `None` for any other type.
    #[must_use]
    pub fn as_array(&self) -> Option<&Vec<JsonValue>> {
        match self {
            Self::Array(items) => Some(items),
            _ => None,
        }
    }

    /// The object payload as ordered pairs, or `None` for any other type.
    #[must_use]
    pub fn as_object(&self) -> Option<&Vec<(String, JsonValue)>> {
        match self {
            Self::Object(entries) => Some(entries),
            _ => None,
        }
    }

    /// Numeric field with a fallback: a missing key, a wrong type or a
    /// non-finite value all yield `default`.
    #[must_use]
    pub fn get_f32(&self, key: &str, default: f32) -> f32 {
        self.get(key).and_then(Self::as_f32).unwrap_or(default)
    }

    /// Numeric field with a fallback, as `f64`.
    #[must_use]
    pub fn get_f64(&self, key: &str, default: f64) -> f64 {
        self.get(key).and_then(Self::as_f64).unwrap_or(default)
    }

    /// Integer field with a fallback.
    #[must_use]
    pub fn get_i32(&self, key: &str, default: i32) -> i32 {
        self.get(key).and_then(Self::as_i32).unwrap_or(default)
    }

    /// Unsigned integer field with a fallback.
    #[must_use]
    pub fn get_u32(&self, key: &str, default: u32) -> u32 {
        self.get(key).and_then(Self::as_u32).unwrap_or(default)
    }

    /// Boolean field with a fallback.
    #[must_use]
    pub fn get_bool(&self, key: &str, default: bool) -> bool {
        self.get(key).and_then(Self::as_bool).unwrap_or(default)
    }

    /// String field: `None` when the key is missing or is not a string.
    #[must_use]
    pub fn get_str(&self, key: &str) -> Option<&str> {
        self.get(key).and_then(Self::as_str)
    }

    /// Array field: `None` when the key is missing or is not an array.
    #[must_use]
    pub fn get_array(&self, key: &str) -> Option<&Vec<JsonValue>> {
        self.get(key).and_then(Self::as_array)
    }

    /// Serialises to compact JSON with no whitespace.
    ///
    /// The inherent method deliberately shadows `ToString::to_string` so the
    /// compact form is what `value.to_string()` produces for a `JsonValue`.
    #[allow(clippy::inherent_to_string, clippy::inherent_to_string_shadow_display)]
    #[must_use]
    pub fn to_string(&self) -> String {
        let mut out = String::new();
        self.write_compact(&mut out);
        out
    }

    /// Serialises to indented JSON using two spaces per level.
    #[must_use]
    pub fn to_string_pretty(&self) -> String {
        let mut out = String::new();
        self.write_pretty(&mut out, 0);
        out
    }

    /// Appends the compact representation to `out`.
    pub fn write_compact(&self, out: &mut String) {
        match self {
            Self::Null => out.push_str("null"),
            Self::Bool(true) => out.push_str("true"),
            Self::Bool(false) => out.push_str("false"),
            Self::Number(n) => write_number(*n, out),
            Self::String(s) => write_string(s, out),
            Self::Array(items) => {
                out.push('[');
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    item.write_compact(out);
                }
                out.push(']');
            }
            Self::Object(entries) => {
                out.push('{');
                for (i, (key, value)) in entries.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    write_string(key, out);
                    out.push(':');
                    value.write_compact(out);
                }
                out.push('}');
            }
        }
    }

    /// Appends the indented representation to `out`.
    pub fn write_pretty(&self, out: &mut String, depth: usize) {
        let pad = |out: &mut String, n: usize| {
            for _ in 0..n {
                out.push_str("  ");
            }
        };
        match self {
            Self::Array(items) if !items.is_empty() => {
                out.push_str("[\n");
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push_str(",\n");
                    }
                    pad(out, depth + 1);
                    item.write_pretty(out, depth + 1);
                }
                out.push('\n');
                pad(out, depth);
                out.push(']');
            }
            Self::Object(entries) if !entries.is_empty() => {
                out.push_str("{\n");
                for (i, (key, value)) in entries.iter().enumerate() {
                    if i > 0 {
                        out.push_str(",\n");
                    }
                    pad(out, depth + 1);
                    write_string(key, out);
                    out.push_str(": ");
                    value.write_pretty(out, depth + 1);
                }
                out.push('\n');
                pad(out, depth);
                out.push('}');
            }
            other => other.write_compact(out),
        }
    }
}

impl fmt::Display for JsonValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_string())
    }
}

fn write_number(n: f64, out: &mut String) {
    if !n.is_finite() {
        // JSON has no NaN/Infinity literal. `null` is the only valid encoding;
        // use `JsonValue::Null` explicitly when that is what you mean.
        out.push_str("null");
        return;
    }
    if n == 0.0 {
        // Normalises `-0.0` to `0`; the two compare equal after a round trip.
        out.push('0');
        return;
    }
    out.push_str(&format!("{n}"));
}

fn write_string(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0C}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04X}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

/// A JSON syntax error, with the position that caused it.
///
/// `line` and `column` are 1-based. The column counts bytes within the line, so
/// a file whose first line is `héllo` reports column 3 for the `l`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JsonError {
    /// Human-readable description of the problem.
    pub message: String,
    /// 1-based line number.
    pub line: usize,
    /// 1-based byte column within the line.
    pub column: usize,
}

impl JsonError {
    /// Builds an error at an explicit line/column.
    pub fn new(message: impl Into<String>, line: usize, column: usize) -> Self {
        Self {
            message: message.into(),
            line,
            column,
        }
    }

    /// Builds an error from a byte offset into `bytes`.
    #[must_use]
    pub fn at_offset(bytes: &[u8], offset: usize, message: impl Into<String>) -> Self {
        let end = offset.min(bytes.len());
        let mut line = 1usize;
        let mut column = 1usize;
        for &b in &bytes[..end] {
            if b == b'\n' {
                line += 1;
                column = 1;
            } else {
                column += 1;
            }
        }
        Self::new(message, line, column)
    }
}

impl fmt::Display for JsonError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "line {} column {}: {}",
            self.line, self.column, self.message
        )
    }
}

impl std::error::Error for JsonError {}

/// Parses a complete JSON document from UTF-8 text.
///
/// Trailing whitespace is allowed; trailing anything else is an error.
pub fn parse_str(s: &str) -> Result<JsonValue, JsonError> {
    parse(s.as_bytes())
}

/// Parses a complete JSON document from bytes.
///
/// The bytes must be valid UTF-8. A byte-order mark is skipped when present.
pub fn parse(bytes: &[u8]) -> Result<JsonValue, JsonError> {
    let bytes = if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        &bytes[3..]
    } else {
        bytes
    };
    let mut p = Parser { bytes, pos: 0 };
    p.skip_ws();
    let value = p.parse_value(0)?;
    p.skip_ws();
    if p.pos != p.bytes.len() {
        return Err(p.err_at(p.pos, "trailing characters after the top-level value"));
    }
    Ok(value)
}

struct Parser<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl Parser<'_> {
    fn err_at(&self, pos: usize, message: impl Into<String>) -> JsonError {
        JsonError::at_offset(self.bytes, pos, message)
    }

    fn err(&self, message: impl Into<String>) -> JsonError {
        self.err_at(self.pos, message)
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn bump(&mut self) -> Option<u8> {
        let b = self.peek();
        if b.is_some() {
            self.pos += 1;
        }
        b
    }

    fn skip_ws(&mut self) {
        while let Some(b) = self.peek() {
            if matches!(b, b' ' | b'\t' | b'\n' | b'\r') {
                self.pos += 1;
            } else {
                break;
            }
        }
    }

    fn expect(&mut self, expected: u8) -> Result<(), JsonError> {
        let at = self.pos;
        match self.bump() {
            Some(c) if c == expected => Ok(()),
            Some(c) => Err(self.err_at(
                at,
                format!("expected '{}', found '{}'", expected as char, c as char),
            )),
            None => Err(self.err_at(
                at,
                format!("expected '{}', found end of input", expected as char),
            )),
        }
    }

    fn literal(&mut self, word: &[u8]) -> Result<(), JsonError> {
        let at = self.pos;
        for &expected in word {
            match self.bump() {
                Some(c) if c == expected => {}
                Some(c) => {
                    return Err(self.err_at(
                        at,
                        format!(
                            "expected `{}`, found a value starting with `{}`",
                            String::from_utf8_lossy(word),
                            c as char
                        ),
                    ));
                }
                None => {
                    return Err(self.err_at(
                        at,
                        format!(
                            "expected `{}`, found end of input",
                            String::from_utf8_lossy(word)
                        ),
                    ));
                }
            }
        }
        Ok(())
    }

    fn parse_value(&mut self, depth: usize) -> Result<JsonValue, JsonError> {
        if depth > MAX_DEPTH {
            return Err(self.err(format!("nesting deeper than {MAX_DEPTH} levels")));
        }
        self.skip_ws();
        match self.peek() {
            None => Err(self.err("unexpected end of input, expected a value")),
            Some(b'{') => self.parse_object(depth),
            Some(b'[') => self.parse_array(depth),
            Some(b'"') => Ok(JsonValue::String(self.parse_string()?)),
            Some(b't') => {
                self.literal(b"true")?;
                Ok(JsonValue::Bool(true))
            }
            Some(b'f') => {
                self.literal(b"false")?;
                Ok(JsonValue::Bool(false))
            }
            Some(b'n') => {
                self.literal(b"null")?;
                Ok(JsonValue::Null)
            }
            Some(b'-' | b'0'..=b'9') => self.parse_number(),
            Some(c) => Err(self.err(format!("unexpected character '{}'", c as char))),
        }
    }

    fn parse_array(&mut self, depth: usize) -> Result<JsonValue, JsonError> {
        self.expect(b'[')?;
        let mut items = Vec::new();
        self.skip_ws();
        if self.peek() == Some(b']') {
            self.pos += 1;
            return Ok(JsonValue::Array(items));
        }
        loop {
            let value = self.parse_value(depth + 1)?;
            items.push(value);
            self.skip_ws();
            match self.bump() {
                Some(b',') => {}
                Some(b']') => return Ok(JsonValue::Array(items)),
                Some(c) => {
                    return Err(self.err_at(
                        self.pos - 1,
                        format!("expected ',' or ']', found '{}'", c as char),
                    ));
                }
                None => return Err(self.err("unterminated array")),
            }
        }
    }

    fn parse_object(&mut self, depth: usize) -> Result<JsonValue, JsonError> {
        self.expect(b'{')?;
        let mut entries: Vec<(String, JsonValue)> = Vec::new();
        self.skip_ws();
        if self.peek() == Some(b'}') {
            self.pos += 1;
            return Ok(JsonValue::Object(entries));
        }
        loop {
            self.skip_ws();
            if self.peek() != Some(b'"') {
                return Err(self.err("expected a double-quoted object key"));
            }
            let key = self.parse_string()?;
            self.skip_ws();
            self.expect(b':')?;
            let value = self.parse_value(depth + 1)?;
            entries.push((key, value));
            self.skip_ws();
            match self.bump() {
                Some(b',') => {}
                Some(b'}') => return Ok(JsonValue::Object(entries)),
                Some(c) => {
                    return Err(self.err_at(
                        self.pos - 1,
                        format!("expected ',' or '}}', found '{}'", c as char),
                    ));
                }
                None => return Err(self.err("unterminated object")),
            }
        }
    }

    fn parse_number(&mut self) -> Result<JsonValue, JsonError> {
        let start = self.pos;
        if self.peek() == Some(b'-') {
            self.pos += 1;
        }
        match self.peek() {
            Some(b'0') => self.pos += 1,
            Some(b'1'..=b'9') => {
                while matches!(self.peek(), Some(b'0'..=b'9')) {
                    self.pos += 1;
                }
            }
            _ => return Err(self.err_at(start, "expected a digit")),
        }
        if self.peek() == Some(b'.') {
            self.pos += 1;
            if !matches!(self.peek(), Some(b'0'..=b'9')) {
                return Err(self.err("expected a digit after the decimal point"));
            }
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.pos += 1;
            }
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            self.pos += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.pos += 1;
            }
            if !matches!(self.peek(), Some(b'0'..=b'9')) {
                return Err(self.err("expected a digit in the exponent"));
            }
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.pos += 1;
            }
        }
        let text = std::str::from_utf8(&self.bytes[start..self.pos])
            .map_err(|_| self.err_at(start, "invalid UTF-8 in number"))?;
        let value: f64 = text
            .parse()
            .map_err(|_| self.err_at(start, format!("`{text}` is not a valid number")))?;
        if !value.is_finite() {
            return Err(self.err_at(
                start,
                format!("`{text}` overflows the range of a JSON number"),
            ));
        }
        Ok(JsonValue::Number(value))
    }

    fn parse_string(&mut self) -> Result<String, JsonError> {
        let open = self.pos;
        self.expect(b'"')?;
        let mut buf: Vec<u8> = Vec::new();
        loop {
            let at = self.pos;
            let b = self
                .bump()
                .ok_or_else(|| self.err_at(open, "unterminated string"))?;
            match b {
                b'"' => break,
                0x00..=0x1F => {
                    return Err(self.err_at(at, "raw control character in string; escape it"));
                }
                b'\\' => {
                    let esc_at = self.pos;
                    let esc = self
                        .bump()
                        .ok_or_else(|| self.err_at(open, "unterminated escape"))?;
                    match esc {
                        b'"' => buf.push(b'"'),
                        b'\\' => buf.push(b'\\'),
                        b'/' => buf.push(b'/'),
                        b'b' => buf.push(0x08),
                        b'f' => buf.push(0x0C),
                        b'n' => buf.push(b'\n'),
                        b'r' => buf.push(b'\r'),
                        b't' => buf.push(b'\t'),
                        b'u' => {
                            let cp = self.parse_unicode_escape(esc_at)?;
                            let mut tmp = [0u8; 4];
                            buf.extend_from_slice(cp.encode_utf8(&mut tmp).as_bytes());
                        }
                        c => {
                            return Err(
                                self.err_at(esc_at, format!("unknown escape `\\{}`", c as char))
                            );
                        }
                    }
                }
                other => buf.push(other),
            }
        }
        String::from_utf8(buf).map_err(|e| {
            let bad = open + 1 + e.utf8_error().valid_up_to();
            self.err_at(bad, "invalid UTF-8 in string")
        })
    }

    fn parse_hex4(&mut self) -> Result<u32, JsonError> {
        let at = self.pos;
        if self.pos + 4 > self.bytes.len() {
            return Err(self.err_at(at, "truncated \\u escape"));
        }
        let mut value = 0u32;
        for _ in 0..4 {
            let c = self.bump().expect("length checked above");
            let digit = match c {
                b'0'..=b'9' => u32::from(c - b'0'),
                b'a'..=b'f' => u32::from(c - b'a') + 10,
                b'A'..=b'F' => u32::from(c - b'A') + 10,
                _ => return Err(self.err_at(at, "\\u escape must be followed by four hex digits")),
            };
            value = value * 16 + digit;
        }
        Ok(value)
    }

    fn parse_unicode_escape(&mut self, esc_at: usize) -> Result<char, JsonError> {
        let first = self.parse_hex4()?;
        let cp = if (0xD800..=0xDBFF).contains(&first) {
            // High surrogate: a low surrogate must follow immediately.
            if self.peek() == Some(b'\\') && self.bytes.get(self.pos + 1) == Some(&b'u') {
                self.pos += 2;
                let second = self.parse_hex4()?;
                if !(0xDC00..=0xDFFF).contains(&second) {
                    return Err(
                        self.err_at(esc_at, "high surrogate must be followed by a low surrogate")
                    );
                }
                0x1_0000 + ((first - 0xD800) << 10) + (second - 0xDC00)
            } else {
                return Err(self.err_at(esc_at, "unpaired high surrogate in \\u escape"));
            }
        } else if (0xDC00..=0xDFFF).contains(&first) {
            return Err(self.err_at(esc_at, "unpaired low surrogate in \\u escape"));
        } else {
            first
        };
        char::from_u32(cp)
            .ok_or_else(|| self.err_at(esc_at, "\\u escape is not a Unicode scalar value"))
    }
}

impl From<bool> for JsonValue {
    fn from(v: bool) -> Self {
        Self::Bool(v)
    }
}

impl From<f32> for JsonValue {
    fn from(v: f32) -> Self {
        Self::Number(f64::from(v))
    }
}

impl From<f64> for JsonValue {
    fn from(v: f64) -> Self {
        Self::Number(v)
    }
}

impl From<i32> for JsonValue {
    fn from(v: i32) -> Self {
        Self::Number(f64::from(v))
    }
}

impl From<u32> for JsonValue {
    fn from(v: u32) -> Self {
        Self::Number(f64::from(v))
    }
}

impl From<i64> for JsonValue {
    fn from(v: i64) -> Self {
        Self::Number(v as f64)
    }
}

impl From<u64> for JsonValue {
    fn from(v: u64) -> Self {
        Self::Number(v as f64)
    }
}

impl From<usize> for JsonValue {
    fn from(v: usize) -> Self {
        Self::Number(v as f64)
    }
}

impl From<&str> for JsonValue {
    fn from(v: &str) -> Self {
        Self::String(v.to_string())
    }
}

impl From<String> for JsonValue {
    fn from(v: String) -> Self {
        Self::String(v)
    }
}

impl From<&String> for JsonValue {
    fn from(v: &String) -> Self {
        Self::String(v.clone())
    }
}

impl From<Vec<JsonValue>> for JsonValue {
    fn from(v: Vec<JsonValue>) -> Self {
        Self::Array(v)
    }
}

impl From<Vec<(String, JsonValue)>> for JsonValue {
    fn from(v: Vec<(String, JsonValue)>) -> Self {
        Self::Object(v)
    }
}

impl From<Vec<String>> for JsonValue {
    fn from(v: Vec<String>) -> Self {
        Self::Array(v.into_iter().map(Self::String).collect())
    }
}

impl From<Vec<&str>> for JsonValue {
    fn from(v: Vec<&str>) -> Self {
        Self::Array(v.into_iter().map(|s| Self::String(s.to_string())).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_primitives() {
        assert_eq!(parse_str("null").unwrap(), JsonValue::Null);
        assert_eq!(parse_str(" true ").unwrap(), JsonValue::Bool(true));
        assert_eq!(parse_str("false").unwrap(), JsonValue::Bool(false));
        assert_eq!(parse_str("42").unwrap(), JsonValue::Number(42.0));
        assert_eq!(parse_str("\"hi\"").unwrap(), JsonValue::String("hi".into()));
        assert_eq!(parse_str("[]").unwrap(), JsonValue::Array(vec![]));
        assert_eq!(parse_str("{}").unwrap(), JsonValue::Object(vec![]));
        assert!(parse_str("").unwrap_err().message.contains("end of input"));
    }

    #[test]
    fn parses_numbers_with_signs_fractions_and_exponents() {
        assert_eq!(parse_str("-0.5").unwrap().as_f64(), Some(-0.5));
        assert_eq!(parse_str("1e3").unwrap().as_f64(), Some(1000.0));
        assert_eq!(parse_str("1E+3").unwrap().as_f64(), Some(1000.0));
        assert_eq!(parse_str("2.5e-2").unwrap().as_f64(), Some(0.025));
        assert_eq!(parse_str("-12").unwrap().as_i64(), Some(-12));
        assert_eq!(parse_str("7").unwrap().as_u32(), Some(7));
        assert_eq!(parse_str("-7").unwrap().as_u32(), None);
        assert_eq!(parse_str("1.5").unwrap().as_i64(), None);
        assert_eq!(parse_str("1.5").unwrap().as_f32(), Some(1.5));
        assert!(parse_str("01").is_err());
        assert!(parse_str("1e").is_err());
        assert!(parse_str("-").is_err());
        assert!(
            parse_str("1e999").is_err(),
            "overflow must be an error, not inf"
        );
    }

    #[test]
    fn object_keys_keep_insertion_order() {
        let v = parse_str(r#"{"z":1,"a":2,"m":3,"a":4}"#).unwrap();
        let keys: Vec<&str> = v
            .as_object()
            .unwrap()
            .iter()
            .map(|(k, _)| k.as_str())
            .collect();
        assert_eq!(keys, ["z", "a", "m", "a"]);
        // Duplicate keys are preserved, and lookup finds the first.
        assert_eq!(v.get("a").unwrap().as_i64(), Some(2));
        assert_eq!(v.as_object().unwrap().len(), 4);
    }

    #[test]
    fn nested_structures_and_accessors() {
        let v = parse_str(r#"{"a":{"b":[1,{"c":true}]}}"#).unwrap();
        assert!(v.is_object());
        assert_eq!(
            v.get("a")
                .unwrap()
                .get("b")
                .unwrap()
                .index(0)
                .unwrap()
                .as_i64(),
            Some(1)
        );
        let c = v
            .get("a")
            .unwrap()
            .get("b")
            .unwrap()
            .index(1)
            .unwrap()
            .get("c")
            .unwrap();
        assert_eq!(c.as_bool(), Some(true));
        assert_eq!(v.index(0), None, "indexing an object is not an array index");
        assert_eq!(v.get("nope"), None);
        assert!(v.get("a").unwrap().get("b").unwrap().index(9).is_none());
    }

    #[test]
    fn escapes_of_every_kind() {
        let v = parse_str(r#""a\"b\\c\/d\be\ff\ng\rh\ti""#).unwrap();
        assert_eq!(v.as_str(), Some("a\"b\\c/d\u{8}e\u{c}f\ng\rh\ti"));
        assert_eq!(parse_str(r#""\u0041""#).unwrap().as_str(), Some("A"));
        assert_eq!(parse_str("\"\u{00e9}\"").unwrap().as_str(), Some("é"));
        assert!(parse_str(r#""\q""#).is_err());
        assert!(parse_str("\"raw\nnewline\"").is_err());
    }

    #[test]
    fn escapes_unicode_surrogate_pairs() {
        let v = parse_str(r#""\uD83D\uDE00""#).unwrap();
        assert_eq!(v.as_str(), Some("😀"));
        assert_eq!(v.as_str().unwrap().chars().count(), 1);
        assert!(parse_str(r#""\uD83D""#).is_err(), "unpaired high surrogate");
        assert!(parse_str(r#""\uDE00""#).is_err(), "unpaired low surrogate");
        assert!(parse_str(r#""\uD83Dx""#).is_err());
        assert!(parse_str(r#""\uZZZZ""#).is_err());
    }

    #[test]
    fn errors_carry_line_and_column() {
        let err = parse_str("{\n  \"a\": 1,\n  \"b\" 2\n}").unwrap_err();
        assert_eq!(err.line, 3);
        assert_eq!(err.column, 7);
        assert!(err.to_string().starts_with("line 3 column 7: "));

        let err = parse_str("[1, 2,\n3,]").unwrap_err();
        assert_eq!(err.line, 2);
        assert!(err.message.contains("expected a value") || err.message.contains("unexpected"));

        let err = parse_str("{\n\"a\": tru\n}").unwrap_err();
        assert_eq!(err.line, 2);
        assert!(err.message.contains("true"), "{}", err.message);
    }

    #[test]
    fn unterminated_strings_and_containers_are_errors() {
        let e = parse_str("\"abc").unwrap_err();
        assert!(e.message.contains("unterminated string"));
        assert_eq!((e.line, e.column), (1, 1));
        assert!(
            parse_str("[1, 2")
                .unwrap_err()
                .message
                .contains("unterminated array")
        );
        assert!(
            parse_str("{\"a\": 1")
                .unwrap_err()
                .message
                .contains("unterminated object")
        );
        assert!(
            parse_str("{\"a\" 1}")
                .unwrap_err()
                .message
                .contains("expected ':'")
        );
        assert!(
            parse_str("{a: 1}")
                .unwrap_err()
                .message
                .contains("object key")
        );
    }

    #[test]
    fn trailing_characters_are_rejected() {
        let e = parse_str("{} garbage").unwrap_err();
        assert!(e.message.contains("trailing characters"));
        assert!(parse_str("1 2").is_err());
    }

    #[test]
    fn deep_nesting_is_supported_up_to_the_limit() {
        let depth = 100;
        let text = format!("{}{}", "[".repeat(depth), "]".repeat(depth));
        let v = parse_str(&text).unwrap();
        let mut cur = &v;
        for _ in 0..depth - 1 {
            cur = cur.index(0).unwrap();
        }
        assert_eq!(cur, &JsonValue::Array(vec![]));

        let too_deep = format!("{}{}", "[".repeat(MAX_DEPTH + 5), "]".repeat(MAX_DEPTH + 5));
        let err = parse_str(&too_deep).unwrap_err();
        assert!(err.message.contains("nesting deeper"), "{}", err.message);
    }

    #[test]
    fn compact_round_trip_is_stable() {
        let text = r#"{"name":"grass","uv":[0,0,16,16],"flags":{"walkable":true,"water":false},"height":0,"tags":["a","b"],"nested":{"deep":[1,2,{"x":null}]}}"#;
        let v = parse_str(text).unwrap();
        let once = v.to_string();
        let twice = parse_str(&once).unwrap().to_string();
        assert_eq!(once, twice);
        assert_eq!(parse_str(&once).unwrap(), v);
        assert_eq!(once, text.replace(' ', ""));
        assert_eq!(format!("{v}"), once);
    }

    #[test]
    fn pretty_printing_is_indented_and_stable() {
        let v = parse_str(r#"{"a":[1,2],"b":{"c":true},"d":[],"e":{}}"#).unwrap();
        let pretty = v.to_string_pretty();
        assert!(
            pretty.contains("\n  \"a\": [\n    1,\n    2\n  ]"),
            "{pretty}"
        );
        assert!(pretty.contains("\"d\": []"));
        assert!(pretty.contains("\"e\": {}"));
        assert_eq!(parse_str(&pretty).unwrap(), v);
        assert_eq!(v.to_string_pretty(), pretty);
    }

    #[test]
    fn string_writer_escapes_correctly() {
        let v = JsonValue::String("a\"b\\c\n\t\r\u{1}\u{7f}é😀".into());
        let text = v.to_string();
        assert_eq!(text, "\"a\\\"b\\\\c\\n\\t\\r\\u0001\u{7f}é😀\"");
        assert_eq!(parse_str(&text).unwrap(), v);
    }

    #[test]
    fn non_finite_numbers_write_as_null() {
        assert_eq!(JsonValue::Number(f64::NAN).to_string(), "null");
        assert_eq!(JsonValue::Number(f64::INFINITY).to_string(), "null");
        assert_eq!(JsonValue::Number(-0.0).to_string(), "0");
        assert_eq!(JsonValue::Number(1.0).to_string(), "1");
        assert_eq!(JsonValue::Number(0.25).to_string(), "0.25");
        assert_eq!(
            parse_str(&JsonValue::Number(1.0e300).to_string())
                .unwrap()
                .as_f64(),
            Some(1.0e300)
        );
    }

    #[test]
    fn typed_getters_fall_back_to_defaults() {
        let v =
            parse_str(r#"{"f":1.5,"i":7,"u":9,"b":true,"s":"x","a":[1,2],"bad":"no"}"#).unwrap();
        assert_eq!(v.get_f32("f", 0.0), 1.5);
        assert_eq!(v.get_f32("missing", 2.5), 2.5);
        assert_eq!(v.get_f32("bad", 3.0), 3.0);
        assert_eq!(v.get_i32("i", 0), 7);
        assert_eq!(v.get_u32("u", 0), 9);
        assert!(v.get_bool("b", false));
        assert!(!v.get_bool("missing", false));
        assert_eq!(v.get_str("s"), Some("x"));
        assert_eq!(v.get_str("missing"), None);
        assert_eq!(v.get_array("a").map(Vec::len), Some(2));
        assert_eq!(v.get_array("s").map(Vec::len), None);
        assert_eq!(v.type_name(), "object");
        assert!(v.get("missing").is_none());
        assert!(!v.is_null());
        assert!(JsonValue::Null.is_null());
    }

    #[test]
    fn builder_impls_and_helpers() {
        let v = JsonValue::object([
            ("name", JsonValue::from("hero")),
            ("hp", JsonValue::from(10u32)),
            ("scale", JsonValue::from(1.5f32)),
            ("alive", JsonValue::from(true)),
            ("tags", JsonValue::from(vec!["a", "b"])),
            ("nested", JsonValue::object([("x", JsonValue::from(1i32))])),
            (
                "list",
                JsonValue::array([JsonValue::Null, JsonValue::from(2)]),
            ),
        ]);
        assert_eq!(v.get_str("name"), Some("hero"));
        assert_eq!(v.get_u32("hp", 0), 10);
        assert_eq!(v.get_f32("scale", 0.0), 1.5);
        assert_eq!(v.get_array("tags").unwrap().len(), 2);
        assert_eq!(v.get("list").unwrap().index(1).unwrap().as_i64(), Some(2));
        assert_eq!(v.get("nested").unwrap().get_i32("x", 0), 1);
        assert!(!v.is_empty());
        assert!(JsonValue::array([JsonValue::from(1i32), JsonValue::from(2i32)]).is_array());
        assert_eq!(JsonValue::Number(3.0).as_usize(), Some(3));
        assert_eq!(JsonValue::String("s".into()).as_str(), Some("s"));
        assert_eq!(JsonValue::Number(1.0).as_str(), None);
        assert_eq!(JsonValue::Null.len(), 0);
    }

    #[test]
    fn mutable_access_and_utf8_errors() {
        let mut v = parse_str(r#"{"a":1}"#).unwrap();
        *v.get_mut("a").unwrap() = JsonValue::from(2);
        assert_eq!(v.get_u32("a", 0), 2);
        assert!(v.has("a"));
        assert!(!v.has("b"));
        assert!(v.get_mut("b").is_none());

        let bad = [b'"', 0xFF, 0xFE, b'"'];
        let err = parse(&bad).unwrap_err();
        assert!(err.message.contains("invalid UTF-8"), "{}", err.message);
    }
}
