// Tests prioritize: fast, deterministic, isolated, behavior-sensitive, structure-insensitive, specific, readable, writable, predictive, and inspiring.
//! Minimal JSON reader used only at the passive persistence boundary.
//!
//! Contract: parse one complete UTF-8 JSON value with bounded recursion inherited
//! from the small state file. It performs no I/O and has no extension hooks.

use std::collections::BTreeMap;
use std::fmt;

const MAX_NESTING_DEPTH: usize = 64;

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Value {
    Null,
    Bool(bool),
    Number(u64),
    String(String),
    Array(Vec<Value>),
    Object(BTreeMap<String, Value>),
}

pub(crate) fn parse(input: &str) -> Result<Value, Error> {
    let mut parser = Parser {
        bytes: input.as_bytes(),
        position: 0,
    };
    let value = parser.value(0)?;
    parser.whitespace();
    if parser.position != parser.bytes.len() {
        return Err(parser.error("trailing JSON data"));
    }
    Ok(value)
}

pub(crate) fn quote(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len() + 2);
    encoded.push('"');
    for character in value.chars() {
        match character {
            '"' => encoded.push_str("\\\""),
            '\\' => encoded.push_str("\\\\"),
            '\u{08}' => encoded.push_str("\\b"),
            '\u{0c}' => encoded.push_str("\\f"),
            '\n' => encoded.push_str("\\n"),
            '\r' => encoded.push_str("\\r"),
            '\t' => encoded.push_str("\\t"),
            value if value < '\u{20}' => {
                encoded.push_str(&format!("\\u{:04x}", value as u32));
            }
            value => encoded.push(value),
        }
    }
    encoded.push('"');
    encoded
}

struct Parser<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl Parser<'_> {
    fn value(&mut self, depth: usize) -> Result<Value, Error> {
        if depth > MAX_NESTING_DEPTH {
            return Err(self.error("JSON nesting exceeds limit"));
        }
        self.whitespace();
        match self.peek() {
            Some(b'n') => {
                self.keyword(b"null")?;
                Ok(Value::Null)
            }
            Some(b't') => {
                self.keyword(b"true")?;
                Ok(Value::Bool(true))
            }
            Some(b'f') => {
                self.keyword(b"false")?;
                Ok(Value::Bool(false))
            }
            Some(b'"') => self.string().map(Value::String),
            Some(b'[') => self.array(depth + 1),
            Some(b'{') => self.object(depth + 1),
            Some(b'0'..=b'9') => self.number().map(Value::Number),
            Some(_) => Err(self.error("unexpected JSON token")),
            None => Err(self.error("unexpected end of JSON")),
        }
    }

    fn array(&mut self, depth: usize) -> Result<Value, Error> {
        self.expect(b'[')?;
        self.whitespace();
        let mut values = Vec::new();
        if self.consume(b']') {
            return Ok(Value::Array(values));
        }
        loop {
            values.push(self.value(depth)?);
            self.whitespace();
            if self.consume(b']') {
                return Ok(Value::Array(values));
            }
            self.expect(b',')?;
        }
    }

    fn object(&mut self, depth: usize) -> Result<Value, Error> {
        self.expect(b'{')?;
        self.whitespace();
        let mut values = BTreeMap::new();
        if self.consume(b'}') {
            return Ok(Value::Object(values));
        }
        loop {
            self.whitespace();
            let key = self.string()?;
            self.whitespace();
            self.expect(b':')?;
            let value = self.value(depth)?;
            if values.insert(key, value).is_some() {
                return Err(self.error("duplicate JSON object key"));
            }
            self.whitespace();
            if self.consume(b'}') {
                return Ok(Value::Object(values));
            }
            self.expect(b',')?;
        }
    }

    fn string(&mut self) -> Result<String, Error> {
        self.expect(b'"')?;
        let mut decoded = String::new();
        let mut plain_start = self.position;
        while let Some(byte) = self.peek() {
            match byte {
                b'"' => {
                    self.push_plain(&mut decoded, plain_start, self.position)?;
                    self.position += 1;
                    return Ok(decoded);
                }
                b'\\' => {
                    self.push_plain(&mut decoded, plain_start, self.position)?;
                    self.position += 1;
                    decoded.push(self.escape()?);
                    plain_start = self.position;
                }
                0x00..=0x1f => return Err(self.error("control byte in JSON string")),
                _ => self.position += 1,
            }
        }
        Err(self.error("unterminated JSON string"))
    }

    fn escape(&mut self) -> Result<char, Error> {
        let escaped = self
            .next()
            .ok_or_else(|| self.error("unterminated JSON escape"))?;
        match escaped {
            b'"' => Ok('"'),
            b'\\' => Ok('\\'),
            b'/' => Ok('/'),
            b'b' => Ok('\u{08}'),
            b'f' => Ok('\u{0c}'),
            b'n' => Ok('\n'),
            b'r' => Ok('\r'),
            b't' => Ok('\t'),
            b'u' => {
                let codepoint = self.hex_quad()?;
                char::from_u32(codepoint).ok_or_else(|| self.error("invalid JSON Unicode escape"))
            }
            _ => Err(self.error("invalid JSON escape")),
        }
    }

    fn hex_quad(&mut self) -> Result<u32, Error> {
        let mut value = 0_u32;
        for _ in 0..4 {
            let digit = self
                .next()
                .ok_or_else(|| self.error("short JSON Unicode escape"))?;
            value = value * 16
                + match digit {
                    b'0'..=b'9' => u32::from(digit - b'0'),
                    b'a'..=b'f' => u32::from(digit - b'a' + 10),
                    b'A'..=b'F' => u32::from(digit - b'A' + 10),
                    _ => return Err(self.error("invalid JSON Unicode escape")),
                };
        }
        Ok(value)
    }

    fn push_plain(&self, output: &mut String, start: usize, end: usize) -> Result<(), Error> {
        let plain = std::str::from_utf8(&self.bytes[start..end])
            .map_err(|_| self.error("invalid UTF-8 in JSON string"))?;
        output.push_str(plain);
        Ok(())
    }

    fn number(&mut self) -> Result<u64, Error> {
        let start = self.position;
        if self.consume(b'0') {
            if matches!(self.peek(), Some(b'0'..=b'9')) {
                return Err(self.error("leading zero in JSON number"));
            }
        } else {
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.position += 1;
            }
        }
        let text = std::str::from_utf8(&self.bytes[start..self.position])
            .map_err(|_| self.error("invalid JSON number"))?;
        text.parse::<u64>()
            .map_err(|_| self.error("JSON number out of range"))
    }

    fn keyword(&mut self, expected: &[u8]) -> Result<(), Error> {
        if self
            .bytes
            .get(self.position..self.position + expected.len())
            == Some(expected)
        {
            self.position += expected.len();
            Ok(())
        } else {
            Err(self.error("invalid JSON keyword"))
        }
    }

    fn whitespace(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\n' | b'\r' | b'\t')) {
            self.position += 1;
        }
    }

    fn expect(&mut self, expected: u8) -> Result<(), Error> {
        self.whitespace();
        if self.consume(expected) {
            Ok(())
        } else {
            Err(self.error("unexpected JSON punctuation"))
        }
    }

    fn consume(&mut self, expected: u8) -> bool {
        if self.peek() == Some(expected) {
            self.position += 1;
            true
        } else {
            false
        }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.position).copied()
    }

    fn next(&mut self) -> Option<u8> {
        let next = self.peek()?;
        self.position += 1;
        Some(next)
    }

    fn error(&self, message: impl Into<String>) -> Error {
        Error {
            message: message.into(),
            position: self.position,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Error {
    message: String,
    position: usize,
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} at byte {}", self.message, self.position)
    }
}

