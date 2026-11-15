# Host relay phase verification (2026-11-15)

Candidate only, on codex/restart-v0.1.1 from ffb6c154 (v0.1.1-alpha.2).
Original checkout, user documents and installed application remain preserved.
No commit, push, release, install, network-security or feed/signing changes.

## Deliverables

- host_relay.rs: purpose/file budget, signed grant and bounded opaque-byte pump.
- network_host.rs: owner-opt-in bridge, exact scope/generation/single-use claim,
  endpoint-authenticated Noise, direct-first catalog/file fallback, concurrency.
- network.rs: fallback integration, source reuse of existing integrity/commit
  path; direct ConnectionClosed no longer cancels an independent indirect stream.
- engine.rs/model.rs/engine_invites.rs/storage.rs: owner-only persisted setting,
  runtime cancellation/status and old-profile default compatibility.
- main.tsx: owner toggle, host/source online requirement, no cache, 8 GiB fallback
  ceiling, last-confirmed status and actual transfer route on both endpoints.
- Cargo.toml/Cargo.lock: only Noise feature and necessary dependency graph.

## Current exact locked native follow-up

After authorized official Build Tools installation and original crates.io fetch,
locked native core tests pass: 36 passed, 1 ignored. All 5 host relay tests pass,
including the real three-node QUIC file/catalog fallback. UDP loopback passes in
approved Windows host execution. Earlier socket failures below belong to the
restricted execution context; no network/security setting was changed. See
LOCKED_NATIVE_VERIFICATION.md for current provenance, exact commands and gates.
Physical LAN/Tailscale and 8 GiB transfer acceptance remain unperformed.

## Earlier cached native follow-up

Existing Git pthread DLL resolved the cached native linker blocker without any
install or system change. Executed 18 passing native tests: 4 host grant/pump
tests, 4 join contracts, 4 invite-route contracts, 4 route-selection tests and
2 private Hub/Noise/file tests. The bounded in-memory bridge authenticates actual
end peers and calls the real source file-serving path, comparing exact payload
and signed SHA-256. This is not the QUIC/host-admission integration path.

The fifth host test (real three-node QUIC) FAILS before join delivery: loopback
QUIC handshake times out; a plain same-process UDP loopback probe also times out.
Do not mark the suite passed. Exact Cargo dependency/build and physical tests
remain unverified. See NATIVE_ENVIRONMENT_AUDIT.md for evidence, commands and CI
trigger/side-effect audit; UPDATER_POLICY_VERIFICATION.md records the implemented compile-time disabled
policy, optimized native policy checks and pending full desktop build; public
config/feed/key remain unchanged.

## Earlier checks and limits (before native follow-up)

- npm run check: frontend TypeScript/Vite build PASSED and 17 real React/JSDOM
  tests PASSED; native Cargo stage BLOCKED by offline index missing exact
  libp2p-stream =0.5.0-alpha. UI uses ancestor packages already present; no install.
- Cached-artifact direct rustc metadata checks PASSED for current core, core unit
  tests and integration suites (chat, join_contract, lan, invite_routes,
  route_selection, host_relay). These use compatible prebuilt later dependencies,
  not the exact lock graph; no executable linked or Rust test body executed.
- host_relay.rs integration suite contains five tests: grant identity/scope/expiry,
  invalid endpoints/epochs, blocked writer/cancellation, byte budget, and isolated
  three-node approved file/catalog bridge with no supplied direct member route,
  exact output comparison and service-disable failure. Private Hub unit test checks
  one-use source claim, invalid source, quotas, generation cancellation/release.
  These are typechecked test sources, NOT successful runtime evidence.
- Read-only independent code review found direct-disconnect cancellation regression
  in both directions. Fixed by stream-owned liveness; source now marks via_host
  and scope cancellation reaches terminal transfer cleanup. Reviewer rechecked.
- Exact native environment also lacks MSVC tools; cached GNU executable attempt
  previously failed unresolved nanosleep64 in precompiled AWS-LC/pthread objects.
  No shim or toolchain install was used.
- Cargo lock resolution: a disposable ignored probe used the existing local
  libp2p-stream surrogate solely to resolve the Noise feature offline while
  retaining locked versions. Its stream dependencies were checked identical;
  original registry source/checksum block was restored in the resulting lock.
  Existing package versions/checksums were preserved; changes are new Noise graph,
  optional-feature edges and duplicate-version disambiguation. Probe metadata
  could not complete (uncached android_system_properties). No patch/vendor
  override exists in the actual manifest. Exact registry Cargo resolution and
  native build remain gates; lock construction is not a passing build.

## Next required acceptance

Run locked Cargo core and desktop checks with the original registry dependency and
working existing toolchain before handing out a native executable. Resolve updater
channel policy per UPDATER_REBUILD_GATE.md before packaging/native GUI handoff.
Then test fresh profiles on three physical LAN/Tailscale devices, recording actual
fallback, hashes, disable/revoke/cancel and disconnect behavior, slow receivers,
corrupt bytes, >256 MiB and resource ceilings. No loopback test, source inspection
or metadata compilation proves real LAN/Tailscale operation or offline-file access.
