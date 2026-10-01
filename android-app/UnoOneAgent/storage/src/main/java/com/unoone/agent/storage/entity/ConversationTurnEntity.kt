package com.unoone.agent.storage.entity

import androidx.room.Entity
import androidx.room.Index
import androidx.room.PrimaryKey

/**
 * One turn of a conversation with the agent (user command or spoken agent
 * response), persisted so the whole usage history lives in ONE place: the
 * USB drive vault. Room is only the encrypted cache/index — turns mirror to
 * the vault as TRANSCRIPT records (write-through when unlocked, backlog
 * otherwise) and can be re-hydrated on any host.
 *
 * Sessions: one [sessionId] per [com.unoone.agent.AgentOrchestrator.processCommand]
 * invocation; a command that produces several spoken responses records
 * several assistant turns under the same session.
 */
@Entity(
    tableName = "conversation_turns",
    indices = [
        Index("sessionId"),
        Index("vaultRecordId"),
    ],
)
data class ConversationTurnEntity(
    @PrimaryKey(autoGenerate = true)
    val id: Long = 0,
    /** Groups the turns of one command invocation. */
    val sessionId: String,
    /** "user" (the sanitized command) or "assistant" (the spoken response). */
    val role: String,
    /** Full turn text. Lives encrypted (SQLCipher at rest, vault record when synced). */
    val content: String,
    /** How the command arrived: voice/text. */
    val inputType: String,
    val createdAt: Long = System.currentTimeMillis(),
    /**
     * Record id in the shared drive vault once this turn has been mirrored
     * there as a TRANSCRIPT record, or null while it lives only in the local
     * cache (created offline, pending flush on the next unlock). Transcripts
     * are append-only — the revision never bumps — mirroring the
     * note/memory cache→vault link columns.
     */
    val vaultRecordId: String? = null,
    val vaultRevision: Int = 1,
)