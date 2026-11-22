package org.akhterm.mobile.service

import android.Manifest
import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Build
import androidx.core.app.NotificationCompat
import androidx.core.app.NotificationManagerCompat
import androidx.core.app.RemoteInput
import androidx.core.content.ContextCompat
import org.akhterm.mobile.MainActivity
import org.akhterm.mobile.R
import org.akhterm.mobile.data.model.AlertEvent
import org.akhterm.mobile.data.model.ConnectionState
import org.akhterm.mobile.data.model.PaneState

class NotificationHelper(
    private val context: Context,
) {
    private val appContext = context.applicationContext

    fun ensureChannels() {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) return
        val manager = appContext.getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager

        val connectionChannel = NotificationChannel(
            CHANNEL_CONNECTION,
            appContext.getString(R.string.notif_channel_connection_name),
            NotificationManager.IMPORTANCE_LOW,
        ).apply {
            description = appContext.getString(R.string.notif_channel_connection_desc)
            setShowBadge(false)
            enableLights(false)
            enableVibration(false)
            setSound(null, null)
        }

        val alertsChannel = NotificationChannel(
            CHANNEL_ALERTS,
            appContext.getString(R.string.notif_channel_alerts_name),
            NotificationManager.IMPORTANCE_HIGH,
        ).apply {
            description = appContext.getString(R.string.notif_channel_alerts_desc)
            setShowBadge(true)
            enableVibration(true)
        }

        manager.createNotificationChannels(listOf(connectionChannel, alertsChannel))
    }

    fun hasNotificationPermission(): Boolean {
        return if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            ContextCompat.checkSelfPermission(
                appContext,
                Manifest.permission.POST_NOTIFICATIONS,
            ) == PackageManager.PERMISSION_GRANTED
        } else {
            true
        }
    }

    fun buildConnectionNotification(
        hostName: String,
        connectionState: ConnectionState,
        totalPanes: Int,
        waitingPanes: Int,
    ): Notification {
        val openAppIntent = Intent(appContext, MainActivity::class.java).apply {
            flags = Intent.FLAG_ACTIVITY_SINGLE_TOP or Intent.FLAG_ACTIVITY_CLEAR_TOP
        }
        val contentIntent = PendingIntent.getActivity(
            appContext,
            REQUEST_OPEN_APP,
            openAppIntent,
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )

        val title = appContext.getString(R.string.notif_connection_title, hostName)
        val text = when (connectionState) {
            ConnectionState.CONNECTED -> {
                if (waitingPanes > 0) {
                    appContext.getString(R.string.notif_connection_waiting, waitingPanes)
                } else {
                    appContext.getString(R.string.notif_connection_idle, totalPanes)
                }
            }

            ConnectionState.CONNECTING,
            ConnectionState.RECONNECTING,
            -> appContext.getString(R.string.notif_connection_reconnecting, hostName)

            ConnectionState.UNAUTHORIZED -> appContext.getString(R.string.notif_connection_unauthorized)
            ConnectionState.DISCONNECTED,
            ConnectionState.ERROR,
            -> appContext.getString(R.string.notif_connection_reconnecting, hostName)
        }

        return NotificationCompat.Builder(appContext, CHANNEL_CONNECTION)
            .setSmallIcon(R.drawable.ic_stat_akhterm)
            .setContentTitle(title)
            .setContentText(text)
            .setContentIntent(contentIntent)
            .setOngoing(true)
            .setOnlyAlertOnce(true)
            .setSilent(true)
            .setPriority(NotificationCompat.PRIORITY_LOW)
            .setCategory(NotificationCompat.CATEGORY_SERVICE)
            .build()
    }

    fun postAlertNotification(alert: AlertEvent) {
        if (!alert.shouldNotify || !hasNotificationPermission()) return

        val notificationId = alertNotificationId(alert.paneId)
        val openPaneIntent = Intent(appContext, MainActivity::class.java).apply {
            flags = Intent.FLAG_ACTIVITY_SINGLE_TOP or Intent.FLAG_ACTIVITY_CLEAR_TOP
            putExtra(EXTRA_PANE_ID, alert.paneId)
        }
        val contentPendingIntent = PendingIntent.getActivity(
            appContext,
            notificationId,
            openPaneIntent,
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )

        val subText = listOf(alert.repoName, alert.tabTitle)
            .filter { it.isNotBlank() }
            .distinct()
            .joinToString(" · ")

        val builder = NotificationCompat.Builder(appContext, CHANNEL_ALERTS)
            .setSmallIcon(R.drawable.ic_stat_akhterm)
            .setContentTitle(alert.title)
            .setContentText(alert.body)
            .setStyle(NotificationCompat.BigTextStyle().bigText(alert.body))
            .setSubText(subText.ifBlank { null })
            .setContentIntent(contentPendingIntent)
            .setAutoCancel(true)
            .setPriority(NotificationCompat.PRIORITY_HIGH)
            .setCategory(
                if (alert.state == PaneState.WAITING) {
                    NotificationCompat.CATEGORY_MESSAGE
                } else {
                    NotificationCompat.CATEGORY_STATUS
                },
            )

        if (alert.state == PaneState.WAITING) {
            builder.addAction(
                buildButtonAction(
                    action = NotificationActionReceiver.ACTION_APPROVE_YES,
                    label = appContext.getString(R.string.notif_action_approve),
                    paneId = alert.paneId,
                    notificationId = notificationId,
                    requestCodeOffset = 1,
                ),
            )
            builder.addAction(
                buildButtonAction(
                    action = NotificationActionReceiver.ACTION_APPROVE_NO,
                    label = appContext.getString(R.string.notif_action_no),
                    paneId = alert.paneId,
                    notificationId = notificationId,
                    requestCodeOffset = 2,
                ),
            )
            builder.addAction(
                buildReplyAction(
                    paneId = alert.paneId,
                    notificationId = notificationId,
                    requestCodeOffset = 3,
                ),
            )
        }

        try {
            NotificationManagerCompat.from(appContext).notify(notificationId, builder.build())
        } catch (_: SecurityException) {
            // Permission revoked concurrently
        }
    }

    fun postActionError(paneId: String, notificationId: Int, errorMessage: String) {
        if (!hasNotificationPermission()) return
        val openPaneIntent = Intent(appContext, MainActivity::class.java).apply {
            flags = Intent.FLAG_ACTIVITY_SINGLE_TOP or Intent.FLAG_ACTIVITY_CLEAR_TOP
            putExtra(EXTRA_PANE_ID, paneId)
        }
        val contentPendingIntent = PendingIntent.getActivity(
            appContext,
            notificationId,
            openPaneIntent,
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )
        val notification = NotificationCompat.Builder(appContext, CHANNEL_ALERTS)
            .setSmallIcon(R.drawable.ic_stat_akhterm)
            .setContentTitle(appContext.getString(R.string.app_name))
            .setContentText(appContext.getString(R.string.notif_action_failed, errorMessage))
            .setContentIntent(contentPendingIntent)
            .setAutoCancel(true)
            .setPriority(NotificationCompat.PRIORITY_DEFAULT)
            .build()
        try {
            NotificationManagerCompat.from(appContext).notify(notificationId, notification)
        } catch (_: SecurityException) {
            // Ignore
        }
    }

    fun cancelAlertForPane(paneId: String) {
        NotificationManagerCompat.from(appContext).cancel(alertNotificationId(paneId))
    }

    fun cancelNotification(notificationId: Int) {
        NotificationManagerCompat.from(appContext).cancel(notificationId)
    }

    private fun buildButtonAction(
        action: String,
        label: String,
        paneId: String,
        notificationId: Int,
        requestCodeOffset: Int,
    ): NotificationCompat.Action {
        val intent = Intent(appContext, NotificationActionReceiver::class.java).apply {
            this.action = action
            putExtra(EXTRA_PANE_ID, paneId)
            putExtra(EXTRA_NOTIFICATION_ID, notificationId)
        }
        val pendingIntent = PendingIntent.getBroadcast(
            appContext,
            notificationId * 10 + requestCodeOffset,
            intent,
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )
        return NotificationCompat.Action.Builder(0, label, pendingIntent).build()
    }

    private fun buildReplyAction(
        paneId: String,
        notificationId: Int,
        requestCodeOffset: Int,
    ): NotificationCompat.Action {
        val remoteInput = RemoteInput.Builder(KEY_REPLY_TEXT)
            .setLabel(appContext.getString(R.string.notif_reply_hint))
            .build()

        val intent = Intent(appContext, NotificationActionReceiver::class.java).apply {
            action = NotificationActionReceiver.ACTION_REPLY
            putExtra(EXTRA_PANE_ID, paneId)
            putExtra(EXTRA_NOTIFICATION_ID, notificationId)
        }
        val flags = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_MUTABLE
        } else {
            PendingIntent.FLAG_UPDATE_CURRENT
        }
        val pendingIntent = PendingIntent.getBroadcast(
            appContext,
            notificationId * 10 + requestCodeOffset,
            intent,
            flags,
        )
        return NotificationCompat.Action.Builder(
            0,
            appContext.getString(R.string.notif_action_reply),
            pendingIntent,
        )
            .addRemoteInput(remoteInput)
            .setAllowGeneratedReplies(false)
            .build()
    }

    companion object {
        const val CHANNEL_CONNECTION = "channel_connection"
        const val CHANNEL_ALERTS = "channel_alerts"

        const val EXTRA_PANE_ID = "org.akhterm.mobile.extra.PANE_ID"
        const val EXTRA_NOTIFICATION_ID = "org.akhterm.mobile.extra.NOTIFICATION_ID"
        const val KEY_REPLY_TEXT = "org.akhterm.mobile.key.REPLY_TEXT"

        const val NOTIFICATION_ID_FOREGROUND = 1001
        private const val REQUEST_OPEN_APP = 1000

        fun alertNotificationId(paneId: String): Int = 2000 + (paneId.hashCode() and 0x7FFF)
    }
}
