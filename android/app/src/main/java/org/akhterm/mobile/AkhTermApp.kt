package org.akhterm.mobile

import android.app.Application
import org.akhterm.mobile.data.network.RemoteApiClient
import org.akhterm.mobile.data.repository.RemoteRepository
import org.akhterm.mobile.data.storage.AppPreferences
import org.akhterm.mobile.data.storage.SecureTokenStore
import org.akhterm.mobile.service.NotificationHelper

class AkhTermApp : Application() {
    lateinit var preferences: AppPreferences
        private set

    lateinit var tokenStore: SecureTokenStore
        private set

    lateinit var apiClient: RemoteApiClient
        private set

    lateinit var repository: RemoteRepository
        private set

    lateinit var notificationHelper: NotificationHelper
        private set

    override fun onCreate() {
        super.onCreate()
        preferences = AppPreferences(this)
        tokenStore = SecureTokenStore(this)
        apiClient = RemoteApiClient()
        repository = RemoteRepository(apiClient, tokenStore)
        notificationHelper = NotificationHelper(this).apply {
            ensureChannels()
        }
    }
}
