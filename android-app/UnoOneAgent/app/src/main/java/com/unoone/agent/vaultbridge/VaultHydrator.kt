package com.unoone.agent.vaultbridge

import com.unoone.agent.core.util.Logger
import com.unoone.agent.storage.dao.MemoryDao
import com.unoone.agent.storage.dao.SkillDao
import com.unoone.agent.storage.entity.MemoryEntity
import com.unoone.agent.storage.entity.SkillEntity
import com.unoone.agent.vault.VaultRecordReader
import kotlinx.serialization.json.booleanOrNull
import kotlinx.serialization.json.intOrNull
import java.time.Instant

/**
 * The vault→Android read path: hydrates records authored on OTHER hosts
 * (Power, or another phone) into the local cache, so the drive vault is
 * readable on the phone — not just writable. Runs after [VaultMirror.drainBacklog]
 * on unlock (push local first, then pull remote; that way a record both hosts
 * touched resolves to the vault's own revision numbering).
 *
 * Bounded and honest by construction:
 * - Only records carrying the shared {kind:"memory"} / {kind:"skill"} JSON
 *   envelope hydrate. Anything else (e.g. desktop-migrated raw text) is
 *   skipped — never structured into a fake memory.
 * - Tombstoned records are skipped: deleted on any host, stays deleted.
 * - Records the cache already has at the latest revision are skipped without
 *   a decrypt; a known record is re-read only when the vault metadata shows a
 *   strictly newer revision (another host rewrote it).
 * - Key/name conflicts: the row that already reached the vault wins; a
 *   local-only row (null link) is ADOPTED (updated + linked), never silently
 *   overwritten with a different vault record. Same-id updates apply only
 *   when the vault revision is strictly newer.
 * - Every failure is non-fatal (best-effort pull, like the mirror's push).
 */
