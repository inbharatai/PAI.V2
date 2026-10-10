//! Registrable-domain and lookalike analysis from a bounded LOCAL public-suffix snapshot.
//! This is deliberately not the full Public Suffix List and never fetched from the network.
//! Unknown top-level labels are treated conservatively as a one-label suffix.

/// Bounded snapshot (version 1). Multi-label suffixes first so matching is longest-first.
pub const PUBLIC_SUFFIX_SNAPSHOT_VERSION: u32 = 1;
pub const PUBLIC_SUFFIX_SNAPSHOT: &[&str] = &[
    "co.uk", "org.uk", "ac.uk", "gov.uk", "me.uk", "ltd.uk", "co.in", "net.in", "org.in", "gov.in",
    "ac.in", "nic.in", "firm.in", "gen.in", "ind.in", "edu.in", "res.in", "co.jp", "ne.jp",
    "or.jp", "ac.jp", "go.jp", "com.au", "net.au", "org.au", "gov.au", "edu.au", "com.br",
    "net.br", "org.br", "gov.br", "co.za", "org.za", "gov.za", "com.sg", "edu.sg", "gov.sg",
    "com.cn", "net.cn", "org.cn", "gov.cn", "com.hk", "org.hk", "co.nz", "net.nz", "org.nz",
    "govt.nz", "com.mx", "org.mx", "gob.mx", "co.kr", "or.kr", "go.kr", "com.tr", "gov.tr",
    "com.ar", "gob.ar", "com.pk", "gov.pk", "com.bd", "gov.bd", "com.np", "gov.np", "com.lk",
    "gov.lk", "com.my", "gov.my", "co.id", "go.id", "com.ph", "gov.ph", "com.ng", "gov.ng",
    "co.ke", "go.ke", "com.eg", "com.sa", "gov.sa", "com.ae", "gov.ae", "co.il", "gov.il",
    "com.vn", "gov.vn", "com.ua", "gov.ua", "com.pl", "gov.pl", "com.ru", "gov.ru",
    // single-label
    "com", "org", "net", "edu", "gov", "mil", "int", "info", "biz", "name", "pro", "io", "co",
    "app", "dev", "me", "uk", "in", "de", "fr", "it", "es", "nl", "be", "ch", "at", "se", "no",
    "dk", "fi", "pl", "cz", "ru", "ua", "jp", "cn", "au", "ca", "br", "mx", "ar", "za", "ng", "ke",
    "sg", "hk", "tw", "kr", "nz", "ie", "pt", "gr", "tr", "il", "ae", "sa", "pk", "bd", "lk", "np",
    "my", "id", "ph", "vn", "eg", "xyz", "online", "site", "tech", "store", "shop", "club", "top",
    "live", "link", "cloud", "ai", "page", "us", "eu", "asia", "mobi", "tv", "cc", "ws", "to",
    "ly", "gl", "gg", "sh", "icu", "buzz", "work", "click", "zip", "mov",
];

/// Lower-cases, strips a trailing dot and rejects hosts that cannot be labels.
pub fn normalize_host(host: &str) -> Option<String> {
    let h = host.trim().trim_end_matches('.').to_lowercase();
    if h.is_empty() || h.len() > 253 {
        return None;
    }
    if h.split('.').any(|l| l.is_empty() || l.len() > 63) {
        return None;
    }
    Some(h)
}

/// Host exactly as written in the href (before any punycode conversion), for IDN lookalike checks.
pub fn raw_host(href: &str) -> Option<String> {
    let rest = href.trim().split_once("://")?.1;
    let authority = rest.split(['/', '?', '#']).next()?;
    let authority = authority.rsplit('@').next()?;
    let host = if authority.starts_with('[') {
        authority.split(']').next()?.trim_start_matches('[')
    } else {
        authority.split(':').next()?
    };
    normalize_host(host)
}

/// Bounded local list of link shorteners that hide the real destination.
pub const SHORTENERS: &[&str] = &[
    "bit.ly",
    "tinyurl.com",
    "t.co",
    "goo.gl",
    "cutt.ly",
    "rb.gy",
    "is.gd",
    "tiny.cc",
    "shorturl.at",
    "rebrand.ly",
    "ow.ly",
    "buff.ly",
    "t.ly",
    "lnkd.in",
    "bit.do",
];
pub fn is_shortener(host: &str) -> bool {
    registrable_domain(host).is_some_and(|r| SHORTENERS.contains(&r.as_str()))
}
pub fn is_private_ip(host: &str) -> bool {
    match host.parse::<std::net::IpAddr>() {
        Ok(std::net::IpAddr::V4(v4)) => v4.is_private() || v4.is_loopback() || v4.is_link_local(),
        Ok(std::net::IpAddr::V6(v6)) => {
            v6.is_loopback()
                || (v6.segments()[0] & 0xfe00) == 0xfc00
                || (v6.segments()[0] & 0xffc0) == 0xfe80
        }
        Err(_) => false,
    }
}

