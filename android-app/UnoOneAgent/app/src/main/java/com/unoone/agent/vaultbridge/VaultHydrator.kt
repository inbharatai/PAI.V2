package com.unoone.agent.vaultbridge

import com.unoone.agent.core.util.Logger
import com.unoone.agent.storage.dao.ConversationTurnDao
import com.unoone.agent.storage.dao.MemoryDao
import com.unoone.agent.storage.dao.SkillDao
import com.unoone.agent.storage.entity.ConversationTurnEntity
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
 * - Only records carrying the shared {kind:"memory"} / {kind:"skill"} /
 * {kind:"transcript"} / {kind:"envobs"} JSON envelope hydrate. The desktop
 * harness's own envelope ({schema:1, harness_id, scope, namespace, content}
 * — record types MESSAGE / PREFERENCE / CONTEXT_SNAPSHOT) hydrates too, as
 * a typed "harness_memory" row. Anything else (e.g. desktop-migrated raw
 * text, or desktop voice-recording transcripts) is skipped — never
 * structured into a fake memory or turn.
 * - Tombstoned records DELETE the linked cache row: deleted on any host,
 * stays deleted everywhere — a Skill removed on another phone must not stay
 * enabled here.
 * - Records the cache already has at the latest revision are skipped without
 *   a decrypt; a known record is re-read only when the vault metadata shows a
 *   strictly newer revision (another host rewrote it).
 * - Key/name conflicts: the row that already reached the vault wins. A
 *   local-only row (null link) is NEVER overwritten — it either pushes on
 *   the next drain (drain runs before hydration, so push wins by ordering)
 *   or it is device-local and the only copy in existence; adopting the
 *   vault's value for the same key would silently drop local content.
 *   Same-id updates apply only when the vault revision is strictly newer.
 * - Every failure is non-fatal (best-effort pull, like the mirror's push).
 */
class VaultHydrator(
    private val memoryDao: MemoryDao,
    private val skillDao: SkillDao,
    private val turnDao: ConversationTurnDao? = null,
    private val readerProvider: () -> VaultRecordReader?,
) {

    /** What one hydration pass did — surfaced for logs and tests; never hidden. */
    data class Result(
        val memoriesAdded: Int = 0,
        val memoriesUpdated: Int = 0,
        val skillsAdded: Int = 0,
        val skillsUpdated: Int = 0,
        val turnsAdded: Int = 0,
        val envFactsAdded: Int = 0,
        val envFactsUpdated: Int = 0,
        val skippedUnknown: Int = 0,
        /** Cache rows removed because their vault record is tombstoned. */
        val memoriesDeleted: Int = 0,
        val skillsDeleted: Int = 0,
        val turnsDeleted: Int = 0,
    ) {
        val total: Int
            get() = memoriesAdded + memoriesUpdated + skillsAdded + skillsUpdated + turnsAdded +
                envFactsAdded + envFactsUpdated

        operator fun plus(other: Result) = Result(
            memoriesAdded = memoriesAdded + other.memoriesAdded,
            memoriesUpdated = memoriesUpdated + other.memoriesUpdated,
            skillsAdded = skillsAdded + other.skillsAdded,
            skillsUpdated = skillsUpdated + other.skillsUpdated,
            turnsAdded = turnsAdded + other.turnsAdded,
            envFactsAdded = envFactsAdded + other.envFactsAdded,
            envFactsUpdated = envFactsUpdated + other.envFactsUpdated,
            skippedUnknown = skippedUnknown + other.skippedUnknown,
            memoriesDeleted = memoriesDeleted + other.memoriesDeleted,
            skillsDeleted = skillsDeleted + other.skillsDeleted,
            turnsDeleted = turnsDeleted + other.turnsDeleted,
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
        turnDao?.allOnce()?.forEach { it.vaultRecordId?.let { id -> knownRevisions[id] = it.vaultRevision } }
        val metadata = try {
            reader.listRecordMetadata()
        } catch (e: Exception) {
            Logger.w("VaultHydrator metadata listing failed: ${e.message}"); return Result()
        }
        var result = Result()
        for (fields in metadata) {
            val recordId = fields["record_id"] as? String ?: continue
            if (fields["tombstone"] == true) {
                // Deleted on another host — propagate: remove the linked cache
                // row so a tombstoned Skill/memory/turn cannot stay active here.
                result += propagateTombstone(recordId)
                continue
            }
            val type = fields["record_type"] as? String ?: continue
            val vaultRevision = (fields["revision"] as? Int) ?: 0
            val knownRevision = knownRevisions[recordId]
            if (knownRevision != null && vaultRevision <= knownRevision) continue // already current
            // Shared mobile envelopes are MEMORY/DOCUMENT/TRANSCRIPT; the
            // desktop harness memory envelope writes MESSAGE / PREFERENCE /
            // CONTEXT_SNAPSHOT — all payload-gated below.
            if (type != "MEMORY" && type != "DOCUMENT" && type != "TRANSCRIPT" &&
                type != "MESSAGE" && type != "PREFERENCE" && type != "CONTEXT_SNAPSHOT"
            ) continue

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
                "transcript" -> hydrateTurn(recordId, fields, envelope)
                "envobs" -> hydrateEnvFact(recordId, fields, envelope)
                null -> hydrateHarnessMemory(recordId, fields, envelope)
                else -> {
                    // A foreign JSON payload — never structured into a fake
                    // memory/skill/turn.
                    Result(skippedUnknown = 1)
                }
            }
        }
        return result
    }

    /**
     * A tombstoned vault record: delete the cache row linked to it, if any.
     * The vault metadata is the latest state of the record — tombstone=true
     * means the last write was a deletion — so a linked local row is stale by
     * definition and is removed. Unlinked local rows are never touched (they
     * are either push-pending or device-local and the only copy in existence).
     */
    private suspend fun propagateTombstone(recordId: String): Result {
        var result = Result()
        memoryDao.getByVaultRecordId(recordId)?.let { row ->
            memoryDao.delete(row)
            result += Result(memoriesDeleted = 1)
        }
        skillDao.getByVaultRecordId(recordId)?.let { row ->
            skillDao.delete(row)
            result += Result(skillsDeleted = 1)
        }
        turnDao?.getByVaultRecordId(recordId)?.let { row ->
            turnDao.deleteById(row.id)
            result += Result(turnsDeleted = 1)
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
            // Local-only row for the same key: keep it. It pushes on the next
            // drain (drain runs before hydration, so push-first ordering
            // normally links it already); overwriting the only local copy
            // with a vault record would be silent content loss.
            Logger.i("VaultHydrator: keeping local unlinked memory '$key' (push pending)")
            return Result(skippedUnknown = 1)
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
            // Local-only skill for the same name: keep it — it pushes on the
            // next drain; adopting the vault copy would silently drop the
            // local steps. The two-records rule resolves it after both push.
            Logger.i("VaultHydrator: keeping local unlinked skill '$name' (push pending)")
            return Result(skippedUnknown = 1)
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

    /**
     * A conversation turn authored on another host. Turns are append-only —
     * there is no rewrite path — so hydration is insert-only; the
     * revision-aware skip above already covers the (impossible today) case
     * of a same-id newer revision. Unknown fields never become fake turns.
     */
    private suspend fun hydrateTurn(
        recordId: String,
        fields: Map<String, Any?>,
        envelope: kotlinx.serialization.json.JsonObject,
    ): Result {
        val dao = turnDao ?: return Result(skippedUnknown = 1)
        val sessionId = fieldString(envelope, "sessionId")?.takeIf { it.isNotBlank() }
            ?: return Result(skippedUnknown = 1)
        val role = fieldString(envelope, "role")?.takeIf { it.isNotBlank() }
            ?: return Result(skippedUnknown = 1)
        val content = fieldString(envelope, "content") ?: return Result(skippedUnknown = 1)
        val inputType = fieldString(envelope, "inputType")?.takeIf { it.isNotBlank() } ?: "unknown"
        val revision = (fields["revision"] as? Int) ?: 1
        dao.insert(
            ConversationTurnEntity(
                sessionId = sessionId,
                role = role,
                content = content,
                inputType = inputType,
                createdAt = epochOf(fields["created_at"] as? String, System.currentTimeMillis()),
                vaultRecordId = recordId,
                vaultRevision = revision,
            ),
        )
        return Result(turnsAdded = 1)
    }

    /**
     * A user-confirmed environment fact (or correction) authored on ANOTHER
     * host — e.g. the user approved a skill on the Power desktop or a second
     * phone. Only verified facts and corrections ever reach the vault, so
     * hydrating adopts the other host's epistemic conclusion as a local
     * "envobs" row the planner can surface as a confirmed capability. The
     * full contract JSON is carried verbatim in the envelope's
     * observationJson; the envelope index fields are only a shortcut.
     * Unparseable or hypothesis-status payloads are skipped, never faked.
     */
    private suspend fun hydrateEnvFact(
        recordId: String,
        fields: Map<String, Any?>,
        envelope: kotlinx.serialization.json.JsonObject,
    ): Result {
        val subject = fieldString(envelope, "subject")?.takeIf { it.isNotBlank() }
            ?: return Result(skippedUnknown = 1)
        val observationJson = fieldString(envelope, "observationJson")
            ?: return Result(skippedUnknown = 1)
        val status = fieldString(envelope, "epistemicStatus") ?: return Result(skippedUnknown = 1)
        // The envelope claims must agree with the contract body — never trust
        // the shortcut fields alone.
        val record = try {
            com.unoone.agent.core.contracts.ContractJson.decodeFromString(
                com.unoone.agent.core.contracts.EnvObservation.serializer(),
                observationJson,
            )
        } catch (_: Exception) {
            null
        } ?: return Result(skippedUnknown = 1)
        if (record.subject != subject || record.epistemicStatus.serialName != status) {
            return Result(skippedUnknown = 1)
        }
        if (record.epistemicStatus != com.unoone.agent.core.contracts.EpistemicStatus.VERIFIED_FACT &&
            record.epistemicStatus != com.unoone.agent.core.contracts.EpistemicStatus.CORRECTION
        ) {
            return Result(skippedUnknown = 1) // a hypothesis must never leak across hosts
        }
        if (record.validate() !is com.unoone.agent.core.model.Result.Success) {
            return Result(skippedUnknown = 1)
        }
        val revision = (fields["revision"] as? Int) ?: 1
        val key = "envfact:$subject"
        val local = memoryDao.getByKey(key)
        val now = System.currentTimeMillis()
        if (local == null) {
            memoryDao.insert(
                MemoryEntity(
                    key = key,
                    value = observationJson,
                    type = "envobs",
                    createdAt = epochOf(fields["created_at"] as? String, now),
                    updatedAt = epochOf(fields["updated_at"] as? String, now),
                    vaultRecordId = recordId,
                    vaultRevision = revision,
                ),
            )
            return Result(envFactsAdded = 1)
        }
        if (local.vaultRecordId == null) {
            // Local-only fact for the same subject: keep it (push pending) —
            // same keep-local rule as memories; no silent content loss.
            Logger.i("VaultHydrator: keeping local unlinked env fact '$subject' (push pending)")
            return Result(skippedUnknown = 1)
        }
        if (local.vaultRecordId != recordId) {
            Logger.i("VaultHydrator: env fact '$subject' has two vault records; keeping local ${local.vaultRecordId}")
            return Result(skippedUnknown = 1)
        }
        if (revision > local.vaultRevision) {
            memoryDao.update(
                local.copy(
                    value = observationJson,
                    updatedAt = epochOf(fields["updated_at"] as? String, now),
                    vaultRevision = revision,
                ),
            )
            return Result(envFactsUpdated = 1)
        }
        return Result() // already current
    }

    /**
     * A desktop Power-harness memory (record types MESSAGE / PREFERENCE /
     * CONTEXT_SNAPSHOT): the envelope the harness adapter writes is
     * {schema:1, harness_id, scope, namespace, content, attributes} — it
     * carries no {kind} tag, so it lands here. It hydrates as a typed
     * "harness_memory" row keyed "harness:{scope}:{namespace}:{harness_id}",
     * so the phone can read what the desktop harness remembered — one memory,
     * one source. The harness's internal index records
     * (namespace "__pai_harness_internal__", {schema, scope, entries} — no
     * content) never hydrate: they are bookkeeping, not memory. Same
     * conflict rules as any other memory: keep-local for unlinked rows,
     * strictly-newer revisions only, two-records logged.
     */
    private suspend fun hydrateHarnessMemory(
        recordId: String,
        fields: Map<String, Any?>,
        envelope: kotlinx.serialization.json.JsonObject,
    ): Result {
        val schema = fieldInt(envelope, "schema") ?: return Result(skippedUnknown = 1)
        if (schema != 1) return Result(skippedUnknown = 1) // unknown envelope version
        val harnessId = fieldString(envelope, "harness_id")?.takeIf { it.isNotBlank() }
            ?: return Result(skippedUnknown = 1)
        val scope = fieldString(envelope, "scope")?.takeIf { it.isNotBlank() }
            ?: return Result(skippedUnknown = 1)
        val namespace = fieldString(envelope, "namespace") ?: return Result(skippedUnknown = 1)
        if (namespace == "__pai_harness_internal__") return Result(skippedUnknown = 1)
        val content = fieldString(envelope, "content")?.takeIf { it.isNotBlank() }
            ?: return Result(skippedUnknown = 1) // also drops the index records (no content)
        val revision = (fields["revision"] as? Int) ?: 1
        val key = "harness:$scope:$namespace:$harnessId"
        val local = memoryDao.getByKey(key)
        val now = System.currentTimeMillis()
        if (local == null) {
            memoryDao.insert(
                MemoryEntity(
                    key = key,
                    value = content,
                    type = "harness_memory",
                    createdAt = epochOf(fields["created_at"] as? String, now),
                    updatedAt = epochOf(fields["updated_at"] as? String, now),
                    vaultRecordId = recordId,
                    vaultRevision = revision,
                ),
            )
            return Result(memoriesAdded = 1)
        }
        if (local.vaultRecordId == null) {
            Logger.i("VaultHydrator: keeping local unlinked harness memory '$key' (push pending)")
            return Result(skippedUnknown = 1)
        }
        if (local.vaultRecordId != recordId) {
            Logger.i("VaultHydrator: harness memory '$key' has two vault records; keeping local ${local.vaultRecordId}")
            return Result(skippedUnknown = 1)
        }
        if (revision > local.vaultRevision) {
            memoryDao.update(
                local.copy(
                    value = content,
                    updatedAt = epochOf(fields["updated_at"] as? String, now),
                    vaultRevision = revision,
                ),
            )
            return Result(memoriesUpdated = 1)
        }
        return Result() // already current
    }
}