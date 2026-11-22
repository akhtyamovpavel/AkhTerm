package org.akhterm.mobile.data.pairing

import java.net.URI
import java.net.URLDecoder
import java.nio.charset.StandardCharsets

data class PairingPayload(
    val baseUrl: String,
    val code: String,
    val hostName: String?,
    val apiVersion: Int = 1,
)

object QrPairingParser {
    private val HEX_32_REGEX = Regex("^[0-9a-fA-F]{32}$")

    /**
     * Parses both QR payload formats defined in docs/remote-api-v1.md:
     * 1. http(s)://<ip-or-host>[:<port>]/?pair=<32-hex-code>&host=<url-encoded-host-name>&v=1
     * 2. akhterm://pair?v=1&url=<url-encoded-base-url>&code=<32-hex-code>&host=<url-encoded-host-name>
     */
    fun parse(raw: String): PairingPayload? {
        val trimmed = raw.trim()
        if (trimmed.isEmpty()) return null

        val schemeSep = trimmed.indexOf("://")
        if (schemeSep <= 0) return null
        val scheme = trimmed.substring(0, schemeSep).lowercase()

        return when (scheme) {
            "akhterm" -> parseDeepLink(trimmed)
            "http", "https" -> parseHttpUrl(trimmed)
            else -> null
        }
    }

    /**
     * Normalizes a user-typed server URL (must be http:// or https:// with a valid host).
     * Strips trailing slashes, query parameters, and fragments.
     */
    fun normalizeBaseUrl(raw: String): String? {
        val trimmed = raw.trim()
        if (trimmed.isEmpty()) return null
        val withScheme = if (trimmed.contains("://")) trimmed else "http://$trimmed"
        val uri = try {
            URI(withScheme)
        } catch (_: Exception) {
            return null
        }
        val scheme = uri.scheme?.lowercase() ?: return null
        if (scheme != "http" && scheme != "https") return null
        val host = uri.host?.takeIf { it.isNotBlank() } ?: return null
        if (uri.port != -1 && (uri.port !in 1..65535)) return null
        // Reject embedded credentials in URL
        if (!uri.userInfo.isNullOrEmpty()) return null

        val normalizedHost = if (host.contains(':') && !host.startsWith('[')) {
            "[$host]"
        } else {
            host
        }
        val portPart = if (uri.port != -1) ":${uri.port}" else ""
        val cleanPath = (uri.rawPath ?: "").trimEnd('/')
        return "$scheme://$normalizedHost$portPart$cleanPath"
    }

    private fun parseDeepLink(raw: String): PairingPayload? {
        val (prefix, rawQuery) = splitQuery(raw) ?: return null
        val authority = prefix.removePrefix("akhterm://").substringBefore('/').lowercase()
        if (authority != "pair") return null

        val params = parseQueryParams(rawQuery)
        val version = parseVersion(params["v"]) ?: return null
        val rawUrl = params["url"]?.trim().orEmpty()
        val baseUrl = normalizeBaseUrl(rawUrl) ?: return null
        val code = params["code"]?.trim().orEmpty()
        if (!isValidPairingCode(code)) return null

        val hostName = params["host"]?.trim()?.takeIf { it.isNotEmpty() }
        return PairingPayload(
            baseUrl = baseUrl,
            code = code,
            hostName = hostName,
            apiVersion = version,
        )
    }

    private fun parseHttpUrl(raw: String): PairingPayload? {
        val (prefix, rawQuery) = splitQuery(raw) ?: return null
        val baseUrl = normalizeBaseUrl(prefix) ?: return null
        val params = parseQueryParams(rawQuery)
        val version = parseVersion(params["v"]) ?: return null
        val code = params["pair"]?.trim().orEmpty()
        if (!isValidPairingCode(code)) return null

        val hostName = params["host"]?.trim()?.takeIf { it.isNotEmpty() }
        return PairingPayload(
            baseUrl = baseUrl,
            code = code,
            hostName = hostName,
            apiVersion = version,
        )
    }

    private fun parseVersion(rawVersion: String?): Int? {
        if (rawVersion == null) return 1
        val parsed = rawVersion.trim().toIntOrNull() ?: return null
        return if (parsed == 1) 1 else null
    }

    private fun isValidPairingCode(code: String): Boolean = HEX_32_REGEX.matches(code)

    private fun splitQuery(raw: String): Pair<String, String>? {
        val withoutFragment = raw.substringBefore('#')
        val queryIndex = withoutFragment.indexOf('?')
        if (queryIndex < 0) return null
        val prefix = withoutFragment.substring(0, queryIndex)
        val query = withoutFragment.substring(queryIndex + 1)
        if (query.isEmpty()) return null
        return prefix to query
    }

    private fun parseQueryParams(rawQuery: String): Map<String, String> {
        val result = LinkedHashMap<String, String>()
        for (part in rawQuery.split('&')) {
            if (part.isEmpty()) continue
            val eqIndex = part.indexOf('=')
            val rawKey = if (eqIndex >= 0) part.substring(0, eqIndex) else part
            val rawValue = if (eqIndex >= 0) part.substring(eqIndex + 1) else ""
            val key = decodeComponent(rawKey) ?: continue
            val value = decodeComponent(rawValue) ?: continue
            if (key.isNotEmpty() && !result.containsKey(key)) {
                result[key] = value
            }
        }
        return result
    }

    private fun decodeComponent(value: String): String? = try {
        URLDecoder.decode(value, StandardCharsets.UTF_8.name())
    } catch (_: IllegalArgumentException) {
        null
    }
}
