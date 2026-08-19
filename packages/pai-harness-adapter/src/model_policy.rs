//! Pure Pocket-AI host-to-model policy. Task routing remains inside the Harness
//! and is intentionally independent from host/model selection.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HostClass {
    AndroidConstrained,
    AndroidCapable,
    RaspberryPi,
    Desktop,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PocketModelTier {
    E2B,
    E4B,
    TwelveB,
}

/// Selects the preferred Pocket-AI model tier for a host. The caller remains
/// responsible for verifying that the package actually contains the selected
/// model and that runtime-select accepted its hash/backend.
#[must_use]
pub const fn select_model_tier(host: HostClass, usable_ram_gib: u32) -> PocketModelTier {
    match host {
        HostClass::AndroidConstrained => PocketModelTier::E2B,
        HostClass::AndroidCapable => {
            if usable_ram_gib >= 8 {
                PocketModelTier::E4B
            } else {
                PocketModelTier::E2B
            }
        }
        HostClass::RaspberryPi => {
            if usable_ram_gib >= 12 {
                PocketModelTier::E4B
            } else {
                PocketModelTier::E2B
            }
        }
        HostClass::Desktop => {
            if usable_ram_gib >= 16 {
                PocketModelTier::TwelveB
            } else if usable_ram_gib >= 8 {
                PocketModelTier::E4B
            } else {
                PocketModelTier::E2B
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_and_task_complexity_are_not_coupled() {
        assert_eq!(
            select_model_tier(HostClass::Desktop, 32),
            PocketModelTier::TwelveB
        );
    }

    #[test]
    fn pi_falls_back_safely() {
        assert_eq!(
            select_model_tier(HostClass::RaspberryPi, 4),
            PocketModelTier::E2B
        );
        assert_eq!(
            select_model_tier(HostClass::RaspberryPi, 16),
            PocketModelTier::E4B
        );
    }
}
