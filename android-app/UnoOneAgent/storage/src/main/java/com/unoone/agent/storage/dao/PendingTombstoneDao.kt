package com.unoone.agent.storage.dao

import androidx.room.Dao
import androidx.room.Insert
import androidx.room.Query
import com.unoone.agent.storage.entity.PendingTombstoneEntity

@Dao
interface PendingTombstoneDao {
    @Insert
    suspend fun insert(tombstone: PendingTombstoneEntity): Long

    @Query("SELECT * FROM pending_tombstones ORDER BY createdAt ASC")
    suspend fun getAll(): List<PendingTombstoneEntity>

    @Query("DELETE FROM pending_tombstones WHERE id = :id")
    suspend fun deleteById(id: Long): Int

    @Query("DELETE FROM pending_tombstones WHERE vaultRecordId = :vaultRecordId")
    suspend fun deleteByVaultRecordId(vaultRecordId: String): Int

    /** Vault-disconnect cleanup: the queue is drained only while attached. */
    @Query("DELETE FROM pending_tombstones")
    suspend fun clearAll(): Int
}
