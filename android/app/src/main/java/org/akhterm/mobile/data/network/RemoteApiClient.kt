package org.akhterm.mobile.data.network

import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import kotlinx.serialization.KSerializer
import kotlinx.serialization.encodeToString
import okhttp3.HttpUrl
import okhttp3.HttpUrl.Companion.toHttpUrlOrNull
import okhttp3.MediaType.Companion.toMediaType
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.RequestBody.Companion.toRequestBody
import okhttp3.sse.EventSource
import okhttp3.sse.EventSourceListener
import okhttp3.sse.EventSources
import org.akhterm.mobile.data.model.ApprovalAction
import org.akhterm.mobile.data.model.ApproveRequest
import org.akhterm.mobile.data.model.ApproveResponse
import org.akhterm.mobile.data.model.ErrorResponse
import org.akhterm.mobile.data.model.FocusResponse
import org.akhterm.mobile.data.model.HealthResponse
import org.akhterm.mobile.data.model.PairRequest
import org.akhterm.mobile.data.model.PairResponse
import org.akhterm.mobile.data.model.PairedCredentials
import org.akhterm.mobile.data.model.PaneTailResponse
import org.akhterm.mobile.data.model.RemoteJson
import org.akhterm.mobile.data.model.RemoteStatusResponse
import org.akhterm.mobile.data.model.SendKeyRequest
import org.akhterm.mobile.data.model.SendKeyResponse
import org.akhterm.mobile.data.model.SendPromptRequest
import org.akhterm.mobile.data.model.SendPromptResponse
import org.akhterm.mobile.data.model.TuiKey
import org.akhterm.mobile.data.model.UnpairResponse
import java.io.IOException
import java.util.concurrent.TimeUnit

class ApiException(
    val httpCode: Int,
    override val message: String,
) : IOException(message)

