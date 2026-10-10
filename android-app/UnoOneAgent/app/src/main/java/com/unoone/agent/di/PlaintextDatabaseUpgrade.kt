package com.unoone.agent.di

import android.content.Context
import android.database.Cursor
import android.database.sqlite.SQLiteDatabase as PlainDatabase
import android.system.Os
import android.system.OsConstants
import androidx.room.Room
import androidx.room.RoomDatabase
import com.unoone.agent.storage.cache.CacheKeyManager
import com.unoone.agent.storage.cache.DatabaseRecoveryRequired
import com.unoone.agent.storage.cache.LegacyRoomSchema
import com.unoone.agent.storage.cache.PlaintextUpgradeFiles
import com.unoone.agent.storage.db.UnoOneDatabase
import net.zetetic.database.DatabaseErrorHandler
import net.zetetic.database.sqlcipher.SQLiteDatabase as CipherDatabase
import java.io.File
import java.nio.ByteBuffer
import java.security.MessageDigest

/** Offline-only upgrade, invoked under DatabaseProvider's monitor before ANY Room instance exists.
 * Byte-copy main/WAL/SHM/journal first; only that disposable snapshot is opened by plaintext SQLite.
 * Rather than execute arbitrary sqlite_schema SQL via sqlcipher_export, rebuild the trusted v1/v2
 * schema and export every validated value with bindings in one encrypted transaction. Room performs
 * 1->2->3->4->5->6 normally, including its generated schema validation. No original SQLite handle.
 */
