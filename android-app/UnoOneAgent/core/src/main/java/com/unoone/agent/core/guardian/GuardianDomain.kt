package com.unoone.agent.core.guardian

/** Mirror of Rust `privacy-guardian/src/domain.rs`: bounded LOCAL public-suffix snapshot (never fetched),
 * registrable-domain extraction, confusable skeletons and lookalike classification. */
object GuardianDomain {
    const val PUBLIC_SUFFIX_SNAPSHOT_VERSION = 1
    val PUBLIC_SUFFIX_SNAPSHOT: List<String> = listOf(
        "co.uk", "org.uk", "ac.uk", "gov.uk", "me.uk", "ltd.uk", "co.in", "net.in", "org.in", "gov.in",
        "ac.in", "nic.in", "firm.in", "gen.in", "ind.in", "edu.in", "res.in", "co.jp", "ne.jp", "or.jp",
        "ac.jp", "go.jp", "com.au", "net.au", "org.au", "gov.au", "edu.au", "com.br", "net.br",
        "org.br", "gov.br", "co.za", "org.za", "gov.za", "com.sg", "edu.sg", "gov.sg", "com.cn",
        "net.cn", "org.cn", "gov.cn", "com.hk", "org.hk", "co.nz", "net.nz", "org.nz", "govt.nz",
        "com.mx", "org.mx", "gob.mx", "co.kr", "or.kr", "go.kr", "com.tr", "gov.tr", "com.ar",
        "gob.ar", "com.pk", "gov.pk", "com.bd", "gov.bd", "com.np", "gov.np", "com.lk", "gov.lk",
        "com.my", "gov.my", "co.id", "go.id", "com.ph", "gov.ph", "com.ng", "gov.ng", "co.ke",
        "go.ke", "com.eg", "com.sa", "gov.sa", "com.ae", "gov.ae", "co.il", "gov.il", "com.vn",
        "gov.vn", "com.ua", "gov.ua", "com.pl", "gov.pl", "com.ru", "gov.ru",
        "com", "org", "net", "edu", "gov", "mil", "int", "info", "biz", "name", "pro", "io", "co",
        "app", "dev", "me", "uk", "in", "de", "fr", "it", "es", "nl", "be", "ch", "at", "se", "no",
        "dk", "fi", "pl", "cz", "ru", "ua", "jp", "cn", "au", "ca", "br", "mx", "ar", "za", "ng",
        "ke", "sg", "hk", "tw", "kr", "nz", "ie", "pt", "gr", "tr", "il", "ae", "sa", "pk", "bd",
        "lk", "np", "my", "id", "ph", "vn", "eg", "xyz", "online", "site", "tech", "store", "shop",
        "club", "top", "live", "link", "cloud", "ai", "page", "us", "eu", "asia", "mobi", "tv",
        "cc", "ws", "to", "ly", "gl", "gg", "sh", "icu", "buzz", "work", "click", "zip", "mov",
    )
    val SHORTENERS = listOf("bit.ly", "tinyurl.com", "t.co", "goo.gl", "cutt.ly", "rb.gy", "is.gd", "tiny.cc",
        "shorturl.at", "rebrand.ly", "ow.ly", "buff.ly", "t.ly", "lnkd.in", "bit.do")

    fun normalizeHost(host: String): String? {
        val h = host.trim().trimEnd('.').lowercase()
        if (h.isEmpty() || h.toByteArray().size > 253) return null
        if (h.split('.').any { it.isEmpty() || it.toByteArray().size > 63 }) return null
        return h
    }

    private val ipv4 = Regex("^(25[0-5]|2[0-4]\\d|1\\d\\d|[1-9]?\\d)(\\.(25[0-5]|2[0-4]\\d|1\\d\\d|[1-9]?\\d)){3}$")
    private fun isIpv6(s: String): Boolean {
        val t = s.removePrefix("[").removeSuffix("]")
        if (!t.contains(':') || t.count { it == ':' } > 7) return false
        if (t.any { !(it.isDigit() || it in 'a'..'f' || it in 'A'..'F' || it == ':' || it == '.') }) return false
        return t.split("::").size <= 2
    }
    fun isIpLiteral(host: String): Boolean = ipv4.matches(host) || isIpv6(host)
    fun isPrivateIp(host: String): Boolean {
        if (ipv4.matches(host)) {
            val p = host.split('.').map { it.toInt() }
            return p[0] == 10 || (p[0] == 172 && p[1] in 16..31) || (p[0] == 192 && p[1] == 168) || p[0] == 127 || (p[0] == 169 && p[1] == 254)
        }
        if (isIpv6(host)) {
            val t = host.removePrefix("[").removeSuffix("]").lowercase()
            return t == "::1" || t.startsWith("fc") || t.startsWith("fd") || t.startsWith("fe8") || t.startsWith("fe9") || t.startsWith("fea") || t.startsWith("feb")
        }
        return false
    }

