//! Zero-dependency JSON value AST, parser, and compact/pretty serializer.

use std::fmt::Write as _;

const MAX_JSON_DEPTH: usize = 32;
const MAX_JSON_BYTES: usize = 512 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JsonVal {
    Null,
    Bool(bool),
    Number(String),
    Str(String),
    Array(Vec<JsonVal>),
    Object(Vec<(String, JsonVal)>),
}

impl JsonVal {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::Str(s) => Some(s.as_str()),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Self::Number(n) => n.parse::<i64>().ok(),
            _ => None,
        }
    }

    pub fn as_u64(&self) -> Option<u64> {
        match self {
            Self::Number(n) => n.parse::<u64>().ok(),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[JsonVal]> {
        match self {
            Self::Array(items) => Some(items.as_slice()),
            _ => None,
        }
    }

    pub fn as_array_mut(&mut self) -> Option<&mut Vec<JsonVal>> {
        match self {
            Self::Array(items) => Some(items),
            _ => None,
        }
    }

    pub fn get(&self, key: &str) -> Option<&JsonVal> {
        match self {
            Self::Object(pairs) => pairs.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn get_mut(&mut self, key: &str) -> Option<&mut JsonVal> {
        match self {
            Self::Object(pairs) => pairs.iter_mut().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn set_field(&mut self, key: &str, value: JsonVal) {
        if let Self::Object(pairs) = self {
            if let Some((_, slot)) = pairs.iter_mut().find(|(k, _)| k == key) {
                *slot = value;
            } else {
                pairs.push((key.to_owned(), value));
            }
        }
    }
}

pub fn escape_json_str(input: &str) -> String {
    let mut out = String::with_capacity(input.len() + 8);
    for ch in input.chars() {
        push_escaped_char(&mut out, ch);
    }
    out
}

fn push_escaped_char(out: &mut String, ch: char) {
    match ch {
        '"' => out.push_str("\\\""),
        '\\' => out.push_str("\\\\"),
        '\n' => out.push_str("\\n"),
        '\r' => out.push_str("\\r"),
        '\t' => out.push_str("\\t"),
        '\x08' => out.push_str("\\b"),
        '\x0c' => out.push_str("\\f"),
        c if (c as u32) < 0x20 => {
            let _ = write!(out, "\\u{:04x}", c as u32);
        }
        c => out.push(c),
    }
}

pub fn quote_json(input: &str) -> String {
    format!("\"{}\"", escape_json_str(input))
}

pub fn format_compact(val: &JsonVal) -> String {
    let mut out = String::new();
    write_val(val, &mut out, None, 0);
    out
}

pub fn format_pretty(val: &JsonVal) -> String {
    let mut out = String::new();
    write_val(val, &mut out, Some(2), 0);
    out
}

fn write_indent(out: &mut String, indent: Option<usize>, depth: usize) {
    if let Some(step) = indent {
        out.push('\n');
        for _ in 0..(step * depth) {
            out.push(' ');
        }
    }
}

fn write_val(val: &JsonVal, out: &mut String, indent: Option<usize>, depth: usize) {
    match val {
        JsonVal::Null => out.push_str("null"),
        JsonVal::Bool(true) => out.push_str("true"),
        JsonVal::Bool(false) => out.push_str("false"),
        JsonVal::Number(n) => out.push_str(n),
        JsonVal::Str(s) => out.push_str(&quote_json(s)),
        JsonVal::Array(items) => write_array(items, out, indent, depth),
        JsonVal::Object(pairs) => write_object(pairs, out, indent, depth),
    }
}

fn write_array(items: &[JsonVal], out: &mut String, indent: Option<usize>, depth: usize) {
    out.push('[');
    for (idx, item) in items.iter().enumerate() {
        if idx > 0 {
            out.push(',');
        }
        write_indent(out, indent, depth + 1);
        write_val(item, out, indent, depth + 1);
    }
    if !items.is_empty() {
        write_indent(out, indent, depth);
    }
    out.push(']');
}

fn write_object(pairs: &[(String, JsonVal)], out: &mut String, indent: Option<usize>, depth: usize) {
    out.push('{');
    let sep = if indent.is_some() { ": " } else { ":" };
    for (idx, (key, val)) in pairs.iter().enumerate() {
        if idx > 0 {
            out.push(',');
        }
        write_indent(out, indent, depth + 1);
        out.push_str(&quote_json(key));
        out.push_str(sep);
        write_val(val, out, indent, depth + 1);
    }
    if !pairs.is_empty() {
        write_indent(out, indent, depth);
    }
    out.push('}');
}

pub fn parse_json(input: &str) -> Result<JsonVal, String> {
    if input.len() > MAX_JSON_BYTES {
        return Err("JSON payload exceeds size limit".to_owned());
    }
    let mut parser = Parser { src: input.as_bytes(), pos: 0 };
    let val = parser.parse_value(0)?;
    parser.skip_ws();
    if parser.pos != parser.src.len() {
        return Err(format!("trailing bytes at offset {}", parser.pos));
    }
    Ok(val)
}

struct Parser<'a> {
    src: &'a [u8],
    pos: usize,
}

impl<'a> Parser<'a> {
    fn peek(&self) -> Option<u8> {
        self.src.get(self.pos).copied()
    }

    fn bump(&mut self) -> Option<u8> {
        let b = self.peek()?;
        self.pos += 1;
        Some(b)
    }

    fn skip_ws(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\r' | b'\n')) {
            self.pos += 1;
        }
    }

    fn expect_byte(&mut self, expected: u8) -> Result<(), String> {
        self.skip_ws();
        match self.bump() {
            Some(b) if b == expected => Ok(()),
            _ => Err(format!("expected '{}' at offset {}", expected as char, self.pos)),
        }
    }

    fn parse_value(&mut self, depth: usize) -> Result<JsonVal, String> {
        if depth > MAX_JSON_DEPTH {
            return Err("JSON nesting depth exceeded".to_owned());
        }
        self.skip_ws();
        match self.peek() {
            Some(b'"') => self.parse_string().map(JsonVal::Str),
            Some(b'{') => self.parse_object(depth + 1),
            Some(b'[') => self.parse_array(depth + 1),
            Some(b't') => self.expect_lit(b"true", JsonVal::Bool(true)),
            Some(b'f') => self.expect_lit(b"false", JsonVal::Bool(false)),
            Some(b'n') => self.expect_lit(b"null", JsonVal::Null),
            Some(b'-' | b'0'..=b'9') => self.parse_number(),
            _ => Err(format!("unexpected token at offset {}", self.pos)),
        }
    }

    fn expect_lit(&mut self, lit: &[u8], out: JsonVal) -> Result<JsonVal, String> {
        if self.src[self.pos..].starts_with(lit) {
            self.pos += lit.len();
            Ok(out)
        } else {
            Err(format!("invalid literal at offset {}", self.pos))
        }
    }

    fn parse_number(&mut self) -> Result<JsonVal, String> {
        let start = self.pos;
        if self.peek() == Some(b'-') {
            self.pos += 1;
        }
        self.eat_digits()?;
        if self.peek() == Some(b'.') {
            self.pos += 1;
            self.eat_digits()?;
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            self.pos += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.pos += 1;
            }
            self.eat_digits()?;
        }
        let lexeme = std::str::from_utf8(&self.src[start..self.pos]).map_err(|e| e.to_string())?;
        Ok(JsonVal::Number(lexeme.to_owned()))
    }

    fn eat_digits(&mut self) -> Result<(), String> {
        let start = self.pos;
        while matches!(self.peek(), Some(b'0'..=b'9')) {
            self.pos += 1;
        }
        if self.pos == start {
            return Err(format!("expected digit at offset {}", self.pos));
        }
        Ok(())
    }
}

impl<'a> Parser<'a> {
    fn parse_string(&mut self) -> Result<String, String> {
        self.expect_byte(b'"')?;
        let mut out = String::new();
        while let Some(b) = self.bump() {
            match b {
                b'"' => return Ok(out),
                b'\\' => self.parse_escape(&mut out)?,
                0x00..=0x1f => return Err("unescaped control byte in JSON string".to_owned()),
                _ => self.push_utf8_char(b, &mut out)?,
            }
        }
        Err("unterminated JSON string".to_owned())
    }

    fn push_utf8_char(&mut self, first: u8, out: &mut String) -> Result<(), String> {
        if first < 0x80 {
            out.push(first as char);
            return Ok(());
        }
        let start = self.pos - 1;
        let width = utf8_char_width(first).ok_or_else(|| "invalid UTF-8 lead byte".to_owned())?;
        let end = start + width;
        if end > self.src.len() {
            return Err("truncated UTF-8 sequence".to_owned());
        }
        let s = std::str::from_utf8(&self.src[start..end]).map_err(|e| e.to_string())?;
        out.push_str(s);
        self.pos = end;
        Ok(())
    }

    fn parse_escape(&mut self, out: &mut String) -> Result<(), String> {
        let ch = match self.bump() {
            Some(b'"') => '"',
            Some(b'\\') => '\\',
            Some(b'/') => '/',
            Some(b'b') => '\x08',
            Some(b'f') => '\x0c',
            Some(b'n') => '\n',
            Some(b'r') => '\r',
            Some(b't') => '\t',
            Some(b'u') => return self.parse_unicode_escape(out),
            _ => return Err(format!("invalid escape at offset {}", self.pos)),
        };
        out.push(ch);
        Ok(())
    }

    fn parse_unicode_escape(&mut self, out: &mut String) -> Result<(), String> {
        let code = self.read_hex4()?;
        if (0xD800..=0xDBFF).contains(&code) {
            if self.src.get(self.pos..self.pos + 2) != Some(b"\\u") {
                return Err("missing low surrogate escape".to_owned());
            }
            self.pos += 2;
            let low = self.read_hex4()?;
            if !(0xDC00..=0xDFFF).contains(&low) {
                return Err("invalid low surrogate".to_owned());
            }
            let cp = 0x10000 + (((code as u32 - 0xD800) << 10) | (low as u32 - 0xDC00));
            let ch = char::from_u32(cp).ok_or_else(|| "invalid unicode scalar".to_owned())?;
            out.push(ch);
            return Ok(());
        }
        let ch = char::from_u32(code as u32).ok_or_else(|| "invalid unicode escape".to_owned())?;
        out.push(ch);
        Ok(())
    }

    fn read_hex4(&mut self) -> Result<u16, String> {
        if self.pos + 4 > self.src.len() {
            return Err("truncated \\uXXXX escape".to_owned());
        }
        let hex = std::str::from_utf8(&self.src[self.pos..self.pos + 4]).map_err(|e| e.to_string())?;
        self.pos += 4;
        u16::from_str_radix(hex, 16).map_err(|_| format!("invalid hex escape {hex}"))
    }

    fn parse_array(&mut self, depth: usize) -> Result<JsonVal, String> {
        self.expect_byte(b'[')?;
        let mut items = Vec::new();
        self.skip_ws();
        if self.peek() == Some(b']') {
            self.pos += 1;
            return Ok(JsonVal::Array(items));
        }
        loop {
            items.push(self.parse_value(depth)?);
            self.skip_ws();
            match self.bump() {
                Some(b',') => continue,
                Some(b']') => return Ok(JsonVal::Array(items)),
                _ => return Err(format!("expected ',' or ']' at offset {}", self.pos)),
            }
        }
    }

    fn parse_object(&mut self, depth: usize) -> Result<JsonVal, String> {
        self.expect_byte(b'{')?;
        let mut pairs = Vec::new();
        self.skip_ws();
        if self.peek() == Some(b'}') {
            self.pos += 1;
            return Ok(JsonVal::Object(pairs));
        }
        loop {
            let key = self.parse_string()?;
            self.expect_byte(b':')?;
            let val = self.parse_value(depth)?;
            pairs.push((key, val));
            self.skip_ws();
            match self.bump() {
                Some(b',') => continue,
                Some(b'}') => return Ok(JsonVal::Object(pairs)),
                _ => return Err(format!("expected ',' or '}}' at offset {}", self.pos)),
            }
        }
    }
}

fn utf8_char_width(lead: u8) -> Option<usize> {
    match lead {
        0xC2..=0xDF => Some(2),
        0xE0..=0xEF => Some(3),
        0xF0..=0xF4 => Some(4),
        _ => None,
    }
}