internal class PlaintextDatabaseUpgrade(
    private val context: Context,
    database: File,
    root: File,
    private val wrappedKey: File,
    private val keyManager: CacheKeyManager,
    checkpoint: (String) -> Unit = {},
) {
    val files = PlaintextUpgradeFiles(database, root, ::syncDirectory, checkpoint)
    private val checkpoint = checkpoint
    private val cipherError = DatabaseErrorHandler { _, _ -> throw DatabaseRecoveryRequired("Database corruption; recovery files retained.") }
    fun pending(): Boolean = files.exists() && files.read().phase in setOf(
        PlaintextUpgradeFiles.Phase.PREPARED, PlaintextUpgradeFiles.Phase.VERIFIED, PlaintextUpgradeFiles.Phase.PROMOTING)

    fun hasRecoveryCopies(): Boolean = files.exists() && files.read().phase in setOf(
        PlaintextUpgradeFiles.Phase.DONE, PlaintextUpgradeFiles.Phase.CLEANING)

    fun removeRecoveryCopies(consent: Boolean) {
        try {
            files.cleanup(consent) {
                // A valid CURRENT database/key pair is not proof of this migration's lineage.
                throw DatabaseRecoveryRequired("Store/key lineage proof required before cleanup")
            }
        } catch (_: Exception) {
            throw DatabaseRecoveryRequired("Recovery cleanup refused: this migration format cannot prove the original-to-encrypted-store and wrapped-key lineage. Keep all remaining recovery files; do not clear app data.")
        }
    }

    fun run(consent: Boolean) {
        try {
            System.loadLibrary("sqlcipher")
            var journal = if (files.exists()) files.read() else files.prepare(consent)
            if (journal.phase in setOf(PlaintextUpgradeFiles.Phase.DONE, PlaintextUpgradeFiles.Phase.CLEANING,
                    PlaintextUpgradeFiles.Phase.CLEANED)) return // never replay against a live encrypted store
            journal = files.resumeInitialization(consent)
            if (journal.phase != PlaintextUpgradeFiles.Phase.PREPARED || files.candidate.exists()) {
                require(keyManager.hasPersistedKeyMaterial()) { "Existing migration key required" }
            }
            if (journal.phase == PlaintextUpgradeFiles.Phase.PREPARED) {
                files.copySnapshot(journal)
                val source = PlainDatabase.openDatabase(files.snapshot.path, null, PlainDatabase.OPEN_READWRITE or PlainDatabase.NO_LOCALIZED_COLLATORS,
                    android.database.DatabaseErrorHandler { throw DatabaseRecoveryRequired("Invalid source; original files retained.") })
                source.use { plain ->
                    plain.execSQL("PRAGMA trusted_schema=OFF")
                    plain.execSQL("PRAGMA temp_store=MEMORY")
                    plain.beginTransaction()
                    try {
                        val version = validateLegacy(plain)
                        val expected = fingerprints { sql -> plain.rawQuery(sql, null) }
                        // Key creation is allowed ONLY after exact plaintext schema/integrity validation.
                        val key = keyManager.getOrCreate().passphrase
                        try {
                            syncDirectory(wrappedKey.parentFile!!)
                            export(plain, version, key)
                            checkpoint("exported")
                            migrateRoom(key)
                            checkpoint("room6")
                            verify(files.candidate, key, expected)
                            checkpoint("validated")
                        } finally { key.fill(0) }
                    } finally { plain.endTransaction() } // rollback/read-only transaction on the snapshot only
                }
                journal = files.verified(journal)
            }
            val key = keyManager.getOrCreate().passphrase
            try {
                val candidate = if (files.candidate.exists()) files.candidate else files.database
                verify(candidate, key, null) // wrong/missing key never moves files
            } finally { key.fill(0) }
            files.promote(journal)
        } catch (_: Exception) {
            // Do not attach the SQLite exception: startup logs exceptions; those may contain SQL/values.
            throw DatabaseRecoveryRequired("Safe database upgrade stopped. Original data, sidecars and keys are retained. Keep this installation; do not clear data or uninstall. Retry with sufficient free space or request assisted recovery.")
        }
    }

    private fun export(source: PlainDatabase, version: Int, key: ByteArray) {
        require(!files.candidate.exists())
        openCipher(files.candidate, key, create = true).use { target ->
            require(singleString(target.rawQuery("PRAGMA journal_mode=DELETE", null)).equals("delete", ignoreCase = true))
            target.execSQL("PRAGMA synchronous=FULL")
            target.execSQL("PRAGMA temp_store=MEMORY")
            require(singleString(target.rawQuery("PRAGMA page_size", null)) == "4096")
            require(singleString(target.rawQuery("PRAGMA max_page_count=65536", null)) == "65536") // encrypted scratch <= 256 MiB
            target.beginTransaction()
            try {
                LegacyRoomSchema.tables.forEach { (table, columns) ->
                    target.execSQL(LegacyRoomSchema.create(table))
                    val names = columns.joinToString(",") { "`${it.name}`" }
                    target.compileStatement("INSERT INTO `$table` ($names) VALUES (${columns.joinToString(",") { "?" }})").use { statement ->
                        source.rawQuery("SELECT $names FROM `$table` ORDER BY id", null).use { rows ->
                            var count = 0L
                            while (rows.moveToNext()) {
                                require(++count <= LegacyRoomSchema.MAX_ROWS)
                                statement.clearBindings()
                                columns.forEachIndexed { index, column ->
                                    when (rows.getType(index)) {
                                        Cursor.FIELD_TYPE_NULL -> { require(column.nullable); statement.bindNull(index + 1) }
                                        Cursor.FIELD_TYPE_INTEGER -> { require(column.type == "INTEGER"); statement.bindLong(index + 1, rows.getLong(index)) }
                                        Cursor.FIELD_TYPE_STRING -> { require(column.type == "TEXT"); val value = rows.getString(index); require(value.toByteArray().size <= LegacyRoomSchema.MAX_CELL_BYTES); statement.bindString(index + 1, value) }
                                        else -> error("Unexpected SQLite storage class")
                                    }
                                }
                                statement.executeInsert()
                            }
                        }
                    }
                }
                if (version == 2) LegacyRoomSchema.indexes.forEach { target.execSQL(it.sql) }
                // Preserve AUTOINCREMENT high-water marks even when the highest rows were deleted.
                source.rawQuery("SELECT name,seq FROM sqlite_sequence", null).use { sequence ->
                    while (sequence.moveToNext()) {
                        val table = sequence.getString(0)
                        require(table in LegacyRoomSchema.tables && sequence.getType(1) == Cursor.FIELD_TYPE_INTEGER && sequence.getLong(1) >= 0)
                        target.execSQL("DELETE FROM sqlite_sequence WHERE name=?", arrayOf(table))
                        target.execSQL("INSERT INTO sqlite_sequence(name,seq) VALUES (?,?)", arrayOf(table, sequence.getLong(1)))
                    }
                }
                target.execSQL("PRAGMA user_version=$version")
                target.setTransactionSuccessful()
            } finally { target.endTransaction() }
        }
    }

    private fun migrateRoom(key: ByteArray) {
        val room = Room.databaseBuilder(context, UnoOneDatabase::class.java, files.candidate.absolutePath)
            .openHelperFactory(RecoveryOpenHelperFactory(key))
            .setJournalMode(RoomDatabase.JournalMode.TRUNCATE)
            .addMigrations(UnoOneDatabase.MIGRATION_1_2, UnoOneDatabase.MIGRATION_2_3,
                UnoOneDatabase.MIGRATION_3_4, UnoOneDatabase.MIGRATION_4_5, UnoOneDatabase.MIGRATION_5_6).build()
        try { room.openHelper.writableDatabase } finally { room.close() }
        PlaintextUpgradeFiles.requireNoSidecars(files.candidate)
    }

    private fun verify(file: File, key: ByteArray, expected: Map<String, String>?) {
        require(!PlaintextUpgradeFiles.isPlaintext(file))
        openCipher(file, key).use { db ->
            require(db.version == 6)
            require(singleString(db.rawQuery("PRAGMA integrity_check", null)) == "ok")
            db.rawQuery("PRAGMA cipher_integrity_check", null).use { require(!it.moveToFirst()) }
            db.rawQuery("PRAGMA foreign_key_check", null).use { require(!it.moveToFirst()) }
            if (expected != null) {
                require(fingerprints { sql -> db.rawQuery(sql, null) } == expected)
                listOf("pending_tombstones", "pending_writes", "conversation_turns").forEach { table ->
                    require(singleString(db.rawQuery("SELECT count(*) FROM $table", null)) == "0")
                }
            }
        }
    }

    private fun openCipher(file: File, key: ByteArray, create: Boolean = false): CipherDatabase =
        CipherDatabase.openDatabase(file.path, key, null,
            CipherDatabase.NO_LOCALIZED_COLLATORS or (if (create) CipherDatabase.CREATE_IF_NECESSARY else CipherDatabase.OPEN_READONLY), cipherError, null)

    private fun validateLegacy(db: PlainDatabase): Int {
        require(db.version in 1..2)
        require(singleString(db.rawQuery("PRAGMA integrity_check", null)) == "ok")
        db.rawQuery("PRAGMA foreign_key_check", null).use { require(!it.moveToFirst()) }
        val allowedTables = LegacyRoomSchema.tables.keys + setOf("android_metadata", "room_master_table", "sqlite_sequence")
        val seen = mutableSetOf<String>()
        db.rawQuery("SELECT type,name,tbl_name FROM sqlite_master", null).use { objects ->
            while (objects.moveToNext()) {
                val type = objects.getString(0); val name = objects.getString(1)
                if (type == "table") { require(name in allowedTables); seen.add(name) }
                else { require(type == "index" && db.version == 2 && LegacyRoomSchema.indexes.any { it.name == name && it.table == objects.getString(2) }) }
            }
        }
        require(seen.containsAll(LegacyRoomSchema.tables.keys) && "sqlite_sequence" in seen)
        val internalColumns = mapOf("android_metadata" to listOf("locale"), "room_master_table" to listOf("id", "identity_hash"), "sqlite_sequence" to listOf("name", "seq"))
        internalColumns.forEach { (table, columns) ->
            if (table in seen) db.rawQuery("SELECT * FROM `$table` LIMIT 0", null).use { require(it.columnNames.toList() == columns) }
        }
        if ("room_master_table" in seen) db.rawQuery("SELECT id FROM room_master_table", null).use {
            require(it.moveToFirst() && it.getLong(0) == 42L && !it.moveToNext())
        }
        if ("android_metadata" in seen) require(singleString(db.rawQuery("SELECT count(*) FROM android_metadata", null)).toInt() <= 1)
        LegacyRoomSchema.tables.forEach { (table, expected) ->
            // Reject hidden/generated columns, CHECK clauses, collations, virtual tables, etc.
            // Only known Room v1/v2 DDL is accepted; no source SQL is executed in the target.
            val ddl = singleString(db.rawQuery("SELECT sql FROM sqlite_master WHERE type='table' AND name=?", arrayOf(table)))
            fun normalized(sql: String) = sql.replace("`", "").replace("\"", "")
                .replace(Regex("\\s+"), "").lowercase().replace("createtableifnotexists", "createtable")
            require(normalized(ddl) == normalized(LegacyRoomSchema.create(table)))
            db.rawQuery("SELECT * FROM `$table` LIMIT 0", null).use { require(it.columnNames.toList() == expected.map { column -> column.name }) }
            db.rawQuery("PRAGMA table_info(`$table`)", null).use { columns ->
                var index = 0
                while (columns.moveToNext()) {
                    require(index < expected.size)
                    val column = expected[index++]
                    require(columns.getString(1) == column.name && columns.getString(2).uppercase() == column.type)
                    require(columns.getInt(3) == (if (column.nullable) 0 else 1) && columns.isNull(4))
                    require(columns.getInt(5) == (if (column.primary) 1 else 0))
                }
                require(index == expected.size)
            }
            db.rawQuery("PRAGMA foreign_key_list(`$table`)", null).use { require(!it.moveToFirst()) }
            val indexes = mutableSetOf<String>()
            db.rawQuery("PRAGMA index_list(`$table`)", null).use { cursor ->
                while (cursor.moveToNext()) {
                    val name = cursor.getString(1)
                    val expectedIndex = LegacyRoomSchema.indexes.single { it.name == name && it.table == table }
                    require(db.version == 2 && cursor.getInt(2) == (if (expectedIndex.unique) 1 else 0) && cursor.getString(3) == "c" && cursor.getInt(4) == 0)
                    indexes.add(name)
                    db.rawQuery("PRAGMA index_info(`$name`)", null).use { info ->
                        require(info.moveToFirst() && info.getString(2) == expectedIndex.column && !info.moveToNext())
                    }
                    db.rawQuery("PRAGMA index_xinfo(`$name`)", null).use { info ->
                        while (info.moveToNext()) if (info.getInt(5) == 1) {
                            require(info.getString(2) == expectedIndex.column && info.getInt(3) == 0 && info.getString(4) == "BINARY")
                        }
                    }
                }
            }
            require(indexes == (if (db.version == 2) LegacyRoomSchema.indexes.filter { it.table == table }.map { it.name }.toSet() else emptySet()))
        }
        return db.version
    }

    /** Length- and type-delimited SHA-256 over a stable PK order; no row values/hashes are logged.
     * Includes count, NULL vs empty, 64-bit integers and original columns only after additive migrations.
     */
    private fun fingerprints(query: (String) -> Cursor): Map<String, String> {
        val results = linkedMapOf<String, String>()
        LegacyRoomSchema.tables.forEach { (table, columns) ->
            val digest = MessageDigest.getInstance("SHA-256")
            var count = 0L
            query("SELECT ${columns.joinToString(",") { "`${it.name}`" }} FROM `$table` ORDER BY id").use { rows ->
                while (rows.moveToNext()) {
                    require(++count <= LegacyRoomSchema.MAX_ROWS)
                    columns.forEachIndexed { index, column ->
                        val type = rows.getType(index)
                        digest.update(type.toByte())
                        val bytes = when (type) {
                            Cursor.FIELD_TYPE_NULL -> { require(column.nullable); byteArrayOf() }
                            Cursor.FIELD_TYPE_INTEGER -> { require(column.type == "INTEGER"); ByteBuffer.allocate(8).putLong(rows.getLong(index)).array() }
                            Cursor.FIELD_TYPE_STRING -> { require(column.type == "TEXT"); rows.getString(index).toByteArray(Charsets.UTF_8) }
                            else -> error("Unsupported SQLite value")
                        }
                        require(bytes.size <= LegacyRoomSchema.MAX_CELL_BYTES)
                        digest.update(ByteBuffer.allocate(4).putInt(bytes.size).array()); digest.update(bytes)
                    }
                }
            }
            results[table] = "$count:" + digest.digest().joinToString("") { "%02x".format(it) }
            query("SELECT seq FROM sqlite_sequence WHERE name='$table'").use { rows ->
                // Empty table sequences can be absent or zero; both imply identical next id.
                results["sequence:$table"] = if (rows.moveToFirst()) rows.getLong(0).toString().also { require(!rows.moveToNext()) } else "0"
            }
        }
        return results
    }

    private fun singleString(cursor: Cursor): String = cursor.use {
        require(it.moveToFirst()); val result = it.getString(0); require(!it.moveToNext()); result
    }

    companion object {
        private fun syncDirectory(file: File) {
            require(file.isDirectory)
            val fd = Os.open(file.path, OsConstants.O_RDONLY, 0)
            try { Os.fsync(fd) } finally { Os.close(fd) }
        }
    }
}
