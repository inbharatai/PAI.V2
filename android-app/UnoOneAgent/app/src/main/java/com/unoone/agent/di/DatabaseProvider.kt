package com.unoone.agent.di

import android.content.Context
import androidx.room.Room
import com.unoone.agent.storage.cache.CacheKeyManager
import com.unoone.agent.storage.cache.EncryptedDbPolicy
import com.unoone.agent.storage.cache.KeystorePassphraseCipher
import com.unoone.agent.storage.db.UnoOneDatabase
import net.zetetic.database.sqlcipher.SupportOpenHelperFactory
import java.io.File

/**
 * Builds the singleton Room cache database, encrypted at rest with SQLCipher.
 *
 * Key handling: a random 32-byte passphrase is generated once, wrapped by a
 * non-exportable Android Keystore key, and persisted in [Context.getNoBackupFilesDir]
 * ([CacheKeyManager] — all decision logic JVM-tested in the storage module).
 * The database file is therefore ciphertext at rest; [com.unoone.agent.storage.cache.VaultCacheLifecycle]
 * keeps enforcing bounded lifetime + clear-on-disconnect on top.
 *
 * Reset semantics ([EncryptedDbPolicy], JVM-tested): when the database on
 * disk cannot belong to the current key — the first start after the
 * encryption upgrade (a pre-encryption plaintext file) or a lost/corrupted
 * wrapped key — the cache is deleted and recreated empty. That is the same
 * effect as the routine clear-on-USB-detach; the drive vault is the
 * authoritative store, and this cache is bounded-life by design.
 */
object DatabaseProvider {

    private const val DB_NAME = "unoone_database"
    private const val WRAPPED_KEY_FILE = "cache_db_key.wrapped"

    @Volatile
    private var INSTANCE: UnoOneDatabase? = null

    fun getDatabase(context: Context): UnoOneDatabase {
        return INSTANCE ?: synchronized(this) {
            INSTANCE ?: build(context.applicationContext).also { INSTANCE = it }
        }
    }

    private fun build(context: Context): UnoOneDatabase {
        // Native SQLCipher library — required before the factory is used.
        System.loadLibrary("sqlcipher")

        val key = CacheKeyManager(
            cipher = KeystorePassphraseCipher(),
            wrappedKeyFile = File(context.noBackupFilesDir, WRAPPED_KEY_FILE),
        ).getOrCreate()

        val dbFile = context.getDatabasePath(DB_NAME)
        when (EncryptedDbPolicy.decide(dbFile.exists(), key.outcome)) {
            EncryptedDbPolicy.Action.OPEN -> Unit
            EncryptedDbPolicy.Action.RESET_DB_THEN_OPEN ->
                // One-time cache reset (encryption upgrade or key loss) — the
                // same effect as the routine clear-on-USB-detach.
                // deleteDatabase also removes -wal/-shm/-journal siblings.
                context.deleteDatabase(DB_NAME)
        }

        // Absolute path, matching the official sqlcipher-android Room example;
        // resolves to the same file the plaintext build used.
        return Room.databaseBuilder(context, UnoOneDatabase::class.java, dbFile.absolutePath)
            .openHelperFactory(SupportOpenHelperFactory(key.passphrase))
            .addMigrations(UnoOneDatabase.MIGRATION_1_2, UnoOneDatabase.MIGRATION_2_3)
            .build()
    }
}
