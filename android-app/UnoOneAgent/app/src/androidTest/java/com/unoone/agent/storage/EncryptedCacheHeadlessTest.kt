package com.unoone.agent.storage

import androidx.test.core.app.ApplicationProvider
import com.unoone.agent.di.DatabaseProvider
import com.unoone.agent.storage.entity.NoteEntity
import kotlinx.coroutines.runBlocking
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * Real on-device, HEADLESS proof that the Room cache is ciphertext at rest.
 *
 * This is the fails-on-old-code / passes-on-new test for the encrypted cache:
 * against the pre-encryption build the database file begins with the plaintext
 * SQLite magic ("SQLite format 3" + NUL) and contains inserted note content in
 * cleartext; against the SQLCipher build neither is true. It exercises the
 * REAL DatabaseProvider path — Keystore-wrapped passphrase, reset policy,
 * SupportOpenHelperFactory — not an in-memory stand-in, so it can only run on
 * a device (native SQLCipher + AndroidKeyStore). CI compiles it; the
 * physical-acceptance phase runs it.
 *
 * Run: am instrument -e class com.unoone.agent.storage.EncryptedCacheHeadlessTest \
 *   com.unoone.agent.test/androidx.test.runner.AndroidJUnitRunner
 */
class EncryptedCacheHeadlessTest {

    @Test
    fun cache_database_file_is_ciphertext_at_rest() = runBlocking {
        val context = ApplicationProvider.getApplicationContext<android.content.Context>()
        val db = DatabaseProvider.getDatabase(context)

        val sentinel = "ENCRYPTION-SENTINEL-${System.currentTimeMillis()}"
        db.noteDao().insert(NoteEntity(title = "at-rest probe", content = sentinel))

        // Fold the WAL into the main file so the assertions read settled bytes.
        db.openHelper.writableDatabase.query("PRAGMA wal_checkpoint(TRUNCATE)").use {
            it.moveToFirst()
        }

        val dbFile = context.getDatabasePath("unoone_database")
        assertTrue("database file must exist", dbFile.exists())
        val bytes = dbFile.readBytes()

        // The 16-byte plaintext header: "SQLite format 3" then a NUL byte —
        // built explicitly so no raw control character sits in this source file.
        val plaintextMagic = "SQLite format 3".toByteArray(Charsets.UTF_8) + byteArrayOf(0x00)
        val header = bytes.copyOf(plaintextMagic.size)
        assertFalse(
            "plaintext SQLite header found — the cache is NOT encrypted at rest",
            header.contentEquals(plaintextMagic),
        )
        assertFalse(
            "note content found in cleartext inside the database file",
            bytes.toString(Charsets.ISO_8859_1).contains(sentinel),
        )
    }
}
