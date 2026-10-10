package com.unoone.agent.peersync

import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import java.io.InputStream
import java.net.InetAddress
import java.net.InetSocketAddress
import java.net.Socket
import java.security.KeyPairGenerator
import java.security.KeyStore
import java.security.Principal
import java.security.PrivateKey
import java.security.SecureRandom
import java.security.cert.X509Certificate
import java.security.spec.ECGenParameterSpec
import java.util.Date
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit
import javax.net.ssl.*
import javax.security.auth.x500.X500Principal
import kotlinx.serialization.encodeToString

/** Actual Android platform TLS1.3 HTTPS over local numeric IPv4 sockets. No mock, DNS,
 * proxy, redirect, system-root fallback, trust-on-first-message or cleartext transport.
 * Keystore private key is non-exportable; possession is verified by TLS CertificateVerify. */
object NativePeerHttps {
    private fun store() = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
    fun generate(alias: String): String {
        val ks = store(); require(!ks.containsAlias(alias)) { "Key alias already exists; no replacement" }
        val generator = KeyPairGenerator.getInstance(KeyProperties.KEY_ALGORITHM_EC, "AndroidKeyStore")
        generator.initialize(KeyGenParameterSpec.Builder(alias, KeyProperties.PURPOSE_SIGN or KeyProperties.PURPOSE_VERIFY)
            .setAlgorithmParameterSpec(ECGenParameterSpec("secp256r1"))
            .setDigests(KeyProperties.DIGEST_SHA256)
            .setCertificateSubject(X500Principal("CN=unoone.local"))
            .setCertificateSerialNumber(java.math.BigInteger(128, SecureRandom()))
            .setCertificateNotBefore(Date(System.currentTimeMillis() - 86400000))
            .setCertificateNotAfter(Date(System.currentTimeMillis() + 10L * 365 * 86400000))
            .build())
        generator.generateKeyPair()
        return fingerprint(alias)
    }
    fun fingerprint(alias: String): String = PeerProtocol.hash(requireNotNull(store().getCertificate(alias)) { "Pairing key lost; no silent regeneration" }.encoded)
    private class Keys(private val alias: String) : X509ExtendedKeyManager() {
        private val ks = store()
        override fun getClientAliases(keyType: String?, issuers: Array<out Principal>?): Array<String>? = if (keyType == "EC") arrayOf(alias) else null
        override fun chooseClientAlias(keyTypes: Array<out String>?, issuers: Array<out Principal>?, socket: Socket?): String? = if (keyTypes?.contains("EC") == true) alias else null
        override fun chooseEngineClientAlias(keyTypes: Array<out String>?, issuers: Array<out Principal>?, engine: SSLEngine?): String? = chooseClientAlias(keyTypes, issuers, null)
        override fun getServerAliases(keyType: String?, issuers: Array<out Principal>?): Array<String>? = null
        override fun chooseServerAlias(keyType: String?, issuers: Array<out Principal>?, socket: Socket?): String? = null
        override fun getCertificateChain(alias: String?): Array<X509Certificate>? = if (alias == this.alias) arrayOf(ks.getCertificate(this.alias) as X509Certificate) else null
        override fun getPrivateKey(alias: String?): PrivateKey? = if (alias == this.alias) ks.getKey(this.alias, null) as PrivateKey else null
    }
    /**
     * Exact out-of-band certificate pin for a paired peer. Peer certificates are self-issued
     * device keys that no public CA can validate, so the platform default trust manager
     * cannot apply; trust is the user-approved full SHA-256 of the exact DER certificate.
     * TLS 1.3 signature verification of that pinned key is still performed by the TLS stack.
     * Any other certificate, chain length or oversized DER is rejected.
     */
    @android.annotation.SuppressLint("CustomX509TrustManager")
    private class Pin(private val expected: String) : X509TrustManager {
        override fun getAcceptedIssuers(): Array<X509Certificate> = emptyArray()
        override fun checkClientTrusted(chain: Array<out X509Certificate>?, authType: String?) { error("Client-only adapter") }
        @android.annotation.SuppressLint("TrustAllX509TrustManager")
        override fun checkServerTrusted(chain: Array<out X509Certificate>?, authType: String?) {
            require(chain?.size == 1 && chain[0].encoded.size <= 8192 && PeerProtocol.hash(chain[0].encoded) == expected) { "Unapproved peer certificate" }
            chain[0].checkValidity()
        }
    }
    fun address(text: String): InetSocketAddress {
        require(text.length <= 80)
        val split = text.split(':'); require(split.size == 2) { "Use numeric local IPv4:port" }
        val octets = split[0].split('.'); require(octets.size == 4)
        val n = octets.map { require(it.isNotEmpty() && it.all(Char::isDigit) && it.length <= 3); it.toInt().also { i -> require(i in 0..255) } }
        val local = n[0] == 127 || n[0] == 10 || (n[0] == 172 && n[1] in 16..31) || (n[0] == 192 && n[1] == 168) || (n[0] == 169 && n[1] == 254)
        require(local) { "Only loopback/private LAN IPv4 addresses allowed" }
        val port = split[1].toInt(); require(port in 1..65535)
        return InetSocketAddress(InetAddress.getByAddress(n.map(Int::toByte).toByteArray()), port)
    }
    fun exchange(addressText: String, state: PeerState, request: PeerExchange, allowed: () -> Boolean, onSocket: (Socket?) -> Unit): PeerReply {
        require(android.os.Build.VERSION.SDK_INT >= 29) { "TLS 1.3 peer sync requires Android 10 or later; local tasks remain available" }
        val peer = state.active(); require(allowed()); require(fingerprint(state.key_alias) == state.local.fingerprint)
        val ctx = SSLContext.getInstance("TLSv1.3"); ctx.init(arrayOf(Keys(state.key_alias)), arrayOf(Pin(peer.offer.fingerprint)), SecureRandom())
        val endpoint = address(addressText)
        val socket = Socket(); onSocket(socket)
        val deadline = Executors.newSingleThreadScheduledExecutor()
        // Also closes promptly on vault lock/screen stop, even while blocked inside TLS.
        val guard = deadline.scheduleWithFixedDelay({ if (!allowed()) runCatching { socket.close() } }, 0, 50, TimeUnit.MILLISECONDS)
        val timeout = deadline.schedule({ runCatching { socket.close() } }, 15, TimeUnit.SECONDS)
        try {
            socket.connect(endpoint, 3000); socket.soTimeout = 2000; require(allowed())
            (ctx.socketFactory.createSocket(socket, "unoone.local", endpoint.port, true) as SSLSocket).use { tls ->
                tls.enabledProtocols = arrayOf("TLSv1.3"); tls.useClientMode = true; tls.startHandshake(); require(tls.session.protocol == "TLSv1.3" && allowed())
                val body = PeerProtocol.json.encodeToString(request).toByteArray(); require(body.size in 1..PeerProtocol.MAX_BODY)
                val output = tls.outputStream
                output.write(("POST /unoone-peer-v1 HTTP/1.1\r\nHost: unoone.local\r\nContent-Type: application/json\r\nContent-Length: ${body.size}\r\nConnection: close\r\n\r\n").toByteArray(Charsets.US_ASCII)); require(allowed()); output.write(body); output.flush()
                val response = readResponse(tls.inputStream); require(allowed())
                val text = Charsets.UTF_8.newDecoder().decode(java.nio.ByteBuffer.wrap(response)).toString()
                PeerProtocol.preflight(text, PeerProtocol.MAX_BODY)
                return PeerProtocol.json.decodeFromString<PeerReply>(text)
            }
        } finally { timeout.cancel(false); guard.cancel(false); deadline.shutdownNow(); socket.close(); onSocket(null) }
    }
    internal fun readResponse(input: InputStream): ByteArray {
        val headers = java.io.ByteArrayOutputStream()
        while (true) { require(headers.size() < 2048) { "Header bound" }; val b = input.read(); require(b >= 0) { "Truncated TLS stream" }; headers.write(b); if (headers.size() >= 4 && headers.toByteArray().takeLast(4) == listOf<Byte>(13,10,13,10)) break }
        val lines = headers.toString("US-ASCII").split("\r\n"); require(lines.first() == "HTTP/1.1 200 OK")
        var length: Int? = null
        lines.drop(1).filter { it.isNotEmpty() }.forEach { line -> val pair = line.split(':', limit = 2); require(pair.size == 2); when (pair[0].lowercase()) {
            "content-length" -> { require(length == null); length = pair[1].trim().toInt() }
            "host", "content-type", "connection" -> Unit
            else -> error("Unsupported HTTP framing")
        } }
        val size = requireNotNull(length); require(size in 1..PeerProtocol.MAX_BODY)
        val body = ByteArray(size); var n = 0
        while (n < size) { val read = input.read(body, n, size - n); require(read > 0) { "Truncated body; no ACK" }; n += read }; return body
    }
}
