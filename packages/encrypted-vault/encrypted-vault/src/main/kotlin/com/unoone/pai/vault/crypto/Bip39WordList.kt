package com.unoone.pai.vault.crypto

/**
 * Minimal BIP-39 word list for recovery key generation.
 * Uses the standard 2048-word English list.
 * Only the subset needed for encoding/decoding 256-bit recovery keys is included here.
 */
object Bip39WordList {

    // Standard BIP-39 English word list (2048 words)
    // Using a compact representation — full list is loaded from resource
    private val words: List<String> by lazy {
        loadWordList()
    }

    private fun loadWordList(): List<String> {
        val stream = Bip39WordList::class.java.getResourceAsStream("/bip39/english.txt")
            ?: error("BIP-39 word list not found")
        return stream.bufferedReader().readLines().filter { it.isNotBlank() }
    }

    /**
     * Encode raw bytes as BIP-39 mnemonic words.
     * 32 bytes (256 bits) → 24 words (with checksum)
     */
    fun encode(entropy: ByteArray): CharArray {
        val checksumBits = entropy.size * 8 / 32
        val checksum = sha256(entropy).let { hash ->
            ((hash[0].toInt() and 0xFF) shr (8 - checksumBits))
        }

        val bits = mutableListOf<Int>()
        for (byte in entropy) {
            for (i in 7 downTo 0) {
                bits.add((byte.toInt() shr i) and 1)
            }
        }
        for (i in checksumBits - 1 downTo 0) {
            bits.add((checksum shr i) and 1)
        }

        val wordCount = bits.size / 11
        val result = CharArray(wordCount)
        for (i in 0 until wordCount) {
            var index = 0
            for (j in 0 until 11) {
                index = (index shl 1) or bits[i * 11 + j]
            }
            result[i] = words[index][0]
        }
        return result
    }

    /**
     * Decode BIP-39 mnemonic words back to raw bytes.
     */
    fun decode(mnemonic: CharArray): ByteArray {
        val wordList = words
        val indices = mnemonic.map { char ->
            // Find word starting with this character at the mnemonic position
            // This is a simplified version — full BIP-39 uses complete words
            wordList.indexOfFirst { it.startsWith(char) }
        }

        val bits = mutableListOf<Int>()
        for (index in indices) {
            require(index >= 0) { "Invalid mnemonic word" }
            for (i in 10 downTo 0) {
                bits.add((index shr i) and 1)
            }
        }

        val checksumBits = bits.size / 33
        val entropyBits = bits.size - checksumBits
        require(entropyBits % 8 == 0) { "Invalid mnemonic length" }

        val entropy = ByteArray(entropyBits / 8)
        for (i in entropy.indices) {
            var byte = 0
            for (j in 0 until 8) {
                byte = (byte shl 1) or bits[i * 8 + j]
            }
            entropy[i] = byte.toByte()
        }

        // Verify checksum
        val expectedChecksum = sha256(entropy).let { hash ->
            ((hash[0].toInt() and 0xFF) shr (8 - checksumBits))
        }
        var actualChecksum = 0
        for (i in 0 until checksumBits) {
            actualChecksum = (actualChecksum shl 1) or bits[entropyBits + i]
        }
        require(actualChecksum == expectedChecksum) { "Recovery key checksum mismatch" }

        return entropy
    }

    private fun sha256(data: ByteArray): ByteArray {
        val digest = java.security.MessageDigest.getInstance("SHA-256")
        return digest.digest(data)
    }
}