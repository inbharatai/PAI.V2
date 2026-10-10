package com.unoone.agent.core.guardian

import kotlinx.serialization.Serializable
import kotlinx.serialization.encodeToString
import kotlinx.serialization.json.Json

/** Mirror of Rust `privacy-guardian/src/connector.rs`. Default OFFLINE: without a consented manifest every
 * egress is refused. No connector is enabled by default; there is no cloud fallback path. */
const val CONNECTOR_MANIFEST_SCHEMA = "inbharat.pai.connector-manifest"
const val CONNECTOR_MANIFEST_VERSION = 1

internal fun fnv64(bytes: ByteArray): Long {
    var h = -0x340d631b7bdddcdbL // 0xcbf29ce484222325
    for (b in bytes) { h = h xor (b.toLong() and 0xff); h *= 0x100000001b3L }
    return h
}
internal fun hex16(v: Long) = java.lang.Long.toHexString(v).padStart(16, '0')

@Serializable
data class ConnectorManifest(
    val schema: String, val manifest_version: Int, val connector_id: String, val provider: String, val purpose: String,
    val endpoints: List<String>, val permitted_operations: List<String>, val accounts: List<String>, val data_fields: List<String>,
    val token_scopes: List<String>, val retention: String, val expected_cost: String, val max_request_bytes: Long,
    val max_requests_per_day: Int, val expires_at_ms: Long, val revocation: String,
) {
    fun validate() {
        require(schema == CONNECTOR_MANIFEST_SCHEMA && manifest_version == CONNECTOR_MANIFEST_VERSION) { "Unknown manifest schema/version" }
        require(connector_id.isNotBlank() && connector_id.length <= 64 && connector_id.all { it.isLetterOrDigit() && it.code < 128 || it == '-' || it == '.' }) { "Invalid connector id" }
        require(provider.isNotBlank() && provider.length <= 128) { "Provider required" }
        require(purpose.isNotBlank() && purpose.length <= 512) { "Purpose required" }
        require(endpoints.size in 1..16) { "1–16 exact endpoints required" }
        endpoints.forEach { require(GuardianDomain.normalizeHost(it) != null && '/' !in it && '*' !in it && !GuardianDomain.isIpLiteral(it)) { "Endpoints must be exact https host names" } }
        require(permitted_operations.size in 1..32) { "Permitted operations required" }
        require(data_fields.size in 1..64) { "Data fields must be listed" }
        require(accounts.size <= 8 && token_scopes.size <= 32) { "Too many accounts/scopes" }
        require(retention.isNotBlank() && expected_cost.isNotBlank() && revocation.isNotBlank()) { "Retention, cost and revocation text required" }
        require(max_request_bytes in 1..(64L * 1024 * 1024)) { "Request byte ceiling out of range" }
        require(max_requests_per_day in 1..100_000) { "Daily request ceiling out of range" }
        require(expires_at_ms > 0) { "Expiry required" }
    }
    fun isValid() = runCatching { validate() }.isSuccess
    fun digest(): String = hex16(fnv64(Json.encodeToString(this).toByteArray()))
    fun broadScopeReasons(): List<String> {
        val r = mutableListOf<String>()
        val broad = listOf("send", "delete", "export", "write_all", "full_access", "*", "all")
        permitted_operations.forEach { op -> if (broad.any { op.lowercase().contains(it) }) r += "operation '$op' can cause external effects" }
        if (data_fields.size > 12 || data_fields.any { '*' in it || it.lowercase() == "all" }) r += "broad data field scope"
        if (token_scopes.any { it.contains("mail.google.com") || it.endsWith("/gmail.modify") || it.endsWith("/gmail.compose") || it.contains("full_access") }) r += "write-capable token scope"
        if (max_request_bytes > 8L * 1024 * 1024) r += "large per-request byte ceiling"
        if (retention.lowercase().contains("unknown") || retention.lowercase().contains("indefinite")) r += "provider retention is unknown/indefinite"
        return r
    }
    fun consentPreview(): String = "${data_fields.joinToString(", ")} will be sent to $provider (${endpoints.joinToString(", ")}) to $purpose. Operations: ${permitted_operations.joinToString(", ")}. Accounts: ${if (accounts.isEmpty()) "none selected" else accounts.joinToString(", ")}. Token scopes: ${if (token_scopes.isEmpty()) "none" else token_scopes.joinToString(", ")}. Retention: $retention. Expected cost: $expected_cost. Limits: $max_request_bytes bytes/request, $max_requests_per_day requests/day. Expires: $expires_at_ms (ms). Revoke: $revocation."
}

