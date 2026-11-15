//! This rebuild has no approved update channel. Fail closed in every build profile.
//! No feature, environment variable or runtime setting enables the legacy feed.
pub const ENABLED: bool = false;
pub const DISABLED: &str = "Updates are disabled in this development/preview build. Install only a separately approved rebuild manually.";

pub fn check() -> Result<Option<String>, String> {
    Err(DISABLED.into())
}

pub fn install() -> Result<(), String> {
    Err(DISABLED.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_and_install_are_disabled_independently_of_cached_update_state() {
        assert_eq!(check(), Err(DISABLED.into()));
        assert_eq!(install(), Err(DISABLED.into()));
    }

    #[test]
    fn build_policy_cannot_be_enabled_by_runtime_configuration() {
        assert!(!std::hint::black_box(ENABLED));
    }

    #[test]
    #[cfg(not(debug_assertions))]
    fn optimized_release_policy_also_denies_both_operations() {
        assert!(!std::hint::black_box(ENABLED));
        assert!(check().is_err());
        assert!(install().is_err());
    }
}