class RemoteApiClient(
    baseClient: OkHttpClient = defaultOkHttpClient(),
) {
    private val restClient: OkHttpClient = baseClient.newBuilder()
        .connectTimeout(10, TimeUnit.SECONDS)
        .readTimeout(15, TimeUnit.SECONDS)
        .writeTimeout(15, TimeUnit.SECONDS)
        .build()

    // Server emits SSE heartbeats every 15s; 45s read timeout detects silently dead TCP links.
    private val sseClient: OkHttpClient = baseClient.newBuilder()
        .connectTimeout(10, TimeUnit.SECONDS)
        .readTimeout(45, TimeUnit.SECONDS)
        .writeTimeout(15, TimeUnit.SECONDS)
        .retryOnConnectionFailure(true)
        .build()

    private val eventSourceFactory: EventSource.Factory = EventSources.createFactory(sseClient)

    suspend fun health(baseUrl: String): Result<HealthResponse> = executeRequest(
        buildRequest(baseUrl, listOf("api", "v1", "health")) { get() },
        HealthResponse.serializer(),
    )

    suspend fun pair(
        baseUrl: String,
        code: String,
        deviceName: String,
    ): Result<PairResponse> {
        val body = RemoteJson.encodeToString(
            PairRequest(code = code, deviceName = deviceName, platform = "android"),
        ).toRequestBody(JSON_MEDIA_TYPE)
        return executeRequest(
            buildRequest(baseUrl, listOf("api", "v1", "pair")) { post(body) },
            PairResponse.serializer(),
        )
    }

    suspend fun getStatus(creds: PairedCredentials): Result<RemoteStatusResponse> = executeRequest(
        buildRequest(creds.baseUrl, listOf("api", "v1", "status"), token = creds.token) { get() },
        RemoteStatusResponse.serializer(),
    )

    suspend fun getPaneTail(
        creds: PairedCredentials,
        paneId: String,
        lines: Int = 100,
    ): Result<PaneTailResponse> {
        val clamped = lines.coerceIn(1, 200)
        return executeRequest(
            buildRequest(
                creds.baseUrl,
                listOf("api", "v1", "panes", paneId, "tail"),
                query = mapOf("lines" to clamped.toString()),
                token = creds.token,
            ) { get() },
            PaneTailResponse.serializer(),
        )
    }

    suspend fun approve(
        creds: PairedCredentials,
        paneId: String,
        action: ApprovalAction,
    ): Result<ApproveResponse> {
        val body = RemoteJson.encodeToString(ApproveRequest(action = action))
            .toRequestBody(JSON_MEDIA_TYPE)
        return executeRequest(
            buildRequest(
                creds.baseUrl,
                listOf("api", "v1", "panes", paneId, "approve"),
                token = creds.token,
            ) { post(body) },
            ApproveResponse.serializer(),
        )
    }

    suspend fun sendPrompt(
        creds: PairedCredentials,
        paneId: String,
        text: String,
        enter: Boolean = true,
    ): Result<SendPromptResponse> {
        val body = RemoteJson.encodeToString(SendPromptRequest(text = text, enter = enter))
            .toRequestBody(JSON_MEDIA_TYPE)
        return executeRequest(
            buildRequest(
                creds.baseUrl,
                listOf("api", "v1", "panes", paneId, "send"),
                token = creds.token,
            ) { post(body) },
            SendPromptResponse.serializer(),
        )
    }

    suspend fun sendKey(
        creds: PairedCredentials,
        paneId: String,
        key: TuiKey,
    ): Result<SendKeyResponse> {
        val body = RemoteJson.encodeToString(SendKeyRequest(key = key))
            .toRequestBody(JSON_MEDIA_TYPE)
        return executeRequest(
            buildRequest(
                creds.baseUrl,
                listOf("api", "v1", "panes", paneId, "key"),
                token = creds.token,
            ) { post(body) },
            SendKeyResponse.serializer(),
        )
    }

    suspend fun focusPane(
        creds: PairedCredentials,
        paneId: String,
    ): Result<FocusResponse> {
        val emptyBody = "".toRequestBody(null)
        return executeRequest(
            buildRequest(
                creds.baseUrl,
                listOf("api", "v1", "panes", paneId, "focus"),
                token = creds.token,
            ) { post(emptyBody) },
            FocusResponse.serializer(),
        )
    }

    suspend fun unpair(creds: PairedCredentials): Result<UnpairResponse> {
        val emptyBody = "".toRequestBody(null)
        return executeRequest(
            buildRequest(
                creds.baseUrl,
                listOf("api", "v1", "unpair"),
                token = creds.token,
            ) { post(emptyBody) },
            UnpairResponse.serializer(),
        )
    }

    fun openEventStream(
        creds: PairedCredentials,
        paneId: String? = null,
        listener: EventSourceListener,
    ): EventSource {
        val query = if (paneId.isNullOrBlank()) emptyMap() else mapOf("pane_id" to paneId)
        val request = buildRequest(
            creds.baseUrl,
            listOf("api", "v1", "events"),
            query = query,
            token = creds.token,
        ) {
            header("Accept", "text/event-stream")
            header("Cache-Control", "no-cache")
            get()
        }
        return eventSourceFactory.newEventSource(request, listener)
    }

    private fun buildRequest(
        baseUrl: String,
        segments: List<String>,
        query: Map<String, String> = emptyMap(),
        token: String? = null,
        configure: Request.Builder.() -> Unit,
    ): Request {
        val httpUrl = resolveUrl(baseUrl, segments, query)
        return Request.Builder()
            .url(httpUrl)
            .apply {
                header("Accept", "application/json")
                if (!token.isNullOrBlank()) {
                    header("Authorization", "Bearer $token")
                }
                configure()
            }
            .build()
    }

    private fun resolveUrl(
        baseUrl: String,
        segments: List<String>,
        query: Map<String, String>,
    ): HttpUrl {
        val parsed = baseUrl.trimEnd('/').toHttpUrlOrNull()
            ?: throw IOException("Invalid server URL: $baseUrl")
        val builder = parsed.newBuilder()
        for (segment in segments) {
            builder.addPathSegment(segment)
        }
        for ((k, v) in query) {
            builder.addQueryParameter(k, v)
        }
        return builder.build()
    }

    private suspend fun <T> executeRequest(
        request: Request,
        serializer: KSerializer<T>,
    ): Result<T> = withContext(Dispatchers.IO) {
        try {
            restClient.newCall(request).execute().use { response ->
                val rawBody = response.body?.string().orEmpty()
                if (!response.isSuccessful) {
                    val serverError = try {
                        RemoteJson.decodeFromString(ErrorResponse.serializer(), rawBody).error
                    } catch (_: Exception) {
                        rawBody.take(200).ifBlank { "HTTP ${response.code}" }
                    }
                    return@withContext Result.failure(ApiException(response.code, serverError))
                }
                val parsed = RemoteJson.decodeFromString(serializer, rawBody)
                Result.success(parsed)
            }
        } catch (e: Exception) {
            Result.failure(e)
        }
    }

    companion object {
        private val JSON_MEDIA_TYPE = "application/json; charset=utf-8".toMediaType()

        fun defaultOkHttpClient(): OkHttpClient = OkHttpClient.Builder()
            .addInterceptor(PrivateNetworkCleartextInterceptor())
            .build()
    }
}
