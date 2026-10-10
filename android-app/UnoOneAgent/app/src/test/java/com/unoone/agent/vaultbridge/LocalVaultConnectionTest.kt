package com.unoone.agent.vaultbridge

import com.unoone.agent.vault.MobileVaultRepository
import com.unoone.agent.vault.PrivateFileVaultIO
import com.unoone.agent.vault.VaultSession
import org.junit.Assert.*
import org.junit.Test
import java.nio.file.Files

/** Real bridge methods + real crypto/files. Only Android Context/Os creation is bypassed by
 * injecting the exact JVM file adapter; this does not claim Android filesystem qualification. */
class LocalVaultConnectionTest {
    private fun injectIO(): PrivateFileVaultIO {
        VaultConnection.lock()
        val io = PrivateFileVaultIO(Files.createTempDirectory("local-bridge-real").toFile())
        for ((name, value) in mapOf("privateIO" to io, "repository" to MobileVaultRepository(io))) {
            VaultConnection::class.java.getDeclaredField(name).apply { isAccessible = true }.set(null, value)
        }
        return io
    }
    private fun password() = "synthetic bridge phrase only".toByteArray()

    @Test fun `local lifecycle consumes password revokes old handles zeroes master and USB is unrelated`() {
        injectIO()
        val secret = password()
        VaultConnection.createLocal(secret, historicalLinksOrPending = false)
        assertTrue(secret.all { it == 0.toByte() })
        assertTrue(VaultConnection.isBridgeAllowed())
        val writer = VaultConnection.writer()!!
        val reader = VaultConnection.reader()!!
        val session = VaultConnection::class.java.getDeclaredField("session").apply { isAccessible = true }.get(null) as VaultSession
        VaultConnection.detach()
        assertTrue(VaultConnection.isUnlocked())
        assertTrue(reader.listRecordMetadata().isEmpty())
        val revoked = VaultConnection.revoke()
        assertFalse(VaultConnection.isUnlocked())
        assertNull(VaultConnection.writer())
        assertThrows(IllegalStateException::class.java) { reader.listRecordMetadata() }
        assertThrows(IllegalStateException::class.java) { writer.writeRecord(emptyMap(), byteArrayOf(1)) }
        VaultConnection.lock()
        assertTrue(session.masterKey.all { it == 0.toByte() })
        val again = password()
        assertTrue(VaultConnection.unlock(again))
        assertTrue(again.all { it == 0.toByte() })
        VaultConnection.closeRevoked(revoked) // delayed background cleanup cannot close a newer unlock
        assertTrue(VaultConnection.isUnlocked())
        assertThrows(IllegalStateException::class.java) { reader.listRecordMetadata() }
        VaultConnection.lock()
    }

    @Test fun `historical binding stays quarantined through restart and wrong passwords never reset it`() {
        val io = injectIO()
        VaultConnection.createLocal(password(), historicalLinksOrPending = true)
        val before = io.read("room-binding.txt")
        val header = io.read("VAULT/header/header_a.json")
        assertTrue(VaultConnection.isUnlocked())
        assertNull(VaultConnection.writer())
        assertNull(VaultConnection.reader())
        VaultConnection.lock()
        val wrong = "wrong".toByteArray()
        assertThrows(Exception::class.java) { VaultConnection.unlock(wrong) }
        assertTrue(wrong.all { it == 0.toByte() })
        assertFalse(VaultConnection.isUnlocked())
        assertTrue(VaultConnection.unlock(password()))
        assertFalse(VaultConnection.isBridgeAllowed())
        assertArrayEquals(before, io.read("room-binding.txt"))
        assertArrayEquals(header, io.read("VAULT/header/header_a.json"))
        VaultConnection.lock()
    }

    @Test fun `missing durable binding after interruption does not drain any pending operations`() {
        val io = injectIO()
        MobileVaultRepository(io).create(password()).close()
        assertTrue(VaultConnection.unlock(password()))
        assertFalse(VaultConnection.isBridgeAllowed())
        assertNull(VaultConnection.writer())
        assertFalse(io.exists("room-binding.txt"))
        VaultConnection.lock()
    }
}
