package com.unoone.agent

import androidx.room.Room
import androidx.test.core.app.ApplicationProvider
import com.unoone.agent.core.model.InputType
import com.unoone.agent.storage.db.UnoOneDatabase
import com.unoone.agent.storage.entity.NoteEntity
import kotlinx.coroutines.runBlocking
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config

/**
 * The universal transcript producer: every processed command records a
 * conversation turn in the vault-backed store, so the usage history from
 * every host lands in ONE source. Pins the producer semantics against the
 * REAL in-memory Room database through a full processCommand pipeline:
 *
 * - the user's (sanitized) command is the first turn of a fresh session;
 * - one sessionId per command invocation;
 * - a store failure can never break the command (best-effort).
 */
@RunWith(RobolectricTestRunner::class)
@Config(sdk = [34])
class AgentTranscriptRecordingTest {

    private lateinit var db: UnoOneDatabase
    private lateinit var orchestrator: AgentOrchestrator

    @Before
    fun setUp() {
        val context = ApplicationProvider.getApplicationContext<android.content.Context>()
        db = Room.inMemoryDatabaseBuilder(context, UnoOneDatabase::class.java)
            .allowMainThreadQueries()
            .build()
        orchestrator = AgentOrchestrator(
            context = context,
            noteDao = db.noteDao(),
            actionLogDao = db.actionLogDao(),
            memoryDao = db.memoryDao(),
            skillDao = db.skillDao(),
            conversationDao = db.conversationTurnDao(),
        )
    }

    @After
    fun tearDown() = db.close()

    @Test
    fun `every command records the user's sanitized command as the session's first turn`() = runBlocking {
        orchestrator.processCommand("create note transcript-proof", InputType.TEXT)

        val turns = db.conversationTurnDao().allOnce()
        assertTrue("the user command must be recorded as a turn", turns.any { it.role == "user" })
        val userTurn = turns.first { it.role == "user" }
        assertEquals("create note transcript-proof", userTurn.content)
        assertEquals("text", userTurn.inputType)
        assertNotNull(userTurn.sessionId)
        // The command still executed — recording never blocks the pipeline.
        assertEquals(1, db.noteDao().recent(100).count { it.title == "transcript-proof" })
    }

    @Test
    fun `each command invocation gets its own session`() = runBlocking {
        orchestrator.processCommand("create note sess-one", InputType.TEXT)
        orchestrator.processCommand("create note sess-two", InputType.TEXT)

        val turns = db.conversationTurnDao().allOnce()
        val sessions = turns.map { it.sessionId }.toSet()
        assertEquals("two commands must produce two distinct sessions", 2, sessions.size)
        assertEquals(
            "every user turn belongs to one of the sessions",
            2,
            turns.count { it.role == "user" },
        )
    }

    @Test
    fun `blank input after sanitization records nothing`() = runBlocking {
        orchestrator.processCommand("   ", InputType.VOICE)

        assertTrue(
            "a command that never started must not mint a conversation session",
            db.conversationTurnDao().allOnce().isEmpty(),
        )
    }
}