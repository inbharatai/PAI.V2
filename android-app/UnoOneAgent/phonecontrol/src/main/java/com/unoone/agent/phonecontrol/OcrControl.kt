package com.unoone.agent.phonecontrol

import android.content.Context
import android.graphics.Bitmap
import com.google.mlkit.vision.common.InputImage
import com.google.mlkit.vision.text.TextRecognition
import com.google.mlkit.vision.text.TextRecognizer
import com.google.mlkit.vision.text.devanagari.DevanagariTextRecognizerOptions
import com.google.mlkit.vision.text.latin.TextRecognizerOptions
import com.unoone.agent.core.model.Result
import com.unoone.agent.core.util.Logger
import kotlin.coroutines.resume
import kotlin.coroutines.suspendCoroutine

/**
 * Language codes that use Indic scripts and should run the Devanagari recognizer
 * alongside Latin OCR. Devanagari covers Hindi, Marathi, Nepali, and other
 * Devanagari-script languages. For other Indic scripts (Bengali, Tamil, Telugu,
 * Kannada, Malayalam), the Devanagari recognizer has some cross-script coverage
 * and is still preferable to Latin-only OCR.
 */
val INDIC_LANGUAGE_CODES: Set<String> = setOf("hi", "mr", "bn", "ta", "te", "kn", "ml")

class OcrControl(private val context: Context) {

    // Lazily initialized so ML Kit is only spun up if OCR is actually used — avoids the cost (and
    // the MlKitContext requirement) on devices/paths that never run OCR, and lets this class be
    // constructed in unit tests.
    private val latinRecognizer by lazy { TextRecognition.getClient(TextRecognizerOptions.DEFAULT_OPTIONS) }

    /**
     * Devanagari script recognizer for Indic languages (Hindi, Marathi, Nepali, etc.).
     * Lazily initialized because most users run in English and never need this recognizer.
     */
    private val devanagariRecognizer by lazy { TextRecognition.getClient(DevanagariTextRecognizerOptions.Builder().build()) }

    private val screenshotCapture = ScreenshotCapture(context)

    /**
     * Runs OCR on the given bitmap using only the Latin recognizer.
     * Retained for backward compatibility with callers that don't specify a language
     * (e.g. DocumentLoader, AgentViewModel quick-OCR paths).
     */
    suspend fun recognizeText(bitmap: Bitmap): Result<String> = suspendCoroutine { continuation ->
        val image = InputImage.fromBitmap(bitmap, 0)
        latinRecognizer.process(image)
            .addOnSuccessListener { visionText ->
                Logger.d("OCR (Latin) Success: ${visionText.text.take(20)}...")
                continuation.resume(Result.Success(visionText.text))
            }
            .addOnFailureListener { e ->
                Logger.e("OCR (Latin) Failed", e)
                continuation.resume(Result.Error("Failed to read text from screen: ${e.message}"))
            }
    }

    /**
     * Language-aware OCR: runs the appropriate recognizer(s) based on [languageCode].
     *
     * - English and other Latin-script languages: runs only the Latin recognizer (fastest path).
     * - Indic languages (hi, bn, ta, te, kn, ml): runs BOTH Latin and Devanagari recognizers
     *   and merges unique text lines, so English labels and Hindi/Devanagari text on the same
     *   screen are both captured. The Devanagari recognizer also provides some cross-script
     *   coverage for Bengali, Tamil, Telugu, Kannada, and Malayalam scripts.
     */
    suspend fun recognizeText(bitmap: Bitmap, languageCode: String): Result<String> {
        if (languageCode !in INDIC_LANGUAGE_CODES) {
            return recognizeText(bitmap)
        }

        // Indic language: run both recognizers and merge their text.
        val image = InputImage.fromBitmap(bitmap, 0)
        val latinResult = recognizeWithRecognizer(image, latinRecognizer, "Latin")
        val devanagariResult = recognizeWithRecognizer(image, devanagariRecognizer, "Devanagari")

        // Merge: collect unique non-blank lines from both recognizers, Latin first.
        val mergedLines = LinkedHashSet<String>()
        addNonBlankLines(latinResult, mergedLines)
        addNonBlankLines(devanagariResult, mergedLines)

        val merged = mergedLines.joinToString("\n")
        return if (merged.isNotBlank()) {
            Result.Success(merged)
        } else {
            // Both recognizers returned nothing — surface a clear message.
            Result.Error("No readable text on screen (tried both Latin and Indic recognizers)")
        }
    }

    /**
     * Captures the current screen via MediaProjection and runs OCR on it.
     * If projection permission has not been granted yet, this returns an error so the UI layer
     * can launch [com.unoone.agent.screenshot.ScreenshotPermissionActivity].
     */
    suspend fun recognizeScreen(): Result<String> {
        if (!ScreenshotCapture.hasPermission()) {
            return Result.Error("Screenshot OCR requires MediaProjection permission")
        }
        return when (val bitmapResult = screenshotCapture.captureScreen()) {
            is Result.Success -> recognizeText(bitmapResult.data)
            is Result.Error -> Result.Error(bitmapResult.message)
        }
    }

    /**
     * Language-aware screen OCR: captures the current screen and runs OCR with the
     * appropriate recognizer(s) based on [languageCode]. For Indic languages, both
     * Latin and Devanagari recognizers are run and their results merged so mixed-script
     * screens (English labels + Hindi/Devanagari content) are fully captured.
     */
    suspend fun recognizeScreen(languageCode: String): Result<String> {
        if (!ScreenshotCapture.hasPermission()) {
            return Result.Error("Screenshot OCR requires MediaProjection permission")
        }
        return when (val bitmapResult = screenshotCapture.captureScreen()) {
            is Result.Success -> recognizeText(bitmapResult.data, languageCode)
            is Result.Error -> Result.Error(bitmapResult.message)
        }
    }

    /**
     * Release the ML Kit text recognizers to prevent memory leaks.
     * Call this when the OcrControl is no longer needed.
     */
    fun release() {
        try {
            latinRecognizer.close()
            Logger.i("OcrControl: Latin recognizer released")
        } catch (e: Exception) {
            Logger.e("OcrControl: Error releasing Latin recognizer", e)
        }
        try {
            devanagariRecognizer.close()
            Logger.i("OcrControl: Devanagari recognizer released")
        } catch (e: Exception) {
            // Lazy delegate was never accessed — nothing to close.
            Logger.d("OcrControl: Devanagari recognizer not initialized, skip release")
        }
    }

    /**
     * Runs a single recognizer on the given image and returns the raw text on success,
     * or null on failure. Non-blocking: each recognizer runs independently.
     */
    private suspend fun recognizeWithRecognizer(
        image: InputImage,
        recognizer: TextRecognizer,
        label: String
    ): String? = suspendCoroutine { continuation ->
        recognizer.process(image)
            .addOnSuccessListener { visionText ->
                Logger.d("OCR ($label) Success: ${visionText.text.take(20)}...")
                continuation.resume(visionText.text)
            }
            .addOnFailureListener { e ->
                Logger.w("OCR ($label) Failed: ${e.message}")
                continuation.resume(null)
            }
    }

    /**
     * Splits raw OCR text into lines and adds non-blank lines to [target],
     * preserving insertion order and skipping duplicates.
     */
    private fun addNonBlankLines(rawText: String?, target: LinkedHashSet<String>) {
        if (rawText.isNullOrBlank()) return
        for (line in rawText.split("\n")) {
            val cleaned = line.trim()
            if (cleaned.isNotBlank()) target.add(cleaned)
        }
    }
}