class VaultHydrator(
    private val memoryDao: MemoryDao,
    private val skillDao: SkillDao,
    private val readerProvider: () -> VaultRecordReader?,
) {

    /** What one hydration pass did — surfaced for logs and tests; never hidden. */
    data class Result(
        val memoriesAdded: Int = 0,
        val memoriesUpdated: Int = 0,
        val skillsAdded: Int = 0,
        val skillsUpdated: Int = 0,
        val skippedUnknown: Int = 0,
    ) {
        val total: Int get() = memoriesAdded + memoriesUpdated + skillsAdded + skillsUpdated

        operator fun plus(other: Result) = Result(
            memoriesAdded = memoriesAdded + other.memoriesAdded,
            memoriesUpdated = memoriesUpdated + other.memoriesUpdated,
            skillsAdded = skillsAdded + other.skillsAdded,
            skillsUpdated = skillsUpdated + other.skillsUpdated,
            skippedUnknown = skippedUnknown + other.skippedUnknown,
        )
    }

    suspend fun hydrateFromVault(): Result {
        val reader = try {
            readerProvider()
        } catch (e: Exception) {
            Logger.w("VaultHydrator reader unavailable: ${e.message}"); return Result()
        } ?: return Result()
        return try {
            hydrate(reader)
        } catch (e: Exception) {
            Logger.w("VaultHydrator non-fatal: ${e.message}")
            Result()
        }
    }

    private suspend fun hydrate(reader: VaultRecordReader): Result {
        // Known record id → the revision this cache last pulled. A record is
        // re-read ONLY when the vault metadata carries a strictly newer
        // revision (another host rewrote it); otherwise it is skipped without
        // a decrypt.
        val knownRevisions = HashMap<String, Int>()
        memoryDao.allOnce().forEach { it.vaultRecordId?.let { id -> knownRevisions[id] = it.vaultRevision } }
        skillDao.allOnce().forEach { it.vaultRecordId?.let { id -> knownRevisions[id] = it.vaultRevision } }
        val metadata = try {
            reader.listRecordMetadata()
        } catch (e: Exception) {
            Logger.w("VaultHydrator metadata listing failed: ${e.message}"); return Result()
        }
        var result = Result()
        for (fields in metadata) {
            if (fields["tombstone"] == true) continue
            val recordId = fields["record_id"] as? String ?: continue
            val type = fields["record_type"] as? String ?: continue
            val vaultRevision = (fields["revision"] as? Int) ?: 0
            val knownRevision = knownRevisions[recordId]
            if (knownRevision != null && vaultRevision <= knownRevision) continue // already current
            if (type != "MEMORY" && type != "DOCUMENT") continue

            val payload = try {
                reader.readRecord(recordId).second
            } catch (e: Exception) {
                Logger.w("VaultHydrator skip $recordId (unreadable): ${e.message}"); continue
            }
            val envelope = try {
                kotlinx.serialization.json.Json.parseToJsonElement(String(payload, Charsets.UTF_8))
                    .let { it as? kotlinx.serialization.json.JsonObject }
            } catch (_: Exception) {
                null
            }
            if (envelope == null) {
                // Not JSON at all (e.g. desktop-migrated raw text) — skipped,
                // counted, never structured into a fake memory/skill.
                result += Result(skippedUnknown = 1)
                continue
            }
            val kind = (envelope["kind"] as? kotlinx.serialization.json.JsonPrimitive)?.content
            result += when (kind) {
                "memory" -> hydrateMemory(recordId, fields, envelope)
                "skill" -> hydrateSkill(recordId, fields, envelope)
                else -> {
                    // A foreign JSON payload — never structured into a fake
                    // memory/skill.
                    Result(skippedUnknown = 1)
                }
            }
        }
        return result
    }

    private fun fieldString(envelope: kotlinx.serialization.json.JsonObject, key: String): String? =
        (envelope[key] as? kotlinx.serialization.json.JsonPrimitive)?.takeIf { it.isString }?.content

    private fun fieldInt(envelope: kotlinx.serialization.json.JsonObject, key: String): Int? =
        (envelope[key] as? kotlinx.serialization.json.JsonPrimitive)?.intOrNull

    private fun fieldBool(envelope: kotlinx.serialization.json.JsonObject, key: String): Boolean? =
        (envelope[key] as? kotlinx.serialization.json.JsonPrimitive)?.booleanOrNull

    private fun epochOf(iso: String?, fallback: Long): Long = try {
        Instant.parse(iso).toEpochMilli()
    } catch (_: Exception) {
        fallback
    }

    private suspend fun hydrateMemory(
        recordId: String,
        fields: Map<String, Any?>,
        envelope: kotlinx.serialization.json.JsonObject,
    ): Result {
        val key = fieldString(envelope, "key")?.takeIf { it.isNotBlank() } ?: return Result(skippedUnknown = 1)
        val value = fieldString(envelope, "value") ?: return Result(skippedUnknown = 1)
        val type = fieldString(envelope, "type")?.takeIf { it.isNotBlank() } ?: "general"
        val revision = (fields["revision"] as? Int) ?: 1
        val local = memoryDao.getByKey(key)
        val now = System.currentTimeMillis()
        if (local == null) {
            memoryDao.insert(
                MemoryEntity(
                    key = key,
                    value = value,
                    type = type,
                    createdAt = epochOf(fields["created_at"] as? String, now),
                    updatedAt = epochOf(fields["updated_at"] as? String, now),
                    vaultRecordId = recordId,
                    vaultRevision = revision,
                ),
            )
            return Result(memoriesAdded = 1)
        }
        if (local.vaultRecordId == null) {
            // Local-only row for the same key: adopt the vault's version — the
            // vault is canonical, and the row never reached it.
            memoryDao.update(
                local.copy(
                    value = value,
                    type = type,
                    updatedAt = epochOf(fields["updated_at"] as? String, now),
                    vaultRecordId = recordId,
                    vaultRevision = revision,
                ),
            )
            return Result(memoriesUpdated = 1)
        }
        if (local.vaultRecordId != recordId) {
            // Both hosts minted their own record for this key; first writer
            // wins. The local link stays; the vault copy is left for the
            // desktop to reconcile. Never silently re-point a synced row.
            Logger.i("VaultHydrator: key '$key' has two vault records; keeping local ${local.vaultRecordId}")
            return Result(skippedUnknown = 1)
        }
        if (revision > local.vaultRevision) {
            memoryDao.update(
                local.copy(
                    value = value,
                    type = type,
                    updatedAt = epochOf(fields["updated_at"] as? String, now),
                    vaultRevision = revision,
                ),
            )
            return Result(memoriesUpdated = 1)
        }
        return Result() // already current
    }

    private suspend fun hydrateSkill(
        recordId: String,
        fields: Map<String, Any?>,
        envelope: kotlinx.serialization.json.JsonObject,
    ): Result {
        val name = fieldString(envelope, "name")?.takeIf { it.isNotBlank() } ?: return Result(skippedUnknown = 1)
        val triggerPhrases = fieldString(envelope, "triggerPhrases") ?: return Result(skippedUnknown = 1)
        val stepsJson = fieldString(envelope, "stepsJson") ?: return Result(skippedUnknown = 1)
        val riskLevel = fieldInt(envelope, "riskLevel") ?: 0
        val enabled = fieldBool(envelope, "enabled") ?: true
        val revision = (fields["revision"] as? Int) ?: 1
        val local = skillDao.allOnce().firstOrNull { it.name == name }
        val now = System.currentTimeMillis()
        if (local == null) {
            skillDao.insert(
                SkillEntity(
                    name = name,
                    triggerPhrases = triggerPhrases,
                    stepsJson = stepsJson,
                    riskLevel = riskLevel,
                    enabled = enabled,
                    createdAt = epochOf(fields["created_at"] as? String, now),
                    updatedAt = epochOf(fields["updated_at"] as? String, now),
                    vaultRecordId = recordId,
                    vaultRevision = revision,
                ),
            )
            return Result(skillsAdded = 1)
        }
        if (local.vaultRecordId == null) {
            skillDao.update(
                local.copy(
                    triggerPhrases = triggerPhrases,
                    stepsJson = stepsJson,
                    riskLevel = riskLevel,
                    enabled = enabled,
                    updatedAt = epochOf(fields["updated_at"] as? String, now),
                    vaultRecordId = recordId,
                    vaultRevision = revision,
                ),
            )
            return Result(skillsUpdated = 1)
        }
        if (local.vaultRecordId != recordId) {
            Logger.i("VaultHydrator: skill '$name' has two vault records; keeping local ${local.vaultRecordId}")
            return Result(skippedUnknown = 1)
        }
        if (revision > local.vaultRevision) {
            skillDao.update(
                local.copy(
                    triggerPhrases = triggerPhrases,
                    stepsJson = stepsJson,
                    riskLevel = riskLevel,
                    enabled = enabled,
                    updatedAt = epochOf(fields["updated_at"] as? String, now),
                    vaultRevision = revision,
                ),
            )
            return Result(skillsUpdated = 1)
        }
        return Result() // already current
    }
}