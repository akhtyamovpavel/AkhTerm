# AkhTerm Remote API v1

Frozen HTTP/1.1 JSON + Server-Sent Events (SSE) contract shared between:
- **AkhTerm Desktop** (`src/remote/`, zero external Cargo crates, built-in optional server + embedded Web/PWA UI at `/`)
- **AkhTerm Mobile** (`android/`, native Kotlin + Jetpack Compose companion app)

All JSON responses are UTF-8 (`Content-Type: application/json; charset=utf-8`). All error responses use:

```json
{ "error": "human-readable message" }
```

---

## 1. QR Pairing Payload

Desktop renders a single QR code that works with both the AkhTerm Android QR scanner and standard mobile cameras (opening the embedded PWA):

```text
http://<ip-or-host>:<port>/?pair=<32-hex-code>&host=<url-encoded-host-name>&v=1
```

Clients MUST also accept the deep-link scheme form:

```text
akhterm://pair?v=1&url=<url-encoded-base-url>&code=<32-hex-code>&host=<url-encoded-host-name>
```

The pairing code is a single-use 128-bit random hex string valid for **120 seconds**.

---

## 2. Authentication & Security Headers

- `GET /`, `GET /manifest.webmanifest`, `GET /api/v1/health`, and `POST /api/v1/pair` do not require a Bearer token.
- Every other `/api/v1/*` endpoint requires either:
  - `Authorization: Bearer <64-hex-token>`, OR
  - `Cookie: akhterm_remote=<64-hex-token>` (set by `/api/v1/pair` as `HttpOnly; SameSite=Strict; Path=/` for the embedded PWA).
- Token validation uses constant-time comparison against stored SHA-256 digests (`~/.local/state/akhterm/remote-devices.json`, mode `0600`).
- Cross-origin protection: if an `Origin` header is present on `/api/v1/*`, its `host[:port]` MUST match the HTTP `Host` header; otherwise respond `403 Forbidden`.
- Rate limiting on `POST /api/v1/pair`: after 5 invalid attempts within 60 seconds, return `429 Too Many Requests` and rotate/invalidate the active pairing code.
- Request body size limit: `32 KiB` (`413 Payload Too Large` if exceeded).

---

## 3. Data Models

### `RemotePaneSummary`

```json
{
  "pane_id": "1",
  "tab_id": 1,
  "tab_title": "akhterm-rs",
  "active_tab": true,
  "focused_pane": true,
  "agent": "Claude Code",
  "repo_name": "akhterm-rs",
  "branch_badge": "feature/117-remote-bridge",
  "is_worktree": false,
  "ssh_host": null,
  "state": "waiting",
  "elapsed_secs": 42,
  "context_badge": "ctx 40k · cache 80%",
  "unread": true,
  "preview_lines": [
    "Run `cargo test --offline --locked`?",
    "❯ Yes",
    "  No"
  ],
  "approval_snippet": "Run `cargo test --offline --locked`?",
  "approval_kind": "tui_menu"
}
```

Field definitions:
- `pane_id`: string decimal pane identifier (matches `AKHTERM_PANE_ID`).
- `tab_id`: unsigned integer session/tab id.
- `tab_title`: string tab title (UTF-8 / Cyrillic safe).
- `active_tab`: boolean, whether this tab is currently active on desktop.
- `focused_pane`: boolean, whether this pane is focused inside the active tab.
- `agent`: string agent label (`"Claude Code"`, `"Codex"`, `"OpenCode"`, `"AGY"`, `"DeepSeek"`, `"Kimi"`, or `"shell"`).
- `repo_name`: string repository or directory basename.
- `branch_badge`: string git branch badge (empty string `""` if not in a git repo).
- `is_worktree`: boolean, true for linked `git worktree` checkouts.
- `ssh_host`: nullable string SSH host alias when remote.
- `state`: enum string: `"waiting" | "working" | "done" | "idle" | "error"`.
- `elapsed_secs`: nullable unsigned integer turn duration in seconds.
- `context_badge`: nullable string token/context indicator (e.g. `"ctx 40k · cache 80%"`).
- `unread`: boolean.
- `preview_lines`: array of up to 3 trimmed non-empty tail lines.
- `approval_snippet`: nullable string describing the active approval prompt.
- `approval_kind`: nullable enum string: `"yes_no" | "tui_menu" | "enter_confirm"`.

### `RemoteStatusResponse`

```json
{
  "api_version": 1,
  "version": "1.5.1",
  "host_name": "pavel-laptop",
  "desktop_focused": false,
  "mute_when_focused": true,
  "active_tab_id": 1,
  "panes": []
}
```

### `PaneTailResponse`

```json
{
  "pane_id": "1",
  "seq": 17,
  "state": "waiting",
  "approval_snippet": "Run `cargo test`?",
  "approval_kind": "tui_menu",
  "lines": [
    "Compiling akhterm v1.5.1",
    "Run `cargo test`?",
    "❯ Yes",
    "  No"
  ]
}
```

### `AlertEvent`

