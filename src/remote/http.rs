//! Zero-dependency HTTP/1.1 parser, security guards, and Remote API v1 router.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::time::Instant;

use crate::remote::auth::{PairError, RemoteAuthStore};
use crate::remote::protocol::{
    encode_error_json, encode_status_json, encode_tail_json, ApproveAction, PaneTailResponse,
    RemoteKey, RemoteStatusResponse, API_VERSION, DEFAULT_TAIL_LINES, MAX_SEND_BYTES,
    MAX_TAIL_LINES,
};
use crate::remote::pwa::{PWA_INDEX_HTML, PWA_MANIFEST_JSON};
use crate::sys::json_lite::{format_compact, parse_json, JsonVal};

pub const MAX_BODY_BYTES: usize = 32 * 1024;
const MAX_HEADERS: usize = 48;
const MAX_HEADER_LINE: usize = 8 * 1024;

#[derive(Clone, Debug)]
pub struct HttpRequest {
    pub method: String,
    pub path: String,
    pub query: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl HttpRequest {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }

    pub fn query_param(&self, key: &str) -> Option<&str> {
        self.query
            .split('&')
            .filter_map(|p| p.split_once('='))
            .find(|(k, v)| *k == key && !v.is_empty())
            .map(|(_, v)| v)
    }
}

#[derive(Clone, Debug)]
pub struct HttpResponse {
    pub status: u16,
    pub reason: &'static str,
    pub content_type: &'static str,
    pub extra_headers: Vec<String>,
    pub body: Vec<u8>,
}

impl HttpResponse {
    pub fn json(status: u16, reason: &'static str, body: String) -> Self {
        Self {
            status, reason, content_type: "application/json; charset=utf-8",
            extra_headers: vec!["Cache-Control: no-store".into()], body: body.into_bytes(),
        }
    }

    pub fn error(status: u16, reason: &'static str, msg: &str) -> Self {
        Self::json(status, reason, encode_error_json(msg))
    }

    pub fn static_asset(content_type: &'static str, body: &str) -> Self {
        Self {
            status: 200, reason: "OK", content_type,
            extra_headers: vec!["Cache-Control: no-cache".into()], body: body.as_bytes().to_vec(),
        }
    }

    pub fn write_to(&self, stream: &mut impl Write) -> io::Result<()> {
        let head = format_response_head(self);
        stream.write_all(head.as_bytes())?;
        stream.write_all(&self.body)?;
        stream.flush()
    }
}

fn format_response_head(resp: &HttpResponse) -> String {
    let mut head = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n",
        resp.status, resp.reason, resp.content_type, resp.body.len(),
    );
    for h in &resp.extra_headers {
        head.push_str(h);
        head.push_str("\r\n");
    }
    head.push_str("\r\n");
    head
}

#[derive(Clone, Debug)]
pub enum RemoteBridgeRequest {
    Status,
    Tail { pane_id: String, lines: usize },
    Approve { pane_id: String, action: ApproveAction },
    Send { pane_id: String, text: String, enter: bool },
    Key { pane_id: String, key: RemoteKey },
    Focus { pane_id: String },
}

#[derive(Clone, Debug)]
pub enum RemoteBridgeResponse {
    Status(RemoteStatusResponse),
    Tail(PaneTailResponse),
    OkJson(String),
    NotFound(String),
    Error(String),
}

pub enum RouteOutcome {
    Response(HttpResponse),
    SseSubscribe { device_id: String, pane_id: Option<String> },
}

fn bad_req(msg: &str) -> HttpResponse {
    HttpResponse::error(400, "Bad Request", msg)
}

fn read_line_capped<R: BufRead>(reader: &mut R, out: &mut String, max: usize) -> io::Result<usize> {
    reader.take(max as u64).read_line(out)
}

