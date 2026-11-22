package org.akhterm.mobile.service

import android.app.NotificationManager
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.net.ConnectivityManager
import android.net.Network
import android.os.Build
import android.os.IBinder
import androidx.core.content.ContextCompat
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import okhttp3.Response
import okhttp3.sse.EventSource
import okhttp3.sse.EventSourceListener
import org.akhterm.mobile.AkhTermApp
import org.akhterm.mobile.data.model.AlertEvent
import org.akhterm.mobile.data.model.ConnectionState
import org.akhterm.mobile.data.model.HeartbeatEvent
import org.akhterm.mobile.data.model.PaneState
import org.akhterm.mobile.data.model.RemoteJson
import org.akhterm.mobile.data.model.RemoteStatusResponse
import java.util.Collections
import java.util.LinkedHashSet
import java.util.concurrent.atomic.AtomicInteger
import java.util.concurrent.atomic.AtomicReference

class RemoteMonitorService : Service() {
    private val serviceScope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    private val activeEventSource = AtomicReference<EventSource?>(null)
    private val reconnectAttempt = AtomicInteger(0)
    private var reconnectJob: Job? = null

    private var connectivityManager: ConnectivityManager? = null
    private var networkCallback: ConnectivityManager.NetworkCallback? = null
    private var lastActiveNetworkHandle: Long? = null

    private val seenEventIds = Collections.synchronizedSet(LinkedHashSet<String>())
    private val notifiedWaitingPanes = Collections.synchronizedSet(HashSet<String>())

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onCreate() {
        super.onCreate()
        val app = application as AkhTermApp
        app.notificationHelper.ensureChannels()
        registerNetworkMonitor()
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        val app = application as AkhTermApp
        val creds = app.repository.credentials.value

        if (intent?.action == ACTION_STOP || creds == null || !app.preferences.backgroundMonitorEnabled.value) {
            stopSelf()
            return START_NOT_STICKY
        }

        val initialNotification = app.notificationHelper.buildConnectionNotification(
            hostName = creds.hostName,
            connectionState = app.repository.connectionState.value,
            totalPanes = app.repository.status.value?.panes?.size ?: 0,
            waitingPanes = app.repository.status.value?.panes?.count { it.state == PaneState.WAITING } ?: 0,
        )
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
            startForeground(
                NotificationHelper.NOTIFICATION_ID_FOREGROUND,
                initialNotification,
                ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC,
            )
        } else {
            startForeground(NotificationHelper.NOTIFICATION_ID_FOREGROUND, initialNotification)
        }

        if (intent?.action == ACTION_RECONNECT || activeEventSource.get() == null) {
            reconnectAttempt.set(0)
            connectNow()
        }

