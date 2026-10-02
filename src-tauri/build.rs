fn main() {
    if std::env::var_os("CARGO_FEATURE_DESKTOP").is_some() {
        tauri_build::build();
        if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
            // Test executables need the same Common Controls v6 activation as the app's dialogs.
            println!("cargo:rustc-link-arg-tests=/MANIFEST:EMBED");
            println!("cargo:rustc-link-arg-tests=/MANIFESTDEPENDENCY:type='win32' name='Microsoft.Windows.Common-Controls' version='6.0.0.0' processorArchitecture='*' publicKeyToken='6595b64144ccf1df' language='*'");
        }
    }
}
