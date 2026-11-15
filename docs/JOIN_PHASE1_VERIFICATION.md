# Phase 1 join diagnostics: 15 November 2026

Branch `codex/restart-v0.1.1`, base tag `v0.1.1-alpha.2`, HEAD `ffb6c15`.
This is a local uncommitted phase, not a release or a fix claim for the reported
physical LAN incident. The original checkout and installed application remain
unchanged. There is no new chat, Tailscale route or host relay implementation yet.

This is the phase-1 historical snapshot. Subsequently authorized chat source is
recorded separately in CHAT_PHASE_VERIFICATION.md; its runtime gates remain open.

## Implemented

- Owner sees zero received requests or the count of received requests for the
  selected workspace. Existing Engine pending state is the source of this fact;
  no discovery or reachability claim is inferred.
- Review requests from Files or the invitation dialog closes the modal and opens
  Members, including when Members was already selected. Existing approval still
  requires fingerprint confirmation; arrival never auto-approves or switches room.
- Frontend controls additionally require actual owner identity and matching
  pending.workspace_id; that field was already present in the backend View.
- Joining text no longer claims the owner is waiting/has received anything merely
  because joining began. This phase does not add per-attempt discovery/ACK telemetry;
  it directs both users to confirm actual arrival in the owner's Members view.
- Four new Rust tests characterize baseline authenticated admission, manual grant,
  duplicate request handling, wrong workspace/recovery/rejection, forged/expired
  invitation denial and the existing CBOR Waiting response. They use temporary
  protected stores, not profiles, ports or real sockets.

Production Rust/network/model/storage code, Cargo manifests/lock, Tauri app ID,
wire enums, token behavior, timeouts and cryptography are unchanged. Legacy
invitations still admit only one approved device. The later shareable-link ledger
is planned separately and has not been implemented.

## Executed evidence

The first bounded UI regression run on unchanged baseline had six assertion
failures covering missing diagnostics/review, owner scope and misleading joining
text. Minimal UI changes made all six pass. A seventh test then failed when the
dialog was opened from Members; fixing that review condition made all seven pass.
An earlier harness assertion tried to print a whole DOM node, exhausted an array
buffer and delayed failure reporting; it was corrected to a bounded boolean
assertion before the clean six-failure baseline run. This was a test-reporting
problem, not an application or networking failure.

Latest full `npm run check` with Cargo on child-shell PATH and
`CARGO_NET_OFFLINE=true`: **exit 1**. Build passed (37 modules), all seven UI tests
passed, then core resolution failed: `no matching package named libp2p-stream
found`. No Rust test bodies executed. The earlier check without Cargo on PATH
also exited 1 after the same frontend passes; that setup failure is not hidden.

Focused `cargo test --offline --locked --manifest-path src-tauri/Cargo.toml
--no-default-features --test join_contract` had the same dependency-resolution
blocker, exit 1. No native approval/wire test is claimed passed. Native linking
will additionally require resolving the previously observed incomplete MSVC/GNU
toolchain; no toolchain was installed or altered.

Rustfmt, Node script syntax, publisher regression (one test), tracked-production
Rust no-diff check and whitespace check passed. UI tests render real React in
JSDOM with only Tauri calls/events mocked; they prove neither native IPC nor LAN.

`npm install --package-lock-only --offline --ignore-scripts --save-dev
jsdom@27.0.1` could not resolve cached registry metadata (ENOTCACHED). No packages
were installed. jsdom was instead pinned using its already available dependency
closure/integrity entries from the original checkout's lockfile; existing
dependency entries were preserved byte-for-byte semantically (zero changed),
with 46 new dependency entries. The installed ancestor node_modules provides the
test/build tools. Fresh `npm ci` for this new lockfile has NOT been run, and must
be verified in the coordinated clean environment. No original lockfile changed.

## Remaining gates and decisions

Design and phased plan are in docs/superpowers/specs and plans, dated 2026-11-15.
Read-only builder review identified member-route exchange, replay-after-pruning
and workspace-bound relay admission gaps; those requirements and tests were added
to the plan. Study of actual later source confirmed separate LAN short invitations,
online chat-send targets and a relay without filesystem API and with a 256 MiB
circuit limit; none was imported as a working feature of this base.

Proceed with production networking only after native baseline/contract checks can
execute. Minimum environment work is fetching the original pinned registry crate
and its missing dependencies into a permitted cache plus using a complete native
compiler/linker environment. Keep original dependency versions; do not substitute
the later patched stream crate merely to bypass the blocker. Fresh dependency
installation/toolchain changes require coordination, not firewall changes.

Initial relay design is **online streaming only**. Source must remain online and
reachable by the host; host must be online/reachable from both sides for fallback.
Keeping copies on the host is an alternative product scope, not implemented:
it requires explicit storage consent/limits, encryption-at-rest/key ownership,
retention/deletion/revocation semantics and proof that verified copies are actually
available. Initial chat similarly has local history and online delivery outcomes,
not an offline queue. Do not describe remembered manifests as hosted files.

Physical acceptance needs two Windows devices for LAN, two authorized Tailscale
endpoints, and three devices for the mixed topology (dual-reachable owner, LAN-only
member and overlay member). Use separately coordinated disposable Windows profiles
or a purpose-built isolated app ID/data/deep-link scope; a worktree alone does not
isolate the real app profile. Use actual reachable routes without changing policy,
record request/ACK/manual fingerprint approval, prove host circuit use when direct
is unavailable, and compare the final file SHA-256 independently. Exercise source
offline, host loss, large/slow files and cancellation. No physical gate ran here.

No commit, push, main integration, deployment, release, app installation/downgrade,
real-profile access, firewall/network-security change or WisePOS action occurred.
