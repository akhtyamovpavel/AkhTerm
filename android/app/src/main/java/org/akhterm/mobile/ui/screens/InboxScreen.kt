package org.akhterm.mobile.ui.screens

import android.Manifest
import android.content.pm.PackageManager
import android.os.Build
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.rounded.Check
import androidx.compose.material.icons.rounded.Close
import androidx.compose.material.icons.rounded.NotificationsActive
import androidx.compose.material.icons.rounded.Refresh
import androidx.compose.material.icons.rounded.Settings
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.material3.pulltorefresh.PullToRefreshBox
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.core.content.ContextCompat
import kotlinx.coroutines.launch
import org.akhterm.mobile.R
import org.akhterm.mobile.data.model.ApprovalAction
import org.akhterm.mobile.data.model.ConnectionState
import org.akhterm.mobile.data.model.PaneState
import org.akhterm.mobile.data.model.RemotePaneSummary
import org.akhterm.mobile.data.repository.RemoteRepository
import org.akhterm.mobile.domain.InboxGrouping
import org.akhterm.mobile.domain.InboxSectionKind
import org.akhterm.mobile.ui.components.AgentBadge
import org.akhterm.mobile.ui.components.ConnectionStatusPill
import org.akhterm.mobile.ui.components.MetaPill
import org.akhterm.mobile.ui.components.PaneStateBadge
import org.akhterm.mobile.ui.components.formatElapsed
import org.akhterm.mobile.ui.theme.LocalAkhTermColors
import org.akhterm.mobile.ui.theme.TerminalMonoStyle

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun InboxScreen(
    repository: RemoteRepository,
    onOpenPane: (String) -> Unit,
    onOpenSettings: () -> Unit,
    onRepairNeeded: () -> Unit,
) {
    val context = LocalContext.current
    val colors = LocalAkhTermColors.current
    val scope = rememberCoroutineScope()
    val snackbarHostState = remember { SnackbarHostState() }

    val credentials by repository.credentials.collectAsState()
    val connectionState by repository.connectionState.collectAsState()
    val status by repository.status.collectAsState()
    val lastError by repository.lastError.collectAsState()

    var isRefreshing by remember { mutableStateOf(false) }
    var busyPaneId by remember { mutableStateOf<String?>(null) }

    var hasNotificationPermission by remember {
        mutableStateOf(
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
                ContextCompat.checkSelfPermission(
                    context,
                    Manifest.permission.POST_NOTIFICATIONS,
                ) == PackageManager.PERMISSION_GRANTED
            } else {
                true
            },
        )
    }
    val notificationPermLauncher = rememberLauncherForActivityResult(
        contract = ActivityResultContracts.RequestPermission(),
    ) { granted ->
        hasNotificationPermission = granted
    }

    val sections = remember(status) {
        InboxGrouping.groupPanes(status?.panes.orEmpty())
    }

    fun triggerRefresh() {
        scope.launch {
            isRefreshing = true
            repository.refreshStatus().onFailure { err ->
                err.message?.let { snackbarHostState.showSnackbar(it) }
            }
            isRefreshing = false
        }
    }

    fun quickApprove(paneId: String, action: ApprovalAction) {
        scope.launch {
            busyPaneId = paneId
            repository.approve(paneId, action).onFailure { err ->
                err.message?.let { snackbarHostState.showSnackbar(it) }
            }
            busyPaneId = null
        }
    }

    Scaffold(
        containerColor = colors.background,
        snackbarHost = { SnackbarHost(snackbarHostState) },
        topBar = {
            TopAppBar(
                title = {
                    Row(
                        verticalAlignment = Alignment.CenterVertically,
                        horizontalArrangement = Arrangement.spacedBy(10.dp),
                    ) {
                        Text(
                            text = stringResource(R.string.inbox_title),
                            style = MaterialTheme.typography.titleLarge,
                            color = colors.textPrimary,
                        )
                        ConnectionStatusPill(
                            state = connectionState,
                            hostName = status?.hostName ?: credentials?.hostName,
                        )
                    }
                },
                actions = {
                    IconButton(onClick = { triggerRefresh() }) {
                        Icon(
                            imageVector = Icons.Rounded.Refresh,
                            contentDescription = stringResource(R.string.inbox_refresh),
                            tint = colors.textSecondary,
                        )
                    }
                    IconButton(onClick = onOpenSettings) {
                        Icon(
                            imageVector = Icons.Rounded.Settings,
                            contentDescription = stringResource(R.string.inbox_settings),
                            tint = colors.textSecondary,
                        )
                    }
                },
                colors = TopAppBarDefaults.topAppBarColors(
                    containerColor = colors.background,
                ),
            )
        },
    ) { padding ->
        PullToRefreshBox(
            isRefreshing = isRefreshing,
            onRefresh = { triggerRefresh() },
            modifier = Modifier
                .fillMaxSize()
                .padding(padding),
        ) {
            LazyColumn(
                modifier = Modifier.fillMaxSize(),
                contentPadding = PaddingValues(horizontal = 16.dp, vertical = 12.dp),
                verticalArrangement = Arrangement.spacedBy(10.dp),
            ) {
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU && !hasNotificationPermission) {
                    item(key = "notif_banner") {
                        Surface(
                            modifier = Modifier.fillMaxWidth(),
                            shape = RoundedCornerShape(12.dp),
                            color = colors.waiting.copy(alpha = 0.12f),
                            border = BorderStroke(1.dp, colors.waiting.copy(alpha = 0.35f)),
                        ) {
                            Row(
                                modifier = Modifier.padding(horizontal = 12.dp, vertical = 10.dp),
                                verticalAlignment = Alignment.CenterVertically,
                                horizontalArrangement = Arrangement.spacedBy(10.dp),
                            ) {
                                Icon(
                                    imageVector = Icons.Rounded.NotificationsActive,
                                    contentDescription = null,
                                    tint = colors.waiting,
                                    modifier = Modifier.size(20.dp),
                                )
                                Text(
                                    text = stringResource(R.string.inbox_notifications_banner),
                                    style = MaterialTheme.typography.bodySmall,
                                    color = colors.textPrimary,
                                    modifier = Modifier.weight(1f),
                                )
                                TextButton(
                                    onClick = {
                                        notificationPermLauncher.launch(Manifest.permission.POST_NOTIFICATIONS)
                                    },
                                ) {
                                    Text(
                                        text = stringResource(R.string.inbox_notifications_enable),
                                        color = colors.waiting,
                                    )
                                }
                            }
                        }
                    }
                }

                if (connectionState == ConnectionState.UNAUTHORIZED) {
                    item(key = "unauthorized_card") {
                        Surface(
                            modifier = Modifier.fillMaxWidth(),
                            shape = RoundedCornerShape(14.dp),
                            color = colors.error.copy(alpha = 0.10f),
                            border = BorderStroke(1.dp, colors.error.copy(alpha = 0.35f)),
                        ) {
                            Column(
                                modifier = Modifier.padding(16.dp),
                                verticalArrangement = Arrangement.spacedBy(8.dp),
                            ) {
                                Text(
                                    text = stringResource(R.string.inbox_unauthorized_title),
                                    style = MaterialTheme.typography.titleMedium,
                                    color = colors.error,
                                )
                                Text(
                                    text = stringResource(R.string.inbox_unauthorized_body),
                                    style = MaterialTheme.typography.bodySmall,
                                    color = colors.textSecondary,
                                )
                                Button(
                                    onClick = onRepairNeeded,
                                    colors = ButtonDefaults.buttonColors(containerColor = colors.error),
                                ) {
                                    Text(stringResource(R.string.inbox_repair_button))
                                }
                            }
                        }
                    }
                } else if (!lastError.isNullOrBlank() && connectionState != ConnectionState.CONNECTED) {
                    item(key = "error_banner") {
                        Surface(
                            modifier = Modifier.fillMaxWidth(),
                            shape = RoundedCornerShape(12.dp),
                            color = colors.surface,
                            border = BorderStroke(1.dp, colors.border),
                        ) {
                            Row(
                                modifier = Modifier.padding(horizontal = 12.dp, vertical = 10.dp),
                                verticalAlignment = Alignment.CenterVertically,
                                horizontalArrangement = Arrangement.spacedBy(10.dp),
                            ) {
                                Text(
                                    text = lastError.orEmpty(),
                                    style = MaterialTheme.typography.bodySmall,
                                    color = colors.textSecondary,
                                    modifier = Modifier.weight(1f),
                                )
                                TextButton(onClick = { triggerRefresh() }) {
                                    Text(stringResource(R.string.inbox_retry_button))
                                }
                            }
                        }
                    }
                }

                if (sections.isEmpty()) {
                    item(key = "empty_state") {
                        Surface(
                            modifier = Modifier
                                .fillMaxWidth()
                                .padding(top = 24.dp),
                            shape = RoundedCornerShape(16.dp),
                            color = colors.surface,
                            border = BorderStroke(1.dp, colors.border),
                        ) {
                            Column(
                                modifier = Modifier.padding(24.dp),
                                horizontalAlignment = Alignment.CenterHorizontally,
                                verticalArrangement = Arrangement.spacedBy(8.dp),
                            ) {
                                Text(
                                    text = stringResource(R.string.inbox_empty_title),
                                    style = MaterialTheme.typography.titleMedium,
                                    color = colors.textPrimary,
                                )
                                Text(
                                    text = stringResource(R.string.inbox_empty_body),
                                    style = MaterialTheme.typography.bodySmall,
                                    color = colors.textSecondary,
                                )
                            }
                        }
                    }
                } else {
                    sections.forEach { section ->
                        item(key = "header_${section.kind.name}") {
                            SectionHeader(
                                kind = section.kind,
                                count = section.panes.size,
                            )
                        }
                        items(
                            items = section.panes,
                            key = { "pane_${it.paneId}" },
                        ) { pane ->
                            PaneCard(
                                pane = pane,
                                isBusy = busyPaneId == pane.paneId,
                                onClick = { onOpenPane(pane.paneId) },
                                onQuickApprove = { quickApprove(pane.paneId, ApprovalAction.YES) },
                                onQuickReject = { quickApprove(pane.paneId, ApprovalAction.NO) },
                            )
                        }
                    }
                }

                item(key = "bottom_spacer") {
                    Spacer(Modifier.height(16.dp))
                }
            }
        }
    }
}

