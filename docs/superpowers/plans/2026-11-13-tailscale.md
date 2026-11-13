# Tailscale Implementation Plan

> For agentic workers: use superpowers:executing-plans. Track verification and rulings here. The user approved implementation of the linked design on 2026-11-13; proceed without another approval gate.

**Goal:** Invite, approve, transfer files and chat using externally installed Tailscale on Windows, including node sharing.
**Architecture:** Keep existing identities and public/LAN protocols. Add a bounded local CLI adapter, workspace-scoped signed overlay contacts, a separate invitation envelope, listeners on Tailscale interfaces and stream controls that enforce workspace routing policy.
**Tech Stack:** Rust/Tokio/libp2p, Tauri 2, React/TypeScript, installed Tailscale CLI.
**Spec:** ../specs/2026-11-13-tailscale-design.md

## Global constraints

- No automatic account, sharing, grants, firewall or installed-app changes.
- Existing DPAPI identity, membership and invitation v1 remain compatible.
- Overlay TCP/UDP 42043; CLI timeout 3 seconds, output 2 MiB, refresh 15 seconds with bounded backoff.
- Contact lifetime 24 hours, clock tolerance 300 seconds, positive monotonic sequence, cache 256, route bound 8, overlay frames/link 8192 bytes.
- TailscaleOnly applies to every protocol and both directions; never reuse other connections or silently migrate transfers.
- Physical network gates remain distinct from fixtures/loopback tests.

## Review focus

- A shared node's address is remapped locally; node ID/FQDN must resolve through the recipient peer map (Task 1).
- Existing LAN/relay connection with the same Peer ID must not carry TailscaleOnly traffic (Task 3).
- Mode changes or logout during a transfer must wait or cancel explicitly without partial file commit (Task 4).
- Existing members must bootstrap overlay without requiring owner availability or membership reset (Tasks 2, 4).
- JSON schema drift, process output growth, stale bindings and unauthorized contact exchange must fail boundedly (Tasks 1–3).

### Task 1: Installed client adapter

Files: create `src-tauri/src/tailscale.rs`, register in `lib.rs`.
Interfaces: `TailscaleSnapshot::{ready_node,resolve}`, `inspect`, `parse_status`; typed self/peer nodes contain ID/FQDN/IPs/online only.
- [x] Write and observe failing tests for ready/login/invalid/extra-field parsing, remapped peer resolution, ambiguous peers, reserved IPs and output bounds.
- [x] Implement bounded hidden process execution and snapshot parsing; run focused tests to PASS.
- [x] Verify installed CLI identifies this device and the visible Windows peer; record only nonsecret evidence.

### Task 2: Signed overlay links and persistence

Files: create `overlay.rs`, `engine_overlay.rs`, `overlay_tests.rs`; modify `model.rs`, `engine.rs`, `relay_host.rs`.
Interfaces: `OverlayContact::{issue,verify,cache_key}`, `OverlayLink::{parse,url}`, `WorkspaceNetworkPolicy::{Legacy,TailscaleOnly}`; Engine creates invite/connect links, caches member contacts and persists per-workspace policy with legacy defaults.
- [x] Observe RED for signature tamper/expiry/replay, URL bounds, legacy state, join cancellation, existing-member connect link and unauthorized workspace.
- [x] Implement independent envelope and contacts; enforce member keys/workspace/sequence/cache limits, pending join policy and existing-member reconnect.
- [x] Run focused tests; legacy invite/state round trips still PASS.

### Task 3: Overlay network and protocol isolation

Files: modify `network.rs`, `streams.rs`, `connectivity.rs`; create `network_overlay.rs` and network overlay tests.
Interfaces: separate stream control for overlay; actor receives bounded adapter snapshots, binds actual local Tailscale IPs, resolves selected member contacts, dials QUIC then TCP with expected identity and exchanges `/dump/overlay-contacts/1` only with authorized members.
- [x] Observe RED for no multicast overlay join, reused legacy connection rejection, bidirectional traffic over recipient-initiated connection, wrong peer identity and unauthorized exchange.
- [x] Integrate listeners, route pruning/retry/reconnect and contact paging while preserving public AutoNAT/Identify filters.
- [x] Test invite/approve/file/chat and outsider denial using real authenticated loopback sockets with test-only route injection; distinguish this from real Tailscale evidence.

### Task 4: Desktop and UI

