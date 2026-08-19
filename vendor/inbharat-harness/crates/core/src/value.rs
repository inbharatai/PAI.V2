//! Small JSON-compatible canonical value used at persistence and tool boundaries.

use std::collections::BTreeMap;

/// Dependency-free JSON-compatible value with deterministic object ordering.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum Value {
    Null,
    Bool(bool),
    Integer(i64),
    String(String),
    Array(Vec<Value>),
    Object(BTreeMap<String, Value>),
}

impl Value {
    /// Serializes to deterministic compact JSON.
    #[must_use]
    pub fn to_canonical_json(&self) -> String {
        let mut output = String::new();
        self.write_json(&mut output);
        output
    }

    /// Returns a string view for string values.
    #[must_use]
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(value) => Some(value),
            _ => None,
        }
    }

    /// Returns an object view for object values.
    #[must_use]
    pub fn as_object(&self) -> Option<&BTreeMap<String, Value>> {
        match self {
            Self::Object(value) => Some(value),
            _ => None,
        }
    }

    /// Parses bounded JSON without accepting floats, duplicate keys, or trailing data.
    pub fn parse_json(input: &str) -> Result<Self, String> {
        if input.len() > 8 * 1024 * 1024 {
            return Err("JSON input exceeds limit".to_owned());
        }
        let mut parser = Parser::new(input);
        let value = parser.parse_value(0)?;
        parser.skip_whitespace();
        if parser.peek().is_some() {
            return Err("trailing JSON data".to_owned());
        }
        Ok(value)
    }

    fn write_json(&self, output: &mut String) {
        match self {
            Self::Null => output.push_str("null"),
            Self::Bool(value) => output.push_str(if *value { "true" } else { "false" }),
            Self::Integer(value) => output.push_str(&value.to_string()),
            Self::String(value) => write_quoted(value, output),
            Self::Array(values) => {
                output.push('[');
                for (index, value) in values.iter().enumerate() {
                    if index > 0 {
                        output.push(',');
                    }
                    value.write_json(output);
                }
                output.push(']');
            }
            Self::Object(values) => {
                output.push('{');
                for (index, (key, value)) in values.iter().enumerate() {
                    if index > 0 {
                        output.push(',');
                    }
                    write_quoted(key, output);
                    output.push(':');
                    value.write_json(output);
                }
                output.push('}');
            }
        }
    }
}

fn write_quoted(value: &str, output: &mut String) {
    output.push('"');
    for character in value.chars() {
        match character {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            character if character.is_control() => {
                output.push_str("\\u");
                output.push_str(&format!("{:04x}", u32::from(character)));
            }
            character => output.push(character),
        }
    }
    output.push('"');
}

struct Parser<'a> {
    source: &'a [u8],
    cursor: usize,
}

impl<'a> Parser<'a> {
    fn new(source: &'a str) -> Self {
        Self {
            source: source.as_bytes(),
            cursor: 0,
        }
    }

    fn parse_value(&mut self, depth: usize) -> Result<Value, String> {
        if depth > 128 {
            return Err("JSON nesting exceeds limit".to_owned());
        }
        self.skip_whitespace();
        match self.peek() {
            Some(b'n') => {
                self.literal(b"null")?;
                Ok(Value::Null)
            }
            Some(b't') => {
                self.literal(b"true")?;
                Ok(Value::Bool(true))
            }
            Some(b'f') => {
                self.literal(b"false")?;
                Ok(Value::Bool(false))
            }
            Some(b'\"') => self.parse_string().map(Value::String),
            Some(b'[') => self.parse_array(depth + 1),
            Some(b'{') => self.parse_object(depth + 1),
            Some(b'-' | b'0'..=b'9') => self.parse_integer().map(Value::Integer),
            _ => Err("invalid JSON value".to_owned()),
        }
    }

    fn parse_array(&mut self, depth: usize) -> Result<Value, String> {
        self.consume(b'[')?;
        let mut values = Vec::new();
        self.skip_whitespace();
        if self.take_if(b']') {
            return Ok(Value::Array(values));
        }
        loop {
            if values.len() >= 100_000 {
                return Err("JSON array exceeds item limit".to_owned());
            }
            values.push(self.parse_value(depth)?);
            self.skip_whitespace();
            if self.take_if(b']') {
                break;
            }
            self.consume(b',')?;
        }
        Ok(Value::Array(values))
    }

    fn parse_object(&mut self, depth: usize) -> Result<Value, String> {
        self.consume(b'{')?;
        let mut values = BTreeMap::new();
        self.skip_whitespace();
        if self.take_if(b'}') {
            return Ok(Value::Object(values));
        }
        loop {
            self.skip_whitespace();
            let key = self.parse_string()?;
            self.skip_whitespace();
            self.consume(b':')?;
            let value = self.parse_value(depth)?;
            if values.insert(key, value).is_some() {
                return Err("duplicate JSON object key".to_owned());
            }
            self.skip_whitespace();
            if self.take_if(b'}') {
                break;
            }
            self.consume(b',')?;
        }
        Ok(Value::Object(values))
    }

    fn parse_integer(&mut self) -> Result<i64, String> {
        let start = self.cursor;
        let _negative = self.take_if(b'-');
        match self.peek() {
            Some(b'0') => self.cursor += 1,
            Some(b'1'..=b'9') => {
                self.cursor += 1;
                while matches!(self.peek(), Some(b'0'..=b'9')) {
                    self.cursor += 1;
                }
            }
            _ => return Err("invalid JSON integer".to_owned()),
        }
        if matches!(self.peek(), Some(b'.' | b'e' | b'E')) {
            return Err("floating-point JSON values are unsupported".to_owned());
        }
        let bytes = &self.source[start..self.cursor];
        let text = std::str::from_utf8(bytes).map_err(|_| "invalid integer bytes".to_owned())?;
        text.parse::<i64>()
            .map_err(|_| "JSON integer is out of range".to_owned())
    }

