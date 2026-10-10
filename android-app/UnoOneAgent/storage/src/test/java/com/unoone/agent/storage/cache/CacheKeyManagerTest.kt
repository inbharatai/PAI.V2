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
    fun `corrupted or empty blobs fail closed and remain byte identical`() {
        for (bytes in listOf(byteArrayOf(0, 1, 2), ByteArray(0), byteArrayOf(0x7E, 1))) {
            keyFile().writeBytes(bytes)
            repeat(2) {
                try { CacheKeyManager(FakeCipher(), keyFile()).getOrCreate(); org.junit.Assert.fail("Must refuse key replacement") }
                catch (_: DatabaseRecoveryRequired) { }
                assertArrayEquals(bytes, keyFile().readBytes())
            }
        }
    }

    @Test
    fun `no temp file is left behind after a successful persist`() {
        CacheKeyManager(FakeCipher(), keyFile()).getOrCreate()
        assertFalse(File(tmp.root, "cache_db_key.wrapped.tmp").exists())
    }

    @Test
    fun `unavailable keystore never encrypts or mutates the wrapped blob`() {
        val blob = byteArrayOf(9, 8, 7)
        keyFile().writeBytes(blob)
        val unavailable = object : PassphraseCipher {
            override fun encrypt(plaintext: ByteArray): ByteArray = error("Encryption must not be called")
            override fun decrypt(blob: ByteArray): ByteArray = throw IllegalStateException("Key unavailable")
        }
        try { CacheKeyManager(unavailable, keyFile()).getOrCreate(); org.junit.Assert.fail() }
        catch (_: DatabaseRecoveryRequired) { }
        assertArrayEquals(blob, keyFile().readBytes())
    }
    @Test fun authenticatedLoneTemporaryKeyIsAdoptedWithoutCreatingAnother() {
        val passphrase = ByteArray(32) { it.toByte() }
        val blob = FakeCipher().encrypt(passphrase)
        val pending = File(tmp.root, "cache_db_key.wrapped.tmp").apply { writeBytes(blob) }
        val cipher = object : PassphraseCipher {
            override fun encrypt(plaintext: ByteArray): ByteArray = error("must not generate")
            override fun decrypt(blob: ByteArray) = FakeCipher().decrypt(blob)
        }
        val result = CacheKeyManager(cipher, keyFile()).getOrCreate()
        assertEquals(KeyOutcome.UNWRAPPED, result.outcome)
        assertArrayEquals(passphrase, result.passphrase)
        assertArrayEquals(blob, keyFile().readBytes())
        assertFalse(pending.exists())
    }
    @Test fun invalidTemporaryKeyAndConflictingFinalNeverCreateOrDelete() {
        val pending = File(tmp.root, "cache_db_key.wrapped.tmp")
        for (blob in listOf(byteArrayOf(), byteArrayOf(0), byteArrayOf(0x7E, 1))) {
            pending.writeBytes(blob)
            try { CacheKeyManager(FakeCipher(), keyFile()).getOrCreate(); org.junit.Assert.fail() }
            catch (_: DatabaseRecoveryRequired) { }
            assertArrayEquals(blob, pending.readBytes()); assertFalse(keyFile().exists())
        }
        val final = FakeCipher().encrypt(ByteArray(32) { 1 })
        val other = FakeCipher().encrypt(ByteArray(32) { 2 })
        keyFile().writeBytes(final); pending.writeBytes(other)
        try { CacheKeyManager(FakeCipher(), keyFile()).getOrCreate(); org.junit.Assert.fail() }
        catch (_: DatabaseRecoveryRequired) { }
        assertArrayEquals(final, keyFile().readBytes()); assertArrayEquals(other, pending.readBytes())
    }
    @Test fun temporarySymlinkIsNotFollowedOrReplaced() {
        val target = File(tmp.root, "target").apply { writeBytes(FakeCipher().encrypt(ByteArray(32))) }
        val pending = File(tmp.root, "cache_db_key.wrapped.tmp")
        java.nio.file.Files.createSymbolicLink(pending.toPath(), target.toPath())
        try { CacheKeyManager(FakeCipher(), keyFile()).getOrCreate(); org.junit.Assert.fail() }
        catch (_: DatabaseRecoveryRequired) { }
        assertTrue(java.nio.file.Files.isSymbolicLink(pending.toPath())); assertFalse(keyFile().exists())
        assertEquals(33, target.length().toInt())
    }
    @Test fun directorySyncFailureRetainsKeyAndRetryNeverGeneratesReplacement() {
        val passphrase = ByteArray(32) { 9 }
        File(tmp.root, "cache_db_key.wrapped.tmp").writeBytes(FakeCipher().encrypt(passphrase))
        try { CacheKeyManager(FakeCipher(), keyFile(), syncDirectory = { throw java.io.IOException("disk") }).getOrCreate(); org.junit.Assert.fail() }
        catch (_: DatabaseRecoveryRequired) { }
        assertTrue(keyFile().exists())
        assertArrayEquals(passphrase, CacheKeyManager(FakeCipher(), keyFile()).getOrCreate().passphrase)
    }

}
