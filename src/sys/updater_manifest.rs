#![allow(dead_code)]
//! Dependency-free GitHub Releases manifest, SemVer, and SHA256SUMS parser.

use std::cmp::Ordering;
use std::collections::HashMap;
use std::fmt;

const MAX_JSON_DEPTH: usize = 64;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PreId {
    Numeric(u64),
    Text(String),
}

impl Ord for PreId {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self, other) {
            (Self::Numeric(a), Self::Numeric(b)) => a.cmp(b),
            (Self::Numeric(_), Self::Text(_)) => Ordering::Less,
            (Self::Text(_), Self::Numeric(_)) => Ordering::Greater,
            (Self::Text(a), Self::Text(b)) => a.cmp(b),
        }
    }
}

impl PartialOrd for PreId {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Display for PreId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Numeric(n) => write!(f, "{n}"),
            Self::Text(s) => f.write_str(s),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SemVer {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
    pub pre: Vec<PreId>,
}

impl SemVer {
    pub fn parse(raw: &str) -> Result<Self, String> {
        let trimmed = raw.trim();
        let without_v = trimmed.strip_prefix(['v', 'V']).unwrap_or(trimmed);
        let without_build = strip_build_metadata(without_v)?;
        let (core, pre_part) = without_build
            .split_once('-')
            .map_or((without_build, None), |(c, p)| (c, Some(p)));
        let (major, minor, patch) = parse_core_triplet(core)?;
        let pre = pre_part.map(parse_prerelease_ids).transpose()?.unwrap_or_default();
        Ok(Self { major, minor, patch, pre })
    }

    pub fn is_newer_than(&self, current: &Self) -> bool {
        self > current
    }
}

impl Ord for SemVer {
    fn cmp(&self, other: &Self) -> Ordering {
        (self.major, self.minor, self.patch)
            .cmp(&(other.major, other.minor, other.patch))
            .then_with(|| compare_prerelease(&self.pre, &other.pre))
    }
}

impl PartialOrd for SemVer {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Display for SemVer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)?;
        if !self.pre.is_empty() {
            f.write_str("-")?;
            for (idx, id) in self.pre.iter().enumerate() {
                if idx > 0 {
                    f.write_str(".")?;
                }
                write!(f, "{id}")?;
            }
        }
        Ok(())
    }
}

fn strip_build_metadata(input: &str) -> Result<&str, String> {
    match input.split_once('+') {
        Some((_, build)) if build.is_empty() => Err(format!("invalid build metadata in '{input}'")),
        Some((head, _)) => Ok(head),
        None => Ok(input),
    }
}

fn parse_core_triplet(core: &str) -> Result<(u64, u64, u64), String> {
    let mut parts = core.split('.');
    let major = parse_u64_part(parts.next(), core)?;
    let minor = parse_u64_part(parts.next(), core)?;
    let patch = parse_u64_part(parts.next(), core)?;
    if parts.next().is_some() {
        return Err(format!("too many version segments in '{core}'"));
    }
    Ok((major, minor, patch))
}

fn parse_u64_part(part: Option<&str>, whole: &str) -> Result<u64, String> {
    let text = part.ok_or_else(|| format!("missing version segment in '{whole}'"))?;
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return Err(format!("invalid numeric segment '{text}' in '{whole}'"));
    }
    text.parse::<u64>()
        .map_err(|_| format!("version segment overflow in '{whole}'"))
}

fn parse_prerelease_id(part: &str) -> Result<PreId, String> {
    if part.is_empty() || !part.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
        return Err(format!("invalid pre-release token '{part}'"));
    }
    if part.bytes().all(|b| b.is_ascii_digit()) {
        return part
            .parse::<u64>()
            .map(PreId::Numeric)
            .map_err(|_| format!("pre-release number overflow '{part}'"));
    }
    Ok(PreId::Text(part.to_owned()))
}

