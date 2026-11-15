# Authorized toolchain installation and locked native verification — 2026-11-15

This is a follow-up after explicit authorization to install official Rust/C++
build prerequisites and download official registry dependencies. Earlier audit
sections saying no installation was authorized describe the earlier phase only.
No Dump installer/app update, reboot, network/security settings change, commit,
push, PR, CI invocation or release is part of this work.

## Installation and provenance

- Existing Rust stable MSVC 1.99.1 (58cf8f9c3, 2026-10-13) was retained; Rust was
  not reinstalled. GNU remains available but is not the locked build toolchain.
- Microsoft official bootstrapper URL:
  https://aka.ms/vs/17/release/vs_BuildTools.exe . Authenticode status Valid,
  signer Microsoft Corporation; SHA-256
  8aaf70d164808d313ee6226af5d1e3b2db43c60be551bfeb2b70ce139d792c37.
- Installed VS Build Tools 2022 17.14.37906.3 at the standard BuildTools path.
  Explicit components only: VC.Tools.x86.x64, Windows11SDK.26100 and
  VC.CMake.Project (plus vendor-required dependencies). Existing VS18 BuildTools
  did not appear in the C++ component query. No optional whole-workload install.
- Installer exit 0; vswhere isComplete=true, isRebootRequired=false. MSVC directory
  14.44.35207; SDK 10.0.26100.0; CMake 3.31.6-msvc6. No reboot performed.
- cargo fetch --locked succeeded from crates.io, including the original
  libp2p-stream 0.5.0-alpha. No registry patch/vendor override was added. No global
  Cargo config or legacy Cargo config was present. Cargo.lock SHA-256 at fetch:
  8b0fdb9618584de55def13761761b18b4887ce8646b37b3b6c0c7e97ef4e91ca.

## Execution-context distinction

Restricted shell DNS could not resolve aka.ms or index.crates.io, and earlier
restricted UDP loopback timed out. The tool's normal require_escalated permission
mechanism approved the explicitly authorized official downloads/installation and
native testing. No DNS/proxy/firewall/security setting or policy was changed.
The same bounded Node same-process UDP loopback probe PASSED in that approved
Windows host execution context. This is not a claim of physical LAN/Tailscale.

## Isolated test harness

Ignored .tools/native-env.cmd activates the installed compiler environment;
.tools/native-check.ps1 imports only compiler process environment variables and
uses existing Rust, official CMake, CARGO_NET_OFFLINE=true and isolated target
.tools/native-msvc. All profiles/ports/download destinations are fresh test data;
no installed Dump profile is opened. Checks use --locked. Actual source and lock
are used, unlike the earlier cached-artifact workaround.

## Current result ledger

Locked core test command PASSED: 36 passed, 0 failed, 1 ignored, plus empty
feature-gated updater/doc suites. First build took 3m03s. Breakdown: 6 library,
8 chat, 5 host relay, 4 invite routes, 4 join contract, 4 LAN, 4 route selection,
1 update policy. The ignored test writes/transfers 8 GiB and explicitly requires
at least 18 GiB temporary disk space; it was not run.

Host relay five tests include the real three-node authenticated QUIC scenario
that previously timed out in restricted execution. It now passes using fresh
profiles with explicit member-to-host loopback routes only, encrypted source
catalog/file fallback, exact destination bytes, both transfer route flags, no
partial file leftovers and host-disable denial. Direct file integrity/revocation
and QUIC chat storage receipt also pass. These prove local native behavior, not
physical LAN/Tailscale reachability or every host-loss/corruption/large-file case.

Locked desktop library build/tests PASSED: 7 passed, 0 failed, in 5m07s build
plus 0.35s test runtime. Locked updater signature fixtures PASSED (1 test,
no install call). Locked optimized release policy PASSED (2 tests, 6m03s first
build). All-target Clippy initially reported three lints; minimal fixes in
network_host.rs/network_routes.rs box the large Ready field, use if-let and remove
an unnecessary iterator drop. Added an exact old-unboxed vs boxed CBOR assertion
to the existing Hub unit test. No protocol/auth change; focused review found no
concern. Clippy recheck PASSED with -D warnings in 11.91s. Current core rerun also
passes after these fixes. Final locked native desktop executable build PASSED
(dev/debug profile, 55.42s); executable was NOT launched, installed or distributed.
Final optimized locked policy recheck PASSED (2 tests, 23.79s rebuild).
Logs: .tools/locked-core.log and
.tools/locked-desktop.log. No native app installation or physical two/three-device
test follows automatically from compiling tests.


Existing Tailscale read-only status in approved host context reports Running,
SelfOnline=true, two self addresses. No address or peer inventory was printed or
sent externally; no login/configuration change was performed. This indicates
local daemon readiness, not another peer or LAN/Tailscale transfer acceptance.

Official installation references:
- https://learn.microsoft.com/en-us/visualstudio/install/use-command-line-parameters-to-install-visual-studio
- https://learn.microsoft.com/en-us/visualstudio/install/workload-component-id-vs-build-tools


## Final commands and remaining acceptance

Run via the approved host execution route, from the isolated worktree:

```powershell
& .tools/native-check.ps1 -Check core
& .tools/native-check.ps1 -Check desktop
& .tools/native-check.ps1 -Check updater
& .tools/native-check.ps1 -Check clippy
& .tools/native-check.ps1 -Check build
& .tools/native-check.ps1 -Check policy-release
```

Final current-source logs: locked-final-core.log, locked-final-build.log,
locked-final-policy-release.log and locked-clippy-recheck.log under .tools.
Desktop/updater checks passed before the two unrelated relay/route lint fixes;
current-source core rerun and desktop build cover the changed source and CBOR
assertion. Frontend/UI source was unchanged during this installation phase;
its latest production build/18 UI tests already passed. No test was ignored to
obtain green: the existing 8 GiB direct-transfer test is explicitly ignored by
the repository and is reported separately.

Default normal/build dependency graph excludes tauri-plugin-updater, consistent
with the hard-disabled compiled application policy. The plugin is enabled only
by explicit updater-tests fixture feature, not by the production desktop feature.
Public feed/key/signed-version configuration and Cargo.lock remain unchanged.
Only source corrections in this phase were the three Clippy lints in the isolated
network_host.rs/network_routes.rs and a wire-compatibility assertion; main checkout
and its existing untracked documents were preserved. No commit/push/release.

Remaining: three physical devices on actual LAN/Tailscale with separate device
approvals and proven unavailable direct member route; host/source loss, cancellation
under load, corruption and large relay transfers beyond 256 MiB through the ceiling.
The 8 GiB case still needs a coordinated resource test. Before any manual GUI or
installer handoff, review separate preview identity/profile/deep-link registration
so the existing app/data cannot be overwritten. Native build is not a distributable
preview or permission to change the installed application.
