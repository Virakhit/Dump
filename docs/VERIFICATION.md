# Local verification — 2026-10-02

This records checks on the development Windows machine, not approval for a public beta.

Earlier milestone and installer results below are historical. The Internet stabilization round at the end records the failed remote CI run and the newer working-tree checks separately.

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

## v0.2 Stage 3 — encrypted circuit relay client

- Core Rust suite: 17 passed (seven unit, four connectivity integration, four existing LAN integration, two relay integration); 8 GiB remained ignored. Desktop library suite: eight passed. Frontend production build, Rustfmt, diff whitespace check and configured Clippy passed.
- A local circuit relay v2 carried the existing invite/owner approval, authenticated catalog and a 2 MiB + 17 byte file transfer between two Dump nodes. The received bytes matched exactly and no partial remained. These circuit connections negotiate end-peer Noise plus Yamux through the configured libp2p transport.
- An authenticated outsider using the circuit received denial for catalog and file requests, and the relay's real identity failed workspace catalog authorization. A private loopback circuit address was not advertised by the owner's Identify response. No global workspace discovery was added.
- The same peer established a direct QUIC route alongside its circuit. After stopping the relay, an authorized catalog request still succeeded using direct; no forced circuit closure is used for route selection. This is explicit direct takeover, not a DCUtR test.
- Stopping the relay after a 64 MiB receive had written some partial bytes caused a failed/cancelled transfer, no destination file, no `.part`, and no recorded partial remaining.
- A separate direct TCP/Noise/Yamux route passed invite approval and authorized catalog exchange. Address parser tests reject mismatched destination identities, missing relay identity, relay/destination substitution and nested circuits. Existing direct identity, LAN mDNS, owner-offline, signed roster, source-change, pagination and cancellation checks passed.
- Initial relay tests timed out because the test server had no external address to advertise in reservations; adding an explicit test-only loopback advertisement fixed that harness. The next direct-takeover test exposed the default libp2p dial condition, which prevents adding a direct connection while already connected via relay; the explicit upgrade uses `PeerCondition::Always`. Clippy then rejected one collapsible match, which was corrected. Complete checks passed after these fixes.
- No public relay, actual NAT/hole punch or new installer was used. Relay host opt-in/resource ceilings, automatic Internet contact handling and a dedicated hostile-provider byte-corruption transfer test remain later-stage work. This stage does not change persisted identities, workspace data or v1 signed payloads.

## v0.2 Stage 4 — DCUtR

- Core suite: 19 passed; desktop library: ten passed. Frontend build, Rustfmt and configured Clippy passed. The existing large transfer was not repeated.
- Three local swarms exercised the shipping DCUtR adapter: an initial circuit became a direct authenticated connection, with a successful DCUtR event. Loopback candidates are allowed only in that test; shipping candidates must be public literal IP routes tied to the expected Peer ID.
- A production-filtered circuit test confirmed failed hole punching still carried a complete verified file and retained the relay fallback. Private/DNS/mismatched identity hole-punch routes are rejected before dialing. Negotiations are bounded to eight handler events per connection and 1024 per connected lifetime; the native behavior resets after all connections close.
- These tests use no public service and do not prove traversal of a real NAT or firewall. No new installer was published.

## v0.2 Stage 5 — opt-in forwarding

- Core suite: 21 passed; desktop library: 12 passed; frontend build, Rustfmt and configured Clippy passed. The separate patched stream package regression test also passed (4096 connection cycles).
- Two concurrent streams shared the 2 MiB/s application-byte bucket, and the 61st incoming connection attempt in one rolling minute was denied. A real local host swarm denied an excess reservation, then cut off an authorized 1 MiB file when its test circuit exceeded the configured 128 KiB budget. The destination directory and persisted partial list remained empty. Production limits are documented in PROTOCOL.md.
- The first host build exposed the derive macro's visibility requirement; the admission type now has matching crate visibility. The first quota test appended a duplicate destination suffix to an already complete reservation address; removing that test error fixed it. Complete checks passed after correction.
- Advanced settings persist under DPAPI with opt-in disabled by default and changes applied after restart. The host has no Engine or file/disk API. Public hosting, wizard interaction, rate fairness under attack and real NAT/firewall behavior remain unverified; no new installer was published. The hosting role currently forwards circuits, not AutoNAT probes.

