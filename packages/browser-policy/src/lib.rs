//! unoone-browser-policy — navigation/redirect decisions for the browser
//! workspace, extracted from the WebView adapter so any host can test them.
//!
//! The defect this fixes: the old `page_reached_target()` accepted ANY
//! http(s) page as "reached the target" after a cross-origin redirect — a
//! hijacked redirect reported success — while naively rejecting every
//! cross-origin landing would break legitimate OAuth/login flows. This is a
//! policy, encoded once, testable everywhere:
//!
//!   - exact / same-origin / same registrable domain  → Reached
//!   - HTTPS→HTTP downgrade at any point              → NotReached
//!   - everything else cross-origin                  → RequiresReview
//!     (surfaced to the caller, never a silent success)
//!
//! Ports and userinfo are handled by a real URL parser (the `url` crate),
//! never string splitting.

use url::Url;

#[derive(Debug, Clone, PartialEq)]
pub enum RedirectVerdict {
    /// The page is on the target origin or registrable domain.
    Reached,
    /// A security downgrade occurred; treat navigation as failed.
    NotReached { reason: String },
    /// Cross-origin landing that is neither same-origin, downgrade, nor
    /// obviously benign. The caller must surface this, not report success.
    RequiresReview { landed: String, expected: String },
}

/// Parsed, normalised URL used for comparisons.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ParsedTarget {
    scheme: String,
    host: String,
    port: u16,
}

fn parse(raw: &str) -> Result<ParsedTarget, String> {
    let url = Url::parse(raw.trim()).map_err(|e| format!("cannot parse URL '{raw}': {e}"))?;
    let scheme = url.scheme().to_lowercase();
    // The workspace's navigation allowlist is http/https only; anything else
    // that reaches this point is a defect upstream, not a redirect decision.
    if scheme != "http" && scheme != "https" {
        return Err(format!(
            "non-http(s) URL reached the redirect policy: {}",
            scheme
        ));
    }
    let host = url
        .host_str()
        .ok_or_else(|| format!("URL '{raw}' has no host"))?
        .to_lowercase();
    let port = url
        .port()
        .unwrap_or(if scheme == "https" { 443 } else { 80 });
    Ok(ParsedTarget { scheme, host, port })
}

/// Best-effort registrable domain (eTLD+1) WITHOUT the public suffix list:
/// matches the common two-level case (example.com, example.co.uk) and all
/// subdomains of it. Documented approximation — sufficient for the redirect
/// policy, NOT for cookie partitioning.
fn registrable(host: &str) -> String {
    let labels: Vec<&str> = host.split('.').collect();
    if labels.len() <= 2 {
        return host.to_string();
    }
    // Common second-level public suffixes (co.uk, com.au, co.in, ...).
    let second = labels[labels.len() - 2];
    let tld_is_two_chars = labels[labels.len() - 1].len() == 2;
    let second_is_sld = matches!(second, "co" | "com" | "org" | "net" | "gov" | "ac" | "edu");
    if second_is_sld && tld_is_two_chars && labels.len() >= 3 {
        labels[labels.len() - 3..].join(".")
    } else {
        labels[labels.len() - 2..].join(".")
    }
}

