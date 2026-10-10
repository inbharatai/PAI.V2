package com.unoone.agent.personal

import com.unoone.agent.core.guardian.GUARDIAN_RECEIPT_PREFIX
import com.unoone.agent.core.guardian.Receipt
import kotlinx.serialization.json.Json

/** Parses a task draft note written through `recordGuardianNote`. Null for ordinary drafts. Never yields secrets
 * (the note was masked before it entered the ledger). */
fun parseGuardianReceipt(draft: String): Receipt? {
    if (!draft.startsWith(GUARDIAN_RECEIPT_PREFIX)) return null
    val json = draft.substringAfter('\n', "")
    return runCatching { Json { ignoreUnknownKeys = true }.decodeFromString(Receipt.serializer(), json) }.getOrNull()
}