## v0.2 Stage 6 — private contacts and automatic routing

- Final core suite: 26 passed (16 unit, four connectivity, four LAN, two relay); the previous 8 GiB check remained ignored. Desktop library: 17 passed. Native signed-updater test: one passed, its optional public download check was not repeated. Publisher test: one passed. Patched stream package: one passed. Frontend production build, Rustfmt check, diff whitespace check and configured Clippy passed.
- An isolated automatic invitation test supplied a signed circuit locator, with no mDNS and no explicit owner dial by the joining node. It reached the owner, obtained manual approval, discovered a signed manifest and received the exact 2 MiB + 17 byte payload through the bounded host role. A third authenticated nonmember received no contact records. Loopback contacts were injected only into the private unit harness; public invitation parsing and the shipping start entry point reject them.
- Signed-record checks cover route/key/identity modification, expiry, private/DNS routes, invitation owner pinning, stale sequence rejection and conflicting equal sequences. Creating a fresh Internet invite renews its contact for the full token lifetime. Restoring a serialized legacy state with no new fields retained the same identity and verified workspace roster, with assistance off and an empty contact cache.
- A deliberately malicious approved provider authenticated with its real key and returned a valid signed manifest, incorrect raw bytes of the expected size and a framed `Done`. The actual receiver failed final SHA-256 verification; no destination, partial file or recorded partial remained. This tests corrupt application data from an endpoint; it is not a packet-capture test of altering encrypted relay traffic.
- Routing checks cover LAN before public direct before circuits, QUIC/TCP alternatives for one configured relay, and retaining active relay diagnostics when expired entries are evicted at the 256-peer ceiling. Contact requests authorize every page; v1 control/file payloads and signature domains remain unchanged. Host listener failures now report a bounded generic error without stopping LAN/file sharing.
- Intermediate builds caught a moved watch receiver, missing multiaddress type annotations in a test and a Clippy request to use `?` in eviction. These were corrected and the affected full checks rerun. The UI keeps multiline address editing intact and trims/removes blank lines only when saving.
- No new installer or release was built/published for this milestone. Actual different-network NAT/CGNAT/firewall behavior, native UI interaction and packet capture remain unverified. AutoNAT's completed candidates are conservatively withdrawn on expiry and wait for an all-connection reset before re-probing; public listener hints and live relay locators remain separate contact routing options. All tests below use local services only.

### Requested twelve-case regression matrix

| Case | Executed evidence |
|---|---|
| 1. LAN still connects | `lan_mdns_still_finds_invited_owner_without_explicit_addresses`; existing LAN file/revocation tests |
| 2. Explicit Internet-style addresses | `explicit_direct_addresses_identify_and_nonmember_denials`, `authenticated_tcp_route_supports_the_same_workspace_protocol`; literal loopback simulation, not a public Internet route |
| 3. Existing protocols over relay | `circuit_carries_join_catalog_file_and_direct_takes_over`; automatic invitation/circuit file test |
| 4. Relay cannot bypass membership | Real relay identity fails workspace authorization; circuit outsider is denied |
| 5. Tampered bytes rejected | `authorized_provider_wrong_bytes_fail_final_hash_and_leave_no_partial` |
| 6. Wrong Peer ID | `mismatched_transport_identity_never_connects`; suffix/circuit/contact key binding checks |
| 7. Unauthorized catalog | Explicit direct and circuit outsider catalog requests denied |
| 8. Unauthorized file | Explicit direct and circuit outsider file-header requests denied |
| 9. Relay loss cleanup | `relay_loss_during_transfer_never_commits_a_partial_file` after partial bytes exist |
| 10. Direct preference | Direct takeover beside a circuit remains usable after stopping the relay; separate route-order check |
| 11. DCUtR upgrade | `dcutr_negotiates_a_direct_quic_upgrade_through_a_local_relay`; failed-upgrade circuit fallback |
| 12. Relay limits | `shared_bandwidth_and_connection_rate_are_bounded`, `host_rejects_excess_reservations_and_stops_over_budget_payload` |

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

