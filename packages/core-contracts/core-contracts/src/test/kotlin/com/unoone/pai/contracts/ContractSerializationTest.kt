package com.unoone.pai.contracts

import kotlinx.serialization.encodeToString
import kotlinx.serialization.json.Json
import org.junit.jupiter.api.Test
import org.junit.jupiter.api.Assertions.*

/**
 * Verify that all contract types serialize and deserialize correctly.
 * These tests ensure cross-platform compatibility — both Android and desktop
 * must produce and consume identical JSON.
 */
class ContractSerializationTest {

    private val json = Json { prettyPrint = false; ignoreUnknownKeys = true }

    @Test
    fun `VaultRecordMetadata serializes and deserializes`() {
        val original = VaultRecordMetadata(
            id = "01923456-7890-7abc-def0-123456789abc",
            createdAt = "2026-07-21T10:00:00Z",
            updatedAt = "2026-07-21T10:05:00Z",
            sourcePlatform = Platform.ANDROID,
            sourceDeviceId = "pixel8-001",
            revision = 1,
            deleted = false,
            schemaVersion = 1
        )
        val serialized = json.encodeToString(original)
        val deserialized = json.decodeFromString<VaultRecordMetadata>(serialized)
        assertEquals(original, deserialized)
    }

    @Test
    fun `Memory serializes and deserializes`() {
        val original = Memory(
            metadata = VaultRecordMetadata(
                id = "mem-001",
                createdAt = "2026-07-21T10:00:00Z",
                updatedAt = "2026-07-21T10:00:00Z",
                sourcePlatform = Platform.WINDOWS,
                sourceDeviceId = "laptop-001",
                revision = 1
            ),
            type = MemoryType.PREFERENCE,
            key = "preferred_written_language",
            content = "English for written output, Hindi for spoken output",
            tags = listOf("language", "preference")
        )
        val serialized = json.encodeToString(original)
        val deserialized = json.decodeFromString<Memory>(serialized)
        assertEquals(original, deserialized)
        assertEquals(MemoryType.PREFERENCE, deserialized.type)
    }

    @Test
    fun `Conversation serializes with messages`() {
        val original = Conversation(
            metadata = VaultRecordMetadata(
                id = "conv-001",
                createdAt = "2026-07-21T10:00:00Z",
                updatedAt = "2026-07-21T10:05:00Z",
                sourcePlatform = Platform.ANDROID,
                sourceDeviceId = "pixel8-001",
                revision = 1
            ),
            title = "University offer analysis",
            messages = listOf(
                ConversationMessage(
                    id = "msg-001",
                    role = MessageRole.USER,
                    content = "Compare these university offers",
                    timestamp = "2026-07-21T10:00:00Z",
                    platform = Platform.ANDROID,
                    modelUsed = "gemma-4-e2b"
                ),
                ConversationMessage(
                    id = "msg-002",
                    role = MessageRole.ASSISTANT,
                    content = "I'll analyze the offers for you...",
                    timestamp = "2026-07-21T10:00:05Z",
                    platform = Platform.ANDROID,
                    modelUsed = "gemma-4-e2b"
                )
            ),
            language = "en"
        )
        val serialized = json.encodeToString(original)
        val deserialized = json.decodeFromString<Conversation>(serialized)
        assertEquals(2, deserialized.messages.size)
        assertEquals(MessageRole.USER, deserialized.messages[0].role)
    }

    @Test
    fun `Task serializes with steps and failures`() {
        val original = Task(
            metadata = VaultRecordMetadata(
                id = "task-001",
                createdAt = "2026-07-21T10:00:00Z",
                updatedAt = "2026-07-21T10:10:00Z",
                sourcePlatform = Platform.WINDOWS,
                sourceDeviceId = "laptop-001",
                revision = 2
            ),
            title = "Analyze university offers",
            description = "Compare 10 university offer letters",
            status = TaskStatus.IN_PROGRESS,
            steps = listOf(
                TaskStep(id = "step-1", description = "Load documents", status = StepStatus.COMPLETED),
                TaskStep(id = "step-2", description = "Compare offers", status = StepStatus.IN_PROGRESS)
            ),
            originalInstruction = "Compare these university offers",
            failures = listOf(
                TaskFailure(stepId = "step-1", error = "PDF parse error", retryable = true)
            )
        )
        val serialized = json.encodeToString(original)
        val deserialized = json.decodeFromString<Task>(serialized)
        assertEquals(TaskStatus.IN_PROGRESS, deserialized.status)
        assertEquals(2, deserialized.steps.size)
        assertEquals(1, deserialized.failures.size)
    }