```json
{
  "event_id": "evt-42",
  "pane_id": "1",
  "tab_title": "akhterm-rs",
  "agent": "Claude Code",
  "repo_name": "akhterm-rs",
  "state": "waiting",
  "title": "Claude Code · Needs Approval",
  "body": "Run `cargo test`?",
  "approval_kind": "tui_menu",
  "desktop_focused": false,
  "should_notify": true
}
```

`should_notify` is `false` when `mute_when_focused == true && desktop_focused == true`.

---

## 4. REST Endpoints

### `GET /` and `GET /manifest.webmanifest`
Serves the embedded Dark/Light Zinc Web/PWA UI and its Web App Manifest.

### `GET /api/v1/health`
Unauthenticated health check.
- `200 OK`:
  ```json
  { "ok": true, "api_version": 1, "version": "1.5.1" }
  ```

### `POST /api/v1/pair`
Exchanges a 1-time pairing code for a per-device Bearer token.
- Request:
  ```json
  { "code": "<32-hex>", "device_name": "Pixel 9", "platform": "android" }
  ```
- `200 OK` (also sets `Set-Cookie: akhterm_remote=<token>; HttpOnly; SameSite=Strict; Path=/`):
  ```json
  {
    "device_id": "dev-a1b2c3d4",
    "token": "<64-hex>",
    "host_name": "pavel-laptop",
    "akhterm_version": "1.5.1",
    "api_version": 1
  }
  ```
- Errors: `400`, `401`, `429`.

### `GET /api/v1/status`
Authenticated. Returns `RemoteStatusResponse`.

### `GET /api/v1/panes/{id}/tail?lines=100`
Authenticated. `lines` is clamped to `1..=200` (default `100`). Returns `PaneTailResponse`, or `404` if `{id}` does not exist.

### `POST /api/v1/panes/{id}/approve`
Authenticated. Sends an approval action to pane `{id}`.
- Request:
  ```json
  { "action": "yes" }
  ```
  Valid `action` values: `"yes" | "yes_save" | "no" | "enter"`.
- Byte mapping on desktop:
  - `"enter"` -> `\r`
  - `"yes"` -> `y\r` when `approval_kind == "yes_no"`, otherwise `\r`
  - `"yes_save"` -> `\x1b[B\r` (arrow-down + Enter for 2nd option in TUI approval menus)
  - `"no"` -> `n\r` when `approval_kind == "yes_no"`, otherwise `\x1b` (`Esc` to reject/cancel TUI prompt safely)
- `200 OK`:
  ```json
  { "ok": true, "pane_id": "1", "action": "yes" }
  ```

### `POST /api/v1/panes/{id}/send`
Authenticated. Sends prompt text (and optional Enter `\r`) into pane `{id}`.
- Request:
  ```json
  { "text": "Fix the clippy warning", "enter": true }
  ```
  `text` max length: `16384` UTF-8 bytes.
- `200 OK`:
  ```json
  { "ok": true, "pane_id": "1", "bytes_sent": 23 }
  ```

### `POST /api/v1/panes/{id}/key`
Authenticated. Sends a single TUI navigation/control key to pane `{id}`.
- Request:
  ```json
  { "key": "ctrl_c" }
  ```
  Valid `key` values and byte mappings:
  - `"enter"` -> `\r`
  - `"esc"` -> `\x1b`
  - `"up"` -> `\x1b[A`
  - `"down"` -> `\x1b[B`
  - `"tab"` -> `\t`
  - `"ctrl_c"` -> `\x03`
  - `"1"` -> `1`
  - `"2"` -> `2`
  - `"3"` -> `3`
- `200 OK`:
  ```json
  { "ok": true, "pane_id": "1", "key": "ctrl_c" }
  ```

### `POST /api/v1/panes/{id}/focus`
Authenticated. Focuses pane `{id}` in the desktop AkhTerm window.
- `200 OK`: `{ "ok": true, "pane_id": "1" }`

### `POST /api/v1/unpair`
Authenticated. Revokes the caller's own device token immediately.
- `200 OK`: `{ "ok": true }`

---

## 5. Server-Sent Events (`GET /api/v1/events`)

Authenticated. Optional query parameter: `?pane_id=<id>` to additionally subscribe to live `pane_tail` updates for one pane.

Headers:
```http
HTTP/1.1 200 OK
Content-Type: text/event-stream; charset=utf-8
Cache-Control: no-cache
Connection: keep-alive
X-Accel-Buffering: no
```

Event frames (standard SSE `event: <type>\ndata: <compact-json>\n\n`):
1. `snapshot` — `RemoteStatusResponse`. Sent immediately upon connection and whenever any pane status, preview, unread flag, tab list, or desktop focus changes.
2. `pane_tail` — `PaneTailResponse`. Sent when `?pane_id=<id>` is specified and the pane's captured tail changes.
3. `alert` — `AlertEvent`. Sent on transitions into `"waiting"`, `"done"`, or `"error"`.
4. `heartbeat` — `{ "ts": <unix-epoch-secs> }` every **15 seconds**.
