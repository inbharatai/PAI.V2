package com.unoone.agent.storage.db

import androidx.room.Database
import androidx.room.RoomDatabase
import androidx.room.migration.Migration
import androidx.sqlite.db.SupportSQLiteDatabase
import com.unoone.agent.storage.dao.ActionLogDao
import com.unoone.agent.storage.dao.ConversationTurnDao
import com.unoone.agent.storage.dao.MemoryDao
import com.unoone.agent.storage.dao.ModelMetadataDao
import com.unoone.agent.storage.dao.NoteDao
import com.unoone.agent.storage.dao.PendingTombstoneDao
import com.unoone.agent.storage.dao.PendingWriteDao
import com.unoone.agent.storage.dao.SkillDao
import com.unoone.agent.storage.entity.ActionLogEntity
import com.unoone.agent.storage.entity.ConversationTurnEntity
import com.unoone.agent.storage.entity.MemoryEntity
import com.unoone.agent.storage.entity.ModelMetadataEntity
import com.unoone.agent.storage.entity.NoteEntity
import com.unoone.agent.storage.entity.PendingTombstoneEntity
import com.unoone.agent.storage.entity.PendingWriteEntity
import com.unoone.agent.storage.entity.SkillEntity

@Database(
    entities = [
        NoteEntity::class,
        SkillEntity::class,
        MemoryEntity::class,
        ActionLogEntity::class,
        ModelMetadataEntity::class,
        PendingTombstoneEntity::class,
        PendingWriteEntity::class,
        ConversationTurnEntity::class
    ],
    version = 6,
    exportSchema = true
)
abstract class UnoOneDatabase : RoomDatabase() {
    abstract fun noteDao(): NoteDao
    abstract fun skillDao(): SkillDao
    abstract fun memoryDao(): MemoryDao
    abstract fun actionLogDao(): ActionLogDao
    abstract fun modelMetadataDao(): ModelMetadataDao
    abstract fun pendingTombstoneDao(): PendingTombstoneDao
    abstract fun pendingWriteDao(): PendingWriteDao
    abstract fun conversationTurnDao(): ConversationTurnDao

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

        /**
         * v3 → v4: skills learn their cache→vault link, so they mirror to the
         * shared drive as DOCUMENT {kind:"skill"} records exactly like notes
         * and memories, and vault-authored skills can hydrate back. Adding
         * nullable + defaulted columns is non-destructive; existing rows are
         * preserved (they read as local-only, pending first flush).
         */
        val MIGRATION_3_4 = object : Migration(3, 4) {
            override fun migrate(db: SupportSQLiteDatabase) {
                db.execSQL("ALTER TABLE skills ADD COLUMN vaultRecordId TEXT")
                db.execSQL("ALTER TABLE skills ADD COLUMN vaultRevision INTEGER NOT NULL DEFAULT 1")
            }
        }

        /**
         * v4 → v5: the conversation store. Every user command and every
         * spoken agent response is persisted as a turn and mirrored to the
         * shared drive as a TRANSCRIPT record, so the vault holds the whole
         * usage history from every host as ONE source. Creating a new table
         * is non-destructive; existing data is untouched.
         */
        val MIGRATION_4_5 = object : Migration(4, 5) {
            override fun migrate(db: SupportSQLiteDatabase) {
                db.execSQL(
                    "CREATE TABLE IF NOT EXISTS conversation_turns (" +
                        "id INTEGER PRIMARY KEY AUTOINCREMENT NOT NULL, " +
                        "sessionId TEXT NOT NULL, " +
                        "role TEXT NOT NULL, " +
                        "content TEXT NOT NULL, " +
                        "inputType TEXT NOT NULL, " +
                        "createdAt INTEGER NOT NULL, " +
                        "vaultRecordId TEXT, " +
                        "vaultRevision INTEGER NOT NULL DEFAULT 1)"
                )
                db.execSQL("CREATE INDEX IF NOT EXISTS index_conversation_turns_sessionId ON conversation_turns (sessionId)")
                db.execSQL("CREATE INDEX IF NOT EXISTS index_conversation_turns_vaultRecordId ON conversation_turns (vaultRecordId)")
            }
        }

        /**
         * v5 → v6: interrupted-write protection. A vault record id is minted
         * and persisted BEFORE the vault write, so a retry after a crash
         * between the write and the cache-row stamp reuses the SAME record
         * id instead of duplicating the record in the vault. Creating a new
         * table is non-destructive; existing data is untouched.
         */
        val MIGRATION_5_6 = object : Migration(5, 6) {
            override fun migrate(db: SupportSQLiteDatabase) {
                db.execSQL(
                    "CREATE TABLE IF NOT EXISTS pending_writes (" +
                        "id INTEGER PRIMARY KEY AUTOINCREMENT NOT NULL, " +
                        "recordKind TEXT NOT NULL, " +
                        "localId INTEGER NOT NULL, " +
                        "recordId TEXT NOT NULL, " +
                        "createdAt INTEGER NOT NULL)"
                )
                db.execSQL("CREATE UNIQUE INDEX IF NOT EXISTS index_pending_writes_recordKind_localId ON pending_writes (recordKind, localId)")
            }
        }
    }
}