Files: modify `desktop.rs`, `src/main.tsx`, `scripts/invite-ui.test.cjs`.
Interfaces: typed refresh command; View exposes sanitized adapter readiness, workspace policy and overlay transfer paths; actions create overlay invite/connect links and change workspace policy.
- [x] Observe RED for enabled/not-installed/login status, correct invite action, reconnect link and unfinished transfer policy guard.
- [x] Add settings and workspace policy controls with explicit user selection, correct status/errors and Via Tailscale labels; no direct/DERP claim without evidence.
- [x] Run frontend tests/build plus desktop compilation.

### Task 5: Verification and operator guide

Files: add `docs/TAILSCALE.md`; update README, NETWORK_ACCEPTANCE and VERIFICATION.
- [x] Run frontend build/tests, full core/desktop tests, fmt, Clippy and whitespace checks; investigate failures.
- [x] Obtain fresh whole-change review and address important findings with regression tests.
- [x] Record actual physical same-tailnet/sharing/remap/DERP/large-file/owner-offline observations separately; leave unavailable gates NOT RUN with prerequisites.
- [x] Provide source/build location and concrete setup/test steps without publishing or installing automatically.

## Execution ledger

- Setup: managed isolated worktree `C:\Users\verak\.codex\worktrees\dump-tailscale\Dump`, branch `codex/tailscale`, baseline `05e1569`.
- Pre-flight: Task 1 node locator fields feed Tasks 2/3; contact contains node ID/FQDN rather than sender IP. Task 2 workspace policy feeds Tasks 3/4. Task 3 transfer path feeds Task 4 labels. Interfaces agreed before edits.
- Ruling: execute native integration with two isolated-file implementation delegates for adapter and signed envelope — no shared file mutations; root reviews and integrates their results.
- Phase 0 observed before implementation: installed Tailscale reports this Windows client and one visible Windows peer online; Tailscale ping returned DERP(sin), 63ms. This establishes Tailscale peer reachability only, not Dump sockets, cross-tailnet sharing or remapping.
- Toolchain: MSVC baseline could not link because `link.exe` is absent. Reused existing GNU compiler under `%LOCALAPPDATA%\Temp\dump-relay-gnu-20261110\mingw64\bin` with `cargo +stable-x86_64-pc-windows-gnu`; no firewall or system networking changes. Limited Cargo jobs after memory pressure.
- Task 1: adapter RED (7 intended failures) → GREEN 9/9 in isolated harness using actual module; Rustfmt and Clippy warnings denied PASS. Actual installed CLI returned Ready and receiver-local visible peer resolved to two reported IPs. Full project integration still pending.
- Task 3: route scope test on the actual production module observed failing at the implementation stub before implementation, using a reexport shim for the already compiled Multiaddr crate. Final production-library checks remain required.
- Task 4: frontend original baseline 9/9 PASS; six new cases observed RED → GREEN; frontend suite 15/15 and TypeScript/Vite build PASS. Native IPC/network integration remains pending.
- Ruling: inbound connections to a validated local Tailscale listener can originate from a shared recipient not present in the host's outbound peer map — require an overlay source address, libp2p authentication and workspace authorization; outbound dialing still resolves the signed node locator through the local peer map. This is required for asymmetric node sharing and does not grant workspace access.
- Ruling: suspend active-workspace polling and starting chat/file operations while joining another workspace; inbound strictness is retained if either the active workspace or pending join requires Tailscale. This prevents changing pending invitation type from exposing a strict workspace over a legacy connection.
- Fresh review: four actionable findings (cancelled join route escape, reverse-policy liveness, cold-start link loss, 128-member roster frame size). All entered one fix pass with observed failing regressions.
- Review fixes: capture each scheduled join's immutable route and runtime epoch; verify before stream open/write/approval. Close obsolete transport connections on context changes and allow dialing when only incompatible sockets exist. Defer consumed valid links through adapter initialization. Exchange bounded current-roster references after authenticated control exchange instead of full rosters in 8 KiB frames.
- Review fix verification: 28 overlay-focused tests PASS, one installed-client smoke explicitly ignored in that focused run; core all-target Clippy with warnings denied PASS. Cold-start tests observed RED then engine 10/10 GREEN; compact-exchange tests observed RED then 4/4 GREEN; stale join/reverse-policy/max-roster old-code regressions observed RED then GREEN in the 28-test run.
- Actual installed-client smoke: PASS, production adapter inspection and actual local Tailscale TCP/UDP listeners on port 42043, valid signed link and unchanged temporary-profile identity. This is a local adapter smoke, not a second-machine transfer.
- Earlier broad core run: 109 PASS, zero failures, three explicit ignored entry points/scenarios. It preceded the final review fixes and is not substituted for the final broad rerun. Frontend build and all 15 UI tests PASS on root verification.
- Final review ruling on external behavior: cross-tailnet locator stability, remapped addresses, grants/firewall enforcement, DERP file traffic and physical remote transfers remain manual acceptance gates; no source or fixture result is promoted to that evidence.

