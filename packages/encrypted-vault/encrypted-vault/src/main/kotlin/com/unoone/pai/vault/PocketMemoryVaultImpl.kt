package com.unoone.pai.vault

import com.unoone.pai.contracts.*
import com.unoone.pai.vault.crypto.Argon2idKdf
import com.unoone.pai.vault.crypto.VaultCipher
import com.unoone.pai.vault.crypto.VaultCipher.Algorithm
import com.unoone.pai.vault.crypto.VaultHeader
import com.unoone.pai.vault.crypto.VaultHeaderCodec
import com.unoone.pai.vault.journal.WriteAheadJournal
import com.unoone.pai.vault.session.DeviceSessionManager
import com.unoone.pai.vault.storage.VaultStorage
import kotlinx.serialization.encodeToString
import kotlinx.serialization.json.Json
import java.io.File
import java.security.MessageDigest
import java.util.concurrent.atomic.AtomicBoolean

/**
 * PocketMemoryVault — the core vault implementation.
 *
 * This is the single source of truth for UnoOne Mobile and UnoOne Power.
 * All persistent data lives on the encrypted USB drive. Host storage
 * (Android internal, desktop disk) is temporary cache only.
 *
 * Lifecycle:
 * 1. setupVault() — first time: create password, generate recovery key
 * 2. unlockVault() — subsequent use: enter password, derive key
 * 3. CRUD operations (addMemory, searchMemory, saveConversation, etc.)
 * 4. lockVault() — clear all keys from memory
 * 5. emergencyLock() — immediate lock (e.g., USB removal)
 *
 * Encryption:
 * - Password → Argon2id (256MB, 3 iterations, parallelism 4) → master key
 * - Master key → HMAC-SHA256 → domain keys (memories, chats, recordings, etc.)
 * - Data encrypted with domain key using XChaCha20-Poly1305 or AES-256-GCM
 * - Write-ahead journal for crash recovery
 * - Atomic writes for data integrity
 * - Tombstones for deletion propagation
 */
