package org.akhterm.mobile.data.model

import kotlinx.coroutines.test.runTest
import kotlinx.serialization.encodeToString
import okhttp3.mockwebserver.MockResponse
import okhttp3.mockwebserver.MockWebServer
import org.akhterm.mobile.data.network.ApiException
import org.akhterm.mobile.data.network.RemoteApiClient
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class RemoteProtocolTest {

    @Test
    fun parsesExactStatusAndPaneFixtureFromSpec() {
        val json = """
            {
              "api_version": 1,
              "version": "1.5.1",
              "host_name": "pavel-laptop",
              "desktop_focused": false,
              "mute_when_focused": true,
              "active_tab_id": 1,
              "panes": [
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
              ]
            }
        """.trimIndent()

        val parsed = RemoteJson.decodeFromString(RemoteStatusResponse.serializer(), json)
        assertEquals(1, parsed.apiVersion)
        assertEquals("1.5.1", parsed.version)
        assertEquals("pavel-laptop", parsed.hostName)
        assertFalse(parsed.desktopFocused)
        assertTrue(parsed.muteWhenFocused)
        assertEquals(1L, parsed.activeTabId)
        assertEquals(1, parsed.panes.size)

        val pane = parsed.panes.first()
        assertEquals("1", pane.paneId)
        assertEquals(1L, pane.tabId)
        assertEquals("akhterm-rs", pane.tabTitle)
        assertTrue(pane.activeTab)
        assertTrue(pane.focusedPane)
        assertEquals("Claude Code", pane.agent)
        assertEquals("akhterm-rs", pane.repoName)
        assertEquals("feature/117-remote-bridge", pane.branchBadge)
        assertFalse(pane.isWorktree)
        assertNull(pane.sshHost)
        assertEquals(PaneState.WAITING, pane.state)
        assertEquals(42L, pane.elapsedSecs)
        assertEquals("ctx 40k · cache 80%", pane.contextBadge)
        assertTrue(pane.unread)
        assertEquals(3, pane.previewLines.size)
        assertEquals("❯ Yes", pane.previewLines[1])
        assertEquals("Run `cargo test --offline --locked`?", pane.approvalSnippet)
        assertEquals(ApprovalKind.TUI_MENU, pane.approvalKind)
    }

    @Test
    fun parsesExactPairFixtureAndEncodesPairRequest() {
        val reqJson = RemoteJson.encodeToString(
            PairRequest(code = "0123456789abcdef0123456789abcdef", deviceName = "Pixel 9"),
        )
        assertTrue(reqJson.contains("\"code\":\"0123456789abcdef0123456789abcdef\""))
        assertTrue(reqJson.contains("\"device_name\":\"Pixel 9\""))
        assertTrue(reqJson.contains("\"platform\":\"android\""))

        val respJson = """
            {
              "device_id": "dev-a1b2c3d4",
              "token": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
              "host_name": "pavel-laptop",
              "akhterm_version": "1.5.1",
              "api_version": 1
            }
        """.trimIndent()
        val parsed = RemoteJson.decodeFromString(PairResponse.serializer(), respJson)
        assertEquals("dev-a1b2c3d4", parsed.deviceId)
        assertEquals(64, parsed.token.length)
        assertEquals("pavel-laptop", parsed.hostName)
        assertEquals("1.5.1", parsed.akhtermVersion)
        assertEquals(1, parsed.apiVersion)
    }

    @Test
    fun parsesExactPaneTailFixtureFromSpec() {
        val json = """
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
        """.trimIndent()
        val parsed = RemoteJson.decodeFromString(PaneTailResponse.serializer(), json)
        assertEquals("1", parsed.paneId)
        assertEquals(17L, parsed.seq)
        assertEquals(PaneState.WAITING, parsed.state)
        assertEquals("Run `cargo test`?", parsed.approvalSnippet)
        assertEquals(ApprovalKind.TUI_MENU, parsed.approvalKind)
        assertEquals(4, parsed.lines.size)
        assertEquals("Compiling akhterm v1.5.1", parsed.lines.first())
    }

    @Test
    fun parsesExactAlertEventFixtureFromSpec() {
        val json = """
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
        """.trimIndent()
        val parsed = RemoteJson.decodeFromString(AlertEvent.serializer(), json)
        assertEquals("evt-42", parsed.eventId)
        assertEquals("1", parsed.paneId)
        assertEquals("akhterm-rs", parsed.tabTitle)
        assertEquals("Claude Code", parsed.agent)
        assertEquals("akhterm-rs", parsed.repoName)
        assertEquals(PaneState.WAITING, parsed.state)
        assertEquals("Claude Code · Needs Approval", parsed.title)
        assertEquals("Run `cargo test`?", parsed.body)
        assertEquals(ApprovalKind.TUI_MENU, parsed.approvalKind)
        assertFalse(parsed.desktopFocused)
        assertTrue(parsed.shouldNotify)
    }

    @Test
    fun serializesAllApprovalActionsAndTuiKeysToExactWireValues() {
        val expectedActions = mapOf(
            ApprovalAction.YES to "\"yes\"",
            ApprovalAction.YES_SAVE to "\"yes_save\"",
            ApprovalAction.NO to "\"no\"",
            ApprovalAction.ENTER to "\"enter\"",
        )
        expectedActions.forEach { (action, wire) ->
            assertEquals("{\"action\":$wire}", RemoteJson.encodeToString(ApproveRequest(action)))
        }

        val expectedKeys = mapOf(
            TuiKey.ENTER to "\"enter\"",
            TuiKey.ESC to "\"esc\"",
            TuiKey.UP to "\"up\"",
            TuiKey.DOWN to "\"down\"",
            TuiKey.TAB to "\"tab\"",
            TuiKey.CTRL_C to "\"ctrl_c\"",
            TuiKey.KEY_1 to "\"1\"",
            TuiKey.KEY_2 to "\"2\"",
            TuiKey.KEY_3 to "\"3\"",
        )
        expectedKeys.forEach { (key, wire) ->
            assertEquals("{\"key\":$wire}", RemoteJson.encodeToString(SendKeyRequest(key)))
        }
    }

    @Test
    fun ignoresUnknownFieldsForForwardCompatibility() {
        val json = """{ "ok": true, "api_version": 1, "version": "1.6.0", "future_field": "ignored" }"""
        val health = RemoteJson.decodeFromString(HealthResponse.serializer(), json)
        assertTrue(health.ok)
        assertEquals(1, health.apiVersion)
        assertEquals("1.6.0", health.version)
    }

    @Test
    fun remoteApiClientSendsExpectedPathsHeadersAndPayloads() = runTest {
        val server = MockWebServer()
        server.start()
        try {
            val baseUrl = server.url("/").toString().trimEnd('/')
            val client = RemoteApiClient()
            val creds = PairedCredentials(
                baseUrl = baseUrl,
                deviceId = "dev-1",
                token = "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789",
                hostName = "test-host",
            )

            // Health
            server.enqueue(MockResponse().setBody("""{"ok":true,"api_version":1,"version":"1.5.1"}"""))
            val health = client.health(baseUrl).getOrThrow()
            assertTrue(health.ok)
            val healthReq = server.takeRequest()
            assertEquals("/api/v1/health", healthReq.path)
            assertNull(healthReq.getHeader("Authorization"))

            // Approve
            server.enqueue(MockResponse().setBody("""{"ok":true,"pane_id":"7","action":"yes_save"}"""))
            val approve = client.approve(creds, "7", ApprovalAction.YES_SAVE).getOrThrow()
            assertEquals(ApprovalAction.YES_SAVE, approve.action)
            val approveReq = server.takeRequest()
            assertEquals("/api/v1/panes/7/approve", approveReq.path)
            assertEquals("Bearer ${creds.token}", approveReq.getHeader("Authorization"))
            assertEquals("""{"action":"yes_save"}""", approveReq.body.readUtf8())

            // Send prompt
            server.enqueue(MockResponse().setBody("""{"ok":true,"pane_id":"7","bytes_sent":23}"""))
            val sent = client.sendPrompt(creds, "7", "Fix the clippy warning", enter = true).getOrThrow()
            assertEquals(23, sent.bytesSent)
            val sendReq = server.takeRequest()
            assertEquals("/api/v1/panes/7/send", sendReq.path)
            assertEquals("""{"text":"Fix the clippy warning","enter":true}""", sendReq.body.readUtf8())

            // Send key
            server.enqueue(MockResponse().setBody("""{"ok":true,"pane_id":"7","key":"ctrl_c"}"""))
            val keyResp = client.sendKey(creds, "7", TuiKey.CTRL_C).getOrThrow()
            assertEquals(TuiKey.CTRL_C, keyResp.key)
            val keyReq = server.takeRequest()
            assertEquals("/api/v1/panes/7/key", keyReq.path)
            assertEquals("""{"key":"ctrl_c"}""", keyReq.body.readUtf8())

            // 401 error response parsing
            server.enqueue(
                MockResponse()
                    .setResponseCode(401)
                    .setBody("""{"error":"token revoked"}"""),
            )
            val errResult = client.getStatus(creds)
            assertTrue(errResult.isFailure)
            val apiEx = errResult.exceptionOrNull() as ApiException
            assertEquals(401, apiEx.httpCode)
            assertEquals("token revoked", apiEx.message)
        } finally {
            server.shutdown()
        }
    }
}
