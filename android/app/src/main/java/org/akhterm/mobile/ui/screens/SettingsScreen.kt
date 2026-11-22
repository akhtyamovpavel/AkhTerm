package org.akhterm.mobile.ui.screens

import android.Manifest
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Build
import android.provider.Settings
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.rounded.ArrowBack
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SegmentedButton
import androidx.compose.material3.SegmentedButtonDefaults
import androidx.compose.material3.SingleChoiceSegmentedButtonRow
import androidx.compose.material3.Surface
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.core.content.ContextCompat
import kotlinx.coroutines.launch
import org.akhterm.mobile.R
import org.akhterm.mobile.data.model.ThemeMode
import org.akhterm.mobile.data.repository.RemoteRepository
import org.akhterm.mobile.data.storage.AppPreferences
import org.akhterm.mobile.service.RemoteMonitorService
import org.akhterm.mobile.ui.components.ConnectionStatusPill
import org.akhterm.mobile.ui.theme.LocalAkhTermColors

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun SettingsScreen(
    repository: RemoteRepository,
    preferences: AppPreferences,
    onBack: () -> Unit,
    onUnpaired: () -> Unit,
) {
    val context = LocalContext.current
    val colors = LocalAkhTermColors.current
    val scope = rememberCoroutineScope()

    val credentials by repository.credentials.collectAsState()
    val status by repository.status.collectAsState()
    val connectionState by repository.connectionState.collectAsState()
    val themeMode by preferences.themeMode.collectAsState()
    val bgMonitorEnabled by preferences.backgroundMonitorEnabled.collectAsState()

    var showUnpairDialog by remember { mutableStateOf(false) }
    var isUnpairing by remember { mutableStateOf(false) }

    var hasNotifPermission by remember {
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
    val notifLauncher = rememberLauncherForActivityResult(
        contract = ActivityResultContracts.RequestPermission(),
    ) { granted ->
        hasNotifPermission = granted
    }

    Scaffold(
        containerColor = colors.background,
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
                    Text(
                        text = stringResource(R.string.settings_title),
                        style = MaterialTheme.typography.titleLarge,
                        color = colors.textPrimary,
                    )
                },
                colors = TopAppBarDefaults.topAppBarColors(containerColor = colors.background),
            )
        },
    ) { padding ->
        Column(
            modifier = Modifier
                .fillMaxSize()
                .padding(padding)
                .verticalScroll(rememberScrollState())
                .padding(horizontal = 16.dp, vertical = 12.dp),
            verticalArrangement = Arrangement.spacedBy(16.dp),
        ) {
            // Paired desktop info
            SettingsCard(title = stringResource(R.string.settings_host_section)) {
                Row(
                    modifier = Modifier.fillMaxWidth(),
                    horizontalArrangement = Arrangement.SpaceBetween,
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Text(
                        text = stringResource(R.string.settings_host_name),
                        style = MaterialTheme.typography.bodySmall,
                        color = colors.textSecondary,
                    )
                    ConnectionStatusPill(
                        state = connectionState,
                        hostName = status?.hostName ?: credentials?.hostName,
                    )
                }
                HorizontalDivider(color = colors.border)
                KeyValueRow(
                    label = stringResource(R.string.settings_server_url),
                    value = credentials?.baseUrl ?: "—",
                    monospace = true,
                )
                KeyValueRow(
                    label = stringResource(R.string.settings_device_id),
                    value = credentials?.deviceId ?: "—",
                    monospace = true,
                )
                status?.version?.let { desktopVersion ->
                    KeyValueRow(
                        label = stringResource(R.string.settings_desktop_version),
                        value = "v$desktopVersion (API v${status?.apiVersion ?: 1})",
                        monospace = true,
                    )
                }
            }

            // Background monitor & notifications
            SettingsCard(title = stringResource(R.string.settings_monitor_section)) {
                Row(
                    modifier = Modifier.fillMaxWidth(),
                    horizontalArrangement = Arrangement.SpaceBetween,
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Column(modifier = Modifier.weight(1f).padding(end = 12.dp)) {
                        Text(
                            text = stringResource(R.string.settings_monitor_toggle),
                            style = MaterialTheme.typography.titleMedium,
                            color = colors.textPrimary,
                        )
                        Text(
                            text = stringResource(R.string.settings_monitor_desc),
                            style = MaterialTheme.typography.bodySmall,
                            color = colors.textSecondary,
                        )
                    }
                    Switch(
                        checked = bgMonitorEnabled,
                        onCheckedChange = { enabled ->
                            preferences.setBackgroundMonitorEnabled(enabled)
                            if (enabled && credentials != null) {
                                RemoteMonitorService.start(context, reconnect = true)
                            } else {
                                RemoteMonitorService.stop(context)
                            }
                        },
                    )
                }

                HorizontalDivider(color = colors.border)

                Row(
                    modifier = Modifier.fillMaxWidth(),
                    horizontalArrangement = Arrangement.SpaceBetween,
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Column {
                        Text(
                            text = stringResource(R.string.settings_notifications_permission),
                            style = MaterialTheme.typography.bodyMedium,
                            color = colors.textPrimary,
                        )
                        Text(
                            text = stringResource(
                                if (hasNotifPermission) {
                                    R.string.settings_notifications_granted
                                } else {
                                    R.string.settings_notifications_denied
                                },
                            ),
                            style = MaterialTheme.typography.bodySmall,
                            color = if (hasNotifPermission) colors.done else colors.waiting,
                        )
                    }
                    if (!hasNotifPermission) {
                        TextButton(
                            onClick = {
                                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
                                    notifLauncher.launch(Manifest.permission.POST_NOTIFICATIONS)
                                } else {
                                    val intent = Intent(Settings.ACTION_APP_NOTIFICATION_SETTINGS).apply {
                                        putExtra(Settings.EXTRA_APP_PACKAGE, context.packageName)
                                    }
                                    runCatching { context.startActivity(intent) }
                                }
                            },
                        ) {
                            Text(stringResource(R.string.inbox_notifications_enable))
                        }
                    }
                }
            }

            // Theme selector
            SettingsCard(title = stringResource(R.string.settings_theme_section)) {
                val options = listOf(
                    ThemeMode.SYSTEM to R.string.settings_theme_system,
                    ThemeMode.DARK to R.string.settings_theme_dark,
                    ThemeMode.LIGHT to R.string.settings_theme_light,
                )
                SingleChoiceSegmentedButtonRow(modifier = Modifier.fillMaxWidth()) {
                    options.forEachIndexed { index, (mode, labelRes) ->
                        SegmentedButton(
                            selected = themeMode == mode,
                            onClick = { preferences.setThemeMode(mode) },
                            shape = SegmentedButtonDefaults.itemShape(index = index, count = options.size),
                        ) {
                            Text(
                                text = stringResource(labelRes),
                                maxLines = 1,
                            )
                        }
                    }
                }
            }

            // Danger zone: Unpair
            SettingsCard(title = stringResource(R.string.settings_danger_section)) {
                Button(
                    onClick = { showUnpairDialog = true },
                    enabled = !isUnpairing,
                    colors = ButtonDefaults.buttonColors(containerColor = colors.error),
                    modifier = Modifier.fillMaxWidth(),
                ) {
                    Text(
                        text = stringResource(R.string.settings_unpair_button),
                        fontWeight = FontWeight.SemiBold,
                    )
                }
            }

            Spacer(Modifier.height(16.dp))
        }
    }

    if (showUnpairDialog) {
        AlertDialog(
            onDismissRequest = { if (!isUnpairing) showUnpairDialog = false },
            containerColor = colors.surface,
            titleContentColor = colors.textPrimary,
            textContentColor = colors.textSecondary,
            title = { Text(stringResource(R.string.settings_unpair_dialog_title)) },
            text = { Text(stringResource(R.string.settings_unpair_dialog_body)) },
            dismissButton = {
                TextButton(
                    onClick = { showUnpairDialog = false },
                    enabled = !isUnpairing,
                ) {
                    Text(stringResource(R.string.settings_cancel))
                }
            },
            confirmButton = {
                Button(
                    onClick = {
                        scope.launch {
                            isUnpairing = true
                            RemoteMonitorService.stop(context)
                            repository.unpair()
                            isUnpairing = false
                            showUnpairDialog = false
                            onUnpaired()
                        }
                    },
                    enabled = !isUnpairing,
                    colors = ButtonDefaults.buttonColors(containerColor = colors.error),
                ) {
                    Text(stringResource(R.string.settings_confirm_unpair))
                }
            },
        )
    }
}