pub fn read_http_request(stream: &TcpStream) -> Result<HttpRequest, HttpResponse> {
    let mut reader = BufReader::new(stream);
    let mut req_line = String::new();
    let n = read_line_capped(&mut reader, &mut req_line, MAX_HEADER_LINE).map_err(|_| bad_req("read error"))?;
    if n == 0 {
        return Err(bad_req("empty request"));
    }
    let (method, path, query) = parse_request_line(&req_line)?;
    let headers = read_headers(&mut reader)?;
    let body = read_body(&mut reader, &headers)?;
    Ok(HttpRequest { method, path, query, headers, body })
}

fn parse_request_line(line: &str) -> Result<(String, String, String), HttpResponse> {
    let mut parts = line.trim().split_whitespace();
    let method = parts.next().ok_or_else(|| bad_req("missing method"))?;
    let target = parts.next().ok_or_else(|| bad_req("missing target"))?;
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    Ok((method.to_ascii_uppercase(), path.to_owned(), query.to_owned()))
}

fn read_headers<R: BufRead>(reader: &mut R) -> Result<Vec<(String, String)>, HttpResponse> {
    let mut headers = Vec::new();
    for _ in 0..MAX_HEADERS {
        let mut line = String::new();
        read_line_capped(reader, &mut line, MAX_HEADER_LINE).map_err(|_| bad_req("malformed headers"))?;
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            return Ok(headers);
        }
        if let Some((k, v)) = trimmed.split_once(':') {
            headers.push((k.trim().to_owned(), v.trim().to_owned()));
        }
    }
    Err(HttpResponse::error(431, "Request Header Fields Too Large", "too many headers"))
}

fn content_length(headers: &[(String, String)]) -> usize {
    headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, v)| v.parse::<usize>().ok())
        .unwrap_or(0)
}

fn read_body<R: BufRead>(reader: &mut R, headers: &[(String, String)]) -> Result<Vec<u8>, HttpResponse> {
    let len = content_length(headers);
    if len > MAX_BODY_BYTES {
        return Err(HttpResponse::error(413, "Payload Too Large", "request body exceeds 32 KiB"));
    }
    let mut body = vec![0u8; len];
    if len > 0 {
        reader.read_exact(&mut body).map_err(|_| bad_req("truncated request body"))?;
    }
    Ok(body)
}

pub fn origin_matches_host(req: &HttpRequest) -> bool {
    let Some(origin) = req.header("origin") else { return true };
    let Some(host) = req.header("host") else { return false };
    let bare = origin.strip_prefix("http://").or_else(|| origin.strip_prefix("https://")).unwrap_or(origin);
    bare.trim_end_matches('/').eq_ignore_ascii_case(host.trim())
}

fn extract_bearer(req: &HttpRequest) -> Option<String> {
    let auth = req.header("authorization")?;
    let tok = auth.strip_prefix("Bearer ").or_else(|| auth.strip_prefix("bearer "))?.trim();
    (!tok.is_empty()).then(|| tok.to_owned())
}

fn extract_cookie_token(req: &HttpRequest) -> Option<String> {
    let cookie = req.header("cookie")?;
    cookie
        .split(';')
        .filter_map(|p| p.trim().split_once('='))
        .find(|(k, v)| k.trim() == "akhterm_remote" && !v.trim().is_empty())
        .map(|(_, v)| v.trim().to_owned())
}

pub fn extract_token(req: &HttpRequest) -> Option<String> {
    extract_bearer(req).or_else(|| extract_cookie_token(req))
}

fn try_static_route(req: &HttpRequest) -> Option<HttpResponse> {
    if req.method != "GET" {
        return None;
    }
    match req.path.as_str() {
        "/" => Some(HttpResponse::static_asset("text/html; charset=utf-8", PWA_INDEX_HTML)),
        "/manifest.webmanifest" => Some(HttpResponse::static_asset("application/manifest+json", PWA_MANIFEST_JSON)),
        _ => None,
    }
}

