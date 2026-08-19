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

    /** Deletes every cached memory (vault disconnect cleanup). Returns rows deleted. */
    @Query("DELETE FROM memories")
    suspend fun deleteAll(): Int

    /** Link a cache row to the vault record + revision it last wrote. */
    @Query("UPDATE memories SET vaultRecordId = :vaultRecordId, vaultRevision = :vaultRevision WHERE id = :id")
    suspend fun setVaultLink(id: Long, vaultRecordId: String, vaultRevision: Int): Int

    /** Single row by id (one-shot), for vault write-through and rewrites. */
    @Query("SELECT * FROM memories WHERE id = :id")
    suspend fun getByIdOnce(id: Long): MemoryEntity?

    /**
     * User-meaningful memories not yet written to the vault. Planner telemetry
     * ("outcome:" keys, type="outcome") is device-local cache and is
     * deliberately excluded — it is not canonical vault memory.
     */
    @Query("SELECT * FROM memories WHERE vaultRecordId IS NULL AND type != 'outcome' ORDER BY id ASC")
    suspend fun notSynced(): List<MemoryEntity>
}