@Composable
private fun SettingsCard(
    title: String,
    content: @Composable () -> Unit,
) {
    val colors = LocalAkhTermColors.current
    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
        Text(
            text = title.uppercase(),
            style = MaterialTheme.typography.labelMedium,
            fontWeight = FontWeight.SemiBold,
            color = colors.textMuted,
            modifier = Modifier.padding(horizontal = 4.dp),
        )
        Surface(
            modifier = Modifier.fillMaxWidth(),
            shape = RoundedCornerShape(14.dp),
            color = colors.surface,
            border = BorderStroke(1.dp, colors.border),
        ) {
            Column(
                modifier = Modifier.padding(14.dp),
                verticalArrangement = Arrangement.spacedBy(10.dp),
            ) {
                content()
            }
        }
    }
}

@Composable
private fun KeyValueRow(
    label: String,
    value: String,
    monospace: Boolean = false,
) {
    val colors = LocalAkhTermColors.current
    Column(verticalArrangement = Arrangement.spacedBy(2.dp)) {
        Text(
            text = label,
            style = MaterialTheme.typography.labelSmall,
            color = colors.textMuted,
        )
        Text(
            text = value,
            style = MaterialTheme.typography.bodyMedium.copy(
                fontFamily = if (monospace) FontFamily.Monospace else FontFamily.SansSerif,
            ),
            color = colors.textPrimary,
        )
    }
}