pub fn route_request<F>(
    req: &HttpRequest,
    auth: &mut RemoteAuthStore,
    host_name: &str,
    version: &str,
    on_unpair_device: impl FnOnce(&str),
    dispatch: F,
) -> RouteOutcome
where
    F: FnOnce(RemoteBridgeRequest) -> RemoteBridgeResponse,
{
    if let Some(asset) = try_static_route(req) {
        return RouteOutcome::Response(asset);
    }
    if !origin_matches_host(req) {
        return RouteOutcome::Response(HttpResponse::error(403, "Forbidden", "origin mismatch"));
    }
    if req.method == "GET" && req.path == "/api/v1/health" {
        return RouteOutcome::Response(health_response(version));
    }
    if req.method == "POST" && req.path == "/api/v1/pair" {
        return RouteOutcome::Response(handle_pair(req, auth, host_name, version));
    }
    route_authenticated(req, auth, on_unpair_device, dispatch)
}

fn health_response(version: &str) -> HttpResponse {
    let body = format_compact(&JsonVal::Object(vec![
        ("ok".into(), JsonVal::Bool(true)),
        ("api_version".into(), JsonVal::Number(API_VERSION.to_string())),
        ("version".into(), JsonVal::Str(version.to_owned())),
    ]));
    HttpResponse::json(200, "OK", body)
}

fn map_pair_err(err: PairError) -> HttpResponse {
    match err {
        PairError::RateLimited => HttpResponse::error(429, "Too Many Requests", "pairing rate limited"),
        PairError::Internal(m) => HttpResponse::error(500, "Internal Server Error", &m),
        _ => HttpResponse::error(401, "Unauthorized", "invalid or expired pairing code"),
    }
}

fn handle_pair(req: &HttpRequest, auth: &mut RemoteAuthStore, host_name: &str, version: &str) -> HttpResponse {
    let val = match parse_json_body(req) { Ok(v) => v, Err(r) => return r };
    let Some(code) = val.get("code").and_then(JsonVal::as_str) else { return bad_req("missing code") };
    let name = val.get("device_name").and_then(JsonVal::as_str).unwrap_or("Mobile Device");
    let platform = val.get("platform").and_then(JsonVal::as_str).unwrap_or("android");
    match auth.complete_pairing(code, name, platform, Instant::now()) {
        Ok((dev, token)) => pair_success_response(&dev.device_id, &token, host_name, version),
        Err(err) => map_pair_err(err),
    }
}

fn pair_success_response(device_id: &str, token: &str, host_name: &str, version: &str) -> HttpResponse {
    let body = format_compact(&JsonVal::Object(vec![
        ("device_id".into(), JsonVal::Str(device_id.to_owned())),
        ("token".into(), JsonVal::Str(token.to_owned())),
        ("host_name".into(), JsonVal::Str(host_name.to_owned())),
        ("akhterm_version".into(), JsonVal::Str(version.to_owned())),
        ("api_version".into(), JsonVal::Number(API_VERSION.to_string())),
    ]));
    let mut resp = HttpResponse::json(200, "OK", body);
    resp.extra_headers.push(format!("Set-Cookie: akhterm_remote={token}; HttpOnly; SameSite=Strict; Path=/"));
    resp
}

fn handle_unpair(auth: &mut RemoteAuthStore, raw_token: &str, device_id: &str, on_unpair: impl FnOnce(&str)) -> HttpResponse {
    auth.revoke_by_token(raw_token);
    on_unpair(device_id);
    let mut resp = HttpResponse::json(200, "OK", "{\"ok\":true}".into());
    resp.extra_headers.push("Set-Cookie: akhterm_remote=; Max-Age=0; HttpOnly; SameSite=Strict; Path=/".into());
    resp
}

fn route_authenticated<F>(
    req: &HttpRequest,
    auth: &mut RemoteAuthStore,
    on_unpair: impl FnOnce(&str),
    dispatch: F,
) -> RouteOutcome
where
    F: FnOnce(RemoteBridgeRequest) -> RemoteBridgeResponse,
{
    let Some(raw_token) = extract_token(req) else {
        return RouteOutcome::Response(HttpResponse::error(401, "Unauthorized", "unauthorized"));
    };
    let Some(device_id) = auth.verify_token(&raw_token) else {
        return RouteOutcome::Response(HttpResponse::error(401, "Unauthorized", "unauthorized"));
    };
    if req.method == "GET" && req.path == "/api/v1/events" {
        return RouteOutcome::SseSubscribe { device_id, pane_id: req.query_param("pane_id").map(str::to_owned) };
    }
    if req.method == "POST" && req.path == "/api/v1/unpair" {
        return RouteOutcome::Response(handle_unpair(auth, &raw_token, &device_id, on_unpair));
    }
    RouteOutcome::Response(route_bridge_api(req, dispatch))
}

