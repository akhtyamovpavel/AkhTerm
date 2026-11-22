package org.akhterm.mobile

import android.content.Intent
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.BackHandler
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import kotlinx.coroutines.flow.MutableStateFlow
import org.akhterm.mobile.data.pairing.PairingPayload
import org.akhterm.mobile.data.pairing.QrPairingParser
import org.akhterm.mobile.service.NotificationHelper
import org.akhterm.mobile.service.RemoteMonitorService
import org.akhterm.mobile.ui.screens.InboxScreen
import org.akhterm.mobile.ui.screens.PaneDetailScreen
import org.akhterm.mobile.ui.screens.PairingScreen
import org.akhterm.mobile.ui.screens.SettingsScreen
import org.akhterm.mobile.ui.theme.AkhTermTheme

private sealed interface AppRoute {
    data object Pairing : AppRoute
    data object Inbox : AppRoute
    data class PaneDetail(val paneId: String) : AppRoute
    data object Settings : AppRoute
}

class MainActivity : ComponentActivity() {
    private val pendingPaneId = MutableStateFlow<String?>(null)
    private val pendingDeepLink = MutableStateFlow<PairingPayload?>(null)

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        consumeIntent(intent)

        val app = application as AkhTermApp

        setContent {
            val themeMode by app.preferences.themeMode.collectAsState()
            val bgMonitorEnabled by app.preferences.backgroundMonitorEnabled.collectAsState()
            val credentials by app.repository.credentials.collectAsState()
            val targetPaneId by pendingPaneId.collectAsState()
            val deepLinkPayload by pendingDeepLink.collectAsState()

            var route by remember {
                mutableStateOf<AppRoute>(
                    if (credentials == null) AppRoute.Pairing else AppRoute.Inbox,
                )
            }

            // Sync foreground monitor service lifecycle with credentials & user preference
            LaunchedEffect(credentials, bgMonitorEnabled) {
                if (credentials != null && bgMonitorEnabled) {
                    RemoteMonitorService.start(this@MainActivity)
                } else {
                    RemoteMonitorService.stop(this@MainActivity)
                }
            }

            // Ensure route stays valid when credentials appear/disappear
            LaunchedEffect(credentials) {
                if (credentials == null && route !is AppRoute.Pairing) {
                    route = AppRoute.Pairing
                } else if (credentials != null && route is AppRoute.Pairing) {
                    route = AppRoute.Inbox
                }
            }

            // Open target pane directly when user taps an alert notification
            LaunchedEffect(targetPaneId, credentials) {
                val paneId = targetPaneId
                if (paneId != null && credentials != null) {
                    route = AppRoute.PaneDetail(paneId)
                    pendingPaneId.value = null
                }
            }

            // Deep link pairing forces PairingScreen even if already paired
            LaunchedEffect(deepLinkPayload) {
                if (deepLinkPayload != null) {
                    route = AppRoute.Pairing
                }
            }

            AkhTermTheme(themeMode = themeMode) {
                when (val current = route) {
                    AppRoute.Pairing -> {
                        PairingScreen(
                            repository = app.repository,
                            pendingDeepLinkPayload = deepLinkPayload,
                            onDeepLinkConsumed = { pendingDeepLink.value = null },
                            onPaired = {
                                if (app.preferences.backgroundMonitorEnabled.value) {
                                    RemoteMonitorService.start(this@MainActivity, reconnect = true)
                                }
                                route = AppRoute.Inbox
                            },
                        )
                    }

                    AppRoute.Inbox -> {
                        InboxScreen(
                            repository = app.repository,
                            onOpenPane = { paneId -> route = AppRoute.PaneDetail(paneId) },
                            onOpenSettings = { route = AppRoute.Settings },
                            onRepairNeeded = {
                                RemoteMonitorService.stop(this@MainActivity)
                                app.repository.clearLocalCredentials()
                                route = AppRoute.Pairing
                            },
                        )
                    }

                    is AppRoute.PaneDetail -> {
                        BackHandler { route = AppRoute.Inbox }
                        PaneDetailScreen(
                            paneId = current.paneId,
                            repository = app.repository,
                            onBack = { route = AppRoute.Inbox },
                        )
                    }

                    AppRoute.Settings -> {
                        BackHandler { route = AppRoute.Inbox }
                        SettingsScreen(
                            repository = app.repository,
                            preferences = app.preferences,
                            onBack = { route = AppRoute.Inbox },
                            onUnpaired = { route = AppRoute.Pairing },
                        )
                    }
                }
            }
        }
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        setIntent(intent)
        consumeIntent(intent)
    }

    private fun consumeIntent(intent: Intent?) {
        if (intent == null) return
        val paneId = intent.getStringExtra(NotificationHelper.EXTRA_PANE_ID)
        if (!paneId.isNullOrBlank()) {
            pendingPaneId.value = paneId
        }
        val uriString = intent.dataString
        if (!uriString.isNullOrBlank() && uriString.startsWith("akhterm://", ignoreCase = true)) {
            QrPairingParser.parse(uriString)?.let { payload ->
                pendingDeepLink.value = payload
            }
        }
    }
}
