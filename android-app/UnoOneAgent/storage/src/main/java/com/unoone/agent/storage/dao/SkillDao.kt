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

    /** Cache eviction: deletes skills older than [cutoff] epoch millis. Returns rows deleted. */
    @Query("DELETE FROM skills WHERE createdAt < :cutoff")
    suspend fun deleteOlderThan(cutoff: Long): Int

    /** Deletes every cached skill (vault disconnect cleanup). Returns rows deleted. */
    @Query("DELETE FROM skills")
    suspend fun deleteAll(): Int
}
