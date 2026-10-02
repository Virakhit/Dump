# Local verification — 2026-10-02

This records checks on the development Windows machine, not approval for a public beta.

## v0.2 Stage 1 — Identify and explicit direct dialing

- `cargo test --manifest-path src-tauri/Cargo.toml --no-default-features --locked`: 11 passed (four unit, three connectivity integration, four existing LAN integration); the existing 8 GiB test remains intentionally ignored and was not repeated for this stage.
- New integration checks passed: explicit literal-IP QUIC dial using a loopback simulation; Identify exchange with no interface address advertisement or loopback public candidate; real mDNS discovery of an invited owner without explicit addresses, followed by approval; matching-address suffix validation and wrong expected identity handshake rejection; catalog and file-header denial to an authenticated nonmember; clean offline diagnostics after the remote shuts down.
- New unit checks passed: direct-address shape/port/IP/identity checks, deduplication and 256-peer/eight-address bounds; conservative public/private/CGNAT/reserved IPv4 and IPv6 candidate filtering.
- `cargo test --manifest-path src-tauri/Cargo.toml --lib --locked`: five passed, including the updater active-transfer/preparation guard.
- `npm.cmd run build`, Rustfmt check, `git diff --check`, and configured Clippy (`--all-targets --locked -- -D warnings`) passed. Existing LAN transfer, owner-offline, eventual revocation, persistence, pagination and cancellation tests passed with Identify enabled.
- No public service, real Internet route, NAT gateway or relay was used. This establishes core direct-dial and local discovery behavior, not cross-network end-user availability. AutoNAT, relays, DCUtR, new invitation contacts and opt-in network assistance remain pending. No new installer was built or published; the existing signed updater/0.1.1 installer is unchanged. This stage changes no persisted identity/workspace or signed membership/manifest format.

## v0.2 Stage 2 — AutoNAT v2 and advertisement bounds

- Core Rust suite: 13 passed (six unit, three connectivity integration and four existing LAN integration); the existing 8 GiB test was not repeated. Desktop library suite: seven passed, including the updater guard. Frontend production build, Rustfmt, diff whitespace check and configured Clippy passed.
- Real AutoNAT v2 exchange on authenticated loopback QUIC confirmed the client's actual listener and rejected a route that could not authenticate as that client. Only the reachable route appeared in the swarm's external addresses. The harness permits loopback candidates under `cfg(test)`; shipping filtering rejects them. This is a protocol test, not a real NAT/firewall experiment.
- Unit checks cover private/CGNAT/invalid-transport candidate exclusion, duplicate suppression and eight-candidate ceiling, no advertisement without successful evidence, local-address exclusion even after a claimed success, ten-minute confirmation expiry and withdrawal when the probe server disconnects. Existing mDNS-only join and nonmember denial checks still pass with the client enabled.
- The first dial-back harness timed out because it omitted Identify, which AutoNAT uses to learn server protocol support. The initial broader run failed for the same reason. Adding Identify to both test swarms fixed the harness; the focused test and complete suites then passed. The shipped node already included Identify.
- No public probe/relay/bootstrap service was contacted. AutoNAT server assistance, circuit relay transport, DCUtR, invitation contacts and real cross-network acceptance remain pending. The current bounded client does not re-probe completed candidates until all connections close; expired confirmations are withdrawn. Long-lived-session reconfirmation remains an integration item in the delivery plan.

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
- Remote Windows frontend build, Rust formatting, and core/integration tests passed for commit `b1330b0`. The [GitHub workflow](https://github.com/Virakhit/Dump/actions/runs/36969011717) records the remaining checks and their final status.

## Updater verification — 0.1.1

- Frontend build, Rust formatting, and Clippy passed locally after adding the native updater.
- Real Tauri updater requests over loopback passed: older/equal versions return no update, the signed fixture downloads successfully, tampered bytes fail verification, and an inflated manifest version paired with the earlier signed version is rejected.
- Desktop guard test passed: queued/active transfers and file preparation block installation; completed/cancelled/failed transfers do not.
- Node's built-in test runner verified release announcement, refusal of incomplete/unsigned manifests and unexpected installer URLs, matching tags, and prevention of channel rollback.
- The shipping configuration requires HTTPS and the version covered by the pinned public-key signature. HTTP is enabled only in the isolated updater test context.
- [Remote Windows checks](https://github.com/Virakhit/Dump/actions/runs/36972960239) passed for the updater application source, including core, desktop guard, native updater, publishing tests, formatting, frontend build, and Clippy. Packaging runs separately against the version tag.
- [Installer packaging](https://github.com/Virakhit/Dump/actions/runs/36974524963) and the [release announcement](https://github.com/Virakhit/Dump/actions/runs/36976427561) passed. Publishing `v0.1.1-alpha.2` automatically committed the public update channel on `main`, including both Windows platform aliases and public installer URLs.
- The optional live updater check fetched the public HTTPS channel and verified all 219,912,487 bytes of the published Windows installer against its pinned key and signed version. SHA-256 of the separate downloaded installer: `074c4c81540c1ea94e7859e837644e5ab0b361911cdce00932ecf29dbe215183`.
- Silent upgrade to 0.1.1 returned exit code 0 on the development machine. The installed executable reports product/file version 0.1.1, launched successfully, and opened its LAN UDP listeners without a development server.
- Full installation/restart through the in-app update button and clean-machine updater behavior remain unverified. Native automation could observe the window but could not activate it or expose its controls for interaction. The 0.1.0 installer below predates this feature and needs a one-time manual upgrade.

## Original 0.1.0 artifact

`src-tauri/target/release/bundle/nsis/Dump_0.1.0_x64-setup.exe` is a Windows x64, current-user NSIS installer with the WebView2 offline installer bundled. The application/installer are unsigned. Bundling passed; installation on a clean machine has not been tested.

Final installer: 219,279,799 bytes. SHA-256: `c35eb85ecf6df7e892cc95f83f6cf0d8a6b20fbaac2c983b2ba93fbfbc83b258`. An adjacent `.sha256` file contains the same checksum.

## Still required

Physical two/three-machine Windows LAN checks, discovery/firewall scenarios, removal/reconnect during an active transfer, target-machine memory observations, native keyboard/DPI/drop checks, interactive wizard navigation, clean installation/uninstallation and deep-link launch, packet capture, and the five-user pilot are listed in `PLAN.md`. The source repository has been pushed to GitHub. The first remote Windows CI run found a test sequencing race: it requested a stream before the asynchronous dial was registered. The test now waits for authenticated catalog admission before continuing, and the remote tests passed. No prospective users have been contacted, and the installer remains an alpha preview rather than a validated beta.
