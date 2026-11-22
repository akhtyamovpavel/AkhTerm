package org.akhterm.mobile.data.repository

import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import okhttp3.Response
import okhttp3.sse.EventSource
import okhttp3.sse.EventSourceListener
import org.akhterm.mobile.data.model.ApprovalAction
import org.akhterm.mobile.data.model.ApproveResponse
import org.akhterm.mobile.data.model.ConnectionState
import org.akhterm.mobile.data.model.FocusResponse
import org.akhterm.mobile.data.model.HealthResponse
import org.akhterm.mobile.data.model.PairResponse
import org.akhterm.mobile.data.model.PairedCredentials
import org.akhterm.mobile.data.model.PaneTailResponse
import org.akhterm.mobile.data.model.RemoteJson
import org.akhterm.mobile.data.model.RemoteStatusResponse
import org.akhterm.mobile.data.model.SendKeyResponse
import org.akhterm.mobile.data.model.SendPromptResponse
import org.akhterm.mobile.data.model.TuiKey
import org.akhterm.mobile.data.network.ApiException
import org.akhterm.mobile.data.network.RemoteApiClient
import org.akhterm.mobile.data.storage.SecureTokenStore

class RemoteRepository(
    private val apiClient: RemoteApiClient,
    private val tokenStore: SecureTokenStore,
) {
    private val _credentials = MutableStateFlow(tokenStore.load())
    val credentials: StateFlow<PairedCredentials?> = _credentials.asStateFlow()

    private val _connectionState = MutableStateFlow(ConnectionState.DISCONNECTED)
    val connectionState: StateFlow<ConnectionState> = _connectionState.asStateFlow()

    private val _status = MutableStateFlow<RemoteStatusResponse?>(null)
    val status: StateFlow<RemoteStatusResponse?> = _status.asStateFlow()

    private val _lastError = MutableStateFlow<String?>(null)
    val lastError: StateFlow<String?> = _lastError.asStateFlow()

    private val _lastUpdatedMillis = MutableStateFlow<Long?>(null)
    val lastUpdatedMillis: StateFlow<Long?> = _lastUpdatedMillis.asStateFlow()

    suspend fun testHealth(baseUrl: String): Result<HealthResponse> = apiClient.health(baseUrl)

    suspend fun pair(
        baseUrl: String,
        code: String,
        deviceName: String,
    ): Result<PairResponse> {
        val result = apiClient.pair(baseUrl, code, deviceName)
        result.onSuccess { response ->
            val creds = PairedCredentials(
                baseUrl = baseUrl,
                deviceId = response.deviceId,
                token = response.token,
                hostName = response.hostName,
            )
            tokenStore.save(creds)
            _credentials.value = creds
            _connectionState.value = ConnectionState.CONNECTING
            _lastError.value = null
            refreshStatus()
        }
        return result
    }

    suspend fun refreshStatus(): Result<RemoteStatusResponse> {
        val creds = _credentials.value
            ?: return Result.failure(IllegalStateException("Not paired"))
        val result = apiClient.getStatus(creds)
        result.onSuccess { snapshot ->
            updateFromSnapshot(snapshot)
        }.onFailure { err ->
            if (err is ApiException && err.httpCode == 401) {
                _connectionState.value = ConnectionState.UNAUTHORIZED
            } else if (_connectionState.value != ConnectionState.CONNECTED) {
                _connectionState.value = ConnectionState.ERROR
            }
            _lastError.value = err.message
        }
        return result
    }

    suspend fun getPaneTail(paneId: String, lines: Int = 100): Result<PaneTailResponse> {
        val creds = _credentials.value
            ?: return Result.failure(IllegalStateException("Not paired"))
        return apiClient.getPaneTail(creds, paneId, lines)
    }

    suspend fun approve(paneId: String, action: ApprovalAction): Result<ApproveResponse> {
        val creds = _credentials.value
            ?: return Result.failure(IllegalStateException("Not paired"))
        val result = apiClient.approve(creds, paneId, action)
        if (result.isSuccess) {
            refreshStatus()
        }
        return result
    }

    suspend fun sendPrompt(
        paneId: String,
        text: String,
        enter: Boolean = true,
    ): Result<SendPromptResponse> {
        val creds = _credentials.value
            ?: return Result.failure(IllegalStateException("Not paired"))
        return apiClient.sendPrompt(creds, paneId, text, enter)
    }

    suspend fun sendKey(paneId: String, key: TuiKey): Result<SendKeyResponse> {
        val creds = _credentials.value
            ?: return Result.failure(IllegalStateException("Not paired"))
        return apiClient.sendKey(creds, paneId, key)
    }

    suspend fun focusPane(paneId: String): Result<FocusResponse> {
        val creds = _credentials.value
            ?: return Result.failure(IllegalStateException("Not paired"))
        return apiClient.focusPane(creds, paneId)
    }

    suspend fun unpair(): Result<Unit> {
        val creds = _credentials.value
        if (creds != null) {
            // Best-effort remote revoke; local credentials are always cleared even when offline.
            runCatching { apiClient.unpair(creds) }
        }
        clearLocalCredentials()
        return Result.success(Unit)
    }

    fun clearLocalCredentials() {
        tokenStore.clear()
        _credentials.value = null
        _status.value = null
        _connectionState.value = ConnectionState.DISCONNECTED
        _lastError.value = null
        _lastUpdatedMillis.value = null
    }

    fun openGlobalEventStream(listener: EventSourceListener): EventSource? {
        val creds = _credentials.value ?: return null
        return apiClient.openEventStream(creds, paneId = null, listener = listener)
    }

    fun openPaneTailStream(
        paneId: String,
        onTail: (PaneTailResponse) -> Unit,
        onSnapshot: (RemoteStatusResponse) -> Unit = { updateFromSnapshot(it) },
        onError: (Throwable?, Int?) -> Unit = { _, _ -> },
    ): EventSource? {
        val creds = _credentials.value ?: return null
        return apiClient.openEventStream(
            creds = creds,
            paneId = paneId,
            listener = object : EventSourceListener() {
                override fun onEvent(
                    eventSource: EventSource,
                    id: String?,
                    type: String?,
                    data: String,
                ) {
                    when (type) {
                        "pane_tail" -> runCatching {
                            RemoteJson.decodeFromString(PaneTailResponse.serializer(), data)
                        }.onSuccess(onTail)

                        "snapshot" -> runCatching {
                            RemoteJson.decodeFromString(RemoteStatusResponse.serializer(), data)
                        }.onSuccess(onSnapshot)
                    }
                }

                override fun onFailure(
                    eventSource: EventSource,
                    t: Throwable?,
                    response: Response?,
                ) {
                    onError(t, response?.code)
                }
            },
        )
    }

    fun updateFromSnapshot(snapshot: RemoteStatusResponse) {
        _status.value = snapshot
        _connectionState.value = ConnectionState.CONNECTED
        _lastError.value = null
        _lastUpdatedMillis.value = System.currentTimeMillis()
    }

    fun updateConnectionState(state: ConnectionState, errorMessage: String? = null) {
        _connectionState.value = state
        if (errorMessage != null) {
            _lastError.value = errorMessage
        } else if (state == ConnectionState.CONNECTED) {
            _lastError.value = null
        }
    }
}
