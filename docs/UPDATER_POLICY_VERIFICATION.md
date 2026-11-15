# Development/preview updater policy verification — 2026-11-15

Implemented in the isolated codex/restart-v0.1.1 worktree after authorization.
The application is deliberately unable to check/download/install updates from the
old line. No public feed, signing key, signed-version requirement, version number,
installed app or real data was changed. No package, commit, push, PR or CI run.

## Enforcement

src-tauri/src/update_policy.rs is unconditional compiled code: ENABLED=false and
both check/install return the disabled error. It has no runtime/environment/feature
switch. desktop.rs keeps IPC names/arguments for stale frontend callers but only
calls this policy. The former updater client/network/download/install and cached
Update state are removed from production commands. run() registers no updater
plugin. Cargo desktop no longer enables tauri-plugin-updater; updater-tests still
enables the dependency for existing standalone legacy signature fixtures, not the
application's policy. Default capabilities never granted updater permissions.

Engine View reports the compiled false value. Frontend controls require explicit
true and are replaced by a Development / preview build — Updates disabled label.
Absent status fails closed. UI is explanatory; native commands remain the guard.
Old transfer-readiness tests are preserved as historical updater preconditions;
they are not an alternative route to installing updates.

## Current toolchain follow-up

Authorized Build Tools installation and crates.io fetch have resolved the missing
registry/core toolchain blocker. Locked native core suite passes, including policy
integration. Locked desktop tests, native executable build, updater signature fixtures and
optimized locked policy checks now PASS; details in LOCKED_NATIVE_VERIFICATION.md. Earlier blocked results below are
historical, not evidence of a current missing registry package. No app was built
for distribution, installed or updated.

## Earlier guard checks

- Native rustc directly compiled the actual update_policy.rs with -O and
  debug-assertions=no using the existing GNU toolchain. All THREE unit tests passed,
  including the release-only case for both operations. No cached dependencies,
  network, updater plugin or GUI was needed for this policy executable.
- Frontend TypeScript/Vite production build and all 18 React/JSDOM UI tests pass,
  including no updater controls/calls for false or missing policy.
- Cached metadata of core/unit/integration sources passes (additional evidence,
  not an exact Cargo or desktop build).
- Locked release Cargo command is BLOCKED before compilation: offline registry
  lacks libp2p-stream =0.5.0-alpha. Full desktop/macros, packaged NSIS preview,
  native Tauri IPC invocation and all-target Clippy are NOT claimed validated.
- Added test update_policy.rs and a CI check command for --release
  --no-default-features --locked --test update_policy. No workflow was run.
- Source review confirmed no policy enable via updater-tests, no registered
  production plugin/client and unchanged public configuration.
- rustfmt, diff check and publisher test pass.

Latest local logs: .tools/updater-policy-release.log,
.tools/updater-guard-metadata.log, .tools/updater-guard-locked.log.

## Safe future release separation (requires coordination)

Keep development/preview disabled by default. Before enabling any future updater:
choose an approved rebuild identity/version/channel and data/deep-link migration
policy; preserve the legacy channel for existing users. Do not republish 0.1.1 or
replace old public feed assets with this rebuild. A separate preview application
identifier/data directory should be reviewed before installer handoff so testing
cannot overwrite the installed app/profile or seize its invitation registration.
No such identifier/profile migration is implemented here.

Use a distinct approved channel and cryptographically authenticated channel
identity (or separately approved channel signing key), pinned in the new client.
Numeric bumps or a different URL using version-only signed comments do not by
themselves prove branch identity. Default must remain denied until that full
source/config/signing change is reviewed. Retain publisher signature and signed
version verification; do not weaken them to make previews update.

Test signed legacy/newer/wrong-channel/equal/older/tampered feeds and stale cached
state before a distributable build. Disabled builds must make no check/download/
install calls, including optimized release-mode previews and direct IPC attempts.
Full locked desktop/updater checks on a suitable existing runner remain required,
followed independently by LAN/Tailscale acceptance. User coordination is required
before main integration, installed-app changes or any release/CI publication.

## Existing environment follow-up

The two Desktop Commander devices were rechecked: both remain offline. Targeted
workspace/cache searches found no original registry archive or additional compiler.
MSVC tools remain absent; WSL enumeration is inaccessible. Earlier loopback QUIC
and plain same-process UDP probes failed in this execution context; no security
or network-policy change or bypass was attempted. See NATIVE_ENVIRONMENT_AUDIT.md.
Necessary next step is authorized access to an already provisioned environment
with original locked dependencies, native compiler/linker and functioning loopback
UDP, or explicit coordination for restoring missing dependencies/toolchain. No
usable alternate environment satisfying all of these was found in accessible tools.