fn route_bridge_api<F>(req: &HttpRequest, dispatch: F) -> HttpResponse
where
    F: FnOnce(RemoteBridgeRequest) -> RemoteBridgeResponse,
{
    let bridge_req = match parse_bridge_request(req) { Ok(r) => r, Err(resp) => return resp };
    match dispatch(bridge_req) {
        RemoteBridgeResponse::Status(s) => HttpResponse::json(200, "OK", encode_status_json(&s)),
        RemoteBridgeResponse::Tail(t) => HttpResponse::json(200, "OK", encode_tail_json(&t)),
        RemoteBridgeResponse::OkJson(json) => HttpResponse::json(200, "OK", json),
        RemoteBridgeResponse::NotFound(m) => HttpResponse::error(404, "Not Found", &m),
        RemoteBridgeResponse::Error(m) => HttpResponse::error(500, "Internal Server Error", &m),
    }
}

fn parse_bridge_request(req: &HttpRequest) -> Result<RemoteBridgeRequest, HttpResponse> {
    if req.method == "GET" && req.path == "/api/v1/status" {
        return Ok(RemoteBridgeRequest::Status);
    }
    let Some(rest) = req.path.strip_prefix("/api/v1/panes/") else {
        return Err(HttpResponse::error(404, "Not Found", "unknown route"));
    };
    let Some((pane_id, action)) = rest.split_once('/') else {
        return Err(HttpResponse::error(404, "Not Found", "unknown pane route"));
    };
    if pane_id.is_empty() || !pane_id.chars().all(|c| c.is_ascii_digit()) {
        return Err(bad_req("invalid pane_id"));
    }
    parse_pane_action(req, pane_id.to_owned(), action)
}

fn parse_json_body(req: &HttpRequest) -> Result<JsonVal, HttpResponse> {
    let s = std::str::from_utf8(&req.body).map_err(|_| bad_req("invalid utf-8"))?;
    parse_json(s).map_err(|_| bad_req("invalid json"))
}

fn parse_tail_action(req: &HttpRequest, pane_id: String) -> RemoteBridgeRequest {
    let lines = req.query_param("lines").and_then(|v| v.parse().ok()).unwrap_or(DEFAULT_TAIL_LINES);
    RemoteBridgeRequest::Tail { pane_id, lines: lines.clamp(1, MAX_TAIL_LINES) }
}

fn parse_approve_action(req: &HttpRequest, pane_id: String) -> Result<RemoteBridgeRequest, HttpResponse> {
    let val = parse_json_body(req)?;
    let raw = val.get("action").and_then(JsonVal::as_str).unwrap_or("");
    let action = ApproveAction::parse(raw).ok_or_else(|| bad_req("invalid approve action"))?;
    Ok(RemoteBridgeRequest::Approve { pane_id, action })
}

fn parse_send_action(req: &HttpRequest, pane_id: String) -> Result<RemoteBridgeRequest, HttpResponse> {
    let val = parse_json_body(req)?;
    let text = val.get("text").and_then(JsonVal::as_str).ok_or_else(|| bad_req("missing text"))?;
    if text.len() > MAX_SEND_BYTES {
        return Err(HttpResponse::error(413, "Payload Too Large", "text exceeds 16 KiB"));
    }
    let enter = val.get("enter").and_then(JsonVal::as_bool).unwrap_or(true);
    Ok(RemoteBridgeRequest::Send { pane_id, text: text.to_owned(), enter })
}

