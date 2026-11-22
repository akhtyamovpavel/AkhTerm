//! End-to-end headless Broadway GTK + real TCP HTTP/SSE + VTE integration tests for AkhTerm Remote.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use crate::app_state;
use crate::ffi::gtk_widget_show_all;
use crate::pane_registry::PaneId;
use crate::proc_radar::tests::{pump_gtk_events, with_headless_gtk};
use crate::remote::http::{RemoteBridgeRequest as Req, RemoteBridgeResponse as Resp};
use crate::remote::qr::encode_qr;
use crate::remote::server::RemoteServer;
use crate::remote::wire;
use crate::session::SessionCategory;
use crate::session_ops;
use crate::sys::json_lite::parse_json;
use crate::tabs::Tab;
use crate::tests::{cleanup_test_window, install_test_window};

fn send_http(port: u16, method: &str, path: &str, token: Option<&str>, body: &str) -> (u16, String) {
    let mut s = TcpStream::connect(("127.0.0.1", port)).expect("tcp connect");
    let _ = s.set_read_timeout(Some(Duration::from_secs(4)));
    let auth = token.map(|t| format!("Authorization: Bearer {t}\r\n")).unwrap_or_default();
    let req = format!("{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n{auth}Content-Length: {}\r\n\r\n{body}", body.len());
    s.write_all(req.as_bytes()).expect("write req");
    let mut raw = String::new();
    s.read_to_string(&mut raw).expect("read resp");
    let (head, payload) = raw.split_once("\r\n\r\n").unwrap_or((&raw, ""));
    let code = head.split_whitespace().nth(1).and_then(|c| c.parse().ok()).unwrap_or(0);
    (code, payload.to_owned())
}

fn run_bridged(srv: &RemoteServer, method: &'static str, path: String, token: &str, body: &str) -> (u16, String) {
    let (port, tok, b) = (srv.port(), token.to_owned(), body.to_owned());
    let handle = thread::spawn(move || send_http(port, method, &path, Some(&tok), &b));
    for _ in 0..80 {
        for pending in srv.drain_pending() {
            let resp = dispatch_test_req(pending.request.clone());
            pending.respond(resp);
        }
        pump_gtk_events();
        if handle.is_finished() { break; }
        thread::sleep(Duration::from_millis(20));
    }
    handle.join().expect("http thread")
}

fn feed_test_pane(pane_id: &str, text: &str, enter: bool) -> Resp {
    let pid = pane_id.parse::<u64>().unwrap();
    let vte = app_state::with_app(|a| a.sessions[0].pane_registry.get(PaneId::new(pid)).map(|p| p.vte)).flatten().unwrap();
    let payload = if enter { format!("{text}\r") } else { text.to_owned() };
    crate::file_drop::feed_vte_text(vte, &payload, false);
    Resp::OkJson("{\"ok\":true}".into())
}

fn dispatch_test_req(req: Req) -> Resp {
    match req {
        Req::Status => Resp::Status(app_state::with_app(wire::collect_remote_status).unwrap()),
        Req::Tail { pane_id, lines } => wire::capture_pane_tail(&pane_id, lines).map_or_else(Resp::NotFound, Resp::Tail),
        Req::Send { pane_id, text, enter } => feed_test_pane(&pane_id, &text, enter),
        Req::Key { pane_id, key } => wire::send_pane_key(&pane_id, key).map_or_else(Resp::NotFound, |()| Resp::OkJson("{\"ok\":true}".into())),
        Req::Approve { .. } | Req::Focus { .. } => Resp::OkJson("{\"ok\":true}".into()),
    }
}

fn pair_via_qr(srv: &RemoteServer) -> String {
    let (qr_url, ttl) = srv.ensure_pairing_url().unwrap();
    assert!(ttl > 0 && encode_qr(qr_url.as_bytes()).is_ok());
    let code = qr_url.split("pair=").nth(1).and_then(|r| r.split('&').next()).unwrap_or("");
    let body = format!("{{\"code\":\"{code}\",\"device_name\":\"Pixel 9\",\"platform\":\"android\"}}");
    let (st, resp) = send_http(srv.port(), "POST", "/api/v1/pair", None, &body);
    assert_eq!(st, 200);
    parse_json(&resp).unwrap().get("token").unwrap().as_str().unwrap().to_owned()
}

fn fetch_first_pane_id(srv: &RemoteServer, token: &str) -> String {
    let (st, body) = run_bridged(srv, "GET", "/api/v1/status".into(), token, "");
    assert_eq!(st, 200);
    let parsed = parse_json(&body).unwrap();
    parsed.get("panes").unwrap().as_array().unwrap()[0].get("pane_id").unwrap().as_str().unwrap().to_owned()
}

fn wait_for_tail_marker(srv: &RemoteServer, token: &str, pane_id: &str, marker: &str) {
    let path = format!("/api/v1/panes/{pane_id}/tail?lines=40");
    for _ in 0..40 {
        pump_gtk_events();
        let (st, body) = run_bridged(srv, "GET", path.clone(), token, "");
        if st == 200 && body.contains(marker) {
            return;
        }
        thread::sleep(Duration::from_millis(40));
    }
    panic!("VTE tail did not return {marker}");
}

fn run_e2e_remote_checks() {
    let store = std::env::temp_dir().join(format!("akhterm-e2e-remote-{}.json", std::process::id()));
    let _ = std::fs::remove_file(&store);
    let (win, _) = install_test_window();
    let sid = session_ops::add_tab("Проект", "", "/tmp", SessionCategory::Project, &["bash", "--noprofile", "--norc", "-i"], Tab::local("shell"));
    unsafe { gtk_widget_show_all(win) };
    pump_gtk_events();

    let srv = RemoteServer::start("127.0.0.1".into(), 0, store.clone(), Some(Arc::new(|| {}))).unwrap();
    let token = pair_via_qr(&srv);
    let pane_id = fetch_first_pane_id(&srv, &token);
    let send_path = format!("/api/v1/panes/{pane_id}/send");
    assert_eq!(run_bridged(&srv, "POST", send_path, &token, "{\"text\":\"echo Привет_Remote_OK\",\"enter\":true}").0, 200);
    wait_for_tail_marker(&srv, &token, &pane_id, "Привет_Remote_OK");
    assert_eq!(run_bridged(&srv, "POST", format!("/api/v1/panes/{pane_id}/key"), &token, "{\"key\":\"ctrl_c\"}").0, 200);
    assert_eq!(send_http(srv.port(), "POST", "/api/v1/unpair", Some(&token), "{}").0, 200);
    assert_eq!(send_http(srv.port(), "GET", "/api/v1/status", Some(&token), "").0, 401);

    drop(srv);
    cleanup_test_window(win, &[sid]);
    let _ = std::fs::remove_file(&store);
}

#[test]
fn headless_broadway_remote_pair_status_send_tail_key_and_unpair() {
    if std::env::var_os("AKHTERM_REMOTE_E2E_CHILD").is_some() {
        with_headless_gtk(run_e2e_remote_checks);
        return;
    }
    let exe = std::env::current_exe().expect("test exe");
    let test_name = "remote::remote_tests::headless_broadway_remote_pair_status_send_tail_key_and_unpair";
    let status = std::process::Command::new(exe).args(["--exact", test_name]).env("AKHTERM_REMOTE_E2E_CHILD", "1").status().expect("spawn child");
    assert!(status.success());
}