fn parse_prerelease_ids(pre_str: &str) -> Result<Vec<PreId>, String> {
    if pre_str.is_empty() {
        return Err("empty pre-release identifier".to_owned());
    }
    let mut ids = Vec::new();
    for part in pre_str.split('.') {
        ids.push(parse_prerelease_id(part)?);
    }
    Ok(ids)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReleaseAsset {
    pub name: String,
    pub browser_download_url: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReleaseManifest {
    pub tag_name: String,
    pub version: SemVer,
    pub html_url: String,
    pub assets: Vec<ReleaseAsset>,
}

impl ReleaseManifest {
    pub fn sha256sums_asset(&self) -> Option<&ReleaseAsset> {
        self.assets.iter().find(|a| a.name == "SHA256SUMS")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AssetKind {
    Tarball,
    Deb,
    Rpm,
}

#[derive(Debug)]
enum JsonValue {
    Null,
    Bool,
    Number,
    Str(String),
    Array(Vec<JsonValue>),
    Object(Vec<(String, JsonValue)>),
}

struct JsonParser<'a> {
    bytes: &'a [u8],
    pos: usize,
}

fn compare_prerelease(a: &[PreId], b: &[PreId]) -> Ordering {
    match (a.is_empty(), b.is_empty()) {
        (true, true) => Ordering::Equal,
        (true, false) => Ordering::Greater,
        (false, true) => Ordering::Less,
        (false, false) => a.iter().cmp(b.iter()),
    }
}

impl<'a> JsonParser<'a> {
    fn new(input: &'a str) -> Self {
        Self { bytes: input.as_bytes(), pos: 0 }
    }

    fn skip_ws(&mut self) {
        while matches!(self.bytes.get(self.pos), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.pos += 1;
        }
    }

    fn peek(&mut self) -> Option<u8> {
        self.skip_ws();
        self.bytes.get(self.pos).copied()
    }

    fn next_byte(&mut self) -> Result<u8, String> {
        let b = *self.bytes.get(self.pos).ok_or("unexpected end of JSON")?;
        self.pos += 1;
        Ok(b)
    }

    fn expect_byte(&mut self, expected: u8) -> Result<(), String> {
        self.skip_ws();
        let actual = self.next_byte()?;
        if actual != expected {
            return Err(format!("expected '{}' at offset {}", expected as char, self.pos));
        }
        Ok(())
    }

    fn parse_root(&mut self) -> Result<JsonValue, String> {
        let val = self.parse_value(0)?;
        self.skip_ws();
        if self.pos != self.bytes.len() {
            return Err(format!("trailing JSON bytes at offset {}", self.pos));
        }
        Ok(val)
    }

    fn parse_value(&mut self, depth: usize) -> Result<JsonValue, String> {
        if depth > MAX_JSON_DEPTH {
            return Err("JSON nesting depth exceeded".to_owned());
        }
        match self.peek() {
            Some(b'{') => self.parse_object(depth + 1),
            Some(b'[') => self.parse_array(depth + 1),
            Some(b'"') => self.parse_string().map(JsonValue::Str),
            Some(b't') => self.parse_literal(b"true", JsonValue::Bool),
            Some(b'f') => self.parse_literal(b"false", JsonValue::Bool),
            Some(b'n') => self.parse_literal(b"null", JsonValue::Null),
            Some(b'-' | b'0'..=b'9') => self.parse_number(),
            Some(other) => Err(format!("unexpected byte '{}' at {}", other as char, self.pos)),
            None => Err("unexpected end of JSON value".to_owned()),
        }
    }

    fn parse_literal(&mut self, lit: &[u8], val: JsonValue) -> Result<JsonValue, String> {
        if !self.bytes[self.pos..].starts_with(lit) {
            return Err(format!("invalid JSON literal at offset {}", self.pos));
        }
        self.pos += lit.len();
        Ok(val)
    }

    fn parse_number(&mut self) -> Result<JsonValue, String> {
        let start = self.pos;
        self.advance_if(b'-');
        self.consume_digits()?;
        if self.advance_if(b'.') {
            self.consume_digits()?;
        }
        self.consume_exponent()?;
        if self.pos == start {
            return Err(format!("invalid JSON number at offset {start}"));
        }
        Ok(JsonValue::Number)
    }

    fn consume_exponent(&mut self) -> Result<(), String> {
        if self.advance_if(b'e') || self.advance_if(b'E') {
            let _ = self.advance_if(b'+') || self.advance_if(b'-');
            self.consume_digits()?;
        }
        Ok(())
    }

    fn advance_if(&mut self, target: u8) -> bool {
        if self.bytes.get(self.pos) == Some(&target) {
            self.pos += 1;
            return true;
        }
        false
    }

    fn consume_digits(&mut self) -> Result<(), String> {
        let start = self.pos;
        while matches!(self.bytes.get(self.pos), Some(b'0'..=b'9')) {
            self.pos += 1;
        }
        if self.pos == start {
            return Err(format!("expected digit at offset {}", self.pos));
        }
        Ok(())
    }

    fn parse_array(&mut self, depth: usize) -> Result<JsonValue, String> {
        self.expect_byte(b'[')?;
        let mut items = Vec::new();
        if self.peek() == Some(b']') {
            self.pos += 1;
            return Ok(JsonValue::Array(items));
        }
        loop {
            items.push(self.parse_value(depth)?);
            if !self.consume_sep(b']')? {
                return Ok(JsonValue::Array(items));
            }
        }
    }

    fn parse_object(&mut self, depth: usize) -> Result<JsonValue, String> {
        self.expect_byte(b'{')?;
        let mut fields = Vec::new();
        if self.peek() == Some(b'}') {
            self.pos += 1;
            return Ok(JsonValue::Object(fields));
        }
        loop {
            fields.push(self.parse_kv_pair(depth)?);
            if !self.consume_sep(b'}')? {
                return Ok(JsonValue::Object(fields));
            }
        }
    }

    fn parse_kv_pair(&mut self, depth: usize) -> Result<(String, JsonValue), String> {
        self.skip_ws();
        let key = self.parse_string()?;
        self.expect_byte(b':')?;
        Ok((key, self.parse_value(depth)?))
    }

    fn consume_sep(&mut self, end_byte: u8) -> Result<bool, String> {
        match self.peek() {
            Some(b',') => {
                self.pos += 1;
                Ok(true)
            }
            Some(b) if b == end_byte => {
                self.pos += 1;
                Ok(false)
            }
            _ => Err(format!("expected ',' or '{}' at {}", end_byte as char, self.pos)),
        }
    }

    fn parse_string(&mut self) -> Result<String, String> {
        self.expect_byte(b'"')?;
        let mut out = String::new();
        loop {
            let b = self.next_byte()?;
            match b {
                b'"' => return Ok(out),
                b'\\' => self.push_escaped_char(&mut out)?,
                0..=0x1F => return Err(format!("unescaped control byte at {}", self.pos)),
                _ => self.push_utf8_byte(b, &mut out)?,
            }
        }
    }

    fn push_utf8_byte(&mut self, first: u8, out: &mut String) -> Result<(), String> {
        if first < 0x80 {
            out.push(first as char);
            return Ok(());
        }
        let start = self.pos - 1;
        let end = start + utf8_char_width(first)?;
        let slice = self.bytes.get(start..end).ok_or("truncated UTF-8 in JSON")?;
        let s = std::str::from_utf8(slice).map_err(|_| "invalid UTF-8 in JSON")?;
        out.push_str(s);
        self.pos = end;
        Ok(())
    }

    fn push_escaped_char(&mut self, out: &mut String) -> Result<(), String> {
        let esc = self.next_byte()?;
        if esc == b'u' {
            return self.push_unicode_escape(out);
        }
        let ch = simple_escape_char(esc)
            .ok_or_else(|| format!("invalid JSON escape '\\{}'", esc as char))?;
        out.push(ch);
        Ok(())
    }

    fn push_unicode_escape(&mut self, out: &mut String) -> Result<(), String> {
        let code = self.read_hex_u16()?;
        let scalar = if (0xD800..=0xDBFF).contains(&code) {
            self.decode_surrogate_pair(code)?
        } else {
            code as u32
        };
        let ch = char::from_u32(scalar).ok_or("invalid unicode scalar")?;
        out.push(ch);
        Ok(())
    }

    fn decode_surrogate_pair(&mut self, high: u16) -> Result<u32, String> {
        if !self.bytes[self.pos..].starts_with(b"\\u") {
            return Err("unpaired high surrogate in JSON".to_owned());
        }
        self.pos += 2;
        let low = self.read_hex_u16()?;
        if !(0xDC00..=0xDFFF).contains(&low) {
            return Err("invalid low surrogate in JSON".to_owned());
        }
        Ok(0x10000 + (((high as u32 - 0xD800) << 10) | (low as u32 - 0xDC00)))
    }

    fn read_hex_u16(&mut self) -> Result<u16, String> {
        let slice = self.bytes.get(self.pos..self.pos + 4).ok_or("truncated \\u escape")?;
        let text = std::str::from_utf8(slice).map_err(|_| "invalid \\u escape")?;
        self.pos += 4;
        u16::from_str_radix(text, 16).map_err(|_| format!("invalid hex escape '\\u{text}'"))
    }
}

fn simple_escape_char(esc: u8) -> Option<char> {
    match esc {
        b'"' => Some('"'),
        b'\\' => Some('\\'),
        b'/' => Some('/'),
        b'b' => Some('\x08'),
        b'f' => Some('\x0C'),
        b'n' => Some('\n'),
        b'r' => Some('\r'),
        b't' => Some('\t'),
        _ => None,
    }
}

fn utf8_char_width(first: u8) -> Result<usize, String> {
    match first {
        0xC0..=0xDF => Ok(2),
        0xE0..=0xEF => Ok(3),
        0xF0..=0xF7 => Ok(4),
        _ => Err("invalid UTF-8 lead byte in JSON".to_owned()),
    }
}

fn find_field<'a>(fields: &'a [(String, JsonValue)], key: &str) -> Option<&'a JsonValue> {
    fields.iter().find(|(k, _)| k == key).map(|(_, v)| v)
}

fn require_non_empty_str(fields: &[(String, JsonValue)], key: &str) -> Result<String, String> {
    match find_field(fields, key) {
        Some(JsonValue::Str(s)) if !s.trim().is_empty() => Ok(s.clone()),
        Some(JsonValue::Str(_)) => Err(format!("JSON field '{key}' is empty")),
        _ => Err(format!("missing string field '{key}'")),
    }
}

pub fn parse_release_manifest(json_text: &str) -> Result<ReleaseManifest, String> {
    let root = JsonParser::new(json_text).parse_root()?;
    let JsonValue::Object(fields) = root else {
        return Err("release manifest JSON root must be an object".to_owned());
    };
    let tag_name = require_non_empty_str(&fields, "tag_name")?;
    let version = SemVer::parse(&tag_name)?;
    let html_url = require_non_empty_str(&fields, "html_url")?;
    let Some(JsonValue::Array(raw_assets)) = find_field(&fields, "assets") else {
        return Err("missing 'assets' array in release manifest".to_owned());
    };
    let assets = parse_assets_array(raw_assets)?;
    Ok(ReleaseManifest { tag_name, version, html_url, assets })
}

fn parse_assets_array(raw_assets: &[JsonValue]) -> Result<Vec<ReleaseAsset>, String> {
    raw_assets.iter().map(parse_single_asset).collect()
}

fn parse_single_asset(item: &JsonValue) -> Result<ReleaseAsset, String> {
    let JsonValue::Object(fields) = item else {
        return Err("release asset entry must be a JSON object".to_owned());
    };
    Ok(ReleaseAsset {
        name: require_non_empty_str(fields, "name")?,
        browser_download_url: require_non_empty_str(fields, "browser_download_url")?,
    })
}

pub fn parse_sha256sums(text: &str) -> HashMap<String, String> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter_map(parse_sha256_line)
        .collect()
}