/** Explicit scoped opt-in bound to the exact manifest digest; constructed only by a native consent UI. */
class ConnectorConsent private constructor(val connectorId: String, val manifestDigest: String, val grantedAtMs: Long, val expiresAtMs: Long) {
    var revokedAtMs: Long? = null; private set
    fun revoke(now: Long) { if (revokedAtMs == null) revokedAtMs = now }
    fun active(manifest: ConnectorManifest, now: Long) = revokedAtMs == null && connectorId == manifest.connector_id && manifestDigest == manifest.digest() && now < expiresAtMs && now < manifest.expires_at_ms
    companion object {
        fun grant(manifest: ConnectorManifest, now: Long): ConnectorConsent { manifest.validate(); require(manifest.expires_at_ms > now) { "Manifest already expired" }; return ConnectorConsent(manifest.connector_id, manifest.digest(), now, manifest.expires_at_ms) }
    }
}

data class EgressReceipt(val connectorId: String, val host: String, val bytes: Long, val atMs: Long)

/** Host-owned outbound policy. The model never sees or influences this object. */
class EgressPolicy {
    private val consents = mutableListOf<ConnectorConsent>()
    private val counters = mutableMapOf<Pair<String, Long>, Int>()
    val audit = mutableListOf<EgressReceipt>()
    fun consent(c: ConnectorConsent) { consents.removeAll { it.connectorId == c.connectorId }; consents += c }
    fun revoke(connectorId: String, now: Long): Boolean { var hit = false; consents.filter { it.connectorId == connectorId && it.revokedAtMs == null }.forEach { it.revoke(now); hit = true }; return hit }
    fun isOffline(manifests: List<ConnectorManifest>, now: Long) = consents.none { c -> manifests.any { c.active(it, now) } }
    /** Every outbound request must pass here with its exact URL and byte size. */
    fun authorize(manifests: List<ConnectorManifest>, url: String, bytes: Long, now: Long): EgressReceipt {
        val parsed = GuardianUrl.parse(url) ?: throw IllegalStateException("Egress refused: unparseable destination")
        check(parsed.scheme == "https") { "Egress refused: only https connector endpoints" }
        check(parsed.userInfo == null) { "Egress refused: credentials in URL" }
        val host = parsed.host?.let { GuardianDomain.normalizeHost(it) } ?: throw IllegalStateException("Egress refused: no host")
        for (manifest in manifests) {
            if (!manifest.isValid()) continue
            val consent = consents.find { it.connectorId == manifest.connector_id } ?: continue
            if (!consent.active(manifest, now)) continue
            if (manifest.endpoints.none { GuardianDomain.normalizeHost(it) == host }) continue
            check(bytes <= manifest.max_request_bytes) { "Egress refused: $bytes bytes exceeds connector ceiling ${manifest.max_request_bytes}" }
            val day = now / 86_400_000
            counters.keys.filter { it.second != day }.forEach { counters.remove(it) }
            val key = manifest.connector_id to day
            val count = counters[key] ?: 0
            check(count < manifest.max_requests_per_day) { "Egress refused: connector daily request ceiling reached" }
            counters[key] = count + 1
            val receipt = EgressReceipt(manifest.connector_id, host, bytes, now)
            if (audit.size >= 512) audit.removeAt(0)
            audit += receipt
            return receipt
        }
        throw IllegalStateException("Egress refused: no consented connector declares host $host; device stays offline")
    }
}

/** Minimal deterministic URL splitter shared by the guardian (avoids java.net.URI's IDN/authority quirks). */
object GuardianUrl {
    data class Parts(val scheme: String, val userInfo: String?, val host: String?, val path: String, val query: String?)
    private val schemeRe = Regex("^[A-Za-z][A-Za-z0-9+.-]*$")
    fun parse(raw: String): Parts? {
        val s = raw.trim()
        val colon = s.indexOf(':'); if (colon <= 0) return null
        val scheme = s.substring(0, colon); if (!schemeRe.matches(scheme)) return null
        val rest = s.substring(colon + 1)
        if (!rest.startsWith("//")) return Parts(scheme.lowercase(), null, null, rest, null)
        val afterSlashes = rest.substring(2)
        val authorityEnd = afterSlashes.indexOfFirst { it == '/' || it == '?' || it == '#' }.let { if (it < 0) afterSlashes.length else it }
        val authority = afterSlashes.substring(0, authorityEnd)
        val tail = afterSlashes.substring(authorityEnd)
        val userInfo = if ('@' in authority) authority.substringBeforeLast('@') else null
        val hostPort = authority.substringAfterLast('@')
        val host = if (hostPort.startsWith("[")) hostPort.substringBefore(']') + "]" else hostPort.substringBefore(':')
        val path = tail.substringBefore('?').substringBefore('#').ifEmpty { "/" }
        val query = if ('?' in tail) tail.substringAfter('?').substringBefore('#') else null
        return Parts(scheme.lowercase(), userInfo, host.ifEmpty { null }, path, query)
    }
}
