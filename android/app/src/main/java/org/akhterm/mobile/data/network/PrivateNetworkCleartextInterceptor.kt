package org.akhterm.mobile.data.network

import okhttp3.Interceptor
import okhttp3.Response
import java.io.IOException

class CleartextNotPermittedException(
    val host: String,
) : IOException(
    "Cleartext HTTP is only allowed on localhost, RFC 1918 LAN, and Tailscale (100.64.0.0/10); use HTTPS for $host",
)

/**
 * Enforces transport security for AkhTerm Mobile:
 * - HTTPS is always allowed.
 * - Cleartext HTTP is allowed ONLY for:
 *   - localhost
 *   - 127.0.0.0/8 (IPv4 loopback)
 *   - 10.0.0.0/8 (RFC 1918)
 *   - 172.16.0.0/12 (RFC 1918)
 *   - 192.168.0.0/16 (RFC 1918)
 *   - 100.64.0.0/10 (CGNAT / Tailscale)
 *   - ::1 (IPv6 loopback)
 * - Public IP addresses and domains MUST use HTTPS.
 */
class PrivateNetworkCleartextInterceptor : Interceptor {
    override fun intercept(chain: Interceptor.Chain): Response {
        val request = chain.request()
        val url = request.url
        if (!url.isHttps) {
            if (!isPrivateCleartextAllowed(url.host)) {
                throw CleartextNotPermittedException(url.host)
            }
        }
        return chain.proceed(request)
    }

    companion object {
        fun isPrivateCleartextAllowed(rawHost: String): Boolean {
            val host = rawHost.trim().removePrefix("[").removeSuffix("]").lowercase()
            if (host.isEmpty()) return false

            if (host == "localhost" || host == "localhost.") {
                return true
            }

            parseIpv4(host)?.let { octets ->
                return isAllowedIpv4(octets)
            }

            if (host.contains(':')) {
                val ipv6 = parseIpv6(host) ?: return false
                return isIpv6Loopback(ipv6)
            }

            return false
        }

        private fun isAllowedIpv4(octets: IntArray): Boolean {
            val b0 = octets[0]
            val b1 = octets[1]
            return when {
                b0 == 127 -> true // 127.0.0.0/8
                b0 == 10 -> true // 10.0.0.0/8
                b0 == 172 && b1 in 16..31 -> true // 172.16.0.0/12
                b0 == 192 && b1 == 168 -> true // 192.168.0.0/16
                b0 == 100 && b1 in 64..127 -> true // 100.64.0.0/10 (CGNAT / Tailscale)
                else -> false
            }
        }

        private fun parseIpv4(host: String): IntArray? {
            val parts = host.split('.')
            if (parts.size != 4) return null
            val octets = IntArray(4)
            for (i in 0 until 4) {
                val part = parts[i]
                if (part.isEmpty() || part.length > 3) return null
                if (part.length > 1 && part[0] == '0') return null
                var value = 0
                for (ch in part) {
                    if (ch !in '0'..'9') return null
                    value = value * 10 + (ch - '0')
                }
                if (value !in 0..255) return null
                octets[i] = value
            }
            return octets
        }

        private fun parseIpv6(raw: String): ByteArray? {
            val host = raw.substringBefore('%')
            val doubleColonCount = Regex("::").findAll(host).count()
            if (doubleColonCount > 1) return null

            val groups = mutableListOf<Int>()
            if (doubleColonCount == 1) {
                val sides = host.split("::")
                if (sides.size != 2) return null
                val left = parseIpv6Groups(sides[0]) ?: return null
                val right = parseIpv6Groups(sides[1]) ?: return null
                val missing = 8 - (left.size + right.size)
                if (missing < 1) return null
                groups.addAll(left)
                repeat(missing) { groups.add(0) }
                groups.addAll(right)
            } else {
                val parsed = parseIpv6Groups(host) ?: return null
                if (parsed.size != 8) return null
                groups.addAll(parsed)
            }

            val bytes = ByteArray(16)
            for (i in 0 until 8) {
                bytes[i * 2] = ((groups[i] ushr 8) and 0xFF).toByte()
                bytes[i * 2 + 1] = (groups[i] and 0xFF).toByte()
            }
            return bytes
        }

        private fun parseIpv6Groups(segment: String): List<Int>? {
            if (segment.isEmpty()) return emptyList()
            val parts = segment.split(':')
            val out = ArrayList<Int>(parts.size + 1)
            for ((index, part) in parts.withIndex()) {
                if (part.isEmpty()) return null
                if (part.contains('.')) {
                    if (index != parts.lastIndex) return null
                    val ipv4 = parseIpv4(part) ?: return null
                    out.add((ipv4[0] shl 8) or ipv4[1])
                    out.add((ipv4[2] shl 8) or ipv4[3])
                } else {
                    if (part.length > 4) return null
                    val value = part.toIntOrNull(16) ?: return null
                    if (value !in 0..0xFFFF) return null
                    out.add(value)
                }
            }
            return out
        }

        private fun isIpv6Loopback(bytes: ByteArray): Boolean {
            if (bytes.size != 16) return false
            for (i in 0 until 15) {
                if (bytes[i] != 0.toByte()) return false
            }
            return bytes[15] == 1.toByte()
        }
    }
}
