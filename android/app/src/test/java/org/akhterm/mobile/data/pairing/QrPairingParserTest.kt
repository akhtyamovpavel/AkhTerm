package org.akhterm.mobile.data.pairing

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Test

class QrPairingParserTest {
    private val validCode = "0123456789abcdef0123456789abcdef"
    private val upperCode = "ABCDEF0123456789ABCDEF0123456789"

    @Test
    fun parsesHttpQrUrlFormat() {
        val raw = "http://192.168.1.42:8787/?pair=$validCode&host=pavel-laptop&v=1"
        val parsed = QrPairingParser.parse(raw)
        assertNotNull(parsed)
        assertEquals("http://192.168.1.42:8787", parsed!!.baseUrl)
        assertEquals(validCode, parsed.code)
        assertEquals("pavel-laptop", parsed.hostName)
        assertEquals(1, parsed.apiVersion)
    }

    @Test
    fun parsesHttpsQrUrlWithSubpathAndUppercaseCode() {
        val raw = "https://terminal.example.com:8443/akhterm/?pair=$upperCode&host=workstation&v=1"
        val parsed = QrPairingParser.parse(raw)
        assertNotNull(parsed)
        assertEquals("https://terminal.example.com:8443/akhterm", parsed!!.baseUrl)
        assertEquals(upperCode, parsed.code)
        assertEquals("workstation", parsed.hostName)
    }

    @Test
    fun parsesAkhtermDeepLinkFormat() {
        val raw = "akhterm://pair?v=1&url=http%3A%2F%2F100.64.12.34%3A8787&code=$validCode&host=pavel-laptop"
        val parsed = QrPairingParser.parse(raw)
        assertNotNull(parsed)
        assertEquals("http://100.64.12.34:8787", parsed!!.baseUrl)
        assertEquals(validCode, parsed.code)
        assertEquals("pavel-laptop", parsed.hostName)
        assertEquals(1, parsed.apiVersion)
    }

    @Test
    fun parsesPercentEncodedCyrillicHostNameInBothFormats() {
        val encodedCyrillic = "%D0%9D%D0%BE%D1%83%D1%82%D0%B1%D1%83%D0%BA+%D0%9F%D0%B0%D0%B2%D0%BB%D0%B0"

        val httpRaw = "http://10.0.0.5:8787/?pair=$validCode&host=$encodedCyrillic&v=1"
        val httpParsed = QrPairingParser.parse(httpRaw)
        assertNotNull(httpParsed)
        assertEquals("Ноутбук Павла", httpParsed!!.hostName)

        val deepRaw = "akhterm://pair?v=1&url=https%3A%2F%2Fakhterm.example.org&code=$validCode&host=$encodedCyrillic"
        val deepParsed = QrPairingParser.parse(deepRaw)
        assertNotNull(deepParsed)
        assertEquals("https://akhterm.example.org", deepParsed!!.baseUrl)
        assertEquals("Ноутбук Павла", deepParsed.hostName)
    }

    @Test
    fun parsesUnencodedUtf8CyrillicHostName() {
        val raw = "http://100.80.1.2:8787/?pair=$validCode&host=Рабочий-ПК&v=1"
        val parsed = QrPairingParser.parse(raw)
        assertNotNull(parsed)
        assertEquals("Рабочий-ПК", parsed!!.hostName)
    }

    @Test
    fun parsesIpv6LoopbackUrlAndMissingOptionalHost() {
        val raw = "http://[::1]:8787/?pair=$validCode&v=1"
        val parsed = QrPairingParser.parse(raw)
        assertNotNull(parsed)
        assertEquals("http://[::1]:8787", parsed!!.baseUrl)
        assertNull(parsed.hostName)
    }

    @Test
    fun rejectsInvalidInputs() {
        val invalidPayloads = listOf(
            "",
            "   ",
            "not-a-url",
            "ftp://192.168.1.1:21/?pair=$validCode&v=1",
            "http://192.168.1.1:8787/",
            "http://192.168.1.1:8787/?pair=tooshort&v=1",
            "http://192.168.1.1:8787/?pair=zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz&v=1",
            "http://192.168.1.1:8787/?pair=$validCode&v=2",
            "http://192.168.1.1:8787/?pair=$validCode&v=abc",
            "akhterm://other?v=1&url=http%3A%2F%2F127.0.0.1%3A8787&code=$validCode",
            "akhterm://pair?v=2&url=http%3A%2F%2F127.0.0.1%3A8787&code=$validCode",
            "akhterm://pair?v=1&url=ftp%3A%2F%2F127.0.0.1&code=$validCode",
            "akhterm://pair?v=1&code=$validCode",
            "akhterm://pair?v=1&url=http%3A%2F%2F127.0.0.1%3A8787",
            "http://user:pass@127.0.0.1:8787/?pair=$validCode&v=1",
        )
        for (payload in invalidPayloads) {
            assertNull("Expected null for invalid payload: $payload", QrPairingParser.parse(payload))
        }
    }

    @Test
    fun normalizesManualUrlsCleanly() {
        assertEquals("http://192.168.0.10:8787", QrPairingParser.normalizeBaseUrl("192.168.0.10:8787/"))
        assertEquals("https://box.example.com", QrPairingParser.normalizeBaseUrl("https://box.example.com///"))
        assertEquals("http://[::1]:8787", QrPairingParser.normalizeBaseUrl("http://[::1]:8787/"))
        assertNull(QrPairingParser.normalizeBaseUrl(""))
        assertNull(QrPairingParser.normalizeBaseUrl("ftp://example.com"))
        assertNull(QrPairingParser.normalizeBaseUrl("http://user:secret@10.0.0.1:8787"))
    }
}
