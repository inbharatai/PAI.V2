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

    /** Vault-disconnect cleanup: writes are drained only while attached. */
    @Query("DELETE FROM pending_writes")
    suspend fun clearAll(): Int
}