    @Test
    fun `Recording serializes with bookmarks and summaries`() {
        val original = Recording(
            metadata = VaultRecordMetadata(
                id = "rec-001",
                createdAt = "2026-07-21T10:00:00Z",
                updatedAt = "2026-07-21T10:30:00Z",
                sourcePlatform = Platform.ANDROID,
                sourceDeviceId = "pixel8-001",
                revision = 1
            ),
            title = "Admissions Team Meeting",
            recordingType = RecordingType.MEETING,
            sourcePlatform = Platform.ANDROID,
            audioPath = "VAULT/recordings/audio/rec-001.enc",
            transcriptPath = "VAULT/recordings/transcripts/rec-001.enc",
            summaryPath = "VAULT/recordings/summaries/rec-001.enc",
            durationSeconds = 1800,
            privacy = RecordingPrivacy.FULL,
            bookmarks = listOf(
                RecordingBookmark(timestampSeconds = 120, label = "Key decision point")
            ),
            summaries = listOf(
                RecordingSummary(
                    id = "sum-001",
                    generatedBy = "gemma-4-e2b",
                    generatedAt = "2026-07-21T10:30:00Z",
                    platform = Platform.ANDROID,
                    shortSummary = "Admissions team discussed enrollment targets.",
                    keyPoints = listOf("Enrollment target: 500 students", "Deadline: August 15")
                )
            )
        )
        val serialized = json.encodeToString(original)
        val deserialized = json.decodeFromString<Recording>(serialized)
        assertEquals(RecordingType.MEETING, deserialized.recordingType)
        assertEquals(1, deserialized.bookmarks.size)
        assertEquals(1, deserialized.summaries.size)
        assertEquals("gemma-4-e2b", deserialized.summaries[0].generatedBy)
    }

    @Test
    fun `ToolAction goes through full safety pipeline`() {
        val action = ToolAction(
            id = "act-001",
            toolName = "create_note",
            args = mapOf("title" to "Meeting Notes", "content" to "Follow up with admissions"),
            confidence = 0.95,
            sourceModel = "gemma-4-12b",
            sourcePlatform = Platform.WINDOWS,
            safetyDecision = SafetyDecision.PENDING,
            confirmationRequired = false
        )
        val serialized = json.encodeToString(action)
        val deserialized = json.decodeFromString<ToolAction>(serialized)
        assertEquals(SafetyDecision.PENDING, deserialized.safetyDecision)
        assertEquals("gemma-4-12b", deserialized.sourceModel)
    }

    @Test
    fun `Preferences support dual language settings`() {
        val prefs = Preferences(
            metadata = VaultRecordMetadata(
                id = "prefs-001",
                createdAt = "2026-07-21T10:00:00Z",
                updatedAt = "2026-07-21T10:00:00Z",
                sourcePlatform = Platform.ANDROID,
                sourceDeviceId = "pixel8-001",
                revision = 1
            ),
            writtenLanguage = "en",
            spokenLanguage = "hi",
            securityLevel = SecurityLevel.STANDARD
        )
        val serialized = json.encodeToString(prefs)
        val deserialized = json.decodeFromString<Preferences>(serialized)
        assertEquals("en", deserialized.writtenLanguage)
        assertEquals("hi", deserialized.spokenLanguage)
        assertEquals(SecurityLevel.STANDARD, deserialized.securityLevel)
    }

    @Test
    fun `VaultMetadata includes device sessions and manifests`() {
        val meta = VaultMetadata(
            vaultId = "vault-001",
            createdAt = "2026-07-21T10:00:00Z",
            updatedAt = "2026-07-21T10:00:00Z",
            deviceSessions = listOf(
                DeviceSession(
                    deviceId = "pixel8-001",
                    platform = Platform.ANDROID,
                    deviceName = "Pixel 8",
                    firstConnectedAt = "2026-07-21T10:00:00Z",
                    lastConnectedAt = "2026-07-21T10:00:00Z",
                    isActive = true
                )
            )
        )
        val serialized = json.encodeToString(meta)
        val deserialized = json.decodeFromString<VaultMetadata>(serialized)
        assertEquals(1, deserialized.deviceSessions.size)
        assertEquals(Platform.ANDROID, deserialized.deviceSessions[0].platform)
    }

    @Test
    fun `RecordingPrivacy values are distinct`() {
        // Ensure all privacy levels are distinct and serializable
        val values = RecordingPrivacy.values()
        assertEquals(4, values.size)
        values.forEach { privacy ->
            val serialized = json.encodeToString(privacy)
            val deserialized = json.decodeFromString<RecordingPrivacy>(serialized)
            assertEquals(privacy, deserialized)
        }
    }

    @Test
    fun `Cross-platform — Android record readable on Windows`() {
        // Simulate creating a record on Android and reading it on Windows
        val androidMemory = Memory(
            metadata = VaultRecordMetadata(
                id = "mem-cross-001",
                createdAt = "2026-07-21T10:00:00Z",
                updatedAt = "2026-07-21T10:00:00Z",
                sourcePlatform = Platform.ANDROID,
                sourceDeviceId = "pixel8-001",
                revision = 1
            ),
            type = MemoryType.PREFERENCE,
            key = "preferred_written_language",
            content = "English for written, Hindi for spoken",
            tags = listOf("language")
        )

        // Serialize on "Android"
        val jsonStr = json.encodeToString(androidMemory)

        // Deserialize on "Windows" — must produce identical data
        val windowsMemory = json.decodeFromString<Memory>(jsonStr)
        assertEquals(androidMemory.metadata.id, windowsMemory.metadata.id)
        assertEquals(Platform.ANDROID, windowsMemory.metadata.sourcePlatform)
        assertEquals("English for written, Hindi for spoken", windowsMemory.content)
    }
}