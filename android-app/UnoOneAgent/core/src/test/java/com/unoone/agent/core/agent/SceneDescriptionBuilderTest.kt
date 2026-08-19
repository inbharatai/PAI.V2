package com.unoone.agent.core.agent

import com.unoone.agent.core.model.StructuredNode
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * Pure-JVM tests for the scene-description builder: foreground-context framing, OCR line
 * extraction + dedup + cap, aspect focus, structured node formatting, and the honest "could not
 * read" fallback when no signal is available. The LiteRT-LM vision path that augments this is
 * device-time-only; these tests pin the always-available fallback contract.
 */
class SceneDescriptionBuilderTest {

    @Test
    fun framesForegroundAppAndActivity() {
        val desc = SceneDescriptionBuilder.build(
            SceneInput(currentPackage = "com.example.bank", currentActivity = "LoginActivity")
        )
        assertTrue(desc.startsWith("Scene: com.example.bank (LoginActivity)"))
    }

    @Test
    fun usesPackageOnlyWhenActivityAbsent() {
        val desc = SceneDescriptionBuilder.build(SceneInput(currentPackage = "com.android.chrome"))
        assertTrue(desc.startsWith("Scene: com.android.chrome"))
        assertFalse(desc.contains("("))
    }

    @Test
    fun extractsAndDedupsOcrLines() {
        val desc = SceneDescriptionBuilder.build(
            SceneInput(ocrText = "Welcome back\nUsername\nUsername\nPassword\nLogin")
        )
        assertTrue(desc.contains("Visible text: Welcome back; Username; Password; Login"))
        // duplicate "Username" collapsed to one entry
        assertEquals(1, desc.split("Username").size - 1)
    }

    @Test
    fun capsOcrLinesToMax() {
        val many = (1..20).joinToString("\n") { "line$it" }
        val desc = SceneDescriptionBuilder.build(SceneInput(ocrText = many))
        // Only MAX_OCR_LINES (8) lines appear.
        val visible = desc.substringAfter("Visible text: ")
        assertEquals(SceneDescriptionBuilder.MAX_OCR_LINES, visible.split("; ").size)
    }

    @Test
    fun includesAspectFocusWhenProvided() {
        val desc = SceneDescriptionBuilder.build(
            SceneInput(currentPackage = "com.example.shop", aspect = "the total amount")
        )
        assertTrue(desc.contains("Looking for: the total amount"))
    }

    @Test
    fun honestlyReportsUnreadableScreenWhenNoSignals() {
        val desc = SceneDescriptionBuilder.build(SceneInput())
        assertEquals("I could not read any text on the screen.", desc)
    }

    @Test
    fun unreadableScreenMentionsAspectWhenProvided() {
        val desc = SceneDescriptionBuilder.build(SceneInput(aspect = "any OTP"))
        assertEquals("I could not read any text on the screen to look for any OTP.", desc)
    }

    @Test
    fun blankOcrIsIgnoredNotReportedAsEmptyLine() {
        val desc = SceneDescriptionBuilder.build(
            SceneInput(currentPackage = "com.example.app", ocrText = "\n  \n\t")
        )
        assertTrue(desc.startsWith("Scene: com.example.app"))
        assertFalse(desc.contains("Visible text:"))
    }

    @Test
    fun neverReturnsBlank() {
        // Every combination of empty inputs still yields a non-blank, honest sentence.
        assertFalse(SceneDescriptionBuilder.build(SceneInput()).isBlank())
        assertFalse(SceneDescriptionBuilder.build(SceneInput(aspect = "x")).isBlank())
        assertFalse(SceneDescriptionBuilder.build(SceneInput(ocrText = "hi")).isBlank())
    }

    // --- Structured node tests ---

    @Test
    fun structuredNodesAreIncludedInDescription() {
        val nodes = listOf(
            StructuredNode(nodeId = "com.app:id/login_btn", text = "Login", type = "button", clickable = true, bounds = "0,400-1080,500", depth = 3),
            StructuredNode(nodeId = "com.app:id/email_field", text = "", type = "edit", clickable = true, bounds = "0,200-1080,300", depth = 2)
        )
        val desc = SceneDescriptionBuilder.build(
            SceneInput(currentPackage = "com.app", structuredNodes = nodes)
        )
        assertTrue(desc.contains("UI nodes:"))
        assertTrue(desc.contains("[com.app:id/login_btn]"))
        assertTrue(desc.contains("button \"Login\" clickable"))
        assertTrue(desc.contains("[com.app:id/email_field]"))
        assertTrue(desc.contains("edit clickable"))
    }

