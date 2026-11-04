# Local group chat implementation plan

**Goal:** Add usable local group chat to existing Dump workspaces.

**Architecture:** Signed, bounded messages travel over a separate authenticated
libp2p stream, with membership checks on both ends. History and delivery status
are stored in the existing protected state and streamed to the React interface.

**Tech stack:** Existing Rust, Tauri, libp2p, React and TypeScript; no new runtime dependency.

**Spec:** `docs/superpowers/specs/2026-11-04-local-group-chat-design.md`.

## Global constraints

- Preserve existing workspace identity, file transfers and protocol versions.
- 4,096 UTF-8 bytes per message, 500 messages per workspace, 2,000 per device.
- All history is local; acknowledgments are durable-device receipts, not read receipts.
- Work on `feat/local-group-chat`; no main-branch merge, release or deployment.

## Review focus

- An old composer or pending send must not send into a newly selected workspace.
- Removed or forged senders must not append messages, including replayed frames.
- A failed disk write must not create an acknowledged message or lose the draft.
- Partial/offline delivery and restart interruption must be visible without implying a read receipt.
- Thai/IME input, multiline text, and HTML-like text must behave as ordinary text.

## Tasks

1. [x] Model and persistence: signed `ChatMessage`, local `StoredChat` and typed
   delivery state, backward-compatible persisted history, bounds/deduplication,
   engine compose/receive/finish operations and tests.
2. [x] Transport and desktop: `/dump/chat/1`, bounded incoming/fan-out, explicit
   workspace send command, durable receipt validation, cancellation/revalidation,
   and real QUIC integration tests.
3. [x] Interface: Group chat tab, scoped drafts, sender/time/history, UTF-8 limit,
   IME-safe Enter/Shift+Enter handling, per-message delivery feedback, existing style.
4. [x] Documentation and verification: local development/use instructions,
   build/test evidence, focused independent review, and a reviewable feature branch.
   The recorded evidence distinguishes passing chat checks from relay
   test timeouts and leaves physical LAN/native-window acceptance explicit.

## Environment

The editing environment is Linux with Node.js available, but without Cargo or
Windows DPAPI. Frontend checks can run locally. Attempt the repository's existing
Windows CI for Rust/desktop checks, and report actual evidence and any blocked checks.
