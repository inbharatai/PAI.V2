package com.unoone.agent.vaultbridge

import android.content.Context
import androidx.room.Room
import androidx.test.core.app.ApplicationProvider
import com.unoone.agent.skills.SkillsModule
import com.unoone.agent.storage.db.UnoOneDatabase
import kotlinx.coroutines.runBlocking
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config

@RunWith(RobolectricTestRunner::class)
@Config(sdk=[34],manifest=Config.NONE)
class SkillLifecycleDurabilityTest {
    @Test fun editDisableEnableDeleteCallbacksObserveCommittedOutbox() = runBlocking {
        val db=Room.inMemoryDatabaseBuilder(ApplicationProvider.getApplicationContext<Context>(),UnoOneDatabase::class.java)
            .allowMainThreadQueries().build()
        try {
            val events=mutableListOf<String>()
            val module=SkillsModule(db.skillDao(),db.memoryDao(),
                onSkillSaved={ assertNotNull(db.pendingWriteDao().get("SKILL",it.id)); events += "saved" },
                onSkillDeleted={ assertNull(db.skillDao().getById(it.id)); assertTrue(db.pendingTombstoneDao().getAll().isNotEmpty()); events += "deleted" },
                onSkillEnabled={ assertTrue(it.enabled); events += "approved" },
                onSkillDisabled={ assertFalse(it.enabled); events += "corrected" })
            module.saveSkill("fixture",listOf("fixture"),listOf("fixture"),enabled=false)
            assertEquals(listOf("saved"),events) // save is NOT approval
            val first=db.skillDao().allOnce().single()
            val recordId=db.pendingWriteDao().get("SKILL",first.id)!!.recordId
            module.updateSkill(first.copy(triggerPhrases="edited"))
            module.disableSkill(db.skillDao().getById(first.id)!!)
            module.enableSkill(db.skillDao().getById(first.id)!!)
            module.deleteSkill(db.skillDao().getById(first.id)!!)
            assertEquals(listOf("saved","saved","saved","corrected","saved","approved","deleted"),events)
            assertEquals(recordId,db.pendingTombstoneDao().getAll().single().vaultRecordId)
        } finally { db.close() }
    }

    @Test fun learnedSuggestionIsHypothesisNotApproval() = runBlocking {
        val db=Room.inMemoryDatabaseBuilder(ApplicationProvider.getApplicationContext<Context>(),UnoOneDatabase::class.java)
            .allowMainThreadQueries().build()
        try {
            var hypotheses=0; var approvals=0
            val module=SkillsModule(db.skillDao(),db.memoryDao(),
                onSuggestionCreated={skill,_,count -> assertFalse(skill.enabled); assertEquals(3,count); hypotheses++},
                onSkillEnabled={approvals++})
            repeat(3) { module.recordSuccessfulUse("open my calendar","open_calendar") }
            assertEquals(1,hypotheses)
            assertEquals(0,approvals)
            assertNull(module.findSkillByTrigger("open my calendar"))
        } finally { db.close() }
    }

    @Test fun actualBuiltinSeedIsFreshEvenWithDurablePendingWork() = runBlocking {
        val db=Room.inMemoryDatabaseBuilder(ApplicationProvider.getApplicationContext<Context>(),UnoOneDatabase::class.java)
            .allowMainThreadQueries().build()
        try {
            SkillsModule(db.skillDao(),db.memoryDao()).ensureBuiltIns()
            assertTrue(db.skillDao().allOnce().isNotEmpty())
            assertTrue(db.pendingWriteDao().getAll().isNotEmpty())
            assertFalse(db.pendingWriteDao().hasHistoricalAuthority())
        } finally { db.close() }
    }
}
