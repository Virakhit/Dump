# Workspace LAN/Tailscale Implementation Plan

**Goal:** Build workspace chat and one invitation for LAN/Tailscale, with bounded
owner relay fallback, starting from v0.1.1-alpha.2.

**Architecture:** Preserve direct QUIC and the v1 control/file payloads. Add
independent signed route-envelope, chat and encrypted owner-relay modules.

**Tech stack:** Rust, libp2p, Tauri, React/TypeScript; already authorized Tailscale.
**Spec:** ../specs/2026-11-15-workspace-lan-tailscale-design.md
**Execution:** Native work in this isolated branch; no commits or main integration.

## Constraints and review focus

Keep fingerprints/manual approval, signature/integrity checks, bounded memory and
fresh stores/ports. No production profile, app installation, public service or
network-policy changes. Wrong-workspace/cancelled joins must never admit; a route
hint never authorizes; delayed ACKs must not revive a replaced operation; host
loss must fail rather than imply source availability; signed but corrupt file
bytes must never commit. Pin each failure class in its owning phase's tests.

## Phase 1: Baseline join evidence (start now)

- [x] Add scripts/join-ui.test.cjs with real React/JSDOM components and mocked
  Tauri boundary only: receipt notice/review from Files and open invite dialog,
  no-request Members, active-owner scope, unchanged fingerprint gate and neutral
  joining text that does not infer delivery from discovery. Observe failures on
  unchanged baseline before modifying src/main.tsx.
- [x] Change only those UI diagnostics; do not add transport or protocol fields.
  Review requests closes the invitation dialog and selects Members. Zero requests
  must be shown as zero received, never as an app/network failure diagnosis.
- [x] Add src-tauri/tests/join_contract.rs using fresh Engine stores: authenticated
  pending fingerprint, Waiting before manual approval, signed grant after approval,
  wrong selected workspace/recovery, wrong owner/token/expiry denial, rejection
  invalidating the invitation and duplicate request idempotence. These are baseline
  characterization/security tests, not proof of mDNS or real network delivery.
- [x] Run UI/build/publisher/format/whitespace checks and offline core suite.
  Preserve all failures; document dependency/toolchain blockers and no native pass.

Phase 1 execution record: docs/JOIN_PHASE1_VERIFICATION.md. UI tests pass; the
four added Rust contracts and complete native suite remain unexecuted because
offline resolution is blocked. The user subsequently authorized implementing
Phase 4 chat before the native gate can run; see docs/CHAT_PHASE_VERIFICATION.md.
Phase 3 source is now present; native and physical gates remain pending. Phases
2, 5 and 6 remain pending. No feature is shipped from this worktree.

## Phase 2: Executable native baseline and isolated runtime

- [ ] Coordinate only missing dependency/cache and complete native toolchain.
  Keep Cargo.lock pinned; no substitution with the later vendored stream fork.
- [ ] Run complete baseline plus new contract tests, cargo check and all-target
  Clippy. Failures block production networking changes.
- [ ] Add test-only isolated profiles/app IDs/deep-link scope and OS-process
  fixtures, retaining independent stores/ports and guaranteed child cleanup.
- [ ] Add transient join-attempt diagnostics in engine/network: actual discovery,
  stream request and Waiting ACK; test cancellation, replacement, denial and owner
  restart using real authenticated loopback. Do not change v1 wire enums.
- [ ] Complete unchanged direct LAN two-device gate with full invitation and
  owner card/ACK/manual approval plus verified file transfer.

## Phase 3: One signed route envelope

- [x] Create src-tauri/src/invite_routes.rs and tests/invite_routes.rs. Define
  RouteEnvelope::parse_url/url with embedded v1 Invitation and signed owner routes
  per spec, accept old full invites and reject malformed/expired/tampered or
  identity-substituted envelopes, oversized URLs and conflicting destinations.
- [x] Add a separate persisted shareable-invite ledger and owner issuance command;
  retain legacy single-device tokens. Test the identical URL for two authenticated
  peers requiring two independent approvals, per-peer rejection, full-link
  revocation, expiry, replay, member cap and legacy-state migration. Recheck live
  membership before returning a peer-bound grant. Do not add fields to v1 Join.
- [x] Create src-tauri/src/route_selection.rs with deduplicated bounded LAN /
  Tailscale ordering, two-attempt ceiling, expiry/backoff and cancellation tests.
  Wire only verified literal routes to Node and retain mDNS fallback.
- [x] Add read-only installed-Tailscale adapter (no login/config commands).
  Test absent/offline/unauthorized Tailscale; UI must keep working on LAN.
