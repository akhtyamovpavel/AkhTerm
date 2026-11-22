package org.akhterm.mobile.ui.screens

import android.app.Activity
import android.content.ActivityNotFoundException
import android.content.Intent
import android.speech.RecognizerIntent
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.background
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.rounded.ArrowBack
import androidx.compose.material.icons.automirrored.rounded.Send
import androidx.compose.material.icons.rounded.DesktopWindows
import androidx.compose.material.icons.rounded.Mic
import androidx.compose.material.icons.rounded.Refresh
import androidx.compose.material.icons.rounded.VerticalAlignBottom
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import org.akhterm.mobile.R
import org.akhterm.mobile.data.model.ApprovalAction
import org.akhterm.mobile.data.model.PaneState
import org.akhterm.mobile.data.model.PaneTailResponse
import org.akhterm.mobile.data.model.TuiKey
import org.akhterm.mobile.data.network.ApiException
import org.akhterm.mobile.data.repository.RemoteRepository
import org.akhterm.mobile.ui.components.AgentBadge
import org.akhterm.mobile.ui.components.PaneStateBadge
import org.akhterm.mobile.ui.theme.LocalAkhTermColors
import org.akhterm.mobile.ui.theme.TerminalMonoStyle
import java.util.Locale

private data class KeyBarItem(
    val labelRes: Int? = null,
    val rawLabel: String? = null,
    val key: TuiKey,
    val isDanger: Boolean = false,
)

