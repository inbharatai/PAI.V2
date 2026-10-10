package com.unoone.agent.vault

import org.junit.Assert.*
import org.junit.Test
import java.io.File
import java.nio.file.Files
import java.nio.channels.FileChannel
import java.nio.file.StandardOpenOption.READ

class PrivateFileVaultIOTest {
    private fun root() = Files.createTempDirectory("private-file-vault-test").toFile()

    @Test fun `interruption after temp fsync retains prior record and pending candidate`() {
        val root = root()
        PrivateFileVaultIO(root).write("record", "old ciphertext".toByteArray())
        val io = PrivateFileVaultIO(root, beforeAtomicReplace = { throw java.io.IOException("injected crash") })
        assertThrows(java.io.IOException::class.java) { io.write("record", "new ciphertext".toByteArray()) }
        assertEquals("old ciphertext", String(PrivateFileVaultIO(root).read("record")))
        val pending = root.listFiles()!!.single { it.name.startsWith("record.pending-") }
        assertEquals("new ciphertext", pending.readText())
    }

    @Test fun `atomic nested writes survive reopening with no leftover temp on success`() {
        val root = root()
        val io = PrivateFileVaultIO(root)
        io.write("VAULT/records/a.enc.json", "old".toByteArray())
        io.write("VAULT/records/a.enc.json", "new".toByteArray())
        assertEquals("new", String(PrivateFileVaultIO(root).read("VAULT/records/a.enc.json")))
        assertEquals(listOf("a.enc.json"), io.list("VAULT/records"))
    }

    @Test fun `reject traversal absolute empty and ambiguous paths`() {
        val io = PrivateFileVaultIO(root())
        for (path in listOf("../x", "/tmp/x", "", "VAULT//x", "VAULT/./x", "VAULT/../x", "VAULT\\x", "VAULT/x\u0000")) {
            assertThrows(IllegalArgumentException::class.java) { io.write(path, byteArrayOf(1)) }
        }
    }

    @Test fun `reject root and intermediate and leaf symlinks`() {
        val parent = root()
        val elsewhere = root()
        Files.createSymbolicLink(File(parent, "link").toPath(), elsewhere.toPath())
        assertThrows(IllegalArgumentException::class.java) { PrivateFileVaultIO(File(parent, "link")) }
        val io = PrivateFileVaultIO(parent)
        assertThrows(IllegalArgumentException::class.java) { io.write("link/file", byteArrayOf(1)) }
        File(elsewhere, "target").writeText("keep")
        Files.createSymbolicLink(File(parent, "leaf").toPath(), File(elsewhere, "target").toPath())
        assertThrows(IllegalArgumentException::class.java) { io.read("leaf") }
        assertThrows(IllegalArgumentException::class.java) { io.write("leaf", byteArrayOf(2)) }
        assertEquals("keep", File(elsewhere, "target").readText())
    }

    @Test fun `bounded read write and list refuse without deleting data`() {
        val root = root()
        val io = PrivateFileVaultIO(root, maxBytes = 8, maxEntries = 1)
        assertThrows(IllegalArgumentException::class.java) { io.write("big", ByteArray(9)) }
        File(root, "big").writeBytes(ByteArray(9))
        assertThrows(IllegalArgumentException::class.java) { io.read("big") }
        File(root, "other").writeText("keep")
        assertThrows(IllegalArgumentException::class.java) { io.list("") }
        assertTrue(File(root, "big").exists())
    }

    @Test fun `directory fsync failure is reported but committed bytes retained`() {
        val root = root()
        val io = PrivateFileVaultIO(root, syncDirectory = { dir -> if (dir == root) throw java.io.IOException("injected fsync fault") })
        assertThrows(java.io.IOException::class.java) { io.write("file", "retained".toByteArray()) }
        assertEquals("retained", File(root, "file").readText())
    }

    @Test fun `pending interrupted files and prior records are never cleaned automatically`() {
        val root = root()
        File(root, "a.pending-old").writeText("preserve")
        val io = PrivateFileVaultIO(root)
        io.write("a", "new".toByteArray())
        assertEquals("preserve", File(root, "a.pending-old").readText())
        assertThrows(VaultAccessException::class.java) { io.delete("a") }
        assertEquals("new", String(io.read("a")))
    }

    @Test fun `fsync is called after each new directory and final rename`() {
        val root = root()
        val synced = mutableListOf<String>()
        val io = PrivateFileVaultIO(root, syncDirectory = { dir ->
            FileChannel.open(dir.toPath(), READ).use { it.force(true) }
            synced.add(dir.name)
        })
        io.write("VAULT/records/one", byteArrayOf(1))
        assertEquals(listOf(checkNotNull(root.parentFile).name, root.name, "VAULT", "records"), synced)
    }
}
