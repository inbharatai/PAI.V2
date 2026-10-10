package com.unoone.agent.storage.dao

import androidx.room.Dao
import androidx.room.Delete
import androidx.room.Insert
import androidx.room.Query
import androidx.room.Update
import com.unoone.agent.storage.entity.NoteEntity
import kotlinx.coroutines.flow.Flow

data class BoundedNoteSnippet(val title: String, val content: String)

@Dao
interface NoteDao {
    /** Bounded projection: no full entity/body crosses the SQLite cursor boundary. Literal search. */
    @Query("SELECT substr(title,1,256) AS title, substr(content,1,2048) AS content FROM notes WHERE length(:query) BETWEEN 1 AND 4000 AND (instr(lower(title),lower(:query)) > 0 OR instr(lower(content),lower(:query)) > 0 OR instr(lower(tags),lower(:query)) > 0) ORDER BY createdAt DESC LIMIT 50")
    suspend fun searchBounded(query: String): List<BoundedNoteSnippet>

    @Insert
    suspend fun insert(note: NoteEntity): Long

    @Update
    suspend fun update(note: NoteEntity)

    @Delete
    suspend fun delete(note: NoteEntity)

    @Query("SELECT * FROM notes ORDER BY createdAt DESC")
    fun getAll(): Flow<List<NoteEntity>>

    @Query("SELECT * FROM notes WHERE title LIKE '%' || :query || '%' OR content LIKE '%' || :query || '%' OR tags LIKE '%' || :query || '%' ORDER BY createdAt DESC")
    fun search(query: String): Flow<List<NoteEntity>>

    @Query("SELECT * FROM notes WHERE id = :id")
    suspend fun getById(id: Long): NoteEntity?

    /** One-shot (non-Flow) search used by the search_notes tool. */
    @Query("SELECT * FROM notes WHERE title LIKE '%' || :query || '%' OR content LIKE '%' || :query || '%' OR tags LIKE '%' || :query || '%' ORDER BY createdAt DESC")
    suspend fun searchOnce(query: String): List<NoteEntity>

    /** Most-recent notes for the context snapshot (one-shot). */
    @Query("SELECT * FROM notes ORDER BY createdAt DESC LIMIT :limit")
    suspend fun recent(limit: Int): List<NoteEntity>

    /** Deletes notes whose title/content/tags match the query (used by delete_notes after CONFIRM). */
    @Query("DELETE FROM notes WHERE title LIKE '%' || :query || '%' OR content LIKE '%' || :query || '%' OR tags LIKE '%' || :query || '%'")
    suspend fun deleteByQuery(query: String): Int

    /** Deletes every note (used by delete_all_notes after STRONG_CONFIRM). Returns rows deleted. */
    @Query("DELETE FROM notes")
    suspend fun deleteAll(): Int

    /**
     * Vault-disconnect cleanup that can never lose data: deletes only rows
     * that already reached the vault. Unsynchronized rows are the ONLY copy
     * in existence and must survive the vault going away — used by
     * [com.unoone.agent.storage.cache.VaultCacheLifecycle.clearOnVaultDisconnect].
     */
    @Query("DELETE FROM notes WHERE vaultRecordId IS NOT NULL AND id NOT IN (SELECT localId FROM pending_writes WHERE recordKind IN ('NOTE'))")
    suspend fun deleteSynced(): Int

    /** Cache eviction: deletes notes older than [cutoff] epoch millis. Returns rows deleted. */
    @Query("DELETE FROM notes WHERE createdAt < :cutoff")
    suspend fun deleteOlderThan(cutoff: Long): Int

    /**
     * Cache eviction that can never lose data: deletes only rows that already
     * reached the vault (`vaultRecordId` set). Unsynchronized rows are the
     * ONLY copy in existence and are deliberately excluded — used by
     * [com.unoone.agent.storage.cache.VaultCacheLifecycle.evictExpired].
     */
    @Query("DELETE FROM notes WHERE createdAt < :cutoff AND vaultRecordId IS NOT NULL AND id NOT IN (SELECT localId FROM pending_writes WHERE recordKind IN ('NOTE'))")
    suspend fun deleteOlderThanSynced(cutoff: Long): Int

    /** Link a cache row to the vault record it was written to. */
    @Query("UPDATE notes SET vaultRecordId = :vaultRecordId WHERE id = :id")
    suspend fun setVaultRecordId(id: Long, vaultRecordId: String): Int

    /** Rows not yet written to the vault (created while detached/locked). */
    @Query("SELECT * FROM notes WHERE (vaultRecordId IS NULL OR id IN (SELECT localId FROM pending_writes WHERE recordKind IN ('NOTE'))) ORDER BY id ASC")
    suspend fun notSynced(): List<NoteEntity>

    /** Every note, one-shot — used to capture vault links before bulk deletes. */
    @Query("SELECT * FROM notes")
    suspend fun allOnce(): List<NoteEntity>
}
