package com.unoone.agent.skills

import com.unoone.agent.core.util.Logger
import com.unoone.agent.storage.dao.MemoryDao
import com.unoone.agent.storage.dao.SkillDao
import com.unoone.agent.storage.entity.MemoryEntity
import com.unoone.agent.storage.entity.SkillEntity
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.first
import kotlinx.serialization.builtins.ListSerializer
import kotlinx.serialization.json.Json
import kotlinx.serialization.serializer

/**
 * @param onSkillSaved invoked with the FULL saved/updated entity so the app
 * layer can mirror it to the shared drive vault (a suspend callback keeps
 * this module free of vault coupling, like MemoryModule). Built-in seeding
 * and learned suggestions fire it too — a skill the user approved is user
 * data and belongs in the canonical store.
 * @param onSkillDeleted invoked AFTER the local row is deleted. The vault link
 * is not lost: the storage layer's BEFORE DELETE trigger captured the linked
 * and pending vault identities as durable tombstones inside the delete
 * transaction, so this callback only wakes the drain.
 * @param onSuggestionCreated invoked when the usage counter crosses the
 * threshold and a DISABLED suggestion is created — the epistemic moment a
 * hypothesis comes to exist (P1-C env learning). Never fires for built-ins.
 * @param onSkillEnabled invoked after the user explicitly enables a skill —
 * the explicit approval the promotion gate requires.
 * @param onSkillDisabled invoked after the user explicitly disables a skill —
 * an honest epistemic correction that demotes any promotion built on the
 * earlier approval.
 */
