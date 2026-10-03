//! Conservative desktop admission against the system's TOTAL RAM.
//! Estimates are not measurements of peak RAM. Do not credit unmeasured GPU
//! memory or silently download a missing tier. The budget is what the system
//! holds — its maximum (user directive 2026-10-03: "ram budget of what the
//! system holds and should be maximum … it was working accurately before"):
//! momentary free-RAM pressure does not veto boot, because the OS reclaims
//! standby pages and the shipped flagship lane was verified live against the
//! machine's total. Invalid measurements still fail closed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Tier {
    E2B,
    E4B,
    TwelveB,
}

pub(crate) fn tier(label: &str) -> Option<Tier> {
    let label = label.to_ascii_lowercase();
    if label.contains("12b") {
        Some(Tier::TwelveB)
    } else if label.contains("e4b") || label.contains("4b") {
        Some(Tier::E4B)
    } else if label.contains("e2b") || label.contains("2b") {
        Some(Tier::E2B)
    } else {
        None
    }
}

/// True when the model's estimated footprint fits the system's TOTAL RAM
/// (weights with a 10% load margin + projector + KV estimate + a 1 GiB
/// runtime reserve). `total_gib` is the machine's maximum physical RAM —
/// current free-RAM pressure is deliberately not a veto.
pub(crate) fn fits(
    tier: Tier,
    total_gib: f64,
    weights_gib: f64,
    projector_gib: f64,
    kv_gib: f64,
) -> bool {
    let minimum = match tier {
        Tier::E2B => 4.0,
        Tier::E4B => 8.0,
        Tier::TwelveB => 16.0,
    };
    [total_gib, weights_gib, projector_gib, kv_gib]
        .iter()
        .all(|v| v.is_finite() && *v >= 0.0)
        && weights_gib > 0.0
        && total_gib.round() >= minimum
        && weights_gib * 1.1 + projector_gib + kv_gib + 1.0 <= total_gib
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn total_ram_is_the_budget_even_under_pressure() {
        // The flagship 12B lane (7.14 GiB weights + 12.75 GiB KV at the
        // granted 16K q8_0 context) fits a 24 GiB machine's TOTAL — the
        // configuration that ran live before the admission existed. There is
        // no free-RAM parameter to fail it anymore.
        assert!(fits(Tier::TwelveB, 24.0, 7.14, 0.16, 12.75));
        // A machine whose TOTAL cannot hold the estimate is still refused.
        assert!(!fits(Tier::TwelveB, 16.0, 7.14, 0.16, 12.75));
        assert!(!fits(Tier::TwelveB, 8.0, 3.0, 0.0, 0.5));
    }
    #[test]
    fn every_artifact_and_runtime_reserve_count() {
        assert!(fits(Tier::E4B, 8.0, 3.0, 0.0, 1.0));
        assert!(!fits(Tier::E4B, 6.0, 3.0, 1.0, 1.0));
        assert!(fits(Tier::E2B, 4.0, 1.5, 0.0, 0.5));
    }
    #[test]
    fn invalid_measurements_fail_closed() {
        assert!(!fits(Tier::E2B, f64::NAN, 1.0, 0.0, 0.5));
        assert!(!fits(Tier::E2B, 8.0, 0.0, 0.0, 0.5));
        assert_eq!(tier("unknown"), None);
        assert_eq!(tier("gemma-4-e4b-q4"), Some(Tier::E4B));
    }
}
