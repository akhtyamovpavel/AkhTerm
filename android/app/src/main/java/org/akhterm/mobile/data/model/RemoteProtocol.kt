package org.akhterm.mobile.data.model

import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.Json

val RemoteJson: Json = Json {
    ignoreUnknownKeys = true
    encodeDefaults = true
    explicitNulls = false
}

@Serializable
data class ErrorResponse(
    val error: String,
)

@Serializable
data class HealthResponse(
    val ok: Boolean,
    @SerialName("api_version") val apiVersion: Int,
    val version: String,
)

@Serializable
data class PairRequest(
    val code: String,
    @SerialName("device_name") val deviceName: String,
    val platform: String = "android",
)

@Serializable
data class PairResponse(
    @SerialName("device_id") val deviceId: String,
    val token: String,
    @SerialName("host_name") val hostName: String,
    @SerialName("akhterm_version") val akhtermVersion: String,
    @SerialName("api_version") val apiVersion: Int,
)

@Serializable
enum class PaneState {
    @SerialName("waiting")
    WAITING,

    @SerialName("working")
    WORKING,

    @SerialName("done")
    DONE,

    @SerialName("idle")
    IDLE,

    @SerialName("error")
    ERROR,
}

@Serializable
enum class ApprovalKind {
    @SerialName("yes_no")
    YES_NO,

    @SerialName("tui_menu")
    TUI_MENU,

    @SerialName("enter_confirm")
    ENTER_CONFIRM,
}

@Serializable
enum class ApprovalAction {
    @SerialName("yes")
    YES,

    @SerialName("yes_save")
    YES_SAVE,

    @SerialName("no")
    NO,

    @SerialName("enter")
    ENTER,
}

@Serializable
enum class TuiKey {
    @SerialName("enter")
    ENTER,

    @SerialName("esc")
    ESC,

    @SerialName("up")
    UP,

    @SerialName("down")
    DOWN,

    @SerialName("tab")
    TAB,

    @SerialName("ctrl_c")
    CTRL_C,

    @SerialName("1")
    KEY_1,

    @SerialName("2")
    KEY_2,

    @SerialName("3")
    KEY_3,
}

@Serializable
data class RemotePaneSummary(
    @SerialName("pane_id") val paneId: String,
    @SerialName("tab_id") val tabId: Long,
    @SerialName("tab_title") val tabTitle: String,
    @SerialName("active_tab") val activeTab: Boolean,
    @SerialName("focused_pane") val focusedPane: Boolean,
    val agent: String,
    @SerialName("repo_name") val repoName: String,
    @SerialName("branch_badge") val branchBadge: String = "",
    @SerialName("is_worktree") val isWorktree: Boolean = false,
    @SerialName("ssh_host") val sshHost: String? = null,
    val state: PaneState,
    @SerialName("elapsed_secs") val elapsedSecs: Long? = null,
    @SerialName("context_badge") val contextBadge: String? = null,
    val unread: Boolean = false,
    @SerialName("preview_lines") val previewLines: List<String> = emptyList(),
    @SerialName("approval_snippet") val approvalSnippet: String? = null,
    @SerialName("approval_kind") val approvalKind: ApprovalKind? = null,
)

@Serializable
data class RemoteStatusResponse(
    @SerialName("api_version") val apiVersion: Int,
    val version: String,
    @SerialName("host_name") val hostName: String,
    @SerialName("desktop_focused") val desktopFocused: Boolean,
    @SerialName("mute_when_focused") val muteWhenFocused: Boolean,
    @SerialName("active_tab_id") val activeTabId: Long? = null,
    val panes: List<RemotePaneSummary> = emptyList(),
)

@Serializable
data class PaneTailResponse(
    @SerialName("pane_id") val paneId: String,
    val seq: Long,
    val state: PaneState,
    @SerialName("approval_snippet") val approvalSnippet: String? = null,
    @SerialName("approval_kind") val approvalKind: ApprovalKind? = null,
    val lines: List<String> = emptyList(),
)

@Serializable
data class AlertEvent(
    @SerialName("event_id") val eventId: String,
    @SerialName("pane_id") val paneId: String,
    @SerialName("tab_title") val tabTitle: String,
    val agent: String,
    @SerialName("repo_name") val repoName: String,
    val state: PaneState,
    val title: String,
    val body: String,
    @SerialName("approval_kind") val approvalKind: ApprovalKind? = null,
    @SerialName("desktop_focused") val desktopFocused: Boolean,
    @SerialName("should_notify") val shouldNotify: Boolean,
)

@Serializable
data class HeartbeatEvent(
    val ts: Long,
)

@Serializable
data class ApproveRequest(
    val action: ApprovalAction,
)

@Serializable
data class ApproveResponse(
    val ok: Boolean,
    @SerialName("pane_id") val paneId: String,
    val action: ApprovalAction,
)

@Serializable
data class SendPromptRequest(
    val text: String,
    val enter: Boolean = true,
)

@Serializable
data class SendPromptResponse(
    val ok: Boolean,
    @SerialName("pane_id") val paneId: String,
    @SerialName("bytes_sent") val bytesSent: Int,
)

@Serializable
data class SendKeyRequest(
    val key: TuiKey,
)

@Serializable
data class SendKeyResponse(
    val ok: Boolean,
    @SerialName("pane_id") val paneId: String,
    val key: TuiKey,
)

@Serializable
data class FocusResponse(
    val ok: Boolean,
    @SerialName("pane_id") val paneId: String,
)

@Serializable
data class UnpairResponse(
    val ok: Boolean,
)

@Serializable
data class PairedCredentials(
    val baseUrl: String,
    val deviceId: String,
    val token: String,
    val hostName: String,
)

enum class ConnectionState {
    DISCONNECTED,
    CONNECTING,
    CONNECTED,
    RECONNECTING,
    UNAUTHORIZED,
    ERROR,
}

enum class ThemeMode {
    SYSTEM,
    DARK,
    LIGHT,
}
