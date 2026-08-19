package com.unoone.agent.storage.cache

import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder
import java.io.File

/**
 * The whole passphrase lifecycle, JVM-tested with a fake cipher — no Android,
 * no Keystore, no native SQLCipher. The Keystore adapter only supplies the
 * two primitives this fake stands in for.
 */
class CacheKeyManagerTest {

    @get:Rule
    val tmp = TemporaryFolder()

    /** AEAD stand-in: magic byte + XOR. Tampered/foreign blobs throw, like GCM. */
    private class FakeCipher : PassphraseCipher {
        override fun encrypt(plaintext: ByteArray): ByteArray =
            byteArrayOf(MAGIC) + plaintext.map { (it.toInt() xor 0x5A).toByte() }.toByteArray()

        override fun decrypt(blob: ByteArray): ByteArray {
            require(blob.isNotEmpty() && blob[0] == MAGIC) { "not a wrapped blob" }
            return blob.drop(1).map { (it.toInt() xor 0x5A).toByte() }.toByteArray()
        }

        companion object {
            const val MAGIC: Byte = 0x7E
        }
    }

    private fun keyFile(): File = File(tmp.root, "cache_db_key.wrapped")

    @Test
    fun `first run creates a 32-byte passphrase and persists the wrapped blob`() {
        val manager = CacheKeyManager(FakeCipher(), keyFile())
        val result = manager.getOrCreate()

        assertEquals(KeyOutcome.CREATED, result.outcome)
        assertEquals(CacheKeyManager.PASSPHRASE_LEN, result.passphrase.size)
        assertFalse(
            "passphrase must not be all zeros",
            result.passphrase.all { it == 0.toByte() },
        )
        assertTrue("wrapped blob must be persisted", keyFile().exists())
        assertFalse(
            "wrapped blob must not contain the raw passphrase",
            keyFile().readBytes().contentEquals(result.passphrase),
        )
    }

    @Test
    fun `second run unwraps the same passphrase`() {
        val first = CacheKeyManager(FakeCipher(), keyFile()).getOrCreate()
        val second = CacheKeyManager(FakeCipher(), keyFile()).getOrCreate()

        assertEquals(KeyOutcome.UNWRAPPED, second.outcome)
        assertArrayEquals(first.passphrase, second.passphrase)
    }

    @Test
    fun `corrupted blob resets to a fresh passphrase and reports RESET`() {
        val original = CacheKeyManager(FakeCipher(), keyFile()).getOrCreate()
        keyFile().writeBytes(byteArrayOf(0x00, 0x01, 0x02)) // no magic → decrypt throws

        val reset = CacheKeyManager(FakeCipher(), keyFile()).getOrCreate()

        assertEquals(KeyOutcome.RESET, reset.outcome)
        assertEquals(CacheKeyManager.PASSPHRASE_LEN, reset.passphrase.size)
        assertFalse(
            "reset must produce a NEW passphrase",
            reset.passphrase.contentEquals(original.passphrase),
        )

        // And the replacement blob is durable: the next start unwraps it.
        val after = CacheKeyManager(FakeCipher(), keyFile()).getOrCreate()
        assertEquals(KeyOutcome.UNWRAPPED, after.outcome)
        assertArrayEquals(reset.passphrase, after.passphrase)
    }

    @Test
    fun `truncated empty blob also resets`() {
        CacheKeyManager(FakeCipher(), keyFile()).getOrCreate()
        keyFile().writeBytes(ByteArray(0)) // crash-truncation stand-in

        val reset = CacheKeyManager(FakeCipher(), keyFile()).getOrCreate()
        assertEquals(KeyOutcome.RESET, reset.outcome)
    }

    @Test
    fun `no temp file is left behind after a successful persist`() {
        CacheKeyManager(FakeCipher(), keyFile()).getOrCreate()
        assertFalse(File(tmp.root, "cache_db_key.wrapped.tmp").exists())
    }

    @Test
    fun `distinct resets produce distinct passphrases`() {
        val a = CacheKeyManager(FakeCipher(), keyFile()).getOrCreate()
        keyFile().writeBytes(byteArrayOf(1))
        val b = CacheKeyManager(FakeCipher(), keyFile()).getOrCreate()
        keyFile().writeBytes(byteArrayOf(2))
        val c = CacheKeyManager(FakeCipher(), keyFile()).getOrCreate()

        assertFalse(a.passphrase.contentEquals(b.passphrase))
        assertFalse(b.passphrase.contentEquals(c.passphrase))
        assertFalse(a.passphrase.contentEquals(c.passphrase))
    }
}
