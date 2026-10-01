package com.unoone.agent.storage.dao

import androidx.room.Dao
import androidx.room.Delete
import androidx.room.Insert
import androidx.room.Query
import androidx.room.Update
import com.unoone.agent.storage.entity.SkillEntity
import kotlinx.coroutines.flow.Flow

@Dao
interface SkillDao {
    @Insert
    suspend fun insert(skill: SkillEntity): Long

    @Update
    suspend fun update(skill: SkillEntity)

    @Delete
    suspend fun delete(skill: SkillEntity)

    @Query("SELECT * FROM skills WHERE enabled = 1 ORDER BY createdAt DESC")
    fun getEnabled(): Flow<List<SkillEntity>>

    @Query("SELECT * FROM skills ORDER BY createdAt DESC")
    fun getAll(): Flow<List<SkillEntity>>

    @Query("SELECT * FROM skills WHERE id = :id")
    suspend fun getById(id: Long): SkillEntity?

    /** Every skill, one-shot — used by vault hydration to dedupe already-known records. */
    @Query("SELECT * FROM skills")
    suspend fun allOnce(): List<SkillEntity>

    /** Rows not yet mirrored to the vault (created while detached/locked). */
    @Query("SELECT * FROM skills WHERE vaultRecordId IS NULL ORDER BY id ASC")
    suspend fun notSynced(): List<SkillEntity>

    /** Link a cache row to the vault record + revision it last wrote. */
    @Query("UPDATE skills SET vaultRecordId = :vaultRecordId, vaultRevision = :vaultRevision WHERE id = :id")
    suspend fun setVaultLink(id: Long, vaultRecordId: String, vaultRevision: Int): Int

    /** Cache eviction: deletes skills older than [cutoff] epoch millis. Returns rows deleted. */
    @Query("DELETE FROM skills WHERE createdAt < :cutoff")
    suspend fun deleteOlderThan(cutoff: Long): Int

    /** Deletes every cached skill (vault disconnect cleanup). Returns rows deleted. */
    @Query("DELETE FROM skills")
    suspend fun deleteAll(): Int
}