    fn parse_string(&mut self) -> Result<String, String> {
        self.consume(b'\"')?;
        let mut output = String::new();
        while let Some(byte) = self.next() {
            match byte {
                b'\"' => return Ok(output),
                b'\\' => {
                    let escaped = self
                        .next()
                        .ok_or_else(|| "unterminated escape".to_owned())?;
                    match escaped {
                        b'\"' => output.push('\"'),
                        b'\\' => output.push('\\'),
                        b'/' => output.push('/'),
                        b'b' => output.push('\u{0008}'),
                        b'f' => output.push('\u{000c}'),
                        b'n' => output.push('\n'),
                        b'r' => output.push('\r'),
                        b't' => output.push('\t'),
                        b'u' => output.push(self.parse_unicode_escape()?),
                        _ => return Err("invalid string escape".to_owned()),
                    }
                }
                0..=31 => return Err("control byte in JSON string".to_owned()),
                32..=127 => output.push(char::from(byte)),
                _ => {
                    self.cursor = self.cursor.saturating_sub(1);
                    let remaining = std::str::from_utf8(&self.source[self.cursor..])
                        .map_err(|_| "invalid UTF-8 in JSON string".to_owned())?;
                    let character = remaining
                        .chars()
                        .next()
                        .ok_or_else(|| "unterminated JSON string".to_owned())?;
                    self.cursor += character.len_utf8();
                    output.push(character);
                }
            }
        }
        Err("unterminated JSON string".to_owned())
    }

    fn parse_unicode_escape(&mut self) -> Result<char, String> {
        let high = self.parse_hex_u16()?;
        let scalar = if (0xd800..=0xdbff).contains(&high) {
            if self.source.get(self.cursor..self.cursor.saturating_add(2)) != Some(b"\\u") {
                return Err("high surrogate is missing a low surrogate".to_owned());
            }
            self.cursor += 2;
            let low = self.parse_hex_u16()?;
            if !(0xdc00..=0xdfff).contains(&low) {
                return Err("invalid low unicode surrogate".to_owned());
            }
            0x1_0000 + ((u32::from(high) - 0xd800) << 10) + (u32::from(low) - 0xdc00)
        } else if (0xdc00..=0xdfff).contains(&high) {
            return Err("unpaired low unicode surrogate".to_owned());
        } else {
            u32::from(high)
        };
        char::from_u32(scalar).ok_or_else(|| "invalid unicode scalar".to_owned())
    }

    fn parse_hex_u16(&mut self) -> Result<u16, String> {
        let start = self.cursor;
        let end = start.saturating_add(4);
        let bytes = self
            .source
            .get(start..end)
            .ok_or_else(|| "short unicode escape".to_owned())?;
        self.cursor = end;
        let text = std::str::from_utf8(bytes).map_err(|_| "invalid unicode escape".to_owned())?;
        u16::from_str_radix(text, 16).map_err(|_| "invalid unicode escape".to_owned())
    }

    fn literal(&mut self, literal: &[u8]) -> Result<(), String> {
        let end = self.cursor.saturating_add(literal.len());
        if self.source.get(self.cursor..end) != Some(literal) {
            return Err("invalid JSON literal".to_owned());
        }
        self.cursor = end;
        Ok(())
    }

    fn consume(&mut self, expected: u8) -> Result<(), String> {
        self.skip_whitespace();
        if self.next() == Some(expected) {
            Ok(())
        } else {
            Err(format!("expected JSON byte {}", char::from(expected)))
        }
    }

    fn take_if(&mut self, expected: u8) -> bool {
        if self.peek() == Some(expected) {
            self.cursor += 1;
            true
        } else {
            false
        }
    }

    fn skip_whitespace(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\n' | b'\r' | b'\t')) {
            self.cursor += 1;
        }
    }

    fn peek(&self) -> Option<u8> {
        self.source.get(self.cursor).copied()
    }

    fn next(&mut self) -> Option<u8> {
        let value = self.peek()?;
        self.cursor += 1;
        Some(value)
    }
}

impl From<&str> for Value {
    fn from(value: &str) -> Self {
        Self::String(value.to_owned())
    }
}

impl From<String> for Value {
    fn from(value: String) -> Self {
        Self::String(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_json_round_trips_unicode_and_sorted_objects() -> Result<(), String> {
        let parsed = Value::parse_json(r#"{"z":"\ud83d\ude80","a":[true,null,-7],"text":"भारत"}"#)?;
        assert_eq!(
            parsed.to_canonical_json(),
            r#"{"a":[true,null,-7],"text":"भारत","z":"🚀"}"#
        );
        Ok(())
    }

    #[test]
    fn parser_rejects_duplicate_keys_and_floats() {
        assert!(Value::parse_json(r#"{"a":1,"a":2}"#).is_err());
        assert!(Value::parse_json("1.5").is_err());
    }

    #[test]
    fn deterministic_malformed_json_campaign_does_not_panic() -> Result<(), String> {
        let mut state = 0x4942_4841_524e_4553_u64;
        for _case in 0..50_000 {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            let len = usize::try_from(state & 0xff).unwrap_or(0);
            let mut bytes = Vec::with_capacity(len);
            for _ in 0..len {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                bytes.push(u8::try_from(state & 0xff).unwrap_or(0));
            }
            let input = String::from_utf8_lossy(&bytes);
            if let Ok(value) = Value::parse_json(&input) {
                let canonical = value.to_canonical_json();
                let reparsed = Value::parse_json(&canonical)?;
                assert_eq!(reparsed, value);
            }
        }
        Ok(())
    }
}