    @Test
    fun structuredNodesWithNoIdOmitBrackets() {
        val nodes = listOf(
            StructuredNode(nodeId = null, text = "Hello", type = "text", clickable = false, bounds = "0,0-100,50", depth = 1)
        )
        val desc = SceneDescriptionBuilder.build(
            SceneInput(structuredNodes = nodes)
        )
        assertTrue(desc.contains("text \"Hello\""))
        assertFalse(desc.contains("[]"))
    }

    @Test
    fun structuredNodesWithNoClickableDoNotShowLabel() {
        val nodes = listOf(
            StructuredNode(nodeId = null, text = "Label", type = "text", clickable = false, bounds = "0,0-100,50", depth = 0)
        )
        val desc = SceneDescriptionBuilder.build(SceneInput(structuredNodes = nodes))
        // "clickable" should NOT appear for non-clickable nodes
        assertFalse(desc.contains("clickable"))
    }

    @Test
    fun structuredNodesAloneWithoutOcrOrAppStillProducesOutput() {
        val nodes = listOf(
            StructuredNode(nodeId = "id/btn", text = "OK", type = "button", clickable = true, bounds = "0,0-100,50", depth = 1)
        )
        val desc = SceneDescriptionBuilder.build(SceneInput(structuredNodes = nodes))
        assertTrue(desc.contains("UI nodes:"))
        assertTrue(desc.contains("button \"OK\" clickable"))
    }

    @Test
    fun formatStructuredNodesCapsToMaxChars() {
        // Generate many nodes that would exceed MAX_STRUCTURED_CHARS
        val many = (1..100).map {
            StructuredNode(nodeId = "com.app:id/item_$it", text = "Item number $it with a long description", type = "button", clickable = true, bounds = "0,0-1080,100", depth = it % 5)
        }
        val formatted = SceneDescriptionBuilder.formatStructuredNodes(many)
        assertTrue(formatted.length <= SceneDescriptionBuilder.MAX_STRUCTURED_CHARS + 50) // some slack for line breaks
    }

    @Test
    fun structuredNodesWithEmptyTextDoesNotShowQuotes() {
        val nodes = listOf(
            StructuredNode(nodeId = "id/img", text = "", type = "image", clickable = false, bounds = "0,0-200,200", depth = 1)
        )
        val formatted = SceneDescriptionBuilder.formatStructuredNodes(nodes)
        assertFalse(formatted.contains("\"\""))  // no empty quotes
        assertTrue(formatted.contains("image"))  // but type still present
    }

    // --- M15: Indic script OCR merged text ---

    @Test
    fun mergedIndicOcrTextAppearsInDescription() {
        // Simulates the merged output from OcrControl.recognizeText(bitmap, "hi"):
        // both Latin and Devanagari text on the same screen, merged by line dedup.
        val mergedOcr = "Settings\nसेटिंग्स\nLogin\nलॉग इन करें"
        val desc = SceneDescriptionBuilder.build(
            SceneInput(currentPackage = "com.example.app", ocrText = mergedOcr)
        )
        assertTrue(
            "Merged Indic OCR text should appear in the description (got: $desc)",
            desc.contains("Settings")
        )
        assertTrue(
            "Merged Devanagari text should appear in the description (got: $desc)",
            desc.contains("सेटिंग्स")
        )
    }

    @Test
    fun mergedOcrDeduplicatesLinesFromBothRecognizers() {
        // When Latin and Devanagari recognizers both detect the same text, dedup should keep one copy.
        val mergedOcr = "Home\nHome\nमुख्य पृष्ठ"
        val desc = SceneDescriptionBuilder.build(SceneInput(ocrText = mergedOcr))
        // "Home" appears once (deduped), "मुख्य पृष्ठ" appears once
        assertEquals(1, desc.split("Home").size - 1)
        assertTrue(desc.contains("मुख्य पृष्ठ"))
    }
}