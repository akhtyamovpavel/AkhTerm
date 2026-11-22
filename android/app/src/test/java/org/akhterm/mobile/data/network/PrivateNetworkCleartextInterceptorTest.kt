package org.akhterm.mobile.data.network

import okhttp3.OkHttpClient
import okhttp3.Protocol
import okhttp3.Request
import okhttp3.Response
import okhttp3.ResponseBody.Companion.toResponseBody
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Test

class PrivateNetworkCleartextInterceptorTest {

    @Test
    fun allowsLoopbackHostsOnHttp() {
        val allowed = listOf(
            "localhost",
            "LOCALHOST",
            "localhost.",
            "127.0.0.1",
            "127.0.0.2",
            "127.255.255.254",
            "::1",
            "[::1]",
            "0:0:0:0:0:0:0:1",
            "0000:0000:0000:0000:0000:0000:0000:0001",
        )
        for (host in allowed) {
            assertTrue("Expected allowed loopback: $host", PrivateNetworkCleartextInterceptor.isPrivateCleartextAllowed(host))
        }
    }

    @Test
    fun allowsRfc1918PrivateNetworksOnHttp() {
        val allowed = listOf(
            "10.0.0.0",
            "10.0.0.1",
            "10.255.255.255",
            "172.16.0.0",
            "172.16.0.1",
            "172.24.12.5",
            "172.31.255.255",
            "192.168.0.0",
            "192.168.1.42",
            "192.168.255.255",
        )
        for (host in allowed) {
            assertTrue("Expected allowed RFC1918: $host", PrivateNetworkCleartextInterceptor.isPrivateCleartextAllowed(host))
        }
    }

    @Test
    fun allowsTailscaleCgnatRangeOnHttp() {
        val allowed = listOf(
            "100.64.0.0",
            "100.64.0.1",
            "100.100.100.100",
            "100.127.255.255",
        )
        for (host in allowed) {
            assertTrue("Expected allowed CGNAT/Tailscale: $host", PrivateNetworkCleartextInterceptor.isPrivateCleartextAllowed(host))
        }
    }

    @Test
    fun rejectsPublicIpsRangeBoundariesAndPublicDomainsOnHttp() {
        val rejected = listOf(
            "",
            "0.0.0.0",
            "1.1.1.1",
            "8.8.8.8",
            "9.255.255.255",
            "11.0.0.1",
            "100.63.255.255",
            "100.128.0.0",
            "126.255.255.255",
            "128.0.0.1",
            "172.15.255.255",
            "172.32.0.0",
            "192.167.255.255",
            "192.169.0.1",
            "203.0.113.10",
            "2001:4860:4860::8888",
            "::2",
            "::",
            "example.com",
            "akhterm.org",
            "localhost.evil.com",
            "192.168.1.1.nip.io",
            "my-laptop.ts.net",
            "192.168.01.1",
        )
        for (host in rejected) {
            assertFalse("Expected rejected host on HTTP: $host", PrivateNetworkCleartextInterceptor.isPrivateCleartextAllowed(host))
        }
    }

    @Test
    fun interceptorAllowsHttpsForPublicDomainsAndPublicIps() {
        val client = buildMockTransportClient()

        for (url in listOf("https://example.com/api/v1/health", "https://1.1.1.1/api/v1/health")) {
            val request = Request.Builder().url(url).build()
            client.newCall(request).execute().use { response ->
                assertEquals(200, response.code)
            }
        }
    }

    @Test
    fun interceptorAllowsPrivateHttpAndBlocksPublicHttp() {
        val client = buildMockTransportClient()

        val allowedUrls = listOf(
            "http://localhost:8787/api/v1/health",
            "http://127.0.0.1:8787/api/v1/health",
            "http://10.1.2.3:8787/api/v1/health",
            "http://172.16.5.4:8787/api/v1/health",
            "http://192.168.1.10:8787/api/v1/health",
            "http://100.64.42.7:8787/api/v1/health",
            "http://[::1]:8787/api/v1/health",
        )
        for (url in allowedUrls) {
            client.newCall(Request.Builder().url(url).build()).execute().use { response ->
                assertEquals("Expected 200 for $url", 200, response.code)
            }
        }

        val blockedUrls = listOf(
            "http://example.com/api/v1/health",
            "http://8.8.8.8:8787/api/v1/health",
            "http://172.32.0.1:8787/api/v1/health",
            "http://100.128.0.1:8787/api/v1/health",
            "http://[2001:4860:4860::8888]:8787/api/v1/health",
        )
        for (url in blockedUrls) {
            try {
                client.newCall(Request.Builder().url(url).build()).execute().close()
                fail("Expected CleartextNotPermittedException for $url")
            } catch (_: CleartextNotPermittedException) {
                // expected
            }
        }
    }

    private fun buildMockTransportClient(): OkHttpClient = OkHttpClient.Builder()
        .addInterceptor(PrivateNetworkCleartextInterceptor())
        .addInterceptor { chain ->
            Response.Builder()
                .request(chain.request())
                .protocol(Protocol.HTTP_1_1)
                .code(200)
                .message("OK")
                .body("{}".toResponseBody(null))
                .build()
        }
        .build()
}
