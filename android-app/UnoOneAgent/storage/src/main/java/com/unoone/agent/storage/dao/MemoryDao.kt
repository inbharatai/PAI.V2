package com.unoone.agent.storage.dao

import androidx.room.Dao
import androidx.room.Delete
import androidx.room.Insert
import androidx.room.Query
import androidx.room.Update
import com.unoone.agent.storage.entity.MemoryEntity
import kotlinx.coroutines.flow.Flow

@Dao
interface MemoryDao {
    @Insert
    suspend fun insert(memory: MemoryEntity): Long

    @Update
    suspend fun update(memory: MemoryEntity)

    @Delete
    suspend fun delete(memory: MemoryEntity)

    @Query("SELECT * FROM memories ORDER BY updatedAt DESC")
    fun getAll(): Flow<List<MemoryEntity>>

    @Query("SELECT * FROM memories WHERE `key` = :key LIMIT 1")
    suspend fun getByKey(key: String): MemoryEntity?

    @Query("SELECT * FROM memories WHERE type = :type ORDER BY updatedAt DESC")
    fun getByType(type: String): Flow<List<MemoryEntity>>

    @Query("SELECT * FROM memories WHERE type = :type ORDER BY updatedAt DESC")
    suspend fun getByTypeList(type: String): List<MemoryEntity>

    /** Cache eviction: deletes memories not updated since [cutoff] epoch millis. Returns rows deleted. */
    @Query("DELETE FROM memories WHERE updatedAt < :cutoff")
    suspend fun deleteOlderThan(cutoff: Long): Int

    /**
     * Cache eviction that can never lose data: deletes only rows that already
     * reached the vault (`vaultRecordId` set). Unsynchronized rows (and
     * device-local telemetry, which never syncs) are the ONLY copy in
     * existence and are deliberately excluded — used by
     * [com.unoone.agent.storage.cache.VaultCacheLifecycle.evictExpired].
     */
    @Query("DELETE FROM memories WHERE updatedAt < :cutoff AND vaultRecordId IS NOT NULL")
    suspend fun deleteOlderThanSynced(cutoff: Long): Int

    /** Deletes every cached memory (vault disconnect cleanup). Returns rows deleted. */
    @Query("DELETE FROM memories")
    suspend fun deleteAll(): Int

    /**
     * Vault-disconnect cleanup that can never lose data: deletes only rows
     * that already reached the vault. Unsynchronized rows and device-local
     * telemetry (outcome / skill_usage types) are the ONLY copy in existence
     * and must survive the vault going away — used by
     * [com.unoone.agent.storage.cache.VaultCacheLifecycle.clearOnVaultDisconnect].
     */
    @Query("DELETE FROM memories WHERE vaultRecordId IS NOT NULL")
    suspend fun deleteSynced(): Int

    /** Link a cache row to the vault record + revision it last wrote. */
    @Query("UPDATE memories SET vaultRecordId = :vaultRecordId, vaultRevision = :vaultRevision WHERE id = :id")
    suspend fun setVaultLink(id: Long, vaultRecordId: String, vaultRevision: Int): Int

    /** Single row by id (one-shot), for vault write-through and rewrites. */
    @Query("SELECT * FROM memories WHERE id = :id")
    suspend fun getByIdOnce(id: Long): MemoryEntity?

    /**
     * User-meaningful memories not yet written to the vault. Device-local
     * records are deliberately excluded — they are not canonical vault
     * memory: planner telemetry ("outcome", type="outcome"), env-learning
     * procedure records (type="procedure_outcome") and skill-suggestion
     * HYPOTHESES (type="envobs_hypo" — a hypothesis never leaves the device
     * that recorded it; only verified facts/corrections mirror).
     */
    @Query("SELECT * FROM memories WHERE vaultRecordId IS NULL AND type NOT IN ('outcome', 'procedure_outcome', 'envobs_hypo') ORDER BY id ASC")
    suspend fun notSynced(): List<MemoryEntity>

    /** Every memory, one-shot — used by vault hydration to dedupe known records. */
    @Query("SELECT * FROM memories")
    suspend fun allOnce(): List<MemoryEntity>

    /**
     * The cached row linked to one vault record — used by vault hydration to
     * propagate a tombstone: deleted on any host, stays deleted everywhere.
     */
    @Query("SELECT * FROM memories WHERE vaultRecordId = :vaultRecordId LIMIT 1")
    suspend fun getByVaultRecordId(vaultRecordId: String): MemoryEntity?
}