class PocketMemoryVaultImpl(
    private val vaultRoot: File
) : PocketMemoryVault {

    private val json = Json { prettyPrint = true; ignoreUnknownKeys = true }
    private val isUnlockedFlag = AtomicBoolean(false)
    private val sessionManager = DeviceSessionManager(vaultRoot)
    private var storage: VaultStorage? = null
    private var vaultHeader: VaultHeader? = null
    private var masterKey: ByteArray? = null
    private var failedAttempts = 0
    private var lockoutUntil: Long = 0

    private fun getStorage(): VaultStorage {
        return storage ?: throw IllegalStateException("Vault is not initialized — call setupVault or unlockVault first")
    }

    override val isUnlocked: Boolean
        get() = isUnlockedFlag.get()

    // ========================================
    // Vault lifecycle
    // ========================================

    override suspend fun setupVault(password: String, profileName: String?): VaultSetupResult {
        require(password.length >= 8) { "Password must be at least 8 characters" }

        // Generate salt and derive master key
        val salt = Argon2idKdf.generateSalt()
        val masterKey = Argon2idKdf.deriveKey(password.toCharArray(), salt)

        // Generate recovery key
        val (recoveryWords, recoveryEntropy) = Argon2idKdf.generateRecoveryKey()
        val recoverySalt = Argon2idKdf.generateSalt()
        val recoveryKeyDerived = Argon2idKdf.deriveKey(recoveryWords.toCharArray(), recoverySalt)

        // Detect best cipher for this platform
        val cipherAlgorithm = VaultCipher.detectBestAlgorithm()

        // Create verification tag — encrypt a known string with the master key
        // This allows us to verify password correctness without storing the password
        val verificationTag = VaultCipher.encrypt(
            cipherAlgorithm,
            masterKey,
            "unoone-vault-verification".encodeToByteArray()
        )

        // Create vault header
        val vaultId = java.util.UUID.randomUUID().toString()
        val now = java.time.Instant.now().toString()
        val header = VaultHeader(
            vaultId = vaultId,
            createdAt = now,
            updatedAt = now,
            profileName = profileName,
            cipherAlgorithm = cipherAlgorithm.displayName,
            kdfParams = com.unoone.pai.vault.crypto.KdfParamsHeader(),
            salt = java.util.Base64.getEncoder().encodeToString(salt),
            verificationTag = java.util.Base64.getEncoder().encodeToString(verificationTag),
            recoveryKeySet = true,
            recoveryKeySalt = java.util.Base64.getEncoder().encodeToString(recoverySalt)
        )

        // Create vault directory structure
        createVaultDirectories()

        // Write vault header (encrypted with master key)
        // The header itself is stored with a fixed key derived from the vault ID
        // This allows the header to be read to determine the cipher before unlocking
        val headerKey = deriveHeaderKey(vaultId)
        val headerJson = VaultHeaderCodec.encode(header)
        val headerEncrypted = VaultCipher.encrypt(
            cipherAlgorithm,
            headerKey,
            headerJson.encodeToByteArray()
        )
        val headerFile = File(vaultRoot, "identity/vault.json.enc")
        headerFile.parentFile?.mkdirs()
        headerFile.writeBytes(headerEncrypted)

        // Store recovery key hash (not the key itself)
        val recoveryKeyHash = sha256(recoveryKeyDerived)
        val recoveryFile = File(vaultRoot, "identity/recovery.json.enc")
        val recoveryData = """{"vaultId":"$vaultId","recoveryKeyHash":"${recoveryKeyHash.joinToString("") { "%02x".format(it) }}","recoveryKeySalt":"${java.util.Base64.getEncoder().encodeToString(recoverySalt)}"}"""
        val recoveryEncrypted = VaultCipher.encrypt(cipherAlgorithm, masterKey, recoveryData.encodeToByteArray())
        recoveryFile.writeBytes(recoveryEncrypted)

        // Write vault ID in plaintext (needed to find the header key)
        val vaultIdFile = File(vaultRoot, "identity/vault.id")
        vaultIdFile.writeText(vaultId)

        // Initialize storage
        this.vaultHeader = header
        this.masterKey = masterKey
        this.isUnlockedFlag.set(true)

        storage = VaultStorage(vaultRoot, File(vaultRoot, "identity/journal"))
        getStorage().setMasterKey(masterKey)
        getStorage().setCipherAlgorithm(cipherAlgorithm)

        // Create session
        val platform = detectPlatform()
        sessionManager.createSession(platform, getDeviceName())

        // Clear sensitive data from memory
        recoveryKeyDerived.fill(0)

        return VaultSetupResult(
            success = true,
            vaultId = vaultId,
            recoveryKey = recoveryWords,
            error = null
        )
    }

    override suspend fun unlockVault(password: String): VaultUnlockResult {
        // Check lockout
        if (System.currentTimeMillis() < lockoutUntil) {
            val remainingSeconds = (lockoutUntil - System.currentTimeMillis()) / 1000
            return VaultUnlockResult(
                success = false,
                failedAttempts = failedAttempts,
                lockoutUntil = java.time.Instant.ofEpochMilli(lockoutUntil).toString(),
                error = "Vault is locked for $remainingSeconds more seconds due to too many failed attempts"
            )
        }

        // Read vault ID
        val vaultIdFile = File(vaultRoot, "identity/vault.id")
        if (!vaultIdFile.exists()) {
            return VaultUnlockResult(
                success = false,
                error = "No vault found on this drive. Please set up a new vault first."
            )
        }
        val vaultId = vaultIdFile.readText().trim()

        // Read encrypted vault header
        val headerFile = File(vaultRoot, "identity/vault.json.enc")
        if (!headerFile.exists()) {
            return VaultUnlockResult(
                success = false,
                error = "Vault header not found"
            )
        }
        val headerEncrypted = headerFile.readBytes()

        // Derive header key
        val headerKey = deriveHeaderKey(vaultId)

        // Decrypt header
        val cipherAlgorithm = VaultCipher.detectBestAlgorithm()
        val headerJson = try {
            VaultCipher.decrypt(cipherAlgorithm, headerKey, headerEncrypted)
        } catch (e: SecurityException) {
            return VaultUnlockResult(
                success = false,
                error = "Vault header decryption failed — wrong platform or corrupted vault"
            )
        }

        val header = VaultHeaderCodec.decode(headerJson.decodeToString())

        // Derive key from password using stored salt
        val salt = java.util.Base64.getDecoder().decode(header.salt)
        val candidateKey = Argon2idKdf.deriveKey(password.toCharArray(), salt)

        // Verify password by attempting to decrypt the verification tag
        val verificationTag = java.util.Base64.getDecoder().decode(header.verificationTag)
        try {
            val decrypted = VaultCipher.decrypt(
                Algorithm.entries.find { it.displayName == header.cipherAlgorithm } ?: VaultCipher.detectBestAlgorithm(),
                candidateKey,
                verificationTag
            )
            if (decrypted.decodeToString() != "unoone-vault-verification") {
                // Wrong password
                failedAttempts++
                applyLockout()
                candidateKey.fill(0)
                return VaultUnlockResult(
                    success = false,
                    failedAttempts = failedAttempts,
                    error = "Incorrect password"
                )
            }
        } catch (e: SecurityException) {
            // Authentication failed — wrong password
            failedAttempts++
            applyLockout()
            candidateKey.fill(0)
            return VaultUnlockResult(
                success = false,
                failedAttempts = failedAttempts,
                error = "Incorrect password"
            )
        }

        // Password verified — set up vault
        this.vaultHeader = header
        this.masterKey = candidateKey
        this.isUnlockedFlag.set(true)
        this.failedAttempts = 0

        // Detect actual cipher from header
        val vaultCipher = Algorithm.entries.find { it.displayName == header.cipherAlgorithm } ?: VaultCipher.detectBestAlgorithm()
        storage = VaultStorage(vaultRoot, File(vaultRoot, "identity/journal"))
        getStorage().setMasterKey(candidateKey)
        getStorage().setCipherAlgorithm(vaultCipher)

        // Crash recovery
        getStorage().recoverFromCrash()

        // Create session
        val platform = detectPlatform()
        sessionManager.createSession(platform, getDeviceName())

        return VaultUnlockResult(
            success = true,
            vaultId = vaultId,
            failedAttempts = 0
        )
    }

    override suspend fun lockVault() {
        getStorage().clearKeys()
        masterKey?.fill(0)
        masterKey = null
        vaultHeader = null
        isUnlockedFlag.set(false)
        sessionManager.endSession()
    }

    override suspend fun emergencyLock() {
        // Immediately clear all sensitive data without graceful shutdown
        getStorage().clearKeys()
        masterKey?.fill(0)
        masterKey = null
        vaultHeader = null
        isUnlockedFlag.set(false)
        sessionManager.endSession()
    }

    // ========================================
    // CRUD operations
    // ========================================

    override suspend fun <T : Any> addMemory(record: T, serializer: (T) -> String): String {
        require(isUnlocked) { "Vault is locked" }
        val id = java.util.UUID.randomUUID().toString()
        val data = serializer(record).encodeToByteArray()
        getStorage().write("memory", id, data)
        return id
    }

    override suspend fun <T : Any> updateMemory(id: String, record: T, serializer: (T) -> String): Boolean {
        require(isUnlocked) { "Vault is locked" }
        if (!getStorage().exists("memory", id)) return false
        val data = serializer(record).encodeToByteArray()
        getStorage().write("memory", id, data)
        return true
    }

    override suspend fun searchMemory(query: String, type: MemoryType?, limit: Int): List<Memory> {
        require(isUnlocked) { "Vault is locked" }
        val domain = if (type != null) "memory/${type.name.lowercase()}" else "memory"
        val ids = getStorage().list(domain).take(limit)
        return ids.mapNotNull { id ->
            try {
                val data = getStorage().read(domain, id)
                json.decodeFromString<Memory>(data.decodeToString())
            } catch (e: Exception) {
                null // Skip corrupt records
            }
        }.filter {
            it.content.contains(query, ignoreCase = true) ||
            it.key.contains(query, ignoreCase = true) ||
            it.tags.any { tag -> tag.contains(query, ignoreCase = true) }
        }
    }

    override suspend fun deleteMemory(id: String): Boolean {
        require(isUnlocked) { "Vault is locked" }
        // Search all memory domains for this ID
        val memoryTypes = listOf("memory/personal", "memory/preferences", "memory/conversations",
            "memory/tasks", "memory/knowledge", "memory/accessibility", "memory/skills", "memory")
        for (domain in memoryTypes) {
            if (getStorage().exists(domain, id)) {
                getStorage().delete(domain, id)
                return true
            }
        }
        return false
    }

    override suspend fun retrieveRelevantMemory(context: String, limit: Int): List<Memory> {
        require(isUnlocked) { "Vault is locked" }
        // Simple keyword matching for now; will be enhanced with embeddings later
        return searchMemory(context, null, limit)
    }

    override suspend fun saveConversation(conversation: Conversation): String {
        require(isUnlocked) { "Vault is locked" }
        val id = conversation.metadata.id
        val data = json.encodeToString(Conversation.serializer(), conversation).encodeToByteArray()
        getStorage().write("chats", id, data)
        return id
    }

    override suspend fun saveTask(task: Task): String {
        require(isUnlocked) { "Vault is locked" }
        val id = task.metadata.id
        val data = json.encodeToString(Task.serializer(), task).encodeToByteArray()
        getStorage().write("memory/tasks", id, data)
        return id
    }

    override suspend fun saveRecording(recording: Recording): String {
        require(isUnlocked) { "Vault is locked" }
        val id = recording.metadata.id
        val data = json.encodeToString(Recording.serializer(), recording).encodeToByteArray()
        getStorage().write("recordings", id, data)
        return id
    }

    override suspend fun saveTranscript(transcript: Transcript): String {
        require(isUnlocked) { "Vault is locked" }
        val id = transcript.metadata.id
        val data = json.encodeToString(Transcript.serializer(), transcript).encodeToByteArray()
        getStorage().write("recordings/transcripts", id, data)
        return id
    }

    override suspend fun saveSummary(summary: RecordingSummary): String {
        require(isUnlocked) { "Vault is locked" }
        val id = summary.id
        val data = json.encodeToString(RecordingSummary.serializer(), summary).encodeToByteArray()
        getStorage().write("recordings/summaries", id, data)
        return id
    }

    override suspend fun indexDocument(document: Document): String {
        require(isUnlocked) { "Vault is locked" }
        val id = document.metadata.id
        val data = json.encodeToString(Document.serializer(), document).encodeToByteArray()
        getStorage().write("documents", id, data)
        return id
    }

    override suspend fun saveSkill(skill: Skill): String {
        require(isUnlocked) { "Vault is locked" }
        val id = skill.metadata.id
        val data = json.encodeToString(Skill.serializer(), skill).encodeToByteArray()
        getStorage().write("memory/skills", id, data)
        return id
    }

    override suspend fun rebuildIndex(): Boolean {
        require(isUnlocked) { "Vault is locked" }
        // Read all records and rebuild the search index
        // Placeholder — full implementation will use embeddings
        return true
    }

    override suspend fun verifyIntegrity(): IntegrityCheckResult {
        if (!isUnlocked) {
            return IntegrityCheckResult(
                valid = false,
                errors = listOf("Vault is locked — cannot verify integrity")
            )
        }

        val errors = mutableListOf<String>()
        val warnings = mutableListOf<String>()
        var recoveredFiles = 0
        var corruptedFiles = 0

        // Check vault header
        if (vaultHeader == null) {
            errors.add("Vault header is missing")
        }

        // Check all domains for corrupt files
        val domains = listOf("memory", "chats", "recordings", "documents", "settings", "audit")
        for (domain in domains) {
            val ids = getStorage().list(domain)
            for (id in ids) {
                try {
                    getStorage().read(domain, id)
                } catch (e: SecurityException) {
                    corruptedFiles++
                    errors.add("Corrupt file in $domain/$id: ${e.message}")
                } catch (e: Exception) {
                    corruptedFiles++
                    errors.add("Error reading $domain/$id: ${e.message}")
                }
            }
        }

        // Run crash recovery
        val crashResult = getStorage().recoverFromCrash()
        recoveredFiles = crashResult.recovered

        return IntegrityCheckResult(
            valid = errors.isEmpty(),
            errors = errors,
            warnings = warnings,
            recoveredFiles = recoveredFiles,
            corruptedFiles = corruptedFiles
        )
    }

    override suspend fun getMetadata(): VaultMetadata {
        val header = vaultHeader ?: throw IllegalStateException("Vault is locked")
        return VaultMetadata(
            vaultId = header.vaultId,
            schemaVersion = header.schemaVersion,
            createdAt = header.createdAt,
            updatedAt = header.updatedAt,
            profileName = header.profileName,
            kdfAlgorithm = header.kdfAlgorithm,
            kdfParams = com.unoone.pai.contracts.KdfParams(
                memoryBytes = header.kdfParams.memoryBytes,
                iterations = header.kdfParams.iterations,
                parallelism = header.kdfParams.parallelism
            ),
            cipherAlgorithm = header.cipherAlgorithm,
            deviceSessions = header.deviceSessions.map {
                DeviceSession(
                    deviceId = it.deviceId,
                    platform = Platform.valueOf(it.platform),
                    deviceName = it.deviceName,
                    firstConnectedAt = it.firstConnectedAt,
                    lastConnectedAt = it.lastConnectedAt,
                    isActive = it.isActive
                )
            },
            recoveryKeySet = header.recoveryKeySet
        )
    }

    // ========================================
    // Internal helpers
    // ========================================

    private fun createVaultDirectories() {
        val dirs = listOf(
            "identity", "memory/personal", "memory/preferences", "memory/conversations",
            "memory/tasks", "memory/knowledge", "memory/accessibility", "memory/skills",
            "chats", "recordings/audio", "recordings/transcripts", "recordings/summaries",
            "documents", "reports", "browser", "camera", "settings", "indexes",
            "audit", "recovery", "identity/journal"
        )
        for (dir in dirs) {
            File(vaultRoot, "VAULT/$dir").mkdirs()
        }
    }

    private fun deriveHeaderKey(vaultId: String): ByteArray {
        // Derive a key from the vault ID for encrypting the header
        // This key is NOT the master key — it only protects the header
        val digest = MessageDigest.getInstance("SHA-256")
        return digest.digest("unoone-vault-header-$vaultId".encodeToByteArray())
    }

    private fun applyLockout() {
        // Increasing delay after failed attempts:
        // 1-3: no delay
        // 4-6: 30 seconds
        // 7-9: 5 minutes
        // 10+: 30 minutes
        when {
            failedAttempts >= 10 -> lockoutUntil = System.currentTimeMillis() + 30 * 60 * 1000
            failedAttempts >= 7 -> lockoutUntil = System.currentTimeMillis() + 5 * 60 * 1000
            failedAttempts >= 4 -> lockoutUntil = System.currentTimeMillis() + 30 * 1000
        }
    }

    private fun sha256(data: ByteArray): ByteArray {
        val digest = MessageDigest.getInstance("SHA-256")
        return digest.digest(data)
    }

    private fun detectPlatform(): Platform {
        return try {
            Class.forName("android.app.Activity")
            Platform.ANDROID
        } catch (e: ClassNotFoundException) {
            val os = System.getProperty("os.name").lowercase()
            when {
                os.contains("win") -> Platform.WINDOWS
                os.contains("mac") -> Platform.MACOS
                os.contains("nux") -> Platform.LINUX
                else -> Platform.WINDOWS // default
            }
        }
    }

    private fun getDeviceName(): String {
        return try {
            Class.forName("android.os.Build")
            // Android — would use Build.MODEL
            "Android Device"
        } catch (e: ClassNotFoundException) {
            "${System.getProperty("user.name")}@${java.net.InetAddress.getLocalHost().hostName}"
        }
    }
}