- Full-suite listener fixes: track listener ownership by ID so late close/expire events cannot erase replacement addresses; retry occupied fixed ports every four seconds without another CLI refresh. A real UDP blocker regression failed before the retry fix, then passed after release without settings/generation changes.
- QUIC logout/relogin regression exposed a retained fixed listener socket: Quinn streams can retain it after Swarm reports close. Strict overlay actor dials now allocate an ephemeral outbound port instead of reusing port 42043; Tailscale handles NAT. The unchanged 15-second reconnect observation passed in the focused overlay run (23 PASS, one ignored local-client smoke). Incoming listener failures still use bounded retry.
- Final broad attempt retained: 103 PASS / one failure / two ignored. The remaining failure was a legacy explicit-connect/background-dial race during owner restart. A deterministic bound-but-unanswered UDP regression reproduced the registration error. The API already stores the route; duplicate pending dials now acknowledge registration while preserving handshake/diagnostic checks and other errors. Final verification remains required below.
- A subsequent broad attempt passed 104 cases but still reproduced logout/listener rebind failure under full parallel load; the outbound-port change alone was insufficient. Added a deterministic assertion that a temporary logout retains the owned listener while both transport policies reject requests. Observed RED, then focused 23 PASS / one ignored after retaining validated bindings while enabled/unready, marking readiness false and closing/cancelling overlay connections. Explicit disable or newly validated IP changes retire bindings. This refines the design's listener lifecycle without permitting traffic during logout; the 15-second reconnect deadline is unchanged.
- Narrow follow-up safety inspection found the special LAN invitation handler lacked the strict transport guard used by all other app protocols. Added regressions for LAN creation and handling in a strict workspace, plus exclusion of all non-overlay sources from legacy handlers on the dedicated listener. These checks are part of the same fix pass; final broad validation must follow their fixes.
- Intermediate broad snapshot after the listener-retention fix: 124 PASS (105 library + 19 integration), zero failures, three explicit ignored scenarios/entry points. This precedes the additional LAN/dedicated-listener guards and is not the final source verification.

- Narrow safety regressions observed RED: strict workspace LAN invitation creation (one), valid-code LAN protocol admission (one), non-overlay source incorrectly receiving legacy handlers on the dedicated socket (one), and loss of pending outbound transport classification when status clears the peer map (one). Added strict checks to LAN creation/handler; legacy controls exclude every owned overlay listener even when its remote source is ineligible for overlay; transient suspension retains the last validated classification map while readiness blocks all operations/dials. Final source frozen for broad verification after these corrections.
- Final frozen application source core suite: 127 PASS (108 library + 19 integration), zero failures, three explicit ignores; all final LAN/listener/classification regressions GREEN. Frontend build + 15/15 UI tests and 1/1 publisher contract PASS. Native desktop tests, final Clippy and preview packaging follow without source edits.

- Final desktop library suite: 109 PASS, zero failures, two ignored; installed production-client/listener smoke explicitly run and PASS 1/1. The initial GNU unit binary failed before tests (STATUS_ENTRYPOINT_NOT_FOUND: comctl32 TaskDialogIndirect). Preserved that failure; added Common Controls v6 only to the generated unit binary's activation resource, then ran the complete suite. Product/test source unchanged. See VERIFICATION for the exact local workaround boundary.

- Final default-desktop all-target Clippy with warnings denied, Rustfmt and staged whitespace checks PASS. Local NSIS preview 0.1.4-alpha.1 built successfully (279,715,628 bytes), copied to `.tools/tailscale-preview/`, SHA-256 `c30ac9b553ff0983eb4d3f2cb6d819e77cb817586589059e615f99827b6a10a9`. Preview is unoptimized/unsigned; tracked versions/updater feed unchanged. App PE is Windows GUI, contains Common Controls v6, and controlled loader probe reached main then killed the owned process before setup/profile access. GNU manifest-merge and debugger warnings are retained in VERIFICATION; clean-machine app/installer/deep-link/physical tests remain NOT RUN. No install, push, release, firewall/tailnet or remote infrastructure changes.