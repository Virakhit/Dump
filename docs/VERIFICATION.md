# Local verification — 2026-10-02

This records checks on the development Windows machine, not approval for a public beta.

## Passed

- TypeScript and Vite production build.
- Development watcher isolation: Rust output and local audit tooling are ignored by Vite; the server remained reachable while the audit executable ran and ignored files changed.
- Desktop Rust compilation, Rustfmt, and Clippy with warnings denied.
- Six default core/integration checks: protected Windows persistence and no overwrite; scoped signatures and unsafe Windows names; authenticated three-peer QUIC, owner offline and removal; invitation replay/expiry, oversized frames and empty files; stale share refresh isolation; 384-file pagination and repeated cancellation around partial creation, including busy-part startup recovery.
- Separate release-mode 8 GiB loopback transfer. A real 8,589,934,592-byte source was written, hashed, sent over authenticated QUIC, and independently hashed after receipt. Transfer took 85.20 seconds; complete test took 182.02 seconds. Size/hash matched and no partial remained. Peak process memory was not recorded.
- npm audit: zero reported vulnerabilities, including development dependencies, at the time checked.
- Cargo audit 0.22.2 against RustSec's 2026-10-02 database: zero entries in the vulnerability list. Informational warnings remain for `paste` and `proc-macro-error` (unmaintained), and `glib` (iterator unsoundness). `cargo tree --target x86_64-pc-windows-msvc -i ...` found none of these in the Windows dependency graph; they remain in the multi-platform lockfile. Review them before adding another target; this is not a warning-free cross-platform audit.
- High/critical secret scans of authored frontend, Rust core, and Rust tests: zero findings. Generated artifacts and dependency integrity hashes are outside this source scan.
- Peer static reviews of security and delivery changes; identified findings were corrected and the affected integration checks rerun.
- Bundled Windows app launched; its native accessibility tree displayed the real desktop interface, generated device identity, and running LAN listener. Browser preview layout inspected. Native screenshot capture/input geometry was unavailable through the automation bridge, so native file-picker, drag/drop, modal and DPI interactions remain unverified.
- NSIS silent installation on the development Windows machine returned exit code 0. The installed executable, Start Menu shortcut, uninstaller registration, and `dump://` protocol command were present. The installed application launched from `%LOCALAPPDATA%/Dump/dump.exe` and displayed its bundled interface and running LAN listener while no development server was listening on ports 1420 or 4173. Its import table contains Windows system DLLs; no Node.js or Rust installation is required by the packaged executable.

## Artifact

`src-tauri/target/release/bundle/nsis/Dump_0.1.0_x64-setup.exe` is a Windows x64, current-user NSIS installer with the WebView2 offline installer bundled. The application/installer are unsigned. Bundling passed; installation on a clean machine has not been tested.

Final installer: 219,279,799 bytes. SHA-256: `c35eb85ecf6df7e892cc95f83f6cf0d8a6b20fbaac2c983b2ba93fbfbc83b258`. An adjacent `.sha256` file contains the same checksum.

## Still required

Physical two/three-machine Windows LAN checks, discovery/firewall scenarios, removal/reconnect during an active transfer, target-machine memory observations, native keyboard/DPI/drop checks, interactive wizard navigation, clean installation/uninstallation and deep-link launch, packet capture, and the five-user pilot are listed in `PLAN.md`. The source repository has been pushed to GitHub. The first remote Windows CI run found a test sequencing race: it requested a stream before the asynchronous dial was registered. The test now waits for authenticated catalog admission before continuing; remote rerun status is reported separately. No prospective users have been contacted, and the installer remains an alpha preview rather than a validated beta.
