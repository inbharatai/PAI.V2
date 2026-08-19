package com.unoone.agent.storage.db

import androidx.room.Database
import androidx.room.RoomDatabase
import androidx.room.migration.Migration
import androidx.sqlite.db.SupportSQLiteDatabase
import com.unoone.agent.storage.dao.ActionLogDao
import com.unoone.agent.storage.dao.MemoryDao
import com.unoone.agent.storage.dao.ModelMetadataDao
import com.unoone.agent.storage.dao.NoteDao
import com.unoone.agent.storage.dao.PendingTombstoneDao
import com.unoone.agent.storage.dao.SkillDao
import com.unoone.agent.storage.entity.ActionLogEntity
import com.unoone.agent.storage.entity.MemoryEntity
import com.unoone.agent.storage.entity.ModelMetadataEntity
import com.unoone.agent.storage.entity.NoteEntity
import com.unoone.agent.storage.entity.PendingTombstoneEntity
import com.unoone.agent.storage.entity.SkillEntity

@Database(
    entities = [
        NoteEntity::class,
        SkillEntity::class,
        MemoryEntity::class,
        ActionLogEntity::class,
        ModelMetadataEntity::class,
        PendingTombstoneEntity::class
    ],
    version = 3,
    exportSchema = true
)
abstract class UnoOneDatabase : RoomDatabase() {
    abstract fun noteDao(): NoteDao
    abstract fun skillDao(): SkillDao
    abstract fun memoryDao(): MemoryDao
    abstract fun actionLogDao(): ActionLogDao
    abstract fun modelMetadataDao(): ModelMetadataDao
    abstract fun pendingTombstoneDao(): PendingTombstoneDao

    companion object {
        /**
         * Migration from v1 (no indexes) to v2 (indexes on title, tags, createdAt, etc.).
         * Safe to run on existing databases — CREATE INDEX IF NOT EXISTS is idempotent.
         */
        val MIGRATION_1_2 = object : Migration(1, 2) {
            override fun migrate(db: SupportSQLiteDatabase) {
                // notes table indexes
                db.execSQL("CREATE INDEX IF NOT EXISTS index_notes_title ON notes (title)")
                db.execSQL("CREATE INDEX IF NOT EXISTS index_notes_tags ON notes (tags)")
                db.execSQL("CREATE INDEX IF NOT EXISTS index_notes_createdAt ON notes (createdAt)")

                // action_logs table indexes
                db.execSQL("CREATE INDEX IF NOT EXISTS index_action_logs_status ON action_logs (status)")
                db.execSQL("CREATE INDEX IF NOT EXISTS index_action_logs_createdAt ON action_logs (createdAt)")

                // memories table indexes (unique constraint on key)
                db.execSQL("CREATE UNIQUE INDEX IF NOT EXISTS index_memories_key ON memories (key)")
                db.execSQL("CREATE INDEX IF NOT EXISTS index_memories_type ON memories (type)")

                // skills table indexes (unique constraint on name)
                db.execSQL("CREATE UNIQUE INDEX IF NOT EXISTS index_skills_name ON skills (name)")
            }
        }

        /**
         * v2 → v3: the cache learns its link to the shared vault. Adds a
         * nullable vaultRecordId to notes and memories (null = local-only,
         * pending flush) and a pending_tombstones queue for deletions made
         * while the vault was detached. Room validates structurally (column
         * name/type/nullability, PK), so the exact SQL text is not compared —
         * these statements produce the structures Room expects for the new
         * schema. Adding a nullable column with no default and creating a new
         * table are non-destructive; existing rows are preserved.
         */
        val MIGRATION_2_3 = object : Migration(2, 3) {
            override fun migrate(db: SupportSQLiteDatabase) {
                db.execSQL("ALTER TABLE notes ADD COLUMN vaultRecordId TEXT")
                db.execSQL("ALTER TABLE memories ADD COLUMN vaultRecordId TEXT")
                db.execSQL("ALTER TABLE memories ADD COLUMN vaultRevision INTEGER NOT NULL DEFAULT 1")
                db.execSQL(
                    "CREATE TABLE IF NOT EXISTS pending_tombstones (" +
                        "id INTEGER PRIMARY KEY AUTOINCREMENT NOT NULL, " +
                        "vaultRecordId TEXT NOT NULL, " +
                        "recordKind TEXT NOT NULL, " +
                        "deletedAtIso TEXT NOT NULL, " +
                        "createdAt INTEGER NOT NULL)"
                )
            }
        }
    }
}