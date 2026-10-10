package com.unoone.agent.storage.db

import androidx.sqlite.db.SupportSQLiteDatabase

/**
 * SQLite triggers run INSIDE the caller's Room mutation transaction, including bulk deletes.
 * They are not lifecycle callbacks. Failure to enqueue aborts the mutation. The generation is
 * (`pending_writes.id`, `revision`): revision increases on EVERY edit, id never reuses retirement.
 * A pending identity survives edits until it is linked; a delete captures BOTH identities.
 * Existing v6 pending IDs have unknown provenance and migrate as HISTORICAL, never fresh.
 */
internal object VaultOutboxSchema {
    private data class Table(val name: String, val kind: String, val columns: List<String>, val eligible: String = "1")
    private val tables = listOf(
        Table("notes", "'NOTE'", listOf("title", "content", "tags", "createdAt", "updatedAt", "reminderTime")),
        Table("memories", "CASE WHEN NEW.type = 'envobs' THEN 'ENVOBS' ELSE 'MEMORY' END",
            listOf("key", "value", "type", "createdAt", "updatedAt"),
            "NEW.type NOT IN ('outcome','procedure_outcome','envobs_hypo','harness_memory','skill_usage')"),
        Table("skills", "'SKILL'", listOf("name", "triggerPhrases", "stepsJson", "riskLevel", "enabled", "createdAt", "updatedAt")),
        Table("conversation_turns", "'TRANSCRIPT'", listOf("sessionId", "role", "content", "inputType", "createdAt")),
    )
    // SQLite randomblob is an identity generator only, never cryptographic/key material.
    private const val UUID_SQL = "lower(hex(randomblob(4))) || '-' || lower(hex(randomblob(2))) || '-4' || substr(lower(hex(randomblob(2))),2) || '-a' || substr(lower(hex(randomblob(2))),2) || '-' || lower(hex(randomblob(6)))"
    private const val NOW = "CAST(strftime('%s','now') AS INTEGER) * 1000"

    /** Trigger names owned here; dropped and recreated atomically so the installed body always
     * matches this source (CREATE IF NOT EXISTS alone would keep a stale body forever). */
    private fun triggerNames(table: String) = listOf("insert", "update", "link_guard", "delete").map { "vault_${table}_$it" }

    fun install(db: SupportSQLiteDatabase) {
        db.beginTransaction()
        try {
            for (t in tables) triggerNames(t.name).forEach { db.execSQL("DROP TRIGGER IF EXISTS $it") }
            for (t in tables) installTriggers(db, t)
            db.setTransactionSuccessful()
        } finally { db.endTransaction() }
    }

