package com.unoone.agent.vault

import java.io.File
import java.nio.file.Files

/** Run separately with -Xmx320m, not as a memory-assuming Gradle/JUnit test. */
object LowHeapVaultAdmission {
    @JvmStatic fun main(args: Array<String>) {
        check(Runtime.getRuntime().maxMemory() < 352L * 1024 * 1024)
        check(!NativeVaultKdf.nativeAvailable) { "Run fallback-refusal probe without native library path" }
        val root = Files.createTempDirectory("low-heap-vault-admission").toFile()
        val io = PrivateFileVaultIO(root)
        val repo = MobileVaultRepository(io)
        fun refused(block: () -> Unit) {
            val error = runCatching(block).exceptionOrNull()
            check(error is IllegalArgumentException && error.message!!.contains("app heap is insufficient"))
        }
        refused { repo.create("synthetic low heap phrase".toByteArray()) }
        check(io.list("").isEmpty())
        val header = File(args.single()).readBytes()
        io.write("VAULT/header/header_a.json", header)
        refused { repo.unlock("synthetic local vault phrase only".toByteArray()) }
        check(header.contentEquals(io.read("VAULT/header/header_a.json")))
        println("PASS: low-heap create refused without writes; unlock refused with original header unchanged; no weaker KDF")
    }
}
