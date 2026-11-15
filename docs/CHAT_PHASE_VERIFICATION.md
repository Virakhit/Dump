# Workspace chat implementation, 15 November 2026

Isolated branch: codex/restart-v0.1.1, HEAD/base ffb6c15. The user authorized
continuing chat implementation while exact native checks remain blocked. This
document describes local source, not a built/installed/released chat capability.

Subsequent invitation/route work is recorded in UNIFIED_INVITE_VERIFICATION.md;
the chat-phase checks below remain evidence for their original source snapshot.

## Behavior implemented in source

- Signed messages bind workspace, authenticated sender key/Peer ID, roster
  revision, monotonic sequence, UUID, timestamp and UTF-8 body (1–4096 bytes).
  Every receive requires the active authorized room and current signed roster;
  it rejects nonmembers, sender substitution, stale epochs and modified bodies.
- /dump/chat/2 is separate from unchanged /dump/control/1 and /dump/file/1.
  Later source already uses /dump/chat/1 with a different payload. These messages
  must not negotiate that incompatible protocol. An old peer can still transfer
  files but cannot confirm receipt of chat 2; the sender reports no confirmation.
- A message and its replay receipt are persisted together before Stored ACK.
  Exact duplicates do not append history. Conflicting sequences/UUIDs are denied;
  a separate durable 64-sequence window rejects replays after display pruning.
- History retains 500 messages per room / 2000 total. Sender sequences and replay
  maps are each capped at 1024 entries; reaching capacity refuses new state rather
  than discarding replay protection. Protected-state writes respect the existing
  32 MiB read ceiling before replacing a saved profile.
- Sending saves locally first, then contacts only currently online approved
  members. Four sends/four destinations per send, eight incoming chat streams and
  a ten-second per-peer exchange bound prevent unbounded fan-out. Existing general
  admission rate limits still apply. No offline queue or automatic resend exists.
- Workspace/membership changes or shutdown interrupt active fan-out. Authorization
  is checked again after opening each stream. Restart marks unfinished local sends
  Interrupted. Only a matching Stored message-ID reply increments acknowledgment.
  A terminal Complete means attempts finished; it never means everyone received.
- UI shows local save, number of contacted peers confirming storage, offline/not
  queued and interruption. Storage receipt never claims a human read the message.
  Text is escaped; drafts/errors/pending IPC are scoped per room; late save results
  preserve later edits and other-room drafts. Failed local saves retain the draft.

New persisted chat state has a default for legacy 0.1.1 loading. Existing identity,
signed roster, invitations and shares are not migrated to a new representation.
Do not open a real newer profile with this older-version development branch;
only explicit isolated stores have been used by the test code.

## Checks and limits

- Frontend red stage: all five initial chat tests failed before UI implementation.
  One later expectation had a case-sensitive Not/not mismatch; it was corrected
  without changing delivery behavior. A sixth chat case covers delayed save plus
  room switch/draft editing. Final UI suite: **13 passed** (7 invitation + 6 chat).
- TypeScript/Vite production frontend build: **passed**, 38 modules. This is not
  a desktop application build or an IPC/network integration test.
- Eight authored Rust chat tests cover signature/body bounds; replay/retention;
  room/auth/member denial; save failure without receipt/sequence consumption;
  restart/migration; offline/no-queue; genuine QUIC receiver persistence; and clock
  correction. They use temporary stores and, in two cases, loopback listeners.
  **No Rust test bodies executed.** These are test additions, not runtime passes.
- Exact `cargo test --offline --locked ... --test chat`: blocked at dependency
  resolution: original registry libp2p-stream is absent. Cargo.lock was unchanged;
  no substitute crate or new dependency was installed.
- A direct rustc check with existing GNU cached dependency artifacts **passed
  metadata compilation** of the current core library and chat/join_contract/lan
  integration sources. Reproducer: `.tools/check-cached-metadata.ps1`, ignored local
  tooling. It does not cover desktop feature/IPC, link, execution, or exact locked
  dependencies. Cached libp2p has a feature superset and cached libp2p-stream is
  from later patched work; this evidence cannot qualify the baseline Cargo build.
- Tried the same existing-artifact route with `-RunTests` to produce executables.
  Core rlib code generation succeeded, then the chat executable failed linking:
  AWS-LC references nanosleep64, absent from the available bundled GNU pthread
  runtime. No test ran and no stub/library was supplied to disguise the mismatch.
  Local log: `.tools/cached-test-run.log`. A historical MinGW path from its error
  no longer exists. Full MSVC VC tools are also absent; searches including ignored
  files found no complete alternative. No compiler/runtime was installed.
- Rust formatting, Node syntax, whitespace and existing publisher regression
  are checked separately; a passing frontend does not make aggregate check green.

Read-only code review found one concrete issue: applying current-clock freshness
to saved history could block profile opening after clock rollback. Source now
separates timeless signature/structure validation from incoming freshness. A
future-signed-history reopening test simulates this without changing OS time;
review confirmed the source fix. That regression still needs native execution.

## Files and next gates

Core: src-tauri/src/chat.rs, network_chat.rs; integration in engine/model/storage,
network and desktop. UI: src/chat.tsx and main.tsx/style.css. Tests: tests/chat.rs,
scripts/chat-ui.test.cjs and shared ui-harness.cjs; invitation tests now reuse that
harness. Package test script includes both UI suites. No new Rust dependencies.

Obtain an authorized complete native environment/cache, then run the normal
locked core, desktop/updater, cargo check and Clippy commands plus all new tests.
Use isolated profiles/app identifier/deep-link scope before any native GUI launch.
Two real Windows devices must prove membership, both-way chat, exact duplicate,
disconnect/no-queue and restart behavior. Tailscale and mixed host relay are still
future phases, not features exercised by these tests. No live LAN, overlay, host
relay, installed app, main branch, firewall/security setting, WisePOS service,
commit/push/release/deployment or real user profile was changed.