pub fn is_ip_literal(host: &str) -> bool {
    host.parse::<std::net::IpAddr>().is_ok()
        || host
            .trim_start_matches('[')
            .trim_end_matches(']')
            .parse::<std::net::Ipv6Addr>()
            .is_ok()
}

/// Returns the registrable domain (public suffix + one label), e.g. `mail.google.com` → `google.com`,
/// `a.b.example.co.uk` → `example.co.uk`. A bare suffix or IP literal yields `None`.
pub fn registrable_domain(host: &str) -> Option<String> {
    let host = normalize_host(host)?;
    if is_ip_literal(&host) {
        return None;
    }
    let labels: Vec<&str> = host.split('.').collect();
    if labels.len() < 2 || PUBLIC_SUFFIX_SNAPSHOT.contains(&host.as_str()) {
        return None;
    }
    // Longest matching snapshot suffix wins; unknown TLD → one label.
    let mut suffix_len = 1;
    for suffix in PUBLIC_SUFFIX_SNAPSHOT {
        let n = suffix.split('.').count();
        if n > suffix_len && labels.len() > n && host.ends_with(&format!(".{suffix}")) {
            suffix_len = n;
        }
    }
    if labels.len() <= suffix_len {
        return None;
    }
    Some(labels[labels.len() - suffix_len - 1..].join("."))
}

/// Label before the public suffix, e.g. `google` for `mail.google.com`.
pub fn brand_label(host: &str) -> Option<String> {
    registrable_domain(host).map(|d| d.split('.').next().unwrap_or("").to_string())
}

/// Punycode/IDN marker or any non-ASCII character in a host.
pub fn is_idn_or_non_ascii(host: &str) -> bool {
    !host.is_ascii() || host.split('.').any(|l| l.starts_with("xn--"))
}

/// Confusable → ASCII skeleton. Deliberately small and documented; applied to BOTH sides
/// of a comparison so legitimate digits in known names do not create false alarms.
pub fn skeleton(label: &str) -> String {
    let mut out = String::with_capacity(label.len());
    for c in label.chars() {
        let mapped = match c {
            // Cyrillic
            'а' => 'a',
            'е' => 'e',
            'о' => 'o',
            'р' => 'p',
            'с' => 'c',
            'х' => 'x',
            'у' => 'y',
            'і' => 'i',
            'ј' => 'j',
            'ѕ' => 's',
            'һ' => 'h',
            'ԁ' => 'd',
            'ӏ' => 'l',
            'ԛ' => 'q',
            'ԝ' => 'w',
            'ԍ' => 'g',
            'Ь' => 'b',
            'т' => 't',
            'к' => 'k',
            'м' => 'm',
            'н' => 'h',
            'в' => 'b',
            'г' => 'r',
            'п' => 'n',
            // Greek
            'ο' => 'o',
            'α' => 'a',
            'ν' => 'v',
            'ι' => 'i',
            'ρ' => 'p',
            'τ' => 't',
            'υ' => 'u',
            'κ' => 'k',
            'χ' => 'x',
            'ε' => 'e',
            'η' => 'n',
            'ϲ' => 'c',
            // Latin extended / symbols
            'ł' => 'l',
            'ı' => 'i',
            'ĺ' => 'l',
            'ñ' => 'n',
            'ö' => 'o',
            'ü' => 'u',
            'ä' => 'a',
            'é' => 'e',
            'è' => 'e',
            'ê' => 'e',
            'á' => 'a',
            'à' => 'a',
            'ó' => 'o',
            'ú' => 'u',
            'ç' => 'c',
            'ß' => 'b',
            // digits that read as letters
            '0' => 'o',
            '1' => 'l',
            '5' => 's',
            '3' => 'e',
            '7' => 't',
            '8' => 'b',
            // separators often used to split a brand
            '-' | '_' => continue,
            other => other.to_ascii_lowercase(),
        };
        out.push(mapped);
    }
    out.replace("rn", "m").replace("vv", "w").replace("cl", "d")
}