        return START_STICKY
    }

    override fun onDestroy() {
        reconnectJob?.cancel()
        activeEventSource.getAndSet(null)?.cancel()
        unregisterNetworkMonitor()
        serviceScope.cancel()
        super.onDestroy()
    }

    private fun connectNow() {
        val app = application as AkhTermApp
        val creds = app.repository.credentials.value ?: run {
            stopSelf()
            return
        }

        reconnectJob?.cancel()
        activeEventSource.getAndSet(null)?.cancel()

        val nextState = if (reconnectAttempt.get() == 0) {
            ConnectionState.CONNECTING
        } else {
            ConnectionState.RECONNECTING
        }
        app.repository.updateConnectionState(nextState)
        updateForegroundNotification(creds.hostName, nextState, app.repository.status.value)

        val listener = object : EventSourceListener() {
            override fun onOpen(eventSource: EventSource, response: Response) {
                if (activeEventSource.get() !== eventSource) return
                reconnectAttempt.set(0)
                app.repository.updateConnectionState(ConnectionState.CONNECTED)
                updateForegroundNotification(creds.hostName, ConnectionState.CONNECTED, app.repository.status.value)
            }

            override fun onEvent(
                eventSource: EventSource,
                id: String?,
                type: String?,
                data: String,
            ) {
                if (activeEventSource.get() !== eventSource) return
                when (type) {
                    "snapshot" -> {
                        val snapshot = runCatching {
                            RemoteJson.decodeFromString(RemoteStatusResponse.serializer(), data)
                        }.getOrNull() ?: return
                        app.repository.updateFromSnapshot(snapshot)
                        clearResolvedWaitingAlerts(snapshot)
                        updateForegroundNotification(snapshot.hostName, ConnectionState.CONNECTED, snapshot)
                    }

                    "alert" -> {
                        val alert = runCatching {
                            RemoteJson.decodeFromString(AlertEvent.serializer(), data)
                        }.getOrNull() ?: return
                        handleAlert(alert)
                    }

                    "heartbeat" -> {
                        runCatching {
                            RemoteJson.decodeFromString(HeartbeatEvent.serializer(), data)
                        }
                    }
                }
            }

            override fun onClosed(eventSource: EventSource) {
                if (activeEventSource.get() !== eventSource) return
                activeEventSource.compareAndSet(eventSource, null)
                scheduleReconnect("Stream closed")
            }

            override fun onFailure(
                eventSource: EventSource,
                t: Throwable?,
                response: Response?,
            ) {
                if (activeEventSource.get() !== eventSource) return
                activeEventSource.compareAndSet(eventSource, null)

                if (response?.code == 401) {
                    app.repository.updateConnectionState(
                        ConnectionState.UNAUTHORIZED,
                        "HTTP 401 Unauthorized",
                    )
                    updateForegroundNotification(creds.hostName, ConnectionState.UNAUTHORIZED, null)
                    return
                }
                scheduleReconnect(t?.message ?: response?.let { "HTTP ${it.code}" } ?: "Disconnected")
            }
        }

        val eventSource = app.repository.openGlobalEventStream(listener)
        if (eventSource == null) {
            stopSelf()
        } else {
            activeEventSource.set(eventSource)
        }
    }

    private fun handleAlert(alert: AlertEvent) {
        val app = application as AkhTermApp
        val isNew = synchronized(seenEventIds) {
            if (seenEventIds.contains(alert.eventId)) {
                false
            } else {
                if (seenEventIds.size >= MAX_SEEN_EVENTS) {
                    val first = seenEventIds.iterator().next()
                    seenEventIds.remove(first)
                }
                seenEventIds.add(alert.eventId)
                true
            }
        }
        if (!isNew) return

        if (alert.shouldNotify) {
            if (alert.state == PaneState.WAITING) {
                notifiedWaitingPanes.add(alert.paneId)
            }
            app.notificationHelper.postAlertNotification(alert)
        }
    }

    private fun clearResolvedWaitingAlerts(snapshot: RemoteStatusResponse) {
        val app = application as AkhTermApp
        val currentlyWaiting = snapshot.panes
            .filter { it.state == PaneState.WAITING }
            .map { it.paneId }
            .toSet()

        val resolved = synchronized(notifiedWaitingPanes) {
            val toRemove = notifiedWaitingPanes.filter { it !in currentlyWaiting }
            notifiedWaitingPanes.removeAll(toRemove.toSet())
            toRemove
        }
        for (paneId in resolved) {
            app.notificationHelper.cancelAlertForPane(paneId)
        }
    }

    private fun scheduleReconnect(reason: String) {
        val app = application as AkhTermApp
        val creds = app.repository.credentials.value ?: run {
            stopSelf()
            return
        }
        val attempt = reconnectAttempt.getAndIncrement()
        val delayMs = computeBackoffMs(attempt)
        app.repository.updateConnectionState(ConnectionState.RECONNECTING, reason)
        updateForegroundNotification(creds.hostName, ConnectionState.RECONNECTING, app.repository.status.value)

        reconnectJob?.cancel()
        reconnectJob = serviceScope.launch {
            delay(delayMs)
            connectNow()
        }
    }

    private fun updateForegroundNotification(
        hostName: String,
        connectionState: ConnectionState,
        snapshot: RemoteStatusResponse?,
    ) {
        val app = application as AkhTermApp
        val total = snapshot?.panes?.size ?: 0
        val waiting = snapshot?.panes?.count { it.state == PaneState.WAITING } ?: 0
        val notification = app.notificationHelper.buildConnectionNotification(
            hostName = hostName,
            connectionState = connectionState,
            totalPanes = total,
            waitingPanes = waiting,
        )
        val manager = getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager
        manager.notify(NotificationHelper.NOTIFICATION_ID_FOREGROUND, notification)
    }

    private fun registerNetworkMonitor() {
        val cm = getSystemService(Context.CONNECTIVITY_SERVICE) as? ConnectivityManager ?: return
        connectivityManager = cm
        val callback = object : ConnectivityManager.NetworkCallback() {
            override fun onAvailable(network: Network) {
                val handle = network.networkHandle
                val previous = lastActiveNetworkHandle
                lastActiveNetworkHandle = handle
                val app = application as AkhTermApp
                val state = app.repository.connectionState.value
                if (state == ConnectionState.UNAUTHORIZED) return
                // Reconnect immediately on network switch (Wi-Fi <-> Cellular/VPN) or recovery
                if (previous != null && previous != handle || activeEventSource.get() == null) {
                    reconnectAttempt.set(0)
                    serviceScope.launch {
                        connectNow()
                    }
                }
            }

            override fun onLost(network: Network) {
                if (lastActiveNetworkHandle == network.networkHandle) {
                    val app = application as AkhTermApp
                    val creds = app.repository.credentials.value ?: return
                    if (app.repository.connectionState.value != ConnectionState.UNAUTHORIZED) {
                        app.repository.updateConnectionState(ConnectionState.RECONNECTING)
                        updateForegroundNotification(
                            creds.hostName,
                            ConnectionState.RECONNECTING,
                            app.repository.status.value,
                        )
                    }
                }
            }
        }
        networkCallback = callback
        runCatching { cm.registerDefaultNetworkCallback(callback) }
    }

    private fun unregisterNetworkMonitor() {
        val cm = connectivityManager ?: return
        val cb = networkCallback ?: return
        runCatching { cm.unregisterNetworkCallback(cb) }
        networkCallback = null
    }

    companion object {
        private const val ACTION_START = "org.akhterm.mobile.service.START"
        private const val ACTION_RECONNECT = "org.akhterm.mobile.service.RECONNECT"
        private const val ACTION_STOP = "org.akhterm.mobile.service.STOP"
        private const val MAX_SEEN_EVENTS = 200

        fun computeBackoffMs(attempt: Int): Long {
            val shift = attempt.coerceIn(0, 5)
            val base = 1_000L shl shift
            return base.coerceAtMost(30_000L)
        }

        fun start(context: Context, reconnect: Boolean = false) {
            val intent = Intent(context, RemoteMonitorService::class.java).apply {
                action = if (reconnect) ACTION_RECONNECT else ACTION_START
            }
            ContextCompat.startForegroundService(context, intent)
        }

        fun stop(context: Context) {
            val intent = Intent(context, RemoteMonitorService::class.java).apply {
                action = ACTION_STOP
            }
            context.stopService(intent)
        }
    }
}