## Internet stabilization — 2026-10-02

### Revision and environment

- Started on `main`, HEAD `605f13902b0e909b701a487f6d30adb0495dc44d`, with a clean working tree. Checks below test uncommitted changes based on that HEAD; no new commit hash exists. No checkout/reset/clean, push, release/version/channel change or infrastructure deployment was performed.
- Local runtime: Microsoft Windows 10.0.19045 x64, PowerShell 7.6.5, Rust/Cargo 1.99.0, Node 24.18.0, npm 11.17.0. Windows-gated network/storage tests actually executed; they were not counted from a non-Windows skip.
- All networking tests use local, test-controlled services. No public relay/probe service or another person's endpoint was contacted. No firewall/router configuration was changed.

### CI failure and what is still unknown

The [failed workflow/job](https://github.com/Virakhit/Dump/actions/runs/36994791335/job/110798924201) tested the same base HEAD on Windows Server 2025 with Rust 1.99.0. Compilation succeeded. `cargo test --manifest-path src-tauri/Cargo.toml --no-default-features --locked` failed `network::internet_tests::invite_locator_automatically_joins_and_transfers_and_contacts_stay_private` with `deadline has elapsed`; its unit phase reported 15 passed/one failed in 35.77 seconds. That log contains no failing-stage evidence. Its root cause remains **unconfirmed**.

The unchanged invitation test passed locally once before editing (25.53 seconds). After instrumentation, a dedicated batch of ten fresh Cargo processes passed ten times (24.83–26.69 seconds of test runtime; file-transfer stages 14.48–16.37 seconds). After the final Rust changes, another fresh focused process passed in 28.25 seconds, and the test passed inside each of the three final parallel core runs. The ten-run batch preceded the final signed-file re-probe fixture, listener-withdrawal and partial-ownership edits; it is not presented as ten runs of the final source snapshot. No original invitation runtime timeout was reproduced locally. These passes do not establish the CI root cause or a new CI result.

The invitation test now labels relay listen, owner reservation/circuit locator, member-to-owner connection, owner join request, member approval, signed catalog/manifest, file completion and outsider contact denial. It waits on address/diagnostic watches and Engine notifications, keeps the original stage deadlines, and immediately fails on terminal Failed/Cancelled transfer state with its reason. Diagnostics include bounded counts, route, Identify/protocol negotiation outcome, reservation state and elapsed time, without private keys, invitations, workspace secrets, file bytes or full membership/contact payloads. Nodes and background/relay tasks are cancelled and awaited on success, Result failure and timeout; a Drop fallback cancels on unwinding. No ignore, assertion weakening, hidden retry or suite-wide serial execution was introduced.

### Relay/app identity hypothesis: reproduced separately

Before splitting identities, local runtime probes reproduced both orders with the same app/host key:

- Relay-first: reservation completed, but the direct relay-service connection was treated as the app endpoint; `/dump/control/1` negotiation was unsupported (0.21 seconds).
- App-first: authorized join/approval worked, but relay reservation on the app-service connection timed out at its unchanged ten-second deadline (20.11 seconds overall probe).

This is a confirmed service-endpoint selection defect, **not the proven cause of the CI failure**. The CI invitation fixture already creates a separate relay key.

`owner_relay_first_requires_an_authorized_app_endpoint` now attaches A to B's actual persisted relay identity first, then uses B's R→B invitation circuit, manual owner approval, authorized catalog and a verified 1 MiB + 31 byte file. `owner_app_first_keeps_the_app_usable_when_relay_connects` checks app-first direct transfer plus independent relay reservation. Both deny outsider catalog/contact/file access; the relay identity is absent from the roster. Both passed in all three final core runs. The relay-first transfer captures the relay route; the app-first transfer captures direct.

`legacy_relay_identity_migrates_once_without_changing_member_state_or_remote_pins` uses real Windows DPAPI state: the original device key, owner-signed roster, signed manifest, settings and remote relay pins survive; the relay key persists across reload; old hosting is off; an old local hosting suffix requires explicit correction. No private key bytes are printed by assertions.

### Circuit limits and safe completion

Pinned `libp2p-relay` 0.22.0 forwards a combined byte counter in both directions, covering all streams plus inner Noise/Yamux/protocol framing. Buffered forwarding can slightly overshoot before the next limit check. Circuit age starts when forwarding begins, independently of file start. Earlier traffic consumes budget. Production limits remain 256 MiB/600 seconds per circuit, with the existing reservation, circuit, bandwidth and rate ceilings. A client cannot infer arbitrary remote remaining quota or an exact maximum file size.

The UI warns for known relayed peers and captured active relay transfers, including after another route upgrades to direct. Direct files are not blocked by relay limits. Network close/reset/stall uses a neutral actionable message: the relay connection closed or stalled and its limit **may** have been reached; retry manually using direct or consult the operator. Integrity, authorization and disk errors retain their own cause. No quota-specific client error is invented from a reset, and no resume/retry/circuit churn to evade limits was added.

The six new `relay_host::transfer_tests` passed in each final parallel core run:

| Regression | Test-controlled limits and verified result |
|---|---|
| `file_inside_circuit_budget_completes` | 32 KiB file / 512 KiB circuit; exact bytes, one circuit |
| `byte_budget_closure_fails_without_committing_or_retrying` | 1 MiB / 128 KiB; host byte-limit error, terminal receive, no output/partial and no new transfer on later state updates |
| `earlier_transfer_and_protocol_overhead_consume_the_same_circuit_budget` | 64 KiB then 192 KiB / 256 KiB; first verified, second fails on the same circuit |
| `circuit_lifetime_closure_fails_without_committing` | 4 MiB / 16 MiB / two seconds; host TimedOut, terminal receive, no false completed file |
| `relay_shutdown_during_transfer_never_leaves_a_completed_file` | Stop host after real receiver progress; terminal receive and clean partial state |
| `direct_file_larger_than_relay_budget_is_not_blocked` | 256 KiB direct / one-byte, one-second relay config; verified direct transfer, zero circuits |

The retained excess-reservation/over-budget test also passed. Final length/SHA-256 validation and Windows no-replace completion remain in the real receiver. The existing hostile-provider corrupt-byte test still fails safely. A new partial collision test calls the actual exclusive-create/journal helper and reopens protected state: an unowned colliding file is never recorded or deleted. Owned partials are journaled before payload writes; failed deletion retains/adds that path for startup cleanup if saving succeeds. Storage errors are surfaced. A crash between exclusive creation and recording can leave an empty, unrecorded partial; unknown files are never swept automatically.

### Long-lived AutoNAT revalidation

The narrow vendored patch keeps `libp2p-autonat` at 0.16.0 and changes its v2 client lifecycle, not the networking stack or wire protocol. Completed candidates can be requeued without replacing live handlers or outstanding nonces. Dump retains the eight-candidate/pending ceiling, five-minute recheck backoff, ten-minute evidence TTL, retirement of stale addresses and strict public/expected-peer filtering. Library I/O/unsupported/fake-success failures withdraw evidence and report inconclusive; no usable server leaves Unknown. Listener expiry/closure also withdraws confirmed advertisement.

Three added wrapper checks cover long-lived connections and 32 address-churn cycles, refreshed TTL/failure withdrawal, and a persistent public listener without a server. The existing real authenticated dial-back test now re-probes with a live application stream and checks preserved connection IDs. Five added library checks cover completed retry without handler replacement, exact address/nonce/server binding and forged success claims, pending-request closure/I/O release, inconclusive re-probe withdrawal, and completion of a retired pending candidate without re-advertisement. All twelve library tests passed in the final package check. `.github/workflows/check.yml` now runs that package regression command; it has not been run remotely for these changes.

`signed_file_receive_commits_after_real_reprobe_without_disconnect` additionally uses a genuine owner-signed roster, production catalog/serve-file/receive/commit and a 2 MiB + 17 byte file. After a real `.part` disk write it pauses only the test receiver, completes another authenticated AutoNAT dial-back, checks the owner's connection IDs and Receiving state are unchanged, then resumes and independently checks bytes/size/SHA-256/no partial/no-overwrite. The pause is cancellation-aware and all hooks/loopback allowances are `cfg(test)`. It passed focused in 10.68 seconds and in every final core run.

### Commands and actual repetitions

Final Rust checks below ran outside the filesystem sandbox after an explicit local mDNS comparison. Separate Cargo processes ran sequentially to avoid Windows link-file contention; each core test process used its normal parallel runner. No `--test-threads=1` was used.

| Command | Executed evidence |
|---|---|
| `npm ci` | One successful run; 72 packages, zero npm vulnerabilities reported at that time |
| `npm run build` | Final TypeScript/Vite build passed, including the active-relay warning after direct upgrade. Earlier sandbox invocation failed parent-path access; outside-sandbox builds succeeded. |
| `cargo test --manifest-path src-tauri/Cargo.toml --no-default-features --locked --lib network::internet_tests::invite_locator_automatically_joins_and_transfers_and_contacts_stay_private -- --exact --nocapture` | Dedicated batch 10/10 fresh processes; final-source focused 1/1; original runtime timeout not reproduced |
| `cargo test --manifest-path src-tauri/Cargo.toml --no-default-features --locked` | First three sandbox runs failed mDNS, despite 30/30 unit tests passing each. Final three outside-sandbox runs passed 41 tests each: 31 unit + four connectivity + four LAN + two relay. Durations 90.14/67.71/68.90 seconds including compilation. One 8 GiB test ignored in each run. |
| `cargo test --manifest-path src-tauri/Cargo.toml --locked -p libp2p-stream --lib` | Two recorded batch runs, each one passed (4096 connection cycles) |
| `cargo test --manifest-path src-tauri/Cargo.toml --locked -p libp2p-autonat --lib` | Two final-batch package runs, each 12 passed; earlier development check also exercised ten tests before the last two regressions |
| `cargo test --manifest-path src-tauri/Cargo.toml --features updater-tests --test updater --locked` | Two recorded batch runs, each one passed / one optional public-installer test ignored |
| `cargo test --manifest-path src-tauri/Cargo.toml --lib --locked` | Initial batch compile failed during test-file construction; final run 32 passed, including desktop updater guard |
| `cargo fmt --manifest-path src-tauri/Cargo.toml --check` | Initial batch failed while the referenced test file was not yet written; final check passed |
| `node --test scripts/publish-update.test.cjs` | Initial independent check and both recorded batches passed, one test each |
| `cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets --locked -- -D warnings` | Final run passed; development/build failures are preserved below |
| `git diff --check` | Recorded batches and final documentation check passed |

Ignored tests are **unexecuted**, not passes. The core invocation without `updater-tests` has zero updater integration tests enabled; that feature is exercised by the separate updater command. The previous historical 8 GiB and public-installer checks were not rerun or recycled as new evidence.

### Failure ledger and environment comparison

- The original remote CI invitation timeout above remains unresolved as to cause.
- All three first normal-parallel core commands failed `lan_mdns_still_finds_invited_owner_without_explicit_addresses` at 30 seconds. After adding real listener readiness, watch-based stages and awaited cleanup without changing the 30/15-second deadlines, the focused sandbox run still failed: six listeners, owner route Offline, no Identify, no protocol outcome or dial failure, no owner join request. The identical focused command outside the sandbox passed in 8.17 seconds (join 4.013 seconds; approval 3.982 seconds), then all three full outside-sandbox suites passed. This supports an environment-dependent discovery restriction; it does not identify a firewall/multicast root cause or prove physical LAN availability. No shipping mDNS/filter behavior was loosened.
- The first signed-file re-probe fixture failed after 30.57 seconds. An accelerated second probe could exhaust the node's two-connections-per-peer admission ceiling before probe-only sockets aged out. The test server now retires only completed outbound dial-back sockets, preserving the persistent request and owner/file connections; the current fixture passes. Shipping admission limits and revalidation remain unchanged.
- Intermediate checks caught an `E0509` move from a cleanup type with Drop, an unclosed test-module delimiter, a default-desktop Transfer fixture missing `relayed`, and a Windows `LNK1104` test-executable lock from concurrent Cargo invocations. These were corrected; Cargo processes were serialized, not the tests inside them. The final recorded desktop/fmt/Clippy failures during construction were a missing referenced `network_reachability_tests.rs`; the completed file passed all final checks. An early vendored package check also failed because published integration-test declarations referenced an unavailable upstream workspace test dependency; unused integration declarations/dev-dependencies were removed, preserving library tests and the pinned production dependency graph.
- An early `--locked` identity-test attempt hit the dependency patch/lock transition. The final lock diff only removes the patched AutoNAT registry source/checksum; no unrelated package version/edge upgrade remains. Final commands all use `--locked`.
- The initial frontend build failed sandbox parent-directory access in esbuild; outside-sandbox build/typecheck passed. Every failure was retained rather than converted into a successful assertion or hidden by retry.

Structured batch command/exit/duration records and stage logs remain locally under ignored `.tools/stabilization/final/`; they contain no private state or file payloads. These records include both three-run core batches and failed construction checks.

### Compatibility and remaining gates

Implemented and locally tested: stage diagnostics/cleanup, persistent separate relay key, safe bounded-circuit failure/UI, lifecycle-safe revalidation and the above regressions. CI verified for this working tree: **not run**. Real-network verified: **not run**. Native UI interaction/layout: **not tested in this round**; TypeScript/Vite build is not visual acceptance.

The device/member Peer ID, signed v1 membership/manifests and existing workspaces remain unchanged. Loading old DPAPI state creates the relay key once, disables previously enabled hosting and displays a reconfiguration notice. Old local hosting suffixes and distributed host locators/invitation circuits need explicit refresh using the new Relay Peer ID; saved remote relay pins are not silently changed. LAN invitations remain usable. Migration grants no relay membership and does not enable hosting.

Still required: rerun the Windows CI job with new diagnostics; physical two/three-machine LAN/mDNS and approved-firewall tests; reachable self-hosted relay from another network; NAT/CGNAT and UDP-blocked/TCP fallback cases; long-lived real-address changes and concurrent transfer/route upgrades; native warning/migration settings interaction. No universal NAT support, production readiness, public beta, new installer or release is claimed.

### Changed files

| Files | Purpose |
|---|---|
| `src-tauri/src/network.rs`, `src-tauri/src/connectivity.rs` | Stage/protocol diagnostics, tracked shutdown, correct host key, captured transfer route, truthful relay errors and exclusive-create partial ownership |
| `src-tauri/src/model.rs`, `src-tauri/src/storage.rs`, `src-tauri/src/engine.rs`, `src-tauri/src/desktop.rs` | Additive persistent relay key/DPAPI migration and UI state; validate hosting suffix against relay identity; preserve existing device/signature state |
| `src-tauri/src/relay_host.rs`, new `src-tauri/src/relay_host/transfer_tests.rs` | Track host lifecycle, retain original reservation regression and add six reduced-limit real-transfer tests; production limits unchanged |
| `src-tauri/src/reachability.rs`, new `src-tauri/vendor/libp2p-autonat/`, `src-tauri/Cargo.toml`, `src-tauri/Cargo.lock` | Bounded live-session retry/expiry and narrow pinned dependency patch with upstream source/license/provenance |
| New `src-tauri/src/network_identity_tests.rs`, new `src-tauri/src/network_reachability_tests.rs`, `src-tauri/tests/connectivity.rs` | Both endpoint orders/migration, real signed-file re-probe, and mDNS readiness/stage/cleanup diagnostics |
| `src/main.tsx` | Distinguish Device/Relay Peer IDs, explain migration and actual relay limits, preserve warnings for ongoing relay transfers after direct upgrade |
| `.github/workflows/check.yml` | Include the patched AutoNAT library regression command in future CI |
| `README.md`, `SECURITY.md`, `docs/PROTOCOL.md`, `docs/PLAN.md`, `docs/VERIFICATION.md` | Current behavior, migration, safety/limit semantics and separate historical/local/CI/real-network evidence |