@Composable
private fun SectionHeader(
    kind: InboxSectionKind,
    count: Int,
) {
    val colors = LocalAkhTermColors.current
    val (labelRes, accent) = when (kind) {
        InboxSectionKind.NEEDS_APPROVAL -> R.string.section_needs_approval to colors.waiting
        InboxSectionKind.ERROR -> R.string.section_error to colors.error
        InboxSectionKind.WORKING -> R.string.section_working to colors.working
        InboxSectionKind.DONE -> R.string.section_done to colors.done
        InboxSectionKind.IDLE -> R.string.section_idle to colors.textMuted
    }
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .padding(top = 6.dp, bottom = 2.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        Box(
            modifier = Modifier
                .size(8.dp)
                .clip(CircleShape)
                .background(accent),
        )
        Text(
            text = stringResource(labelRes).uppercase(),
            style = MaterialTheme.typography.labelMedium,
            fontWeight = FontWeight.SemiBold,
            color = accent,
        )
        Text(
            text = count.toString(),
            style = MaterialTheme.typography.labelSmall,
            color = colors.textMuted,
        )
    }
}

@OptIn(ExperimentalLayoutApi::class)
@Composable
private fun PaneCard(
    pane: RemotePaneSummary,
    isBusy: Boolean,
    onClick: () -> Unit,
    onQuickApprove: () -> Unit,
    onQuickReject: () -> Unit,
) {
    val colors = LocalAkhTermColors.current
    val borderColor = when (pane.state) {
        PaneState.WAITING -> colors.waiting.copy(alpha = 0.55f)
        PaneState.ERROR -> colors.error.copy(alpha = 0.55f)
        else -> colors.border
    }

    Surface(
        modifier = Modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(14.dp))
            .clickable(onClick = onClick),
        shape = RoundedCornerShape(14.dp),
        color = colors.surface,
        border = BorderStroke(1.dp, borderColor),
    ) {
        Column(
            modifier = Modifier.padding(14.dp),
            verticalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            Row(
                modifier = Modifier.fillMaxWidth(),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.SpaceBetween,
            ) {
                Row(
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(6.dp),
                    modifier = Modifier.weight(1f),
                ) {
                    if (pane.unread) {
                        Box(
                            modifier = Modifier
                                .size(7.dp)
                                .clip(CircleShape)
                                .background(colors.working),
                        )
                    }
                    AgentBadge(agent = pane.agent)
                    PaneStateBadge(state = pane.state)
                }

                formatElapsed(pane.elapsedSecs)?.let { elapsed ->
                    Text(
                        text = elapsed,
                        style = MaterialTheme.typography.labelSmall,
                        color = colors.textMuted,
                    )
                }
            }

            Text(
                text = pane.tabTitle,
                style = MaterialTheme.typography.titleMedium,
                color = colors.textPrimary,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )

            FlowRow(
                horizontalArrangement = Arrangement.spacedBy(6.dp),
                verticalArrangement = Arrangement.spacedBy(6.dp),
            ) {
                if (pane.repoName.isNotBlank()) {
                    MetaPill(text = pane.repoName, monospace = true)
                }
                if (pane.branchBadge.isNotBlank()) {
                    MetaPill(
                        text = pane.branchBadge,
                        monospace = true,
                        accentColor = if (pane.isWorktree) colors.accent else null,
                    )
                }
                if (pane.isWorktree) {
                    MetaPill(
                        text = stringResource(R.string.worktree_badge),
                        accentColor = colors.accent,
                    )
                }
                if (!pane.sshHost.isNullOrBlank()) {
                    MetaPill(
                        text = "ssh:${pane.sshHost}",
                        monospace = true,
                        accentColor = colors.working,
                    )
                }
                if (!pane.contextBadge.isNullOrBlank()) {
                    MetaPill(text = pane.contextBadge)
                }
            }

            if (pane.previewLines.isNotEmpty()) {
                Column(
                    modifier = Modifier
                        .fillMaxWidth()
                        .clip(RoundedCornerShape(8.dp))
                        .background(colors.terminalBackground)
                        .padding(horizontal = 10.dp, vertical = 8.dp),
                    verticalArrangement = Arrangement.spacedBy(2.dp),
                ) {
                    pane.previewLines.take(3).forEach { line ->
                        Text(
                            text = line,
                            style = TerminalMonoStyle,
                            color = colors.terminalText,
                            maxLines = 1,
                            overflow = TextOverflow.Ellipsis,
                        )
                    }
                }
            }

            if (pane.state == PaneState.WAITING) {
                if (!pane.approvalSnippet.isNullOrBlank()) {
                    Text(
                        text = pane.approvalSnippet,
                        style = MaterialTheme.typography.bodySmall,
                        fontWeight = FontWeight.Medium,
                        color = colors.waiting,
                        maxLines = 2,
                        overflow = TextOverflow.Ellipsis,
                    )
                }
                Row(
                    modifier = Modifier.fillMaxWidth(),
                    horizontalArrangement = Arrangement.spacedBy(8.dp),
                ) {
                    Button(
                        onClick = onQuickApprove,
                        enabled = !isBusy,
                        colors = ButtonDefaults.buttonColors(
                            containerColor = colors.waiting,
                            contentColor = Color(0xFF09090B),
                        ),
                        contentPadding = PaddingValues(horizontal = 14.dp, vertical = 6.dp),
                        modifier = Modifier.weight(1f),
                    ) {
                        Icon(
                            imageVector = Icons.Rounded.Check,
                            contentDescription = null,
                            modifier = Modifier.size(16.dp),
                        )
                        Spacer(Modifier.size(6.dp))
                        Text(
                            text = stringResource(R.string.action_approve),
                            fontWeight = FontWeight.SemiBold,
                        )
                    }
                    OutlinedButton(
                        onClick = onQuickReject,
                        enabled = !isBusy,
                        border = BorderStroke(1.dp, colors.border),
                        contentPadding = PaddingValues(horizontal = 14.dp, vertical = 6.dp),
                    ) {
                        Icon(
                            imageVector = Icons.Rounded.Close,
                            contentDescription = null,
                            tint = colors.textSecondary,
                            modifier = Modifier.size(16.dp),
                        )
                        Spacer(Modifier.size(4.dp))
                        Text(
                            text = stringResource(R.string.action_no),
                            color = colors.textPrimary,
                        )
                    }
                }
            }
        }
    }
}
