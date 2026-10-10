package com.unoone.agent.core.guardian

/** Mirror of Rust `privacy-guardian/src/secrets.rs`. Secret material never reaches prompts, logs,
 * sync or receipts unmasked. Bounded hand-written scanners, no regex engine differences. */
object GuardianSecrets {
    const val MAX_SCAN_CHARS = 64 * 1024
    enum class SecretKind { ONE_TIME_CODE, PASSWORD, RECOVERY_PHRASE, TOKEN, CARD_NUMBER }
    data class Span(val start: Int, val end: Int, var kind: SecretKind)

    private val CODE_WORDS = listOf("otp", "one-time", "one time", "verification code", "security code", "passcode", "pin",
        "auth code", "login code", "2fa", "two-factor", "code")
    private val ADDRESS_WORDS = listOf("flat", "apartment", "street", "road", "nagar", "sector", "colony", "layout", "house no",
        "postal", "zip", "pincode", "district", "lane", "block ", "floor")
    private val PASSWORD_WORDS = listOf("password", "passwd", "pwd", "passphrase")
    private val PHRASE_WORDS = listOf("recovery phrase", "seed phrase", "mnemonic", "secret phrase", "backup phrase", "12 words",
        "24 words", "recovery words")
    private val TOKEN_PREFIXES = listOf("ya29.", "sk-", "ghp_", "xoxb-", "xoxp-", "bearer ", "eyj")
    private val REQUEST_VERBS = listOf("send", "share", "reply with", "provide", "enter", "confirm", "tell us", "give", "forward",
        "read out", "type", "submit", "verify with", "text us", "call us with")
    private val CREDENTIAL_NOUNS = listOf("otp", "one-time code", "one time code", "verification code", "security code", "passcode",
        "password", "pin", "recovery phrase", "seed phrase", "mnemonic", "cvv", "card number", "login code", "2fa code",
        "authentication code", "the code we sent", "code you received", "code sent to", "gift card", "the codes", "scratch")
    private val URGENCY = listOf("urgent", "immediately", "within 24 hours", "within 12 hours", "within the hour", "right now",
        "act now", "final notice", "last warning", "will be suspended", "will be closed", "will be blocked", "will be deactivated",
        "account locked", "legal action", "arrest", "expires today", "today only", "do not tell", "keep this confidential",
        "keep this between us", "between us", "asap", "before midnight", "do not call", "don't call", "cannot talk", "can't talk")
    private val INJECTION = listOf("ignore previous instructions", "ignore all previous", "ignore the above", "disregard your instructions",
        "system prompt", "you are now", "as the agent", "assistant:", "disable safeguards", "disable the guardian", "disable warnings",
        "suppress warning", "turn off safety", "add to allowlist", "add to the allowlist", "whitelist", "grant yourself", "export the vault",
        "export all", "send the vault", "upload the vault", "forward all", "bcc", "change the recipient to", "change recipient", "new payee",
        "approve this", "mark as verified", "mark verified", "do not warn", "without asking", "without confirmation", "spawn",
        "run this command", "execute the following", "<|im_start|>", "[inst]", "###instruction")

    private fun lower(text: String): String = (if (text.length > MAX_SCAN_CHARS) text.substring(0, MAX_SCAN_CHARS) else text).lowercase()
    private fun wordNear(lower: String, pos: Int, words: List<String>, window: Int): Boolean {
        val slice = lower.substring(maxOf(0, pos - window), minOf(lower.length, pos + window))
        return words.any { slice.contains(it) }
    }
    private fun digitRuns(lower: String): List<Pair<Int, Int>> {
        val runs = mutableListOf<Pair<Int, Int>>(); var i = 0
        while (i < lower.length) {
            if (lower[i].isAsciiDigit()) {
                val start = i
                while (i < lower.length && (lower[i].isAsciiDigit() || lower[i] == ' ' || lower[i] == '-')) i++
                var end = i
                while (end > start && !lower[end - 1].isAsciiDigit()) end--
                runs += start to end
            } else i++
        }
        return runs
    }
    private fun Char.isAsciiDigit() = this in '0'..'9'
    private fun luhn(digits: String): Boolean {
        val d = digits.filter { it.isAsciiDigit() }.map { it - '0' }
        if (d.size < 13 || d.size > 19) return false
        var sum = 0
        d.reversed().forEachIndexed { i, v -> var x = v; if (i % 2 == 1) { x *= 2; if (x > 9) x -= 9 }; sum += x }
        return sum % 10 == 0
    }
    private fun wordRun(lower: String, from: Int, to: Int, minWords: Int): Pair<Int, Int>? {
        val slice = lower.substring(from, to)
        var count = 0; var runStart: Int? = null; var lastEnd = 0; var idx = 0
        val tokens = mutableListOf<String>(); val sb = StringBuilder()
        for (ch in slice) { sb.append(ch); if (ch.isWhitespace() || ch == ',') { tokens += sb.toString(); sb.clear() } }
        if (sb.isNotEmpty()) tokens += sb.toString()
        for (token in tokens) {
            val word = token.trimEnd { it.isWhitespace() || it == ',' }
            val ok = word.length in 3..8 && word.all { it in 'a'..'z' }
            if (ok) {
                if (runStart == null) runStart = idx
                count++; lastEnd = idx + word.length
                if (count >= 24) break
            } else {
                if (count >= minWords) break
                count = 0; runStart = null
            }
            idx += token.length
        }
        return if (count >= minWords && runStart != null) (from + runStart!!) to (from + lastEnd) else null
    }

