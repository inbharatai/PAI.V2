package com.unoone.agent.personal

import java.io.File
import kotlinx.serialization.json.*

/** Synthetic host-only wire check. Not peer transport or production identity adoption. */
object PersonalLedgerInterop {
    @JvmStatic fun main(args: Array<String>) {
        val dir = File(args.single()); val bytes = File(dir, "rust-ledger.json").readBytes()
        val vaultId = Json.parseToJsonElement(bytes.toString(Charsets.UTF_8)).jsonObject.getValue("local_vault_id").jsonPrimitive.content
        var ledger = PersonalLedger.decode(bytes, vaultId)
        val tid = ledger.view().tasks.single().spec.task_id
        check(ledger.view().tasks.single().spec.goal == "Shared Unicode goal नमस्ते")
        ledger = ledger.apply(PersonalRequest(PersonalLedger.id(), ledger.mutations.size.toLong(), PersonalAction.EDIT, tid,
            "Shared Unicode goal नमस्ते", "Kotlin draft हिन्दी", null, ledger.replica_id), 1791629205000)
        ledger = ledger.apply(PersonalRequest(PersonalLedger.id(), ledger.mutations.size.toLong(), PersonalAction.ACCEPT, tid, "", "", null, ledger.replica_id), 1791629205000)
        File(dir, "kotlin-ledger.json").writeBytes(ledger.bytes())
        println("Kotlin read/mutation of Rust ledger passed")
    }
}
