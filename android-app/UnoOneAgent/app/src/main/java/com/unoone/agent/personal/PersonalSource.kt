package com.unoone.agent.personal

import com.unoone.agent.core.personal.Operation
import com.unoone.agent.vault.VaultRecordReader
import kotlinx.serialization.json.*

/** Local reviewed source selection; does not deserialize an executable grant. */
data class PersonalSource(val recordIds: List<String> = emptyList(), val query: String = "") {
    fun validate() {
        require(recordIds.size <= 8 && recordIds.distinct().size == recordIds.size)
        require(recordIds.all { runCatching { java.util.UUID.fromString(it).toString() == it }.getOrDefault(false) })
        require(query.toByteArray().size <= 256 && (recordIds.isEmpty() || query.isNotBlank()))
    }
    companion object {
        /** Existing session-bound reader authenticates metadata and decrypts only exact selected IDs. */
        fun read(reader: VaultRecordReader, permit: PersonalDraftPermit, now: Long, generation: Long): String {
            val grant = permit.grant.snapshot()
            check(permit.grant.live(grant.replica_id, now, generation))
            val selection = permit.source
            selection.validate()
            var total = 0
            val hits = selection.recordIds.mapNotNull { id ->
                check(grant.scopes.data.any { it.resource_id == id && Operation.READ in it.operations })
                val (metadata, bytes) = reader.readRecord(id)
                try {
                    check(metadata["record_type"] == "DOCUMENT" && metadata["tombstone"] != true) { "Selected record is not a live note/document" }
                    require(bytes.size <= 64 * 1024)
                    val text = bytes.toString(Charsets.UTF_8)
                    if (!text.contains(selection.query, ignoreCase = true)) null else {
                        total += bytes.size
                        require(total <= 8192) { "Narrow the selected source; context exceeds 8192 bytes" }
                        buildJsonObject { put("record_id", id); put("content", text) }
                    }
                } finally { bytes.fill(0) }
            }
            return JsonArray(hits).toString()
        }
    }
}