    private fun installTriggers(db: SupportSQLiteDatabase, t: Table) {
        run {
            val kinds = if (t.name == "memories") "('MEMORY','ENVOBS')" else "(${t.kind})"
            val pending = "recordKind IN $kinds AND localId = NEW.id"
            fun enqueue(link: String, revision: String) = """
                UPDATE pending_writes SET revision = MAX(revision, $revision) + 1,
                    recordKind = ${t.kind}, createdAt = $NOW WHERE $pending;
                INSERT INTO pending_writes(recordKind,localId,recordId,createdAt,revision,origin)
                SELECT ${t.kind}, NEW.id, COALESCE($link, $UUID_SQL), $NOW, ($revision) + 1,
                    CASE WHEN $link IS NULL THEN 'FRESH' ELSE 'HISTORICAL' END
                WHERE NOT EXISTS (SELECT 1 FROM pending_writes WHERE $pending);
            """.trimIndent()
            db.execSQL("""
                CREATE TRIGGER IF NOT EXISTS vault_${t.name}_insert AFTER INSERT ON ${t.name}
                WHEN NEW.vaultRecordId IS NULL AND ${t.eligible}
                BEGIN ${enqueue("NEW.vaultRecordId", "0")} END
            """.trimIndent())
            val changed = t.columns.joinToString(" OR ") { "NEW.`$it` IS NOT OLD.`$it`" }
            db.execSQL("""
                CREATE TRIGGER IF NOT EXISTS vault_${t.name}_update AFTER UPDATE ON ${t.name}
                WHEN ($changed) AND NEW.vaultRevision <= OLD.vaultRevision AND ${t.eligible}
                BEGIN
                    ${enqueue("COALESCE(OLD.vaultRecordId, NEW.vaultRecordId)", "CASE WHEN OLD.vaultRecordId IS NULL THEN 0 ELSE OLD.vaultRevision END")}
                    UPDATE ${t.name} SET vaultRecordId = COALESCE(OLD.vaultRecordId, NEW.vaultRecordId),
                        vaultRevision = MAX(OLD.vaultRevision, NEW.vaultRevision) WHERE id = NEW.id;
                END
            """.trimIndent())
            // A stale in-memory entity (link still null / older revision) saved through @Update
            // must never erase or roll back a committed link: that would orphan the row from
            // both the outbox and eviction. Deliberate re-links to a DIFFERENT record pass.
            db.execSQL("""
                CREATE TRIGGER IF NOT EXISTS vault_${t.name}_link_guard AFTER UPDATE OF vaultRecordId, vaultRevision ON ${t.name}
                WHEN (NEW.vaultRecordId IS NULL AND OLD.vaultRecordId IS NOT NULL)
                  OR (NEW.vaultRecordId IS OLD.vaultRecordId AND NEW.vaultRevision < OLD.vaultRevision)
                BEGIN
                    UPDATE ${t.name} SET vaultRecordId = OLD.vaultRecordId, vaultRevision = OLD.vaultRevision WHERE id = NEW.id;
                END
            """.trimIndent())
            val oldKind = t.kind.replace("NEW.", "OLD.")
            val oldPending = "recordKind IN $kinds AND localId = OLD.id"
            db.execSQL("""
                CREATE TRIGGER IF NOT EXISTS vault_${t.name}_delete BEFORE DELETE ON ${t.name}
                BEGIN
                    INSERT INTO pending_tombstones(vaultRecordId,recordKind,deletedAtIso,createdAt,revision,origin)
                    SELECT recordId,recordKind,strftime('%Y-%m-%dT%H:%M:%fZ','now'),$NOW,
                        MAX(revision,OLD.vaultRevision) + 1,origin FROM pending_writes WHERE $oldPending;
                    INSERT INTO pending_tombstones(vaultRecordId,recordKind,deletedAtIso,createdAt,revision,origin)
                    SELECT OLD.vaultRecordId,$oldKind,strftime('%Y-%m-%dT%H:%M:%fZ','now'),$NOW,
                        OLD.vaultRevision + 1,'HISTORICAL'
                    WHERE OLD.vaultRecordId IS NOT NULL AND NOT EXISTS
                        (SELECT 1 FROM pending_writes WHERE $oldPending AND recordId = OLD.vaultRecordId);
                    DELETE FROM pending_writes WHERE $oldPending;
                END
            """.trimIndent())
        }
    }

    /** Backfill only never-linked rows lacking an existing outbox; old pending IDs stay quarantined. */
    fun seedUnlinked(db: SupportSQLiteDatabase) {
        for (t in tables) {
            val kind = t.kind.replace("NEW.", "r.")
            db.execSQL("""
                INSERT INTO pending_writes(recordKind,localId,recordId,createdAt,revision,origin)
                SELECT $kind,r.id,$UUID_SQL,$NOW,1,'FRESH' FROM ${t.name} r
                WHERE r.vaultRecordId IS NULL AND ${t.eligible.replace("NEW.", "r.")}
                AND NOT EXISTS (SELECT 1 FROM pending_writes p WHERE p.localId = r.id AND p.recordKind = $kind)
            """.trimIndent())
        }
    }
}