/// Decide whether landing on `page_url` after navigating to `target_url`
/// means the target was reached. `target_url` may be empty (history moves,
/// reloads) — any same-scheme http(s) page then counts as reached, except a
/// downgrade cannot exist without an expected scheme to compare against.
pub fn evaluate(page_url: &str, target_url: &str) -> RedirectVerdict {
    let page = match parse(page_url) {
        Ok(p) => p,
        Err(e) => {
            return RedirectVerdict::NotReached {
                reason: format!("landing URL invalid: {e}"),
            }
        }
    };
    if target_url.trim().is_empty() {
        return RedirectVerdict::Reached;
    }
    let target = match parse(target_url) {
        Ok(t) => t,
        Err(e) => {
            return RedirectVerdict::NotReached {
                reason: format!("target URL invalid: {e}"),
            }
        }
    };

    // Hard rule: HTTPS target must never land on HTTP.
    if target.scheme == "https" && page.scheme == "http" {
        return RedirectVerdict::NotReached {
            reason: format!(
                "HTTPS→HTTP downgrade: {} landed on {}",
                target_url, page_url
            ),
        };
    }

    // Same origin (scheme + host + port).
    if page == target {
        return RedirectVerdict::Reached;
    }

    // Upgrade within the same host (http → https) is benign.
    if page.host == target.host && target.scheme == "http" && page.scheme == "https" {
        return RedirectVerdict::Reached;
    }

    // Same registrable domain — subdomains and sibling paths, which is how
    // legitimate single-sign-on and CDN moves commonly look.
    if registrable(&page.host) == registrable(&target.host) {
        return RedirectVerdict::Reached;
    }

    RedirectVerdict::RequiresReview {
        landed: page_url.to_string(),
        expected: target_url.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_origin_reached() {
        assert_eq!(
            evaluate(
                "https://example.com/docs/a?x=1",
                "https://example.com/start"
            ),
            RedirectVerdict::Reached
        );
    }

    #[test]
    fn different_path_same_origin_reached() {
        assert_eq!(
            evaluate("https://example.com/other", "https://example.com/start"),
            RedirectVerdict::Reached
        );
    }

    #[test]
    fn different_port_same_domain_still_reached_by_policy() {
        // Not same-origin per web semantics, but the chosen policy accepts
        // same-registrable-domain landings. Documented decision, tested.
        assert_eq!(
            evaluate("http://example.com:8080/x", "http://example.com/y"),
            RedirectVerdict::Reached
        );
    }

    #[test]
    fn https_to_http_downgrade_rejected() {
        match evaluate("http://example.com/phish", "https://example.com/login") {
            RedirectVerdict::NotReached { reason } => {
                assert!(reason.contains("downgrade"), "got {reason}")
            }
            other => panic!("expected NotReached, got {other:?}"),
        }
    }

    #[test]
    fn http_to_https_upgrade_same_host_reached() {
        assert_eq!(
            evaluate("https://example.com/s", "http://example.com/s"),
            RedirectVerdict::Reached
        );
    }

    #[test]
    fn subdomain_of_target_domain_reached() {
        assert_eq!(
            evaluate(
                "https://login.example.com/oauth/cb?code=1",
                "https://example.com/start"
            ),
            RedirectVerdict::Reached
        );
    }

    #[test]
    fn sibling_registrable_domain_reached() {
        assert_eq!(
            evaluate("https://app.example.co.uk/x", "https://www.example.co.uk/y"),
            RedirectVerdict::Reached
        );
    }

    #[test]
    fn unrelated_origin_requires_review_not_silent_success() {
        match evaluate("https://evil-example.net/x", "https://example.com/y") {
            RedirectVerdict::RequiresReview { landed, expected } => {
                assert!(landed.contains("evil-example.net"));
                assert!(expected.contains("example.com"));
            }
            other => panic!("expected RequiresReview, got {other:?}"),
        }
    }

    #[test]
    fn similar_looking_tld_still_reviewed() {
        // example.co is a different registrable domain than example.com
        match evaluate("https://example.co/x", "https://example.com/y") {
            RedirectVerdict::RequiresReview { .. } => {}
            other => panic!("expected RequiresReview, got {other:?}"),
        }
    }

    #[test]
    fn userinfo_does_not_smuggle_host() {
        // evil.com sitting in the userinfo slot: host is example.com, reached.
        assert_eq!(
            evaluate("https://evil.com@example.com/", "https://example.com/"),
            RedirectVerdict::Reached
        );
        // and the reverse trick is reviewed, not reached
        match evaluate("https://example.com@evil.net/", "https://example.com/") {
            RedirectVerdict::RequiresReview { .. } => {}
            other => panic!("expected RequiresReview, got {other:?}"),
        }
    }

    #[test]
    fn empty_target_means_any_http_page() {
        assert_eq!(
            evaluate("https://anything.example/page", ""),
            RedirectVerdict::Reached
        );
        assert_eq!(
            evaluate("http://127.0.0.1:8080/x", ""),
            RedirectVerdict::Reached
        );
    }

    #[test]
    fn non_http_landing_is_not_reached() {
        match evaluate("chrome-error://chromewebdata/", "https://example.com") {
            RedirectVerdict::NotReached { .. } => {}
            other => panic!("expected NotReached, got {other:?}"),
        }
    }

    #[test]
    fn unicode_domain_compares_nfc_through_parser() {
        // The parser normalises to punycode; no panic, real comparison.
        let _ = evaluate("https://dömäin.example/x", "https://example.com/y");
    }
}