    fun registrableDomain(host: String): String? {
        val h = normalizeHost(host) ?: return null
        if (isIpLiteral(h)) return null
        val labels = h.split('.')
        if (labels.size < 2 || h in PUBLIC_SUFFIX_SNAPSHOT) return null
        var suffixLen = 1
        for (suffix in PUBLIC_SUFFIX_SNAPSHOT) {
            val n = suffix.split('.').size
            if (n > suffixLen && labels.size > n && h.endsWith(".$suffix")) suffixLen = n
        }
        if (labels.size <= suffixLen) return null
        return labels.subList(labels.size - suffixLen - 1, labels.size).joinToString(".")
    }
    fun brandLabel(host: String): String? = registrableDomain(host)?.substringBefore('.')
    fun isIdnOrNonAscii(host: String): Boolean = host.any { it.code > 127 } || host.split('.').any { it.startsWith("xn--") }
    fun isShortener(host: String): Boolean = registrableDomain(host)?.let { it in SHORTENERS } == true

    /** Host exactly as written in the href (before any punycode conversion). */
    fun rawHost(href: String): String? {
        val rest = href.trim().substringAfter("://", "").ifEmpty { return null }
        val authority = rest.split('/', '?', '#').first()
        val afterUser = authority.substringAfterLast('@')
        val host = if (afterUser.startsWith("[")) afterUser.substringBefore(']').removePrefix("[") else afterUser.substringBefore(':')
        return normalizeHost(host)
    }

    private val confusables = mapOf(
        'а' to 'a', 'е' to 'e', 'о' to 'o', 'р' to 'p', 'с' to 'c', 'х' to 'x', 'у' to 'y', 'і' to 'i', 'ј' to 'j',
        'ѕ' to 's', 'һ' to 'h', 'ԁ' to 'd', 'ӏ' to 'l', 'ԛ' to 'q', 'ԝ' to 'w', 'ԍ' to 'g', 'Ь' to 'b', 'т' to 't',
        'к' to 'k', 'м' to 'm', 'н' to 'h', 'в' to 'b', 'г' to 'r', 'п' to 'n',
        'ο' to 'o', 'α' to 'a', 'ν' to 'v', 'ι' to 'i', 'ρ' to 'p', 'τ' to 't', 'υ' to 'u', 'κ' to 'k', 'χ' to 'x',
        'ε' to 'e', 'η' to 'n', 'ϲ' to 'c',
        'ł' to 'l', 'ı' to 'i', 'ĺ' to 'l', 'ñ' to 'n', 'ö' to 'o', 'ü' to 'u', 'ä' to 'a', 'é' to 'e', 'è' to 'e',
        'ê' to 'e', 'á' to 'a', 'à' to 'a', 'ó' to 'o', 'ú' to 'u', 'ç' to 'c', 'ß' to 'b',
        '0' to 'o', '1' to 'l', '5' to 's', '3' to 'e', '7' to 't', '8' to 'b',
    )
    fun skeleton(label: String): String {
        val sb = StringBuilder()
        for (c in label) {
            if (c == '-' || c == '_') continue
            sb.append(confusables[c] ?: c.lowercaseChar())
        }
        return sb.toString().replace("rn", "m").replace("vv", "w").replace("cl", "d")
    }
    fun editDistance(a: String, b: String): Int {
        var prev = IntArray(b.length + 1) { it }
        for (i in 1..a.length) {
            val cur = IntArray(b.length + 1) { i }
            for (j in 1..b.length) {
                val cost = if (a[i - 1] != b[j - 1]) 1 else 0
                cur[j] = minOf(prev[j] + 1, cur[j - 1] + 1, prev[j - 1] + cost)
            }
            prev = cur
        }
        return prev[b.length]
    }

    enum class Lookalike { EXACT, HOMOGLYPH, NEAR_MISS, BRAND_EMBEDDED, BRAND_SUBDOMAIN, SUFFIX_SWAP }

    fun lookalike(hostIn: String, knownDomains: List<String>): Pair<Lookalike, String>? {
        val host = normalizeHost(hostIn) ?: return null
        val reg = registrableDomain(host) ?: return null
        val brand = reg.substringBefore('.')
        val hostLabels = host.split('.')
        for (known in knownDomains) { val k = registrableDomain(known) ?: continue; if (k == reg) return Lookalike.EXACT to k }
        for (known in knownDomains) {
            val knownReg = registrableDomain(known) ?: continue
            val knownBrand = knownReg.substringBefore('.')
            if (knownBrand.length < 4) continue
            val skKnown = skeleton(knownBrand); val skBrand = skeleton(brand)
            if (brand == knownBrand) return Lookalike.SUFFIX_SWAP to knownReg
            if (skBrand == skKnown) return Lookalike.HOMOGLYPH to knownReg
            if (knownBrand.length >= 5 && editDistance(skBrand, skKnown) == 1) return Lookalike.NEAR_MISS to knownReg
            if (brand.split('-', '_').any { it == knownBrand || skeleton(it) == skKnown }) return Lookalike.BRAND_EMBEDDED to knownReg
            val regCount = reg.split('.').size
            if (hostLabels.size > regCount && hostLabels.subList(0, hostLabels.size - regCount).any { it == knownBrand || skeleton(it) == skKnown }) return Lookalike.BRAND_SUBDOMAIN to knownReg
        }
        return null
    }
}