    fun findSecrets(text: String): List<Span> {
        val lower = lower(text)
        val spans = mutableListOf<Span>()
        for ((s, e) in digitRuns(lower)) {
            val digits = lower.substring(s, e).filter { it.isAsciiDigit() }
            if (digits.length in 4..8 && wordNear(lower, s, CODE_WORDS, 48) && !wordNear(lower, s, ADDRESS_WORDS, 80)) spans += Span(s, e, SecretKind.ONE_TIME_CODE)
            else if (luhn(digits)) spans += Span(s, e, SecretKind.CARD_NUMBER)
        }
        for (w in PASSWORD_WORDS) {
            var from = 0
            while (true) {
                val i = lower.indexOf(w, from); if (i < 0) break
                var pos = i + w.length
                while (pos < lower.length && lower[pos] == ' ') pos++
                var assigned = false
                if (pos < lower.length && (lower[pos] == ':' || lower[pos] == '=')) { pos++; assigned = true }
                else if (lower.startsWith("is ", pos)) { pos += 3; assigned = true }
                if (assigned) {
                    while (pos < lower.length && (lower[pos] == ' ' || lower[pos] == '"' || lower[pos] == '\'')) pos++
                    val rest = lower.substring(pos)
                    var len = rest.indexOfFirst { it.isWhitespace() || it == '"' || it == '\'' || it == ',' }; if (len < 0) len = rest.length
                    len = rest.substring(0, len).trimEnd('.').length
                    if (len >= 4) spans += Span(pos, pos + len, SecretKind.PASSWORD)
                }
                from = i + w.length
            }
        }
        for (w in PHRASE_WORDS) {
            var from = 0
            while (true) {
                val at = lower.indexOf(w, from); if (at < 0) break
                val end = minOf(at + w.length + 260, lower.length)
                wordRun(lower, at + w.length, end, 12)?.let { spans += Span(it.first, it.second, SecretKind.RECOVERY_PHRASE) }
                from = at + w.length
            }
        }
        for (p in TOKEN_PREFIXES) {
            var from = 0
            while (true) {
                val at = lower.indexOf(p, from); if (at < 0) break
                val rest = lower.substring(at + p.length)
                var len = rest.indexOfFirst { !(it.isLetterOrDigit() && it.code < 128 || it == '.' || it == '_' || it == '-') }; if (len < 0) len = rest.length
                val boundary = at == 0 || !(lower[at - 1].isLetterOrDigit() && lower[at - 1].code < 128)
                if (len >= 20 && boundary) spans += Span(at, at + p.length + len, SecretKind.TOKEN)
                from = at + p.length
            }
        }
        spans.sortWith(compareBy({ it.start }, { it.end }))
        val merged = mutableListOf<Span>()
        for (s in spans) {
            val last = merged.lastOrNull()
            if (last != null && last.start < s.end && s.start < last.end) {
                if (last.kind != s.kind && last.kind == SecretKind.CARD_NUMBER) last.kind = s.kind
                last.let { merged[merged.size - 1] = Span(it.start, maxOf(it.end, s.end), it.kind) }
            } else merged += s
        }
        return merged
    }
    fun hasCodeShapedDigits(text: String): Boolean { val l = lower(text); return digitRuns(l).any { (s, e) -> l.substring(s, e).count { it.isAsciiDigit() } in 4..8 } }
    fun kinds(text: String): List<SecretKind> = findSecrets(text).map { it.kind }.distinct().sorted()
    fun containsSecret(text: String) = findSecrets(text).isNotEmpty()
    fun mask(text: String): String {
        val spans = findSecrets(text)
        if (spans.isEmpty()) return if (text.length > MAX_SCAN_CHARS) text.substring(0, MAX_SCAN_CHARS) else text
        val out = StringBuilder(); var cursor = 0
        for (s in spans) {
            val a = minOf(s.start, text.length); val b = minOf(s.end, text.length)
            if (a < cursor) continue
            out.append(text, cursor, a).append("[REDACTED:").append(s.kind.name).append(']')
            cursor = b
        }
        out.append(text.substring(cursor))
        return out.toString()
    }
    fun requestsCredential(text: String): Boolean {
        val l = lower(text)
        for (noun in CREDENTIAL_NOUNS) {
            var from = 0
            while (true) { val at = l.indexOf(noun, from); if (at < 0) break; if (wordNear(l, at, REQUEST_VERBS, 90)) return true; from = at + noun.length }
        }
        return false
    }
    fun urgency(text: String): Boolean { val l = lower(text); return URGENCY.any { l.contains(it) } }
    fun injectionPhrases(text: String): List<String> { val l = lower(text); return INJECTION.filter { l.contains(it) } }
}