impl std::error::Error for Error {}

#[cfg(test)]
mod tests {
    use super::{parse, quote, MAX_NESTING_DEPTH};

    #[test]
    fn malformed_json_corpus_fails_closed() {
        for malformed in [
            "",
            "[",
            "{",
            "[1,]",
            "{\"a\":1,}",
            "{\"a\":1,\"a\":2}",
            "\"unterminated",
            "\"bad\\xescape\"",
            "\"bad\\u12x4\"",
            "01",
            "-1",
            "1.5",
            "null trailing",
            "\u{0000}",
        ] {
            assert!(
                parse(malformed).is_err(),
                "accepted malformed JSON: {malformed:?}"
            );
        }
    }

    #[test]
    fn nesting_bomb_stops_at_the_explicit_depth_limit() {
        let accepted = format!(
            "{}null{}",
            "[".repeat(MAX_NESTING_DEPTH),
            "]".repeat(MAX_NESTING_DEPTH)
        );
        let bomb = format!(
            "{}null{}",
            "[".repeat(MAX_NESTING_DEPTH + 2),
            "]".repeat(MAX_NESTING_DEPTH + 2)
        );

        assert!(
            parse(&accepted).is_ok(),
            "documented nesting boundary must parse"
        );
        assert!(parse(&bomb)
            .expect_err("depth bomb must fail")
            .to_string()
            .contains("nesting exceeds limit"));
    }

    #[test]
    fn every_truncation_of_a_valid_document_is_rejected() {
        let document = "{\"state\":[null,true,false,18446744073709551615,\"tail Ω\"]}";
        assert!(parse(document).is_ok());
        for end in 0..document.len() {
            if document.is_char_boundary(end) {
                assert!(
                    parse(&document[..end]).is_err(),
                    "accepted truncated prefix ending at byte {end}"
                );
            }
        }
    }

    #[test]
    fn quoted_strings_round_trip_deterministic_edge_characters() {
        for value in [
            "",
            "plain",
            "quotes \\\"",
            "line\nfeed",
            "nul \u{0000}",
            "Ω🦀",
        ] {
            let parsed = parse(&quote(value)).expect("quoted string parses");
            assert_eq!(parsed, super::Value::String(value.to_owned()));
        }
    }
}
