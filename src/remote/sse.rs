//! Server-Sent Events (SSE) broadcaster for AkhTerm Remote API v1.

use std::io::Write;
use std::net::{Shutdown, TcpStream};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const MAX_SSE_CLIENTS: usize = 16;
const SSE_WRITE_TIMEOUT: Duration = Duration::from_secs(2);

struct SseClient {
    device_id: String,
    pane_id: Option<String>,
    stream: TcpStream,
}

struct HubState {
    next_id: u64,
    clients: Vec<SseClient>,
}

pub struct SseHub {
    inner: Mutex<HubState>,
}

impl Default for SseHub {
    fn default() -> Self {
        Self::new()
    }
}

pub fn format_sse_frame(event_name: &str, json_data: &str) -> String {
    let mut out = String::with_capacity(event_name.len() + json_data.len() + 20);
    out.push_str("event: ");
    out.push_str(event_name);
    out.push('\n');
    for line in json_data.lines() {
        out.push_str("data: ");
        out.push_str(line);
        out.push('\n');
    }
    out.push('\n');
    out
}

pub fn sse_handshake_headers() -> &'static str {
    concat!(
        "HTTP/1.1 200 OK\r\n",
        "Content-Type: text/event-stream; charset=utf-8\r\n",
        "Cache-Control: no-cache\r\n",
        "Connection: keep-alive\r\n",
        "X-Accel-Buffering: no\r\n\r\n",
    )
}

impl SseHub {
    pub fn new() -> Self {
        Self { inner: Mutex::new(HubState { next_id: 1, clients: Vec::new() }) }
    }

    pub fn add_client(
        &self,
        mut stream: TcpStream,
        device_id: String,
        pane_id: Option<String>,
        initial_snapshot: Option<&str>,
    ) -> Result<u64, String> {
        let _ = stream.set_write_timeout(Some(SSE_WRITE_TIMEOUT));
        let _ = stream.set_nodelay(true);
        stream.write_all(sse_handshake_headers().as_bytes()).map_err(|e| e.to_string())?;
        if let Some(snap) = initial_snapshot {
            let frame = format_sse_frame("snapshot", snap);
            stream.write_all(frame.as_bytes()).map_err(|e| e.to_string())?;
        }
        let _ = stream.flush();

        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if guard.clients.len() >= MAX_SSE_CLIENTS {
            return Err("too many SSE clients".to_owned());
        }
        let id = guard.next_id;
        guard.next_id += 1;
        guard.clients.push(SseClient { device_id, pane_id, stream });
        Ok(id)
    }

    pub fn client_count(&self) -> usize {
        self.inner.lock().map_or(0, |g| g.clients.len())
    }

    pub fn watched_pane_ids(&self) -> Vec<String> {
        let Ok(guard) = self.inner.lock() else { return Vec::new() };
        let mut ids = Vec::new();
        for c in &guard.clients {
            if let Some(pid) = c.pane_id.as_ref() {
                if !ids.contains(pid) {
                    ids.push(pid.clone());
                }
            }
        }
        ids
    }

    pub fn broadcast_all(&self, event_name: &str, json_data: &str) {
        let frame = format_sse_frame(event_name, json_data);
        let bytes = frame.as_bytes();
        let Ok(mut guard) = self.inner.lock() else { return };
        guard.clients.retain_mut(|c| write_frame(&mut c.stream, bytes));
    }

    pub fn broadcast_pane_tail(&self, pane_id: &str, json_data: &str) {
        let frame = format_sse_frame("pane_tail", json_data);
        let bytes = frame.as_bytes();
        let Ok(mut guard) = self.inner.lock() else { return };
        guard.clients.retain_mut(|c| {
            if c.pane_id.as_deref() == Some(pane_id) {
                write_frame(&mut c.stream, bytes)
            } else {
                true
            }
        });
    }

    pub fn send_heartbeat(&self) {
        let ts = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
        self.broadcast_all("heartbeat", &format!("{{\"ts\":{ts}}}"));
    }

    pub fn disconnect_device(&self, device_id: &str) {
        let Ok(mut guard) = self.inner.lock() else { return };
        guard.clients.retain(|c| {
            if c.device_id == device_id {
                let _ = c.stream.shutdown(Shutdown::Both);
                false
            } else {
                true
            }
        });
    }

    pub fn close_all(&self) {
        let Ok(mut guard) = self.inner.lock() else { return };
        for c in guard.clients.drain(..) {
            let _ = c.stream.shutdown(Shutdown::Both);
        }
    }
}

fn write_frame(stream: &mut TcpStream, bytes: &[u8]) -> bool {
    stream.write_all(bytes).and_then(|()| stream.flush()).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use std::net::TcpListener;

    #[test]
    fn sse_hub_streams_initial_snapshot_and_broadcasts_events() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let mut client = TcpStream::connect(addr).unwrap();
        let _ = client.set_read_timeout(Some(Duration::from_secs(2)));
        let (server_stream, _) = listener.accept().unwrap();

        let hub = SseHub::new();
        hub.add_client(server_stream, "dev-1".into(), Some("5".into()), Some("{\"ok\":1}")).unwrap();
        assert_eq!(hub.client_count(), 1);
        assert_eq!(hub.watched_pane_ids(), vec!["5".to_owned()]);

        hub.broadcast_pane_tail("5", "{\"pane_id\":\"5\"}");
        hub.broadcast_all("alert", "{\"state\":\"waiting\"}");
        hub.disconnect_device("dev-1");
        assert_eq!(hub.client_count(), 0);

        let mut raw = String::new();
        let _ = client.read_to_string(&mut raw);
        assert!(raw.contains("text/event-stream"));
        assert!(raw.contains("event: snapshot\ndata: {\"ok\":1}\n\n"));
        assert!(raw.contains("event: pane_tail\ndata: {\"pane_id\":\"5\"}\n\n"));
        assert!(raw.contains("event: alert\ndata: {\"state\":\"waiting\"}\n\n"));
    }
}
