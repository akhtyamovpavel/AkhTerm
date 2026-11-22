package org.akhterm.mobile.service

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import androidx.core.app.RemoteInput
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.launch
import org.akhterm.mobile.AkhTermApp
import org.akhterm.mobile.data.model.ApprovalAction

class NotificationActionReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        val action = intent.action ?: return
        val paneId = intent.getStringExtra(NotificationHelper.EXTRA_PANE_ID) ?: return
        val notificationId = intent.getIntExtra(
            NotificationHelper.EXTRA_NOTIFICATION_ID,
            NotificationHelper.alertNotificationId(paneId),
        )
        val replyText = if (action == ACTION_REPLY) {
            RemoteInput.getResultsFromIntent(intent)
                ?.getCharSequence(NotificationHelper.KEY_REPLY_TEXT)
                ?.toString()
                ?.trim()
        } else {
            null
        }

        val app = context.applicationContext as? AkhTermApp ?: return
        val pendingResult = goAsync()

        CoroutineScope(SupervisorJob() + Dispatchers.IO).launch {
            try {
                val result: Result<Any> = when (action) {
                    ACTION_APPROVE_YES -> app.repository.approve(paneId, ApprovalAction.YES)
                    ACTION_APPROVE_NO -> app.repository.approve(paneId, ApprovalAction.NO)
                    ACTION_REPLY -> {
                        if (replyText.isNullOrEmpty()) {
                            Result.failure(IllegalArgumentException("Empty reply"))
                        } else {
                            app.repository.sendPrompt(paneId, replyText, enter = true)
                        }
                    }

                    else -> return@launch
                }

                result.onSuccess {
                    app.notificationHelper.cancelNotification(notificationId)
                    app.repository.refreshStatus()
                }.onFailure { err ->
                    app.notificationHelper.postActionError(
                        paneId = paneId,
                        notificationId = notificationId,
                        errorMessage = err.message ?: "Request failed",
                    )
                }
            } finally {
                pendingResult.finish()
            }
        }
    }

    companion object {
        const val ACTION_APPROVE_YES = "org.akhterm.mobile.action.APPROVE_YES"
        const val ACTION_APPROVE_NO = "org.akhterm.mobile.action.APPROVE_NO"
        const val ACTION_REPLY = "org.akhterm.mobile.action.REPLY"
    }
}
