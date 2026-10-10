package com.unoone.agent.di

import android.content.Context
import androidx.room.Room
import com.unoone.agent.storage.cache.CacheKeyManager
import com.unoone.agent.storage.cache.DatabaseRecoveryRequired
import com.unoone.agent.storage.cache.DatabaseOpenPolicy
import com.unoone.agent.storage.cache.KeystorePassphraseCipher
import com.unoone.agent.storage.db.UnoOneDatabase
import com.unoone.agent.storage.cache.PlaintextUpgradeFiles
import java.io.File

/** Encrypted schema 7 (6->7 adds the durable vault outbox; also self-registered by UnoOneDatabase.init).
 * Existing data, including detached writes, is never disposable; there is no destructive fallback.
 * Plaintext standalone imports need a verified conversion, not a Room downgrade.
 */
object DatabaseProvider {
    private const val DB_NAME = "unoone_database"
    private const val WRAPPED_KEY_FILE = "cache_db_key.wrapped"
    @Volatile private var INSTANCE: UnoOneDatabase? = null

    fun getDatabase(context: Context): UnoOneDatabase = INSTANCE ?: synchronized(this) {
        INSTANCE ?: build(context.applicationContext).also { INSTANCE = it }
    }

    private fun upgrade(context: Context): PlaintextDatabaseUpgrade {
        val wrapped = File(context.noBackupFilesDir, WRAPPED_KEY_FILE)
        return PlaintextDatabaseUpgrade(context, context.getDatabasePath(DB_NAME),
            File(context.noBackupFilesDir, "plaintext-upgrade-v1"), wrapped,
            CacheKeyManager(KeystorePassphraseCipher(), wrapped, syncDirectory = DatabaseDirectorySync::sync))
    }

    private fun build(context: Context): UnoOneDatabase {
        val dbFile = context.getDatabasePath(DB_NAME)
        val wrappedKey = File(context.noBackupFilesDir, WRAPPED_KEY_FILE)
        // A journal is checked BEFORE ordinary open (including absent main + orphan WAL during
        // promotion). No app database handle exists while the consent-gated worker owns this monitor.
        val upgrade = upgrade(context)
        try {
            val pending = upgrade.pending()
            if (upgrade.files.exists() && !pending && !dbFile.isFile) {
                throw DatabaseRecoveryRequired("Completed upgrade has no live database. Do not create a replacement; retained recovery files require assisted recovery.")
            }
            if (pending || PlaintextUpgradeFiles.isPlaintext(dbFile)) {
                if (upgrade.files.exists() && !pending) {
                    throw DatabaseRecoveryRequired("Completed upgrade marker conflicts with plaintext data. Manual recovery required; conversion will not replay.")
                }
                PlaintextUpgradeConsent.install(context) { synchronized(this) {
                    check(INSTANCE == null) { "Close the application before upgrading" }
                    upgrade.run(consent = true)
                } }
                throw DatabaseRecoveryRequired("Standalone plaintext data requires your approval to encrypt it. The upgrade retains app-private plaintext recovery files, including any committed WAL data. Declining preserves the installation unchanged. Do not clear app data or uninstall.")
            }
        } catch (error: DatabaseRecoveryRequired) { throw error }
        catch (_: Exception) { throw DatabaseRecoveryRequired("Interrupted or invalid upgrade state. Original recovery files retained; assisted recovery required.") }
        // Inspect before generating keys or opening Room. Sidecars also count as data.
        DatabaseOpenPolicy.requireSafeOpen(dbFile, wrappedKey)
        System.loadLibrary("sqlcipher")
        val key = CacheKeyManager(KeystorePassphraseCipher(), wrappedKey, syncDirectory = DatabaseDirectorySync::sync).getOrCreate()
        if (dbFile.exists()) try {
            RecoveryOpenHelperFactory.requireAuthenticatedExisting(dbFile, key.passphrase,
                File(context.noBackupFilesDir, "encrypted-open-recovery"))
        } catch (_: Exception) {
            key.passphrase.fill(0)
            throw DatabaseRecoveryRequired("Existing encrypted database could not pass safe recovery admission. Original files, transaction sidecars and keys are retained. Ensure sufficient space for recovery copies or request assisted recovery.")
        }
        check(com.unoone.agent.storage.cache.EncryptedDbPolicy.decide(dbFile.exists(), key.outcome) ==
            com.unoone.agent.storage.cache.EncryptedDbPolicy.Action.OPEN) { "Database recovery required" }
        val database = Room.databaseBuilder(context, UnoOneDatabase::class.java, dbFile.absolutePath)
            .openHelperFactory(RecoveryOpenHelperFactory(key.passphrase))
            .addMigrations(UnoOneDatabase.MIGRATION_1_2, UnoOneDatabase.MIGRATION_2_3,
                UnoOneDatabase.MIGRATION_3_4, UnoOneDatabase.MIGRATION_4_5, UnoOneDatabase.MIGRATION_5_6,
                UnoOneDatabase.MIGRATION_6_7)
            .build()
        try {
            // Authenticated open before app effects start. Room migration is transactional.
            database.openHelper.writableDatabase
            if (upgrade.hasRecoveryCopies()) PlaintextUpgradeRecovery.install(context)
            return database
        } catch (error: Exception) {
            runCatching { database.close() }
            throw DatabaseRecoveryRequired("Encrypted database could not be opened safely. Preserve this installation and its keys; do not clear app data or uninstall. Verified recovery/export is required.")
        } finally { key.passphrase.fill(0) }
    }
}
