package com.unoone.agent.storage.dao

import androidx.room.Dao
import androidx.room.Insert
import androidx.room.Query
import androidx.room.Update
import com.unoone.agent.storage.entity.ConversationTurnEntity
import kotlinx.coroutines.flow.Flow

@Dao
interface ConversationTurnDao {
    @Insert
    suspend fun insert(turn: ConversationTurnEntity): Long

    @Update
    suspend fun update(turn: ConversationTurnEntity)

    /** Newest turns across all sessions, for the conversation-history UI. */
    @Query("SELECT * FROM conversation_turns ORDER BY createdAt DESC")
    fun getAll(): Flow<List<ConversationTurnEntity>>

    /** One session's turns in spoken order. */
    @Query("SELECT * FROM conversation_turns WHERE sessionId = :sessionId ORDER BY createdAt ASC, id ASC")
    suspend fun getSession(sessionId: String): List<ConversationTurnEntity>

    @Query("SELECT * FROM conversation_turns WHERE id = :id")
    suspend fun getById(id: Long): ConversationTurnEntity?

    /** Every turn, one-shot — used by vault hydration to dedupe known records. */
    @Query("SELECT * FROM conversation_turns")
    suspend fun allOnce(): List<ConversationTurnEntity>

    /** Turns not yet mirrored to the vault (created while detached/locked). */
    @Query("SELECT * FROM conversation_turns WHERE vaultRecordId IS NULL ORDER BY id ASC")
    suspend fun notSynced(): List<ConversationTurnEntity>

    /** Link a cache row to the vault record it was mirrored as. */
    @Query("UPDATE conversation_turns SET vaultRecordId = :vaultRecordId, vaultRevision = :vaultRevision WHERE id = :id")
    suspend fun setVaultLink(id: Long, vaultRecordId: String, vaultRevision: Int): Int

    /** Cache eviction that can never lose data: only already-synced turns. */
    @Query("DELETE FROM conversation_turns WHERE createdAt < :cutoff AND vaultRecordId IS NOT NULL")
    suspend fun deleteOlderThanSynced(cutoff: Long): Int

    /** Vault-disconnect cleanup that can never lose data: only synced turns. */
    @Query("DELETE FROM conversation_turns WHERE vaultRecordId IS NOT NULL")
    suspend fun deleteSynced(): Int

    /**
     * The cached turn linked to one vault record — used by vault hydration to
     * propagate a tombstone authored on another host.
     */
    @Query("SELECT * FROM conversation_turns WHERE vaultRecordId = :vaultRecordId LIMIT 1")
    suspend fun getByVaultRecordId(vaultRecordId: String): ConversationTurnEntity?

    /** Deletes one cached turn (vault-tombstone propagation). */
    @Query("DELETE FROM conversation_turns WHERE id = :id")
    suspend fun deleteById(id: Long): Int

    /** Deletes every cached turn. Returns rows deleted. */
    @Query("DELETE FROM conversation_turns")
    suspend fun deleteAll(): Int
}