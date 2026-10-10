package com.unoone.agent.vault

import java.io.File
import java.nio.file.Files
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicLong
import kotlinx.serialization.json.*

/** Separate Linux JVM, -Xmx192m (also tested at 64m), real JNI. RSS is process RSS, NOT a precise
 * native allocator measurement. No assertion here is a phone/LMKD guarantee. */
object LowHeapNativeVault {
    private fun rss(): Long = File("/proc/self/status").useLines { lines ->
        lines.first { it.startsWith("VmRSS:") }.trim().split(Regex("\\s+"))[1].toLong() * 1024
    }
    @JvmStatic fun main(args: Array<String>) {
        check(Runtime.getRuntime().maxMemory() <= 256L * 1024 * 1024)
        check(NativeVaultKdf.nativeAvailable) { "Actual JNI library required" }
        check(File("/proc/self/maps").useLines { lines -> lines.any { it.contains("libunoone_android_vault_jni.so") } })
        if (args.getOrNull(1) == "--unlock-only") {
            val password = "synthetic low heap native vault phrase".toByteArray()
            try {
                val session = MobileVaultRepository(PrivateFileVaultIO(File(args[0]))).unlock(password)
                check(session.masterKey.any { it != 0.toByte() })
                session.close()
                check(session.masterKey.all { it == 0.toByte() })
            } finally { password.fill(0) }
            println("PASS: separate-process JNI restart-unlock and session key wipe; heapMax=${Runtime.getRuntime().maxMemory()}")
            return
        }
        val base = rss()
        val peak = AtomicLong(base)
        val running = AtomicBoolean(true)
        val monitor = Thread {
            while (running.get()) { peak.accumulateAndGet(rss(), ::maxOf); Thread.sleep(5) }
        }
        monitor.start()
        val password = "synthetic low heap native vault phrase".toByteArray()
        try {
            val root = if (args.isEmpty()) Files.createTempDirectory("native-vault-low-heap").toFile() else File(args[0])
            val io = PrivateFileVaultIO(root)
            val session = MobileVaultRepository(io).create(password)
            val master = session.masterKey.copyOf()
            val id = session.vaultId
            session.close()
            check(session.masterKey.all { it == 0.toByte() })
            val before = io.read("VAULT/header/header_a.json")
            val header = Json.parseToJsonElement(String(before)).jsonObject
            check(header.getValue("version").jsonPrimitive.int == 1)
            val kdf = header.getValue("kdf_params").jsonObject
            check(kdf.getValue("memory_kib").jsonPrimitive.int == 262144)
            check(kdf.getValue("iterations").jsonPrimitive.int == 3)
            check(kdf.getValue("parallelism").jsonPrimitive.int == 4)
            check(kdf.getValue("output_len").jsonPrimitive.int == 32)
            val reopened = MobileVaultRepository(PrivateFileVaultIO(root)).unlock(password)
            check(reopened.vaultId == id && reopened.masterKey.contentEquals(master))
            master.fill(0); reopened.close()
            check(reopened.masterKey.all { it == 0.toByte() })
            val wrong = "wrong password".toByteArray()
            try { check(runCatching { MobileVaultRepository(io).unlock(wrong) }.exceptionOrNull() is VaultAccessException) }
            finally { wrong.fill(0) }
            check(before.contentEquals(io.read("VAULT/header/header_a.json")))
            val tampered = JsonObject(header + ("header_hmac" to JsonPrimitive("00".repeat(32)))).toString().toByteArray()
            io.write("VAULT/header/header_a.json", tampered)
            try {
                check(runCatching { MobileVaultRepository(io).unlock(password) }.exceptionOrNull() is VaultAccessException)
                check(tampered.contentEquals(io.read("VAULT/header/header_a.json")))
            } finally { io.write("VAULT/header/header_a.json", before) }
            // Re-run same shared golden via real JNI in this tiny Java heap.
            NativeVaultKdfTest().`actual JNI agrees with unchanged shared Rust and Kotlin golden vector`()
            // Refused low-memory create writes NOTHING; refused unlock retains bytes.
            NativeVaultKdf.installAndroidMemoryProbe { NativeVaultKdf.Memory(Long.MAX_VALUE, true) }
            try {
                val empty = PrivateFileVaultIO(Files.createTempDirectory("native-vault-lowram").toFile())
                check(runCatching { MobileVaultRepository(empty).create(password) }.exceptionOrNull() is IllegalStateException)
                check(empty.list("").isEmpty())
                check(runCatching { MobileVaultRepository(io).unlock(password) }.exceptionOrNull() is IllegalStateException)
                check(before.contentEquals(io.read("VAULT/header/header_a.json")))
            } finally { NativeVaultKdf.installAndroidMemoryProbe(null) }
        } finally { password.fill(0); running.set(false); monitor.join() }
        val delta = peak.get() - base
        check(delta in (240L * 1024 * 1024)..(384L * 1024 * 1024)) { "Unexpected process RSS delta: $delta" }
        println("PASS: mapped actual JNI create/reopen/wrong-password/tampered-header/shared-vector/key-wipe/low-RAM-no-write. heapMax=${Runtime.getRuntime().maxMemory()} baseRss=$base peakRss=${peak.get()} deltaRss=$delta")
    }
}
