# Dump v0.1 delivery plan

## Product

English Windows LAN desktop for friends/general users. Core loop: create workspace, invite, compare fingerprint, approve device, share, receive, verify. Workspaces have a single creator; existing members transfer while that creator is offline. Member removal is eventual on receipt of signed membership, not instant during partitions.

Original requirements: `requirement.md`. Decisions from the planning conversation and the requested v0.2 Internet milestone are preserved here.

## Architecture decisions

- Tauri 2, React/TypeScript/Vite/CSS, Rust/Tokio/libp2p, authenticated QUIC and mDNS. Reuse transports, signatures, Windows DPAPI and filesystem primitives rather than custom encryption/authentication infrastructure.
- One process and versioned protected local state; no backend, database, generic shell commands, or global public discovery.
- Pin `libp2p-stream` alpha with a lockfile; prove streaming/identity compatibility in executable integration tests before relying on it.
- Source is MIT licensed. Distribute the installer as an explicitly labelled alpha prerelease for testing; a validated beta/stable release still requires the physical-machine, installation and pilot gates.
- SHA-256 and owner-signed manifests; streamed `.part` downloads and verified no-overwrite completion.
- Membership snapshots use a pinned owner key and monotonic revisions. No owner transfer or key recovery workflow in v0.1.
- At most one workspace active. Share refreshes are bound to the original workspace/version and cannot cross groups.
- User-requested 0.1.1 extension: manually check GitHub for newer published releases and install signed updates in the app. A public HTTPS channel manifest and the native Tauri updater bind installer signatures to their app version; LAN discovery and file transfers retain their original scope.

## Milestones

1. Technical proof: two physical Windows machines discover/authenticate and stream one verified file; outsider rejected.
2. Workspace: invite, fingerprint comparison, approval, restart persistence, eventual member removal.
3. File loop: English UI, signed catalogs, streaming, progress, cancellation, source-change handling.
4. Hardening: hostile names, replay/conflicting roster, queue/concurrency bounds, disk errors and cleanup.
5. Packaged beta: NSIS with offline WebView2, clean-machine installation and deep-link testing.
6. Pilot: five prospective users; target four completing the loop unassisted and two repeating a real transfer within two weeks.

Initial estimate was 5–7 full-time developer weeks including ~20% buffer. This is an estimate, not a release promise; re-estimate using measured progress and real-machine findings.

## Remaining external acceptance gates

- Real Windows machines with Ethernet/Wi-Fi/mDNS, firewall permission, guest-network isolation, and owner closing.
- Three-device removal propagation including offline/reconnect and active transfer cancellation.
- Real 8 GiB transfer and memory observations on target machines.
- Clean Windows NSIS install/uninstall, WebView2 installation without Internet, and deep links with app open/closed.
- Packet capture confirming no plaintext protected metadata outside authorization, and log review.
- Five-user pilot. Nobody has been contacted or enrolled by the implementation.

Complete these before calling the build a validated beta. Source builds and loopback tests are distinct evidence.

The local implementation has passed a separate 8 GiB loopback test; target-machine memory observations and multi-machine behavior remain unverified. See `VERIFICATION.md` for executed checks.

## v0.2 Internet milestone

The implementation proceeds in small stages; the published 0.1.1 installer remains the LAN alpha. Priority: security, correctness, interoperability, UX, then optimization. The protocol's contact decision is in `PROTOCOL.md`; no mandatory backend, global workspace discovery, accounts, storage, telemetry or UI rewrite.

| Stage | Work / files | Status |
|---|---|---|
| 1 | Identify, bounded IP/QUIC address registration and expected-identity dialing, candidate observations and diagnostics. `connectivity.rs`, `network.rs`, Cargo feature/lock, unit and network tests. | Implemented; verification recorded in `VERIFICATION.md`. Core API only; no Internet invitation UI yet. |
| 2 | AutoNAT v2 reachability verification, confirmed-address expiry, public address advertisement and no external requirement for LAN. `reachability.rs`, connectivity/network integration. | Implemented; verification recorded in `VERIFICATION.md`. Bounded candidate cache resets after all connections close; re-probing during long-lived sessions remains a Stage 6 integration item. |
| 3 | Configurable relay client core API, native reservation renewal/failure, E2E Noise + Yamux circuits and authenticated TCP fallback. Separate direct/circuit stream controls preserve active transfers while preferring direct. Existing protocols and authorization regression tests. | Implemented; verification recorded in `VERIFICATION.md`. No built-in relay, hosting role, settings UI or automatic invitation locator yet. |
| 4 | DCUtR direct upgrade and routing preference, retain relay fallback without breaking active transfers. | Implemented; public-only candidates and bounded negotiations. Local DCUtR upgrade and failed-upgrade circuit fallback tested. Real NAT behavior remains an external gate. |
| 5 | Opt-in relay peer role, reservation/circuit/bandwidth/bytes/rate ceilings and self-host instructions. Settings persisted with defaults; desktop typed IPC and small UI checkbox. | Implemented and locally tested; applies after restart. Public NAT/firewall hosting remains an external gate. |
| 6 | Bounded signed contact records, invitation parsing/creation, authorized member contact exchange and reconnection. Preserve v1 identities/rosters; minimal direct/relay/connecting/offline UI states. | Pending |

After every stage: core Rust tests, desktop tests/build checks, frontend build, Rustfmt and configured Clippy with warnings denied. Record failures and actual evidence; do not promote candidates or describe planned features as shipped.

Final local regression matrix: LAN/mDNS; explicit direct address; existing protocols through relay; relay identity cannot grant membership; corrupted data fails before commit; wrong Peer ID; unauthorized catalog; unauthorized download; relay loss cleans partial transfer; direct preference; DCUtR upgrade where testable; relay resource ceilings. No public network service in these tests. Loopback test routes simulate explicit Internet address dialing; they do not establish real NAT traversal success. Separate physical cross-network/firewall/CGNAT checks remain necessary before distribution claims.

Resume, folders, chat, previews, telemetry, permanent hosting, WebRTC, Tor, blockchain and public file search remain outside this milestone. Never promise universal NAT/CGNAT connectivity or treat a forwarding relay as file storage.
