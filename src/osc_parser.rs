//! Streaming zero-full-buffering OSC 52 / OSC 133 parser and pure-Rust base64 decoder.

use std::time::Instant;

pub const MAX_OSC52_BYTES: usize = 2_097_152;
const MAX_OSC52_SEL_LEN: usize = 32;
const MAX_OSC133_BUF_LEN: usize = 64;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OscEvent {
    Osc52Write(String),
    Osc52BlockedQuery,
    CommandStart,
    CommandFinish { exit_code: Option<i32> },
}

#[derive(Clone, Debug, Default)]
pub struct OscCommandTimer {
    started_at: Option<Instant>,
}

impl OscCommandTimer {
    pub fn on_start(&mut self, now: Instant) {
        self.started_at = Some(now);
    }

    pub fn on_finish(&mut self, exit_code: Option<i32>, now: Instant) -> (Option<i32>, u64) {
        let elapsed = self
            .started_at
            .take()
            .map_or(0, |t0| now.saturating_duration_since(t0).as_secs());
        (exit_code, elapsed)
    }
}

fn decode_b64_digit(byte: u8) -> Option<u8> {
    match byte {
        b'A'..=b'Z' => Some(byte - b'A'),
        b'a'..=b'z' => Some(byte - b'a' + 26),
        b'0'..=b'9' => Some(byte - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}

fn is_ascii_ws(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\r' | b'\n')
}

struct Base64Stream {
    quad: [u8; 4],
    quad_len: usize,
    decoded: Vec<u8>,
    invalid: bool,
    overflow: bool,
    saw_padding: bool,
}

impl Base64Stream {
    fn new() -> Self {
        Self {
            quad: [0; 4],
            quad_len: 0,
            decoded: Vec::new(),
            invalid: false,
            overflow: false,
            saw_padding: false,
        }
    }

    fn reset(&mut self) {
        self.quad_len = 0;
        self.decoded.clear();
        self.invalid = false;
        self.overflow = false;
        self.saw_padding = false;
    }

    fn abort_overflow(&mut self) {
        self.overflow = true;
        self.decoded.clear();
        self.decoded.shrink_to_fit();
    }

    fn push_byte(&mut self, byte: u8) {
        if self.invalid || self.overflow || is_ascii_ws(byte) {
            return;
        }
        if self.saw_padding {
            self.invalid = true;
            return;
        }
        self.quad[self.quad_len] = byte;
        self.quad_len += 1;
        if self.quad_len == 4 {
            self.flush_quad();
        }
    }

    fn flush_quad(&mut self) {
        let q = self.quad;
        self.quad_len = 0;
        if q[0] == b'=' || q[1] == b'=' {
            self.invalid = true;
            return;
        }
        let (Some(a), Some(b)) = (decode_b64_digit(q[0]), decode_b64_digit(q[1])) else {
            self.invalid = true;
            return;
        };
        self.decode_tail_bytes(a, b, q[2], q[3]);
    }

    fn decode_tail_bytes(&mut self, a: u8, b: u8, c_byte: u8, d_byte: u8) {
        let b0 = (a << 2) | (b >> 4);
        match (c_byte, d_byte) {
            (b'=', b'=') => {
                self.saw_padding = true;
                self.push_decoded(&[b0]);
            }
            (c_raw, b'=') => {
                let Some(c) = decode_b64_digit(c_raw) else {
                    self.invalid = true;
                    return;
                };
                self.saw_padding = true;
                self.push_decoded(&[b0, (b << 4) | (c >> 2)]);
            }
            (c_raw, d_raw) => self.decode_full_triple(b0, b, c_raw, d_raw),
        }
    }

    fn decode_full_triple(&mut self, b0: u8, b: u8, c_raw: u8, d_raw: u8) {
        let (Some(c), Some(d)) = (decode_b64_digit(c_raw), decode_b64_digit(d_raw)) else {
            self.invalid = true;
            return;
        };
        self.push_decoded(&[b0, (b << 4) | (c >> 2), (c << 6) | d]);
    }

    fn push_decoded(&mut self, bytes: &[u8]) {
        if self.decoded.len() + bytes.len() > MAX_OSC52_BYTES {
            self.abort_overflow();
            return;
        }
        self.decoded.extend_from_slice(bytes);
    }

    fn finish(&mut self) -> Option<String> {
        let valid = !self.invalid && !self.overflow && self.quad_len == 0;
        let bytes = std::mem::take(&mut self.decoded);
        self.reset();
        if !valid || bytes.is_empty() {
            return None;
        }
        String::from_utf8(bytes).ok()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OscState {
    Ground,
    Esc,
    OscCode,
    Osc52Sel,
    Osc52SelEsc,
    Osc52Body,
    Osc52BodyEsc,
    Osc133Body,
    Osc133BodyEsc,
}

pub struct OscStreamParser {
    state: OscState,
    code_buf: Vec<u8>,
    sel_len: usize,
    osc133_buf: Vec<u8>,
    b64: Base64Stream,
    saw_query: bool,
    saw_body_non_ws: bool,
}

impl Default for OscStreamParser {
    fn default() -> Self {
        Self::new()
    }
}

impl OscStreamParser {
    pub fn new() -> Self {
        Self {
            state: OscState::Ground,
            code_buf: Vec::with_capacity(16),
            sel_len: 0,
            osc133_buf: Vec::with_capacity(MAX_OSC133_BUF_LEN),
            b64: Base64Stream::new(),
            saw_query: false,
            saw_body_non_ws: false,
        }
    }

    pub fn feed(&mut self, chunk: &[u8], out: &mut Vec<u8>, events: &mut Vec<OscEvent>) {
        let mut idx = 0usize;
        while idx < chunk.len() {
            if self.state == OscState::Ground {
                idx += self.consume_ground_slice(&chunk[idx..], out);
            } else {
                self.step_byte(chunk[idx], out, events);
                idx += 1;
            }
        }
    }

    fn consume_ground_slice(&mut self, slice: &[u8], out: &mut Vec<u8>) -> usize {
        match slice.iter().position(|&b| b == 0x1b) {
            Some(pos) => {
                out.extend_from_slice(&slice[..pos]);
                self.state = OscState::Esc;
                pos + 1
            }
            None => {
                out.extend_from_slice(slice);
                slice.len()
            }
        }
    }

    fn step_byte(&mut self, byte: u8, out: &mut Vec<u8>, events: &mut Vec<OscEvent>) {
        match self.state {
            OscState::Ground => {}
            OscState::Esc => self.step_esc(byte, out),
            OscState::OscCode => self.step_osc_code(byte, out),
            OscState::Osc52Sel => self.step_osc52_sel(byte),
            OscState::Osc52SelEsc => self.step_osc52_esc(byte, out, OscState::Osc52Sel, false, events),
            OscState::Osc52Body => self.step_osc52_body(byte, events),
            OscState::Osc52BodyEsc => self.step_osc52_esc(byte, out, OscState::Osc52Body, true, events),
            OscState::Osc133Body => self.step_osc133_body(byte, out, events),
            OscState::Osc133BodyEsc => self.step_osc133_esc(byte, out, events),
        }
    }

    fn step_esc(&mut self, byte: u8, out: &mut Vec<u8>) {
        if byte == b']' {
            self.code_buf.clear();
            self.state = OscState::OscCode;
        } else if byte == 0x1b {
            out.push(0x1b);
        } else {
            out.extend_from_slice(&[0x1b, byte]);
            self.state = OscState::Ground;
        }
    }

    fn step_osc_code(&mut self, byte: u8, out: &mut Vec<u8>) {
        if byte == b';' {
            self.dispatch_osc_code(out);
            return;
        }
        if byte.is_ascii_digit() && self.code_buf.len() < 12 {
            self.code_buf.push(byte);
            return;
        }
        self.flush_untracked_osc_prefix(byte, out);
    }

    fn dispatch_osc_code(&mut self, out: &mut Vec<u8>) {
        if self.code_buf == b"52" {
            self.sel_len = 0;
            self.state = OscState::Osc52Sel;
            return;
        }
        if self.code_buf == b"133" {
            out.extend_from_slice(b"\x1b]133;");
            self.osc133_buf.clear();
            self.state = OscState::Osc133Body;
            return;
        }
        self.flush_untracked_osc_prefix(b';', out);
    }

    fn flush_untracked_osc_prefix(&mut self, byte: u8, out: &mut Vec<u8>) {
        out.extend_from_slice(b"\x1b]");
        out.extend_from_slice(&self.code_buf);
        self.code_buf.clear();
        if byte == 0x1b {
            self.state = OscState::Esc;
        } else {
            out.push(byte);
            self.state = OscState::Ground;
        }
    }

    fn reset_osc52_body(&mut self) {
        self.b64.reset();
        self.saw_query = false;
        self.saw_body_non_ws = false;
        self.state = OscState::Osc52Body;
    }

    fn step_osc52_sel(&mut self, byte: u8) {
        match byte {
            b';' => self.reset_osc52_body(),
            0x07 | 0x18 | 0x1a => self.state = OscState::Ground,
            0x1b => self.state = OscState::Osc52SelEsc,
            _ if self.sel_len < MAX_OSC52_SEL_LEN => self.sel_len += 1,
            _ => {}
        }
    }

    fn step_osc52_body(&mut self, byte: u8, events: &mut Vec<OscEvent>) {
        match byte {
            0x07 => self.finish_osc52(events),
            0x1b => self.state = OscState::Osc52BodyEsc,
            0x18 | 0x1a => self.abort_osc52(),
            _ => self.consume_osc52_payload_byte(byte),
        }
    }

    fn consume_osc52_payload_byte(&mut self, byte: u8) {
        if !is_ascii_ws(byte) {
            self.saw_body_non_ws = true;
            if byte == b'?' {
                self.saw_query = true;
            }
        }
        if !self.saw_query {
            self.b64.push_byte(byte);
        }
    }

    fn step_osc52_esc(
        &mut self,
        byte: u8,
        out: &mut Vec<u8>,
        _resume: OscState,
        in_body: bool,
        events: &mut Vec<OscEvent>,
    ) {
        if byte == b'\\' {
            if in_body {
                self.finish_osc52(events);
            } else {
                self.state = OscState::Ground;
            }
        } else {
            self.abort_osc52();
            self.handle_aborted_esc(byte, out);
        }
    }

    fn finish_osc52(&mut self, events: &mut Vec<OscEvent>) {
        if self.saw_query {
            self.b64.reset();
            events.push(OscEvent::Osc52BlockedQuery);
        } else if let Some(text) = self.b64.finish() {
            events.push(OscEvent::Osc52Write(text));
        }
        self.state = OscState::Ground;
    }

    fn abort_osc52(&mut self) {
        self.b64.reset();
        self.state = OscState::Ground;
    }

    fn handle_aborted_esc(&mut self, byte: u8, out: &mut Vec<u8>) {
        if byte == b']' {
            self.code_buf.clear();
            self.state = OscState::OscCode;
        } else if byte == 0x1b {
            self.state = OscState::Esc;
        } else {
            out.extend_from_slice(&[0x1b, byte]);
            self.state = OscState::Ground;
        }
    }

    fn step_osc133_body(&mut self, byte: u8, out: &mut Vec<u8>, events: &mut Vec<OscEvent>) {
        if byte == 0x07 {
            out.push(0x07);
            self.finish_osc133(events);
            return;
        }
        if byte == 0x1b {
            self.state = OscState::Osc133BodyEsc;
            return;
        }
        out.push(byte);
        if self.osc133_buf.len() < MAX_OSC133_BUF_LEN {
            self.osc133_buf.push(byte);
        }
    }

    fn step_osc133_esc(&mut self, byte: u8, out: &mut Vec<u8>, events: &mut Vec<OscEvent>) {
        if byte == b'\\' {
            out.extend_from_slice(b"\x1b\\");
            self.finish_osc133(events);
        } else {
            self.osc133_buf.clear();
            self.handle_aborted_esc(byte, out);
        }
    }

    fn finish_osc133(&mut self, events: &mut Vec<OscEvent>) {
        if let Some(ev) = parse_osc133_body(&self.osc133_buf) {
            events.push(ev);
        }
        self.osc133_buf.clear();
        self.state = OscState::Ground;
    }
}

fn parse_osc133_body(raw: &[u8]) -> Option<OscEvent> {
    let text = std::str::from_utf8(raw).ok()?.trim();
    let mut parts = text.split(';');
    match parts.next()? {
        "C" => Some(OscEvent::CommandStart),
        "D" => {
            let exit_code = parts
                .next()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .and_then(|s| s.parse::<i32>().ok());
            Some(OscEvent::CommandFinish { exit_code })
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    fn encode_b64_test(input: &[u8]) -> String {
        const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::new();
        for chunk in input.chunks(3) {
            let b0 = chunk[0] as usize;
            let b1 = *chunk.get(1).unwrap_or(&0) as usize;
            let b2 = *chunk.get(2).unwrap_or(&0) as usize;
            let n = (b0 << 16) | (b1 << 8) | b2;
            out.push(TABLE[(n >> 18) & 63] as char);
            out.push(TABLE[(n >> 12) & 63] as char);
            out.push(if chunk.len() > 1 { TABLE[(n >> 6) & 63] as char } else { '=' });
            out.push(if chunk.len() > 2 { TABLE[n & 63] as char } else { '=' });
        }
        out
    }

    #[test]
    fn parses_osc52_writes_with_bel_and_st_and_wrapped_base64() {
        let mut p = OscStreamParser::new();
        let mut out = Vec::new();
        let mut events = Vec::new();
        let msg = "Привет, AkhTerm!";
        let b64 = encode_b64_test(msg.as_bytes());
        let wrapped = format!("{}\n{}", &b64[..8], &b64[8..]);
        let stream = format!("before\x1b]52;c;{wrapped}\x07mid\x1b]52;;{b64}\x1b\\after");
        p.feed(stream.as_bytes(), &mut out, &mut events);
        assert_eq!(String::from_utf8(out).unwrap(), "beforemidafter");
        assert_eq!(
            events,
            vec![
                OscEvent::Osc52Write(msg.to_owned()),
                OscEvent::Osc52Write(msg.to_owned()),
            ]
        );
    }

    #[test]
    fn unconditionally_blocks_and_strips_osc52_queries_across_split_chunks() {
        let mut p = OscStreamParser::new();
        let mut out = Vec::new();
        let mut events = Vec::new();
        let stream = b"A\x1b]52;c;?\x07B\x1b]52;p; ? \x1b\\C\x1b]52;;?\x1b\\D";
        for &byte in stream {
            p.feed(&[byte], &mut out, &mut events);
        }
        assert_eq!(String::from_utf8(out).unwrap(), "ABCD");
        assert_eq!(
            events,
            vec![
                OscEvent::Osc52BlockedQuery,
                OscEvent::Osc52BlockedQuery,
                OscEvent::Osc52BlockedQuery,
            ]
        );
    }

    #[test]
    fn enforces_2mb_limit_exactly_and_rejects_oversized_or_invalid_payloads() {
        let exact = vec![b'a'; MAX_OSC52_BYTES];
        let over = vec![b'a'; MAX_OSC52_BYTES + 1];
        let mut p = OscStreamParser::new();
        let mut out = Vec::new();
        let mut events = Vec::new();

        let exact_seq = format!("ok1\x1b]52;c;{}\x07", encode_b64_test(&exact));
        p.feed(exact_seq.as_bytes(), &mut out, &mut events);
        assert_eq!(events.len(), 1);
        assert!(matches!(&events[0], OscEvent::Osc52Write(s) if s.len() == MAX_OSC52_BYTES));

        events.clear();
        let over_seq = format!("\x1b]52;c;{}\x07ok2", encode_b64_test(&over));
        p.feed(over_seq.as_bytes(), &mut out, &mut events);
        assert!(events.is_empty());
        assert_eq!(std::str::from_utf8(&out).unwrap(), "ok1ok2");

        let bad_seq = format!("\x1b]52;c;{}\x07\x1b]52;c;!!!\x07", encode_b64_test(&[0xff, 0xfe]));
        out.clear();
        p.feed(bad_seq.as_bytes(), &mut out, &mut events);
        assert!(out.is_empty() && events.is_empty());
    }

    #[test]
    fn streams_non_osc52_sequences_and_detects_osc133_start_and_exit() {
        let mut p = OscStreamParser::new();
        let mut out = Vec::new();
        let mut events = Vec::new();
        let stream = b"\x1b]7;file://box/srv/app\x07\x1b]133;C\x07hi\x1b]133;D;130\x1b\\\x1b]133;D\x07";
        for chunk in stream.chunks(3) {
            p.feed(chunk, &mut out, &mut events);
        }
        assert_eq!(out, stream);
        assert_eq!(
            events,
            vec![
                OscEvent::CommandStart,
                OscEvent::CommandFinish { exit_code: Some(130) },
                OscEvent::CommandFinish { exit_code: None },
            ]
        );
    }

    #[test]
    fn tracks_osc133_elapsed_command_seconds() {
        let mut timer = OscCommandTimer::default();
        let t0 = Instant::now();
        assert_eq!(timer.on_finish(Some(0), t0), (Some(0), 0));
        timer.on_start(t0);
        assert_eq!(timer.on_finish(Some(2), t0 + Duration::from_secs(17)), (Some(2), 17));
        assert_eq!(timer.on_finish(None, t0 + Duration::from_secs(20)), (None, 0));
    }

    #[test]
    fn osc_parser_meets_grade_b_mi_target() {
        let (_, _, mi) = crate::mi::compute_file_mi(include_str!("osc_parser.rs"));
        assert!(mi >= 60.0, "osc_parser.rs MI {mi:.2} < 60.0");
    }
}
