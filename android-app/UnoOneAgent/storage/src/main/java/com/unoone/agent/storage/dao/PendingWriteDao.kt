package com.unoone.agent.storage.dao

import androidx.room.Dao
import androidx.room.Insert
import androidx.room.Query
import com.unoone.agent.storage.entity.PendingWriteEntity

@Dao
interface PendingWriteDao {
    @Insert
    suspend fun insert(pendingWrite: PendingWriteEntity): Long

    /** The id minted for one cache row's first vault write, if it hasn't succeeded yet. */
    @Query("SELECT * FROM pending_writes WHERE recordKind = :recordKind AND localId = :localId LIMIT 1")
    suspend fun get(recordKind: String, localId: Long): PendingWriteEntity?

    /** Called once the vault write succeeded and the cache row is linked. */
    @Query("DELETE FROM pending_writes WHERE recordKind = :recordKind AND localId = :localId")
    suspend fun deleteByRow(recordKind: String, localId: Long): Int

    @Query("SELECT * FROM pending_writes ORDER BY createdAt ASC")
    suspend fun getAll(): List<PendingWriteEntity>

    /** Exact generation CAS: a later edit has a different (id, revision) pair. */
    @Query("DELETE FROM pending_writes WHERE id = :generation AND revision = :revision")
    suspend fun retire(generation: Long, revision: Int): Int

    @Query("UPDATE notes SET vaultRecordId = :recordId, vaultRevision = :revision WHERE id = :localId AND EXISTS (SELECT 1 FROM pending_writes WHERE id = :generation AND revision = :revision AND localId = :localId AND recordId = :recordId AND recordKind = 'NOTE')")
    suspend fun stampNote(generation: Long, localId: Long, recordId: String, revision: Int): Int

    @Query("UPDATE memories SET vaultRecordId = :recordId, vaultRevision = :revision WHERE id = :localId AND EXISTS (SELECT 1 FROM pending_writes WHERE id = :generation AND revision = :revision AND localId = :localId AND recordId = :recordId AND recordKind IN ('MEMORY','ENVOBS'))")
    suspend fun stampMemory(generation: Long, localId: Long, recordId: String, revision: Int): Int

    @Query("UPDATE skills SET vaultRecordId = :recordId, vaultRevision = :revision WHERE id = :localId AND EXISTS (SELECT 1 FROM pending_writes WHERE id = :generation AND revision = :revision AND localId = :localId AND recordId = :recordId AND recordKind = 'SKILL')")
    suspend fun stampSkill(generation: Long, localId: Long, recordId: String, revision: Int): Int

    @Query("UPDATE conversation_turns SET vaultRecordId = :recordId, vaultRevision = :revision WHERE id = :localId AND EXISTS (SELECT 1 FROM pending_writes WHERE id = :generation AND revision = :revision AND localId = :localId AND recordId = :recordId AND recordKind = 'TRANSCRIPT')")
    suspend fun stampTurn(generation: Long, localId: Long, recordId: String, revision: Int): Int

    /** Bounded evidence query: pending work alone does NOT imply an earlier vault identity. */
    @Query("SELECT EXISTS (SELECT 1 FROM pending_writes WHERE origin != 'FRESH' UNION ALL SELECT 1 FROM pending_tombstones WHERE origin != 'FRESH' UNION ALL SELECT 1 FROM notes WHERE vaultRecordId IS NOT NULL UNION ALL SELECT 1 FROM memories WHERE vaultRecordId IS NOT NULL UNION ALL SELECT 1 FROM skills WHERE vaultRecordId IS NOT NULL UNION ALL SELECT 1 FROM conversation_turns WHERE vaultRecordId IS NOT NULL)")
    suspend fun hasHistoricalAuthority(): Boolean

    /** Vault-disconnect cleanup: writes are drained only while attached. */
    @Query("DELETE FROM pending_writes")
    suspend fun clearAll(): Int
}