fn is_valid_sha256_hex(digest: &str) -> bool {
    digest.len() == 64 && digest.bytes().all(|b| b.is_ascii_hexdigit())
}

fn clean_sha256_basename(file_part: &str) -> Option<String> {
    let trimmed = file_part.trim_start_matches([' ', '\t']).trim_start_matches('*');
    let basename = trimmed.rsplit('/').next()?.trim();
    if basename.is_empty() || basename == "." || basename == ".." {
        return None;
    }
    Some(basename.to_owned())
}

fn parse_sha256_line(line: &str) -> Option<(String, String)> {
    if line.len() <= 65 {
        return None;
    }
    let (digest, rest) = line.split_at(64);
    if !is_valid_sha256_hex(digest) || !rest.starts_with([' ', '\t']) {
        return None;
    }
    let name = clean_sha256_basename(rest)?;
    Some((name, digest.to_ascii_lowercase()))
}

pub fn find_sha256sums_asset(manifest: &ReleaseManifest) -> Option<&ReleaseAsset> {
    manifest.assets.iter().find(|a| a.name == "SHA256SUMS")
}

pub fn find_asset(manifest: &ReleaseManifest, kind: AssetKind) -> Option<&ReleaseAsset> {
    find_asset_for_arch(manifest, kind, std::env::consts::ARCH)
}

