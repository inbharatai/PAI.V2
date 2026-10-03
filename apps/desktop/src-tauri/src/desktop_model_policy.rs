//! Conservative desktop admission. Estimates are not measurements of peak RAM.
//! Do not credit unmeasured GPU memory or silently download a missing tier.
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

pub(crate) fn fits(
    tier: Tier,
    total_gib: f64,
    available_gib: f64,
    weights_gib: f64,
    projector_gib: f64,
    kv_gib: f64,
) -> bool {
    let minimum = match tier {
        Tier::E2B => 4.0,
        Tier::E4B => 8.0,
        Tier::TwelveB => 16.0,
    };
    [total_gib, available_gib, weights_gib, projector_gib, kv_gib]
        .iter()
        .all(|v| v.is_finite() && *v >= 0.0)
        && weights_gib > 0.0
        && total_gib.round() >= minimum
        && weights_gib * 1.1 + projector_gib + kv_gib + 1.0 <= available_gib
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn total_ram_does_not_override_current_pressure() {
        assert!(!fits(Tier::TwelveB, 32.0, 4.0, 7.5, 0.9, 0.5));
        assert!(fits(Tier::E2B, 32.0, 4.0, 1.5, 0.0, 0.5));
    }
    #[test]
    fn every_artifact_and_runtime_reserve_count() {
        assert!(fits(Tier::E4B, 8.0, 6.0, 3.0, 0.0, 1.0));
        assert!(!fits(Tier::E4B, 8.0, 6.0, 3.0, 1.0, 1.0));
        assert!(!fits(Tier::TwelveB, 8.0, 20.0, 3.0, 0.0, 0.5));
    }
    #[test]
    fn invalid_measurements_fail_closed() {
        assert!(!fits(Tier::E2B, 8.0, f64::NAN, 1.0, 0.0, 0.5));
        assert!(!fits(Tier::E2B, 8.0, 6.0, 0.0, 0.0, 0.5));
        assert_eq!(tier("unknown"), None);
        assert_eq!(tier("gemma-4-e4b-q4"), Some(Tier::E4B));
    }
}
