package com.unoone.agent.vault

import org.junit.Assert.*
import org.junit.Test
import java.io.File
import java.nio.file.Files
import java.util.UUID

/** Real spec Argon2id, XChaCha wrapping and AES-GCM over real fsynced files (no crypto stubs). */
class LocalFileVaultTest {
    private val password = "synthetic local vault phrase only".toByteArray()
    private val id = "dc593827-0945-48c2-a322-f574cedf63aa"
    private fun fields(content: ByteArray): Map<String, Any?> = linkedMapOf(
        "record_id" to id, "record_type" to "DOCUMENT", "schema_version" to 1,
        "encryption_version" to 1, "created_at" to "2026-10-10T00:00:00Z",
        "updated_at" to "2026-10-10T00:00:00Z", "revision" to 1, "origin_platform" to "ANDROID",
        "origin_device_id" to "android-local-test", "transaction_id" to "8538802f-e50b-4487-bf30-e7a0cedd1b82",
        "content_hash" to VaultCrypto.sha256Hex(content), "parent_record_id" to null,
        "source_record_ids" to emptyList<String>(), "privacy_level" to "PRIVATE", "tombstone" to false, "deleted_at" to null,
    )

    @Test fun `independent creates random keys IDs authenticated restart records tombstones and closed handles`() {
        val export = System.getProperty("local.vault.export")
        val root = if (export == null) Files.createTempDirectory("local-vault-real").toFile() else File(export)
        val io = PrivateFileVaultIO(root)
        val repo = MobileVaultRepository(io)
        val opened = repo.create(password)
        UUID.fromString(opened.vaultId)
        val content = "Local Kotlin file vault — नमस्ते".toByteArray()
        repo.writeRecord(opened, fields(content), content)
        assertArrayEquals(content, repo.readRecord(opened, id).second)
        val headerBytes = io.read("VAULT/header/header_a.json")
        val recordBytes = io.read("VAULT/records/$id.enc.json")
        assertFalse(String(recordBytes).contains(String(content)))
        assertFalse(String(headerBytes).contains(String(password)))
        val other = MobileVaultRepository(PrivateFileVaultIO(Files.createTempDirectory("other-local-vault").toFile())).create(password)
        assertNotEquals(opened.vaultId, other.vaultId)
        assertFalse(opened.masterKey.contentEquals(other.masterKey))
        other.close()
        opened.close()
        assertTrue(opened.masterKey.all { it == 0.toByte() })
        assertThrows(IllegalStateException::class.java) { repo.writeRecord(opened, fields(content), content) }
        assertThrows(IllegalStateException::class.java) { repo.readRecord(opened, id) }
        assertThrows(VaultAccessException::class.java) { repo.unlock("wrong phrase".toByteArray()) }
        assertArrayEquals(headerBytes, io.read("VAULT/header/header_a.json"))
        assertArrayEquals(recordBytes, io.read("VAULT/records/$id.enc.json"))
        val reopened = MobileVaultRepository(PrivateFileVaultIO(root)).unlock(password)
        assertArrayEquals(content, repo.readRecord(reopened, id).second)
        assertThrows(IllegalArgumentException::class.java) { repo.create(password) }
        repo.tombstoneRecord(reopened, id, "2026-10-10T00:01:00Z")
        val (metadata, deleted) = repo.readRecord(reopened, id)
        assertEquals(true, metadata["tombstone"])
        assertEquals(2, metadata["revision"])
        assertTrue(deleted.isEmpty())
        // Export both a live record and tombstone for the Rust real-Vault::open interoperability gate.
        val liveId = "577eb7bd-cab0-4f79-a217-e606239bfe63"
        repo.writeRecord(reopened, fields(content) + ("record_id" to liveId), content)
        val live = io.read("VAULT/records/$liveId.enc.json")
        io.write("VAULT/records/$liveId.enc.json", String(live).replace("\"revision\":1", "\"revision\":9").toByteArray())
        assertThrows(VaultAccessException::class.java) { repo.readRecord(reopened, liveId) }
        io.write("VAULT/records/$liveId.enc.json", live)
        reopened.close()
    }

    @Test fun `interrupted creation unknown material and oversized KDF fail without writes`() {
        val root = Files.createTempDirectory("local-vault-interrupted").toFile()
        val io = PrivateFileVaultIO(root)
        io.write("VAULT/header/header_a.json.pending-interruption", "retain".toByteArray())
        val repo = MobileVaultRepository(io)
        assertThrows(IllegalArgumentException::class.java) { repo.create(password) }
        assertEquals("retain", String(io.read("VAULT/header/header_a.json.pending-interruption")))
        io.write("VAULT/header/header_a.json", """{"version":1,"vault_id":"${UUID.randomUUID()}","kdf_params":{"memory_kib":2147483647,"iterations":3,"parallelism":4,"output_len":32}}""".toByteArray())
        val before = io.read("VAULT/header/header_a.json")
        assertThrows(IllegalArgumentException::class.java) { repo.unlock(password) }
        assertArrayEquals(before, io.read("VAULT/header/header_a.json"))
    }
}
