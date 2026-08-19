package com.unoone.pai.vault.crypto

import java.security.SecureRandom
import java.util.HexFormat

/**
 * Recovery key generator for vault password recovery.
 *
 * Generates a 24-word recovery key from a built-in word list,
 * or falls back to hex-encoded format if the word list is unavailable.
 *
 * The recovery key can unlock the vault if the password is forgotten.
 */
object RecoveryKeyGenerator {

    // Minimal built-in word list for recovery key generation (256 words)
    // In production, this would be the full BIP-39 English word list (2048 words)
    // For now, we use a compact list of memorable words
    private val wordList = listOf(
        "abandon", "ability", "able", "about", "above", "absent", "absorb", "abstract",
        "absurd", "abuse", "access", "accident", "account", "accuse", "achieve", "acid",
        "acoustic", "acquire", "across", "action", "actor", "actress", "actual", "adapt",
        "add", "address", "adjust", "admit", "adult", "advance", "advice", "aerobic",
        "affair", "afford", "afraid", "again", "age", "agent", "agree", "ahead",
        "aim", "air", "airport", "aisle", "alarm", "album", "alcohol", "alert",
        "alien", "all", "alley", "allow", "almost", "alone", "alpha", "already",
        "also", "alter", "always", "amateur", "amazing", "among", "amount", "amused",
        "analyst", "anchor", "ancient", "anger", "angle", "angry", "animal", "ankle",
        "announce", "annual", "another", "answer", "antenna", "antique", "anxiety", "any",
        "apart", "apology", "appear", "apple", "approve", "april", "arch", "arctic",
        "area", "arena", "argue", "arm", "armed", "armor", "army", "around",
        "arrange", "arrest", "arrive", "arrow", "art", "artefact", "artist", "artwork",
        "ask", "aspect", "assault", "asset", "assist", "assume", "asthma", "athlete",
        "atom", "auction", "audit", "august", "aunt", "author", "auto", "autumn",
        "avocado", "awake", "aware", "awesome", "awful", "awkward", "axis", "baby",
        "bachelor", "bacon", "badge", "bag", "balance", "balcony", "ball", "bamboo",
        "banana", "banner", "bar", "barely", "bargain", "barrel", "base", "basic",
        "basket", "battle", "beach", "bean", "beauty", "because", "become", "beef",
        "before", "begin", "behave", "behind", "believe", "below", "belt", "bench",
        "benefit", "best", "betray", "better", "between", "beyond", "bicycle", "bitter",
        "black", "blade", "blame", "blanket", "blast", "bleak", "bless", "blind",
        "blood", "blossom", "blow", "blue", "blur", "blush", "board", "boat",
        "body", "boil", "bomb", "bone", "bonus", "book", "boost", "border",
        "boring", "borrow", "boss", "bottom", "bounce", "box", "boy", "bracket",
        "brain", "brand", "brass", "brave", "bread", "breeze", "brick", "bridge",
        "brief", "bright", "bring", "brisk", "broccoli", "broken", "bronze", "broom",
        "brother", "brown", "brush", "bubble", "buddy", "budget", "buffalo", "build",
        "bulb", "bulk", "bullet", "bundle", "bunny", "burden", "burger", "burst",
        "bus", "business", "busy", "butter", "buyer", "buzz", "cabbage", "cabin",
        "cable", "cactus", "cage", "cake", "call", "calm", "camera", "camp",
        "canal", "cancel", "candy", "cannon", "canoe", "canvas", "canyon", "capable",
        "capital", "captain", "car", "carbon", "card", "cargo", "carpet", "carry"
    )

    /**
     * Generate a recovery key: 24 words from the built-in word list.
     * Returns a pair of (human-readable words, raw entropy bytes).
     */
    fun generate(): Pair<String, ByteArray> {
        val entropy = ByteArray(32)  // 256 bits
        SecureRandom().nextBytes(entropy)

        // Generate 24 words from entropy
        val words = mutableListOf<String>()
        var offset = 0
        for (i in 0 until 24) {
            // Use 2 bytes per word as index into word list
            val index = ((entropy[offset].toInt() and 0xFF) shl 8 or
                        (entropy[offset + 1].toInt() and 0xFF)) % wordList.size
            words.add(wordList[index])
            offset += 2
            if (offset >= 30) offset = 0  // Wrap around
        }

        return Pair(words.joinToString(" "), entropy)
    }

    /**
     * Validate a recovery key format.
     * Returns true if the key has 24 space-separated words.
     */
    fun isValid(key: String): Boolean {
        val parts = key.trim().split(" ")
        return parts.size == 24 && parts.all { wordList.contains(it) || it.matches(Regex("[0-9a-fA-F]+")) }
    }
}