pub fn find_asset_for_arch<'a>(
    manifest: &'a ReleaseManifest,
    kind: AssetKind,
    arch: &str,
) -> Option<&'a ReleaseAsset> {
    let suffix = asset_suffix(kind, arch)?;
    let prefix = match kind {
        AssetKind::Rpm => "akhterm-",
        AssetKind::Tarball | AssetKind::Deb => "akhterm_",
    };
    manifest
        .assets
        .iter()
        .find(|a| a.name.starts_with(prefix) && a.name.ends_with(suffix))
}

fn asset_suffix(kind: AssetKind, arch: &str) -> Option<&'static str> {
    match (kind, arch) {
        (AssetKind::Tarball, "x86_64") => Some("_linux_x86_64.tar.gz"),
        (AssetKind::Tarball, "aarch64") => Some("_linux_aarch64.tar.gz"),
        (AssetKind::Deb, "x86_64") => Some("_amd64.deb"),
        (AssetKind::Deb, "aarch64") => Some("_arm64.deb"),
        (AssetKind::Rpm, "x86_64") => Some(".x86_64.rpm"),
        (AssetKind::Rpm, "aarch64") => Some(".aarch64.rpm"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_release_json() -> &'static str {
        concat!(
            r#"{"id":1,"draft":false,"author":{"login":"p","roles":["owner",{"id":1}]},"#,
            r#""body":"Line1\n\"quoted\" \\slash\\ \u0041\uD83D\uDE80","#,
            r#""tag_name":"v1.1.0","html_url":"https://example.test/v1.1.0","assets":["#,
            r#"{"name":"SHA256SUMS","browser_download_url":"https://example.test/SHA256SUMS"},"#,
            r#"{"name":"akhterm_1.1.0_linux_x86_64.tar.gz","browser_download_url":"https://example.test/t.tar.gz","meta":{"ok":[1,true,null]}},"#,
            r#"{"name":"akhterm_1.1.0-1_amd64.deb","browser_download_url":"https://example.test/d.deb"},"#,
            r#"{"name":"akhterm-1.1.0-1.x86_64.rpm","browser_download_url":"https://example.test/r.rpm"}]}"#
        )
    }

    #[test]
    fn semver_ordering_handles_v_prefix_and_prerelease() {
        let rc1 = SemVer::parse("v1.0.0-rc.1").unwrap();
        let rc2 = SemVer::parse("1.0.0-rc.2").unwrap();
        let rc10 = SemVer::parse("1.0.0-rc.10").unwrap();
        let v100 = SemVer::parse("v1.0.0").unwrap();
        let v101 = SemVer::parse("1.0.1").unwrap();
        let v110 = SemVer::parse("v1.1.0").unwrap();
        assert!(rc1 < rc2 && rc2 < rc10 && rc10 < v100);
        assert!(v100 < v101 && v101 < v110 && v110.is_newer_than(&v100));
        assert!(!v100.is_newer_than(&v100));
        assert_eq!(SemVer::parse("1.0.0+build.5").unwrap(), v100);
    }

    #[test]
    fn semver_rejects_malformed_strings() {
        let bad_cases = ["", "v", "1.0", "1.0.0.1", "1.0.x", "1.0.0-", "1.0.0-rc..1", "1.0.0+"];
        for bad in bad_cases {
            assert!(SemVer::parse(bad).is_err(), "expected error for {bad}");
        }
    }

    #[test]
    fn json_parser_handles_escapes_unicode_and_nested_objects() {
        let manifest = parse_release_manifest(sample_release_json()).unwrap();
        assert_eq!(manifest.tag_name, "v1.1.0");
        assert_eq!(manifest.version, SemVer::parse("1.1.0").unwrap());
        assert_eq!(manifest.assets.len(), 4);
        assert_eq!(find_sha256sums_asset(&manifest).unwrap().name, "SHA256SUMS");
    }

    #[test]
    fn asset_picker_selects_expected_linux_packages() {
        let manifest = parse_release_manifest(sample_release_json()).unwrap();
        let tar = find_asset_for_arch(&manifest, AssetKind::Tarball, "x86_64").unwrap();
        let deb = find_asset_for_arch(&manifest, AssetKind::Deb, "x86_64").unwrap();
        let rpm = find_asset_for_arch(&manifest, AssetKind::Rpm, "x86_64").unwrap();
        assert_eq!(tar.name, "akhterm_1.1.0_linux_x86_64.tar.gz");
        assert_eq!(deb.name, "akhterm_1.1.0-1_amd64.deb");
        assert_eq!(rpm.name, "akhterm-1.1.0-1.x86_64.rpm");
    }

    #[test]
    fn parse_sha256sums_handles_prefixes_and_rejects_bad_lines() {
        let hex_a = "0123456789abcdef".repeat(4);
        let hex_b = "FEDCBA9876543210".repeat(4);
        let text = format!(
            "# c\n{hex_a}  ./dist/a.tar.gz\n{hex_b} *dist/b.deb\nshort bad\n{}  ..\n",
            "z".repeat(64)
        );
        let map = parse_sha256sums(&text);
        assert_eq!(map.len(), 2);
        assert_eq!(map.get("a.tar.gz"), Some(&hex_a));
        assert_eq!(map.get("b.deb"), Some(&hex_b.to_ascii_lowercase()));
    }
}
