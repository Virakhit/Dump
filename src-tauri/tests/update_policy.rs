use dump_core::update_policy;

#[test]
fn development_commands_deny_check_and_install_without_a_cached_client() {
    assert!(!std::hint::black_box(update_policy::ENABLED));
    assert_eq!(update_policy::check(), Err(update_policy::DISABLED.into()));
    assert_eq!(
        update_policy::install(),
        Err(update_policy::DISABLED.into())
    );
}

#[test]
#[cfg(not(debug_assertions))]
fn release_mode_preview_remains_disabled() {
    assert!(!std::hint::black_box(update_policy::ENABLED));
    assert!(update_policy::check().is_err());
    assert!(update_policy::install().is_err());
}
