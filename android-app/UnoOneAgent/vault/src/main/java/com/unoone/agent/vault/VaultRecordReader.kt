package com.unoone.agent.vault

/**
 * The narrow vault-READ surface the app needs to hydrate records authored on
 * other hosts (Power, or another phone), abstracted from
 * [MobileVaultRepository] so the hydrator is JVM-testable against a fake. The
 * session-backed implementation lives in the app module, mirroring
 * [VaultRecordWriter].
 *
 * Metadata listing does NOT decrypt: record metadata is plaintext in the
 * envelope by design (the desktop indexes it the same way), so a hydrator can
 * decide which records to pull without a decrypt storm. Content reads go
 * through [readRecord], which verifies canonical AAD before decrypting.
 */
interface VaultRecordReader {
    /**
     * One metadata map per record in the vault (the same 16 canonical fields
     * [VaultCrypto.canonicalAad] accepts). Records that cannot be parsed are
     * skipped — a foreign/corrupt envelope must not break hydration.
     */
    fun listRecordMetadata(): List<Map<String, Any?>>

    /** Read + decrypt one record by id; throws [VaultAccessException]. */
    fun readRecord(recordId: String): Pair<Map<String, Any?>, ByteArray>
}