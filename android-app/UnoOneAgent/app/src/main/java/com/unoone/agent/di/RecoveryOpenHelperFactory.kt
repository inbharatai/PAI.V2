package com.unoone.agent.di

import androidx.sqlite.db.SupportSQLiteDatabase
import androidx.sqlite.db.SupportSQLiteOpenHelper
import com.unoone.agent.storage.cache.DatabaseRecoveryRequired
import net.zetetic.database.DatabaseErrorHandler
import net.zetetic.database.sqlcipher.SQLiteDatabase
import net.zetetic.database.sqlcipher.SQLiteOpenHelper

/** Room's default corruption callback may delete a database. Recovery must never do that. */
internal class RecoveryOpenHelperFactory(key: ByteArray) : SupportSQLiteOpenHelper.Factory {
    // SQLCipher keeps the password for additional connections; caller may zero its own copy.
    private val password = key.copyOf()
    companion object {
        /** SQLCipher 4.17 OPEN_READONLY does not authorize rollback-journal recovery. With any
         * transaction sidecar, validate a writable PROBE plus immutable main/WAL/SHM/journal
         * snapshot before Room may recover the real store. Wrong keys/corruption never invoke
         * the stock deleting handler. No CREATE flag on an existing database or its probe.
         */
        fun requireAuthenticatedExisting(file: java.io.File, key: ByteArray, recoveryRoot: java.io.File) {
            com.unoone.agent.storage.cache.PlaintextUpgradeFiles.regular(file)
            val sidecars = listOf("-wal", "-shm", "-journal").map { java.io.File(file.path + it) }
            sidecars.forEach { sidecar ->
                if (java.nio.file.Files.exists(sidecar.toPath(), java.nio.file.LinkOption.NOFOLLOW_LINKS))
                    com.unoone.agent.storage.cache.PlaintextUpgradeFiles.regular(sidecar)
            }
            // Zero-length TRUNCATE journals contain no recovery state and need no extra snapshot.
            val hasSidecars = sidecars.any { it.length() > 0 }
            if (!hasSidecars) {
                authenticate(file, key, SQLiteDatabase.OPEN_READONLY)
                return
            }
            val recovery = com.unoone.agent.storage.cache.EncryptedRecoveryCopy(file, recoveryRoot, DatabaseDirectorySync::sync)
            val snapshot = recovery.create()
            authenticate(snapshot.probe, key, SQLiteDatabase.OPEN_READWRITE)
            recovery.requireUnchanged(snapshot)
        }

        private fun authenticate(file: java.io.File, key: ByteArray, flags: Int) {
            SQLiteDatabase.openDatabase(file.path, key, null, flags or SQLiteDatabase.NO_LOCALIZED_COLLATORS,
                DatabaseErrorHandler { _, _ -> throw DatabaseRecoveryRequired("Encrypted database corruption; original retained.") }, null).use { db ->
                // Any shipped schema generation; v7 is the durable vault-outbox schema (MIGRATION_6_7).
                require(db.version in 1..7)
                db.rawQuery("PRAGMA integrity_check", null).use { require(it.moveToFirst() && it.getString(0) == "ok" && !it.moveToNext()) }
                db.rawQuery("PRAGMA cipher_integrity_check", null).use { require(!it.moveToFirst()) }
                db.rawQuery("PRAGMA foreign_key_check", null).use { require(!it.moveToFirst()) }
            }
        }
    }
    override fun create(configuration: SupportSQLiteOpenHelper.Configuration): SupportSQLiteOpenHelper {
        val callback = configuration.callback
        // 4.17 SupportHelper supplies a NULL native errorHandler and does NOT forward corruption
        // to the Room callback. Use its public SQLiteOpenHelper API with a non-deleting handler.
        val helper = object : SQLiteOpenHelper(configuration.context, configuration.name, password,
            null, callback.version, 0, DatabaseErrorHandler { _, _ ->
                throw DatabaseRecoveryRequired("Database corruption detected; files retained for recovery.")
            }, null, false) {
            override fun onCreate(db: SQLiteDatabase) = callback.onCreate(db)
            override fun onUpgrade(db: SQLiteDatabase, oldVersion: Int, newVersion: Int) = callback.onUpgrade(db, oldVersion, newVersion)
            override fun onDowngrade(db: SQLiteDatabase, oldVersion: Int, newVersion: Int) = callback.onDowngrade(db, oldVersion, newVersion)
            override fun onConfigure(db: SQLiteDatabase) = callback.onConfigure(db)
            override fun onOpen(db: SQLiteDatabase) = callback.onOpen(db)
        }
        return object : SupportSQLiteOpenHelper {
            override val databaseName: String? get() = helper.databaseName
            override val writableDatabase: SupportSQLiteDatabase get() = helper.writableDatabase
            override val readableDatabase: SupportSQLiteDatabase get() = helper.readableDatabase
            override fun setWriteAheadLoggingEnabled(enabled: Boolean) = helper.setWriteAheadLoggingEnabled(enabled)
            override fun close() { helper.close(); password.fill(0) }
        }
    }
}
