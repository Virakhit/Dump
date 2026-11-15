# Unified LAN/Tailscale invitation: 15 November 2026

Local source only, codex/restart-v0.1.1 at base ffb6c15; not a release or physical
network acceptance result. The chat changes and original checkout are preserved.

## Added source

- A signed dump://connect envelope holds the unchanged v1 Invitation plus an
  owner-signed expiring route record. Old dump://join full links retain their
  single-device behavior. Both formats enter the same existing Join/Waiting/
  Approved control exchange; no v1 control/file enum or signed-roster format changed.
- New shared invitations require separate explicit approval for every authenticated
  device. Per-peer rejection cannot grant another peer access; explicit revoke
  invalidates the whole current link and its pending requests. Token, owner,
  workspace, expiry, peer fingerprint and current signed membership checks remain.
- Read-only local Tailscale status must report Running/Self.Online. Only self IPs
  matching actual current QUIC listeners are exportable. RFC1918 LAN addresses,
  loopback, public IPs, DNS and TCP/circuits are rejected by the exported route
  schema. Overlay IPs are sensitive too: they appear only in the intentionally
  shared invitation or authenticated current-workspace member route exchange;
  they are never announced to a public directory/service. LAN routes use mDNS.
- /dump/routes/1 exchanges member-signed, scoped records after admission, with
  per-page membership checks, identity pinning, sequence/conflict checks, bounded
  caches/pages and expiry. Thus approved peers can learn overlay routes without
  mutual mDNS. This is source behavior, not proven live member discovery.
- Candidate rounds prefer learned LAN routes, then signed overlay routes, with
  actual connection tracking, at most two pending per peer/four globally and
  six-second retry rounds. Existing QUIC handshake timeout is five seconds in the
  cached matching libp2p-quic source. Rotation pauses when capacity is zero, and
  the invitation owner gets priority. Established connections are not forcibly
  replaced. Route exchange has a ten-second aggregate cap before catalog refresh.
- Active invite owner is excluded from concurrent catalog refresh so that an old
  selected workspace cannot repeatedly occupy its join polling slot. Waiting ACK
  only advances the exact current owner/workspace/token attempt. Late approval/
  denial from replaced attempts cannot select or cancel a different invitation.
- Watched listeners are deduplicated and withdrawn on expiry/closure; expired
  mDNS entries are removed. A new desktop invitation requires a live listener.
  Route records and link addresses can still become unreachable later; signatures
  authenticate claims, not reachability. Owner restart/port or overlay-IP changes
  can require a fresh link if no surviving route can bootstrap updated records.
- UI distinguishes finding/trying routes from an actual owner receipt, displays
  the invitation owner's fingerprint for trusted comparison, labels multi-device
  approval/revocation correctly and makes no Tailscale-connected claim on schedule.

New state fields are additive/defaulted. Timeless signature validation on load
does not make an expired route/invite block opening a profile. Live use checks
expiry. Route cache is bounded to 1024 records; shared/legacy invitation count to
128 and approval/rejection sets to 128. No cleartext file storage or relay exists.

## Executed checks and limitations

Actual read-only CLI probe found tailscale.exe, but status --json exited **1**;
no positive Running/reachability evidence was obtained. It may be a daemon or
access/environment limitation; no settings were changed. No private peer/IP
inventory was printed. The source consequently must not invent overlay endpoints.

UI tests: two new tests failed before UI wiring, then **15 total passed** (9 join,
6 chat). TypeScript/Vite frontend build passed (38 modules). These mock Tauri;
native IPC, deep links and desktop behavior are not established.

Final aggregate npm run check (Cargo on PATH, offline) exited **1**: frontend
build and all 15 UI tests passed, then original libp2p-stream resolution failed.
The result is not an all-green check. Publisher regression passed (one case),
formatting/whitespace passed, and Cargo manifests/lock, Tauri config and local
updater feed have no diff from the base. Existing native linker failures remain.

Four new invite/route security tests and four scheduler/status tests were added,
covering shared approval, per-peer rejection/revoke, owner/address pinning,
expiry/tampering, membership/room scope, equal sequence conflict, bounded fallback,
fairness and ready-self-only Tailscale extraction. The fairness regression models
three peers, two candidates, four slots, five-second pending expiry, two-second
ticks and six-second retries. **None executed natively.**

Current core plus chat/join_contract/lan/invite_routes/route_selection sources pass
direct rustc metadata checks against existing cached GNU artifacts. This is not
an exact Cargo.lock build or test run; cached stream is from later patched work.
The standard offline locked test command cannot resolve original libp2p-stream.
A further attempt to link only route_selection with cached dependencies still
failed on AWS-LC/pthread nanosleep64; it did not reach test execution. No compiler,
library, shim, dependency install or security setting change was made. Full
desktop/updater/Clippy and real two-/three-device gates remain outstanding.

Read-only review found dial starvation and stale listener exports. The first
rotation revision still aliased zero-capacity ticks; source now advances rotation
only with available slots, and the corrected timing regression is included. Review
confirmed those two source fixes. Listener lifecycle and actual network fallback
still need executable coverage; metadata checks cannot establish runtime timing.

## Handoff

See HOST_RELAY_CONTRACT.md for the next phase's owner-signed locator, separate
relay identity, workspace/generation-bound admission, quotas, end-peer encryption,
backpressure, file integrity and host/source online requirements. It is a contract,
not a working relay implementation. See UPDATER_REBUILD_GATE.md before packaging
or native GUI testing; the old public 0.1.3 feed is newer than this 0.1.1 source.

Next gates: coordinated complete native environment and pinned dependency cache;
fresh isolated app/profile/deep-link scope plus updater policy; two actual Windows
LAN devices, two authorized Tailscale endpoints, then the three-device mixed relay
scenario after relay implementation. No app installation, downgrade, main checkout
change, commit/push, deploy/release, firewall/ACL change or real-profile use occurred.