class SkillsModule(
    private val skillDao: SkillDao,
    private val memoryDao: MemoryDao,
    private val onSkillSaved: suspend (SkillEntity) -> Unit = {},
    private val onSkillDeleted: suspend (SkillEntity) -> Unit = {},
    private val onSuggestionCreated: suspend (skill: SkillEntity, tool: String, successCount: Int) -> Unit = { _, _, _ -> },
    private val onSkillEnabled: suspend (SkillEntity) -> Unit = {},
    private val onSkillDisabled: suspend (SkillEntity) -> Unit = {},
) {

    private val json = Json { ignoreUnknownKeys = true }

    val allSkills: Flow<List<SkillEntity>> = skillDao.getAll()
    val enabledSkills: Flow<List<SkillEntity>> = skillDao.getEnabled()

    suspend fun saveSkill(
        name: String,
        triggerPhrases: List<String>,
        steps: List<String>,
        riskLevel: Int = 0,
        enabled: Boolean = true
    ) {
        val cleanName = name.trim()
        val cleanTriggers = triggerPhrases.map { it.trim() }.filter { it.isNotBlank() }.distinct()
        val cleanSteps = steps.map { it.trim() }.filter { it.isNotBlank() }
        require(cleanName.isNotBlank() && cleanName.length <= 80) { "Skill name must be 1–80 characters" }
        require(cleanTriggers.isNotEmpty() && cleanTriggers.size <= 8) { "A skill needs 1–8 trigger phrases" }
        require(cleanSteps.isNotEmpty() && cleanSteps.size <= 12) { "A skill needs 1–12 executable steps" }
        require(cleanTriggers.all { it.length <= 120 }) { "A trigger phrase is too long" }
        require(cleanSteps.all { it.length <= 500 }) { "A skill step is too long" }

        Logger.d("Expert: Saving new skill '$name'")
        val rowId = skillDao.insert(
            SkillEntity(
                name = cleanName,
                triggerPhrases = cleanTriggers.joinToString(","),
                stepsJson = json.encodeToString(ListSerializer(serializer<String>()), cleanSteps),
                riskLevel = riskLevel.coerceIn(0, 3),
                enabled = enabled
            )
        )
        skillDao.getById(rowId)?.let { onSkillSaved(it) }
    }

    /**
     * Idempotently installs and refreshes source-controlled built-ins. The user's enabled/disabled
     * choice is preserved, while new bilingual triggers and corrected safe steps reach existing
     * installations instead of only fresh installs.
     */
    suspend fun ensureBuiltIns() {
        val existingByName = skillDao.getAll().first().associateBy { it.name }
        BuiltInSkillCatalog.definitions.forEach { definition ->
            val existing = existingByName[definition.name]
            if (existing == null) {
                runCatching {
                    saveSkill(
                        name = definition.name,
                        triggerPhrases = definition.triggers,
                        steps = definition.steps,
                        riskLevel = definition.riskLevel,
                        enabled = true
                    )
                }.onFailure { Logger.w("Skills: could not seed '${definition.name}': ${it.message}") }
            } else {
                val legacy = LegacyBuiltInSkillCatalog.definitions.firstOrNull { it.name == existing.name }
                // Names alone never establish ownership. Do not overwrite user-authored/edited
                // routines or a standalone installation's newer definition; enabled choice stays.
                if (legacy == null || existing.triggerPhrases != legacy.triggers.joinToString(",") ||
                    getSkillSteps(existing) != legacy.steps || existing.riskLevel != legacy.riskLevel) return@forEach
                val refreshed = existing.copy(
                    triggerPhrases = definition.triggers.distinct().joinToString(","),
                    stepsJson = json.encodeToString(
                        ListSerializer(serializer<String>()),
                        definition.steps
                    ),
                    riskLevel = definition.riskLevel.coerceIn(0, 3),
                    updatedAt = System.currentTimeMillis()
                )
                if (
                    refreshed.triggerPhrases != existing.triggerPhrases ||
                    refreshed.stepsJson != existing.stepsJson ||
                    refreshed.riskLevel != existing.riskLevel
                ) {
                    skillDao.update(refreshed)
                    onSkillSaved(refreshed)
                    Logger.i("Skills: refreshed built-in '${definition.name}'")
                }
            }
        }
    }

    /**
     * Persistently counts successful safe routines and creates a disabled suggestion after the
     * third use. Suggestions require a visible user enable action before trigger matching sees them.
     */
    suspend fun recordSuccessfulUse(command: String, tool: String): SkillEntity? {
        val suggestion = SkillLearningPolicy.suggestionFor(command, tool) ?: return null
        val key = SkillLearningPolicy.usageKey(tool, suggestion.name)
        val existingUsage = memoryDao.getByKey(key)
        val nextCount = (existingUsage?.value?.toIntOrNull() ?: 0) + 1
        val now = System.currentTimeMillis()
        if (existingUsage == null) {
            memoryDao.insert(MemoryEntity(key = key, value = nextCount.toString(), type = "skill_usage"))
        } else {
            memoryDao.update(existingUsage.copy(value = nextCount.toString(), updatedAt = now))
        }
        if (!SkillLearningPolicy.shouldSuggest(nextCount)) return null

        val existingSkills = skillDao.getAll().first()
        val duplicate = existingSkills.any { getSkillSteps(it) == suggestion.steps }
        if (duplicate) return null
        saveSkill(
            name = suggestion.name,
            triggerPhrases = suggestion.triggers,
            steps = suggestion.steps,
            riskLevel = suggestion.riskLevel,
            enabled = false
        )
        Logger.i("Skills: created disabled learned suggestion '${suggestion.name}'")
        val created = skillDao.getAll().first().firstOrNull { it.name == suggestion.name }
        if (created != null) {
            // P1-C: the epistemic hypothesis is recorded the moment it exists. It stays
            // device-local and the suggestion stays DISABLED — saving is not approval; only
            // the user's explicit enable action (onSkillEnabled) can ever promote it.
            onSuggestionCreated(created, tool, nextCount)
        }
        return created
    }

    /**
     * Every lifecycle mutation below commits through the DAO, whose SQLite outbox triggers
     * atomically capture the mirror work inside the SAME transaction. The callbacks are
     * post-commit wake-ups/semantic events, never the durability mechanism: they receive the
     * row as committed (re-read), so a stale UI copy cannot report a wrong link or state.
     */
    suspend fun updateSkill(skill: SkillEntity) {
        skillDao.update(skill)
        onSkillSaved(skillDao.getById(skill.id) ?: skill)
    }

    suspend fun disableSkill(skill: SkillEntity) {
        skillDao.update(skill.copy(enabled = false))
        val committed = skillDao.getById(skill.id) ?: skill.copy(enabled = false)
        onSkillSaved(committed)
        // Explicit user correction: demotes any environment-learning promotion that was
        // built on the earlier approval. Local matching already ignores disabled skills.
        onSkillDisabled(committed)
    }

    suspend fun enableSkill(skill: SkillEntity) {
        skillDao.update(skill.copy(enabled = true))
        val committed = skillDao.getById(skill.id) ?: skill.copy(enabled = true)
        onSkillSaved(committed)
        // The explicit approval the promotion gate requires — the ONLY path that approves.
        onSkillEnabled(committed)
    }

    suspend fun deleteSkill(skill: SkillEntity) {
        Logger.d("Deleting skill: ${skill.name}")
        // The BEFORE DELETE trigger captures the linked AND any pending vault identity into
        // pending_tombstones within this delete transaction, so the link is never lost even
        // though the row is gone; the callback afterwards only wakes the drain.
        skillDao.delete(skill)
        onSkillDeleted(skill)
    }

    suspend fun findSkillByTrigger(text: String): SkillEntity? {
        return SkillTriggerMatcher.bestMatch(text, enabledSkills.first())
    }

    fun getSkillSteps(skill: SkillEntity): List<String> {
        return try {
            json.decodeFromString(ListSerializer(serializer<String>()), skill.stepsJson)
        } catch (_: Exception) {
            // Fallback for legacy data stored with the old format
            skill.stepsJson.split("\",\"").map { it.replace("\"", "") }
        }
    }
}