fn parse_key_action(req: &HttpRequest, pane_id: String) -> Result<RemoteBridgeRequest, HttpResponse> {
    let val = parse_json_body(req)?;
    let raw = val.get("key").and_then(JsonVal::as_str).unwrap_or("");
    let key = RemoteKey::parse(raw).ok_or_else(|| bad_req("invalid key"))?;
    Ok(RemoteBridgeRequest::Key { pane_id, key })
}

fn parse_pane_action(req: &HttpRequest, pane_id: String, action: &str) -> Result<RemoteBridgeRequest, HttpResponse> {
    match (req.method.as_str(), action) {
        ("GET", "tail") => Ok(parse_tail_action(req, pane_id)),
        ("POST", "approve") => parse_approve_action(req, pane_id),
        ("POST", "send") => parse_send_action(req, pane_id),
        ("POST", "key") => parse_key_action(req, pane_id),
        ("POST", "focus") => Ok(RemoteBridgeRequest::Focus { pane_id }),
        _ => Err(HttpResponse::error(404, "Not Found", "unknown pane action")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn make_req(method: &str, path: &str, headers: Vec<(&str, &str)>, body: &str) -> HttpRequest {
        let (p, q) = path.split_once('?').unwrap_or((path, ""));
        HttpRequest {
            method: method.into(), path: p.into(), query: q.into(),
            headers: headers.into_iter().map(|(k, v)| (k.into(), v.into())).collect(),
            body: body.as_bytes().to_vec(),
        }
    }

    fn temp_auth(tag: &str) -> RemoteAuthStore {
        let path = PathBuf::from(format!("/tmp/akhterm-http-test-{tag}-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&path);
        RemoteAuthStore::load(path)
    }

    #[test]
    fn rejects_cross_origin_and_pairs_cleanly() {
        let mut auth = temp_auth("flow");
        let code = auth.start_pairing(Instant::now()).unwrap();
        let bad = make_req("GET", "/api/v1/health", vec![("Host", "127.0.0.1:7474"), ("Origin", "https://evil.test")], "");
        assert!(matches!(route_request(&bad, &mut auth, "h", "1.5.1", |_| {}, |_| unreachable!()), RouteOutcome::Response(r) if r.status == 403));

        let pair = make_req("POST", "/api/v1/pair", vec![("Host", "127.0.0.1:7474")],
            &format!("{{\"code\":\"{code}\",\"device_name\":\"Pixel\",\"platform\":\"android\"}}"));
        let RouteOutcome::Response(r) = route_request(&pair, &mut auth, "h", "1.5.1", |_| {}, |_| unreachable!()) else { panic!() };
        assert_eq!(r.status, 200);
        let tok = parse_json(std::str::from_utf8(&r.body).unwrap()).unwrap().get("token").unwrap().as_str().unwrap().to_owned();
        let key_req = make_req("POST", "/api/v1/panes/3/key", vec![("Authorization", &format!("Bearer {tok}"))], "{\"key\":\"ctrl_c\"}");
        let out = route_request(&key_req, &mut auth, "h", "1.5.1", |_| {}, |_| RemoteBridgeResponse::OkJson("{\"ok\":true}".into()));
        assert!(matches!(out, RouteOutcome::Response(resp) if resp.status == 200));
    }

    #[test]
    fn enforces_auth_and_sse_subscription() {
        let mut auth = temp_auth("sse");
        let unauth = make_req("GET", "/api/v1/status", vec![], "");
        assert!(matches!(route_request(&unauth, &mut auth, "h", "1.5.1", |_| {}, |_| unreachable!()), RouteOutcome::Response(r) if r.status == 401));
        let code = auth.start_pairing(Instant::now()).unwrap();
        let (_, tok) = auth.complete_pairing(&code, "Phone", "android", Instant::now()).unwrap();
        let sse = make_req("GET", "/api/v1/events?pane_id=7", vec![("Cookie", &format!("akhterm_remote={tok}"))], "");
        assert!(matches!(route_request(&sse, &mut auth, "h", "1.5.1", |_| {}, |_| unreachable!()), RouteOutcome::SseSubscribe { pane_id: Some(ref p), .. } if p == "7"));
    }
}