- [x] Add membership-scoped /dump/routes/1 exchange for signed/expiring member
  routes and the owner-signed relay locator. Test two approved members without
  mutual mDNS discovering each other's catalog and trying direct Tailscale;
  reject outsider/cross-room records, replayed sequences, bad pins and expired
  entries, including unauthorized later pages and revocation during exchange.
- [ ] Prove same URL reaches one owner over isolated LAN and overlay simulations;
  then physical LAN and Tailscale with exact authenticated Peer IDs.

Phase 3 source/verification record: docs/UNIFIED_INVITE_VERIFICATION.md. Eight
new Rust cases are metadata-checked only; 15 UI cases/build passed. Local Tailscale
status probe exited1, with no confirmed overlay endpoint. Host relay contract is
docs/HOST_RELAY_CONTRACT.md, not an implemented relay. Packaging/native GUI
handoff additionally requires docs/UPDATER_REBUILD_GATE.md coordination; current
0.1.1 version still points to the legacy public 0.1.3 updater feed.

## Phase 4: Workspace chat

- [x] Create src-tauri/src/chat.rs and tests/chat.rs with signed, domain-separated
  messages; tests deny forged/wrong sender/workspace, nonmembers, stale membership,
  oversized/empty body, conflicting UUID and replay duplicates. Assert durable
  store before ACK, cancellation/restart and 500-message retention.
- [x] Keep a separate durable 64-sequence replay window per sender/room/roster
  epoch. Test replay after history pruning and restart, changed-content duplicates,
  old/out-of-window/reordered sequences, sender counter rollback and remove/re-add.
- [x] Extend persisted state additively and add tests for legacy 0.1.1 migration preserving
  identity, roster, shares and invitation tokens. Keep v1 control/file payloads.
- [x] Add /dump/chat/2 handler plus send command with bounded concurrency/current
  roster recheck, truthful per-device delivery results and reconnect behavior.
- [x] Add src/chat.tsx and UI tests for room isolation, literal HTML rendering,
  offline delivery, failed persistence and workspace change while sending.

Phase 4 implementation is present, with 13 total UI cases passing. Eight Rust
chat cases have only cached-artifact metadata validation; they did not execute.
Native desktop/IPC, pinned Cargo.lock build and actual network delivery remain
unchecked. A completion checkbox above denotes source/test addition, not runtime
acceptance. Protocol 2 deliberately avoids negotiating the incompatible old chat 1.

## Phase 5: Owner relay fallback

The checklist below preserves the initial circuit proposal and runtime gates.
Implementation refinement: dedicated purpose-bound application streams with inner
Noise replace generic circuits/Yamux, separate key and locator. Current contract
and verification below supersede those transport details.

- [ ] Add only required circuit/Noise/Yamux dependencies and isolated modules
  src-tauri/src/host_relay.rs and tests/host_relay.rs. Test roster-based admission
  at reservation and circuit creation, cross-workspace/nonmember denial, revocation,
  per-peer/host quotas, byte budget and host-role opt-in.
- [ ] Bind each relay instance to one active workspace and a separate protected
  infrastructure identity in the owner's signed locator. Test switching between
  two owner rooms with an overlapping member: old reservations/circuits must close
  and cannot be reused in the new room; the relay identity must not gain membership.
- [ ] Implement relay route after direct attempts fail without interrupting an
  active direct transfer; exercise direct takeover without moving active streams.
- [ ] Use bounded 64 KiB forwarding buffers/backpressure; verify resident-buffer
  limits on a slow receiver, progressing large files, inactivity/cancellation,
  host/source loss, corrupt bytes, no final/partial commit and manual-only retry.
  Include a file larger than 256 MiB and disclose/configure circuit byte/lifetime
  ceilings; do not silently import the old relay's small-transfer policy.
- [ ] Show actual measured route/source/host availability in file/chat UI. Require
  three separate devices with LAN-only member, Tailscale member and dual-reachable
  owner, proving the direct path is unavailable and the observed host circuit is
  used. Source offline must fail even when host stays online.

## Phase 6: Review and integration handoff

- [ ] Run full UI/core/desktop/updater/format/Clippy plus physical matrix and
  source-hash/result ledger. Separate simulated, native and physical evidence.
- [ ] Review auth, endpoint encryption, quotas, file lifecycle and backward-state
  compatibility. Report unresolved cases; coordinate before main integration or
  changing the real installed application. Do not publish app version 0.1.1 again.

Phase 5 implementation update (2026-11-15): source candidate now implements the
narrow application bridge described in docs/HOST_RELAY_CONTRACT.md, replacing the
proposed generic circuit relay/separate identity with endpoint-authenticated Noise
over purpose-bound owner streams. No host cache. Native and physical acceptance
checkboxes above remain open. See docs/HOST_RELAY_VERIFICATION.md for checks and
blockers; do not infer runtime completion from source implementation.