private val TUI_KEYBAR_ITEMS = listOf(
    KeyBarItem(labelRes = R.string.key_enter, key = TuiKey.ENTER),
    KeyBarItem(labelRes = R.string.key_esc, key = TuiKey.ESC),
    KeyBarItem(labelRes = R.string.key_up, key = TuiKey.UP),
    KeyBarItem(labelRes = R.string.key_down, key = TuiKey.DOWN),
    KeyBarItem(labelRes = R.string.key_tab, key = TuiKey.TAB),
    KeyBarItem(labelRes = R.string.key_ctrl_c, key = TuiKey.CTRL_C, isDanger = true),
    KeyBarItem(rawLabel = "1", key = TuiKey.KEY_1),
    KeyBarItem(rawLabel = "2", key = TuiKey.KEY_2),
    KeyBarItem(rawLabel = "3", key = TuiKey.KEY_3),
)

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun PaneDetailScreen(
    paneId: String,
    repository: RemoteRepository,
    onBack: () -> Unit,
) {
    val context = LocalContext.current
    val colors = LocalAkhTermColors.current
    val scope = rememberCoroutineScope()
    val snackbarHostState = remember { SnackbarHostState() }
    val listState = rememberLazyListState()

    val status by repository.status.collectAsState()
    val summary = remember(status, paneId) {
        status?.panes?.firstOrNull { it.paneId == paneId }
    }

    var tail by remember(paneId) { mutableStateOf<PaneTailResponse?>(null) }
    var sseFailed by remember(paneId) { mutableStateOf(false) }
    var paneMissing by remember(paneId) { mutableStateOf(false) }
    var autoScroll by rememberSaveable { mutableStateOf(true) }
    var promptText by rememberSaveable(paneId) { mutableStateOf("") }
    var isSending by remember { mutableStateOf(false) }

    val voicePromptText = stringResource(R.string.pane_voice_prompt)
    val voiceUnavailableText = stringResource(R.string.pane_voice_unavailable)
    val focusSentText = stringResource(R.string.pane_focus_sent)

    val speechLauncher = rememberLauncherForActivityResult(
        contract = ActivityResultContracts.StartActivityForResult(),
    ) { result ->
        if (result.resultCode == Activity.RESULT_OK) {
            val matches = result.data?.getStringArrayListExtra(RecognizerIntent.EXTRA_RESULTS)
            val spoken = matches?.firstOrNull()?.trim().orEmpty()
            if (spoken.isNotEmpty()) {
                promptText = if (promptText.isBlank()) spoken else "$promptText $spoken"
            }
        }
    }

    suspend fun fetchTailOnce() {
        repository.getPaneTail(paneId, lines = 100)
            .onSuccess { fetched ->
                tail = fetched
                paneMissing = false
            }
            .onFailure { err ->
                if (err is ApiException && err.httpCode == 404) {
                    paneMissing = true
                }
            }
    }

    // Initial tail fetch + SSE live tail stream for this pane
    LaunchedEffect(paneId) {
        fetchTailOnce()
    }

    DisposableEffect(paneId) {
        val stream = repository.openPaneTailStream(
            paneId = paneId,
            onTail = { incoming ->
                scope.launch(Dispatchers.Main) {
                    sseFailed = false
                    paneMissing = false
                    tail = incoming
                }
            },
            onError = { _, code ->
                scope.launch(Dispatchers.Main) {
                    if (code == 404) {
                        paneMissing = true
                    } else {
                        sseFailed = true
                    }
                }
            },
        )
        onDispose {
            stream?.cancel()
        }
    }

    // Fallback polling when pane SSE stream fails
    LaunchedEffect(paneId, sseFailed) {
        if (!sseFailed) return@LaunchedEffect
        while (isActive && sseFailed && !paneMissing) {
            delay(3_000L)
            fetchTailOnce()
        }
    }

    val lines = tail?.lines ?: summary?.previewLines.orEmpty()
    val currentState = tail?.state ?: summary?.state ?: PaneState.IDLE
    val currentSnippet = tail?.approvalSnippet ?: summary?.approvalSnippet

    LaunchedEffect(tail?.seq, lines.size, autoScroll) {
        if (autoScroll && lines.isNotEmpty()) {
            listState.animateScrollToItem(lines.lastIndex)
        }
    }

    fun sendApproval(action: ApprovalAction) {
        scope.launch {
            isSending = true
            repository.approve(paneId, action)
                .onSuccess { fetchTailOnce() }
                .onFailure { err ->
                    err.message?.let { snackbarHostState.showSnackbar(it) }
                }
            isSending = false
        }
    }

    fun sendKey(key: TuiKey) {
        scope.launch {
            repository.sendKey(paneId, key)
                .onSuccess {
                    delay(120L)
                    fetchTailOnce()
                }
                .onFailure { err ->
                    err.message?.let { snackbarHostState.showSnackbar(it) }
                }
        }
    }

    fun sendPrompt() {
        val text = promptText
        if (text.isBlank() || isSending) return
        scope.launch {
            isSending = true
            repository.sendPrompt(paneId, text, enter = true)
                .onSuccess {
                    promptText = ""
                    delay(120L)
                    fetchTailOnce()
                }
                .onFailure { err ->
                    err.message?.let { snackbarHostState.showSnackbar(it) }
                }
            isSending = false
        }
    }

    Scaffold(
        containerColor = colors.background,
        snackbarHost = { SnackbarHost(snackbarHostState) },
        topBar = {
            TopAppBar(
                navigationIcon = {
                    IconButton(onClick = onBack) {
                        Icon(
                            imageVector = Icons.AutoMirrored.Rounded.ArrowBack,
                            contentDescription = stringResource(R.string.pane_back),
                            tint = colors.textPrimary,
                        )
                    }
                },
                title = {
                    Column {
                        Text(
                            text = summary?.tabTitle ?: "Pane #$paneId",
                            style = MaterialTheme.typography.titleMedium,
                            color = colors.textPrimary,
                            maxLines = 1,
                            overflow = TextOverflow.Ellipsis,
                        )
                        Row(
                            verticalAlignment = Alignment.CenterVertically,
                            horizontalArrangement = Arrangement.spacedBy(6.dp),
                        ) {
                            summary?.agent?.let { AgentBadge(agent = it) }
                            PaneStateBadge(state = currentState)
                        }
                    }
                },
                actions = {
                    IconButton(
                        onClick = { autoScroll = !autoScroll },
                    ) {
                        Icon(
                            imageVector = Icons.Rounded.VerticalAlignBottom,
                            contentDescription = stringResource(
                                if (autoScroll) R.string.pane_autoscroll_on else R.string.pane_autoscroll_off,
                            ),
                            tint = if (autoScroll) colors.working else colors.textMuted,
                        )
                    }
                    IconButton(
                        onClick = {
                            scope.launch { fetchTailOnce() }
                        },
                    ) {
                        Icon(
                            imageVector = Icons.Rounded.Refresh,
                            contentDescription = stringResource(R.string.inbox_refresh),
                            tint = colors.textSecondary,
                        )
                    }
                    IconButton(
                        onClick = {
                            scope.launch {
                                repository.focusPane(paneId)
                                    .onSuccess { snackbarHostState.showSnackbar(focusSentText) }
                                    .onFailure { err ->
                                        err.message?.let { snackbarHostState.showSnackbar(it) }
                                    }
                            }
                        },
                    ) {
                        Icon(
                            imageVector = Icons.Rounded.DesktopWindows,
                            contentDescription = stringResource(R.string.pane_focus_desktop),
                            tint = colors.textSecondary,
                        )
                    }
                },
                colors = TopAppBarDefaults.topAppBarColors(containerColor = colors.background),
            )
        },
    ) { padding ->
        Column(
            modifier = Modifier
                .fillMaxSize()
                .padding(padding)
                .navigationBarsPadding()
                .imePadding(),
        ) {
            // Terminal output tail viewport
            Box(
                modifier = Modifier
                    .weight(1f)
                    .fillMaxWidth()
                    .padding(horizontal = 12.dp, vertical = 6.dp)
                    .clip(RoundedCornerShape(12.dp))
                    .background(colors.terminalBackground),
            ) {
                when {
                    paneMissing -> {
                        Text(
                            text = stringResource(R.string.pane_not_found),
                            style = MaterialTheme.typography.bodyMedium,
                            color = colors.error,
                            modifier = Modifier
                                .align(Alignment.Center)
                                .padding(16.dp),
                        )
                    }

                    lines.isEmpty() -> {
                        Text(
                            text = stringResource(R.string.pane_empty_tail),
                            style = TerminalMonoStyle,
                            color = colors.textMuted,
                            modifier = Modifier
                                .align(Alignment.Center)
                                .padding(16.dp),
                        )
                    }

                    else -> {
                        SelectionContainer {
                            LazyColumn(
                                state = listState,
                                modifier = Modifier.fillMaxSize(),
                                contentPadding = PaddingValues(12.dp),
                                verticalArrangement = Arrangement.spacedBy(2.dp),
                            ) {
                                itemsIndexed(lines) { index, line ->
                                    Text(
                                        text = line,
                                        style = TerminalMonoStyle,
                                        color = colors.terminalText,
                                    )
                                }
                            }
                        }
                    }
                }
            }

            // Approval banner
            if (currentState == PaneState.WAITING || !currentSnippet.isNullOrBlank()) {
                Surface(
                    modifier = Modifier
                        .fillMaxWidth()
                        .padding(horizontal = 12.dp, vertical = 4.dp),
                    shape = RoundedCornerShape(12.dp),
                    color = colors.waiting.copy(alpha = 0.14f),
                    border = BorderStroke(1.dp, colors.waiting.copy(alpha = 0.45f)),
                ) {
                    Column(
                        modifier = Modifier.padding(12.dp),
                        verticalArrangement = Arrangement.spacedBy(8.dp),
                    ) {
                        Text(
                            text = stringResource(R.string.pane_approval_header),
                            style = MaterialTheme.typography.labelMedium,
                            fontWeight = FontWeight.SemiBold,
                            color = colors.waiting,
                        )
                        if (!currentSnippet.isNullOrBlank()) {
                            Text(
                                text = currentSnippet,
                                style = TerminalMonoStyle,
                                color = colors.textPrimary,
                                maxLines = 3,
                                overflow = TextOverflow.Ellipsis,
                            )
                        }
                        Row(
                            modifier = Modifier.fillMaxWidth(),
                            horizontalArrangement = Arrangement.spacedBy(8.dp),
                        ) {
                            Button(
                                onClick = { sendApproval(ApprovalAction.YES) },
                                enabled = !isSending,
                                colors = ButtonDefaults.buttonColors(
                                    containerColor = colors.waiting,
                                    contentColor = Color(0xFF09090B),
                                ),
                                contentPadding = PaddingValues(horizontal = 10.dp, vertical = 6.dp),
                                modifier = Modifier.weight(1f),
                            ) {
                                Text(
                                    text = stringResource(R.string.action_yes),
                                    fontWeight = FontWeight.SemiBold,
                                    maxLines = 1,
                                )
                            }
                            OutlinedButton(
                                onClick = { sendApproval(ApprovalAction.YES_SAVE) },
                                enabled = !isSending,
                                border = BorderStroke(1.dp, colors.waiting.copy(alpha = 0.6f)),
                                contentPadding = PaddingValues(horizontal = 10.dp, vertical = 6.dp),
                                modifier = Modifier.weight(1.25f),
                            ) {
                                Text(
                                    text = stringResource(R.string.action_yes_save),
                                    color = colors.textPrimary,
                                    maxLines = 1,
                                    overflow = TextOverflow.Ellipsis,
                                )
                            }
                            OutlinedButton(
                                onClick = { sendApproval(ApprovalAction.NO) },
                                enabled = !isSending,
                                border = BorderStroke(1.dp, colors.error.copy(alpha = 0.6f)),
                                contentPadding = PaddingValues(horizontal = 10.dp, vertical = 6.dp),
                            ) {
                                Text(
                                    text = stringResource(R.string.action_no),
                                    color = colors.error,
                                    maxLines = 1,
                                )
                            }
                        }
                    }
                }
            }

            HorizontalDivider(color = colors.border)

            // Compact TUI Keybar
            Row(
                modifier = Modifier
                    .fillMaxWidth()
                    .horizontalScroll(rememberScrollState())
                    .padding(horizontal = 12.dp, vertical = 8.dp),
                horizontalArrangement = Arrangement.spacedBy(6.dp),
            ) {
                TUI_KEYBAR_ITEMS.forEach { item ->
                    val label = item.labelRes?.let { stringResource(it) } ?: item.rawLabel.orEmpty()
                    Surface(
                        onClick = { sendKey(item.key) },
                        shape = RoundedCornerShape(8.dp),
                        color = if (item.isDanger) colors.error.copy(alpha = 0.14f) else colors.surface,
                        border = BorderStroke(
                            1.dp,
                            if (item.isDanger) colors.error.copy(alpha = 0.45f) else colors.border,
                        ),
                    ) {
                        Text(
                            text = label,
                            modifier = Modifier.padding(horizontal = 12.dp, vertical = 7.dp),
                            style = MaterialTheme.typography.labelMedium.copy(
                                fontFamily = FontFamily.Monospace,
                                fontWeight = FontWeight.Medium,
                            ),
                            color = if (item.isDanger) colors.error else colors.textPrimary,
                        )
                    }
                }
            }

            // Prompt composer with voice input + send button
            Row(
                modifier = Modifier
                    .fillMaxWidth()
                    .padding(start = 12.dp, end = 12.dp, bottom = 10.dp),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(6.dp),
            ) {
                OutlinedTextField(
                    value = promptText,
                    onValueChange = { promptText = it },
                    placeholder = { Text(stringResource(R.string.pane_prompt_placeholder)) },
                    singleLine = false,
                    maxLines = 3,
                    keyboardOptions = KeyboardOptions(imeAction = ImeAction.Send),
                    keyboardActions = KeyboardActions(onSend = { sendPrompt() }),
                    modifier = Modifier.weight(1f),
                )

                IconButton(
                    onClick = {
                        val intent = Intent(RecognizerIntent.ACTION_RECOGNIZE_SPEECH).apply {
                            putExtra(
                                RecognizerIntent.EXTRA_LANGUAGE_MODEL,
                                RecognizerIntent.LANGUAGE_MODEL_FREE_FORM,
                            )
                            putExtra(RecognizerIntent.EXTRA_LANGUAGE, Locale.getDefault())
                            putExtra(RecognizerIntent.EXTRA_PROMPT, voicePromptText)
                        }
                        try {
                            speechLauncher.launch(intent)
                        } catch (_: ActivityNotFoundException) {
                            scope.launch {
                                withContext(Dispatchers.Main) {
                                    snackbarHostState.showSnackbar(voiceUnavailableText)
                                }
                            }
                        }
                    },
                ) {
                    Icon(
                        imageVector = Icons.Rounded.Mic,
                        contentDescription = stringResource(R.string.pane_voice_input),
                        tint = colors.textSecondary,
                    )
                }

                Spacer(Modifier.width(2.dp))

                Surface(
                    onClick = { sendPrompt() },
                    enabled = promptText.isNotBlank() && !isSending,
                    shape = RoundedCornerShape(12.dp),
                    color = if (promptText.isNotBlank() && !isSending) colors.working else colors.surfaceElevated,
                ) {
                    Box(
                        modifier = Modifier.padding(12.dp),
                        contentAlignment = Alignment.Center,
                    ) {
                        Icon(
                            imageVector = Icons.AutoMirrored.Rounded.Send,
                            contentDescription = stringResource(R.string.pane_send),
                            tint = if (promptText.isNotBlank() && !isSending) Color.White else colors.textMuted,
                            modifier = Modifier.size(20.dp),
                        )
                    }
                }
            }
        }
    }
}