pub fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for i in 1..=a.len() {
        let mut cur = vec![i; b.len() + 1];
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            cur[j] = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + cost);
        }
        prev = cur;
    }
    prev[b.len()]
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Lookalike {
    /// Same registrable domain: a known destination.
    Exact,
    /// Different registrable domain whose ASCII skeleton equals a known brand label (homoglyph/digit swap).
    Homoglyph,
    /// Different registrable domain within one edit of a known brand label (length ≥ 5).
    NearMiss,
    /// Known brand label appears as a hyphen/sub-token inside an unrelated registrable domain.
    BrandEmbedded,
    /// Known brand only appears as a subdomain of an unrelated registrable domain.
    BrandSubdomain,
    /// Same brand label under a different public suffix (`bank.com` vs `bank.co`).
    SuffixSwap,
}

/// Compares `host` against known registrable domains. `None` means unrelated (not suspicious by itself).
pub fn lookalike(host: &str, known_domains: &[String]) -> Option<(Lookalike, String)> {
    let host = normalize_host(host)?;
    let reg = registrable_domain(&host)?;
    let brand = reg.split('.').next().unwrap_or("").to_string();
    let host_labels: Vec<&str> = host.split('.').collect();
    for known in known_domains {
        let Some(known_reg) = registrable_domain(known) else {
            continue;
        };
        if known_reg == reg {
            return Some((Lookalike::Exact, known_reg));
        }
    }
    for known in known_domains {
        let Some(known_reg) = registrable_domain(known) else {
            continue;
        };
        let known_brand = known_reg.split('.').next().unwrap_or("");
        if known_brand.len() < 4 {
            continue;
        }
        let sk_known = skeleton(known_brand);
        let sk_brand = skeleton(&brand);
        if brand == known_brand {
            return Some((Lookalike::SuffixSwap, known_reg));
        }
        if sk_brand == sk_known {
            return Some((Lookalike::Homoglyph, known_reg));
        }
        if known_brand.len() >= 5
            && brand != known_brand
            && edit_distance(&sk_brand, &sk_known) == 1
        {
            return Some((Lookalike::NearMiss, known_reg));
        }
        if brand != known_brand
            && brand
                .split(['-', '_'])
                .any(|t| t == known_brand || skeleton(t) == sk_known)
        {
            return Some((Lookalike::BrandEmbedded, known_reg));
        }
        if host_labels.len() > reg.split('.').count()
            && host_labels[..host_labels.len() - reg.split('.').count()]
                .iter()
                .any(|l| *l == known_brand || skeleton(l) == sk_known)
        {
            return Some((Lookalike::BrandSubdomain, known_reg));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn registrable_domains_from_snapshot() {
        assert_eq!(
            registrable_domain("mail.google.com").as_deref(),
            Some("google.com")
        );
        assert_eq!(
            registrable_domain("a.b.example.co.uk").as_deref(),
            Some("example.co.uk")
        );
        assert_eq!(
            registrable_domain("sbi.co.in").as_deref(),
            Some("sbi.co.in")
        );
        assert_eq!(
            registrable_domain("www.onlinesbi.sbi.co.in").as_deref(),
            Some("sbi.co.in")
        );
        assert_eq!(registrable_domain("com"), None);
        assert_eq!(registrable_domain("co.uk"), None);
        assert_eq!(registrable_domain("192.168.1.1"), None);
        assert_eq!(
            registrable_domain("foo.unknowntld").as_deref(),
            Some("foo.unknowntld")
        );
        assert_eq!(
            registrable_domain("Example.COM.").as_deref(),
            Some("example.com")
        );
    }
    #[test]
    fn lookalikes() {
        let known = vec!["paypal.com".to_string(), "hdfcbank.com".to_string()];
        assert_eq!(
            lookalike("www.paypal.com", &known).unwrap().0,
            Lookalike::Exact
        );
        assert_eq!(
            lookalike("paypa1.com", &known).unwrap().0,
            Lookalike::Homoglyph
        );
        assert_eq!(
            lookalike("pаypal.com", &known).unwrap().0,
            Lookalike::Homoglyph
        ); // Cyrillic а
        assert_eq!(
            lookalike("paypall.com", &known).unwrap().0,
            Lookalike::NearMiss
        );
        assert_eq!(
            lookalike("paypal-secure-login.com", &known).unwrap().0,
            Lookalike::BrandEmbedded
        );
        assert_eq!(
            lookalike("paypal.com.evil.site", &known).unwrap().0,
            Lookalike::BrandSubdomain
        );
        assert_eq!(
            lookalike("hdfcbank.co.in", &known).unwrap().0,
            Lookalike::SuffixSwap
        );
        assert!(lookalike("wikipedia.org", &known).is_none());
    }
}
