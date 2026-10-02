# Security model

Dump v0.1 assumes untrusted networks, nonmember peers, and untrusted relays. Authorized members are trusted to receive shared files: they can redistribute downloaded copies. Compromised Windows accounts/OS or stolen private keys are outside the guarantee.

## Trust boundaries

Bundled UI ↔ Rust IPC; Rust ↔ protected local state/files; device ↔ LAN peers; workspace creator ↔ members; authorized ↔ unauthorized protocol requests.

The only frontend commands are typed actions, state reads, native file picking, native drop paths, and native destination picking. Rust validates drop paths and their intended workspace; paths are never advertised to peers. UI inputs and peer filenames are untrusted. Frontend strings render as text; CSP limits external scripts/connections. No automatic opening of received files.

## Threat priorities

DREAD numbers are pre-mitigation design estimates, not measured attack probabilities. Owner of every high-priority mitigation: Dump developer.

| Threat / STRIDE | DREAD average | Control and regression evidence required |
|---|---:|---|
| Impersonation/stolen invite — S/E | 7.8 | QUIC identity, owner pin, trusted-channel fingerprint comparison, identity-bound approval |
| Trust-root substitution/roster replay — T/E | 8.2 | Fixed creator key, signed scoped revisions, atomic persistence, reject rollback/equal-revision conflict |
| UI/IPC or plaintext keys — T/I/E | 8.0 | Text rendering, narrow commands, CSP, Windows user DPAPI; no plaintext secrets in state/logs |
| Metadata disclosure/queue abuse — I/D | 8.2 | Minimal mDNS, uniform denial, frame/page/time/rate/concurrency bounds, separate control admission |
| Changed source/tampered transfer — S/T/I | 8.0 | Read-only source handles, immutable manifests, sender and receiver SHA-256/length verification |
| Traversal/overwrite/disk exhaustion — T/D/E | 8.2 | Windows basename validation, exclusive partial creation, no-replace commit, cancellation/error cleanup |
| Disputed membership actions — R | 5.4 | Signed membership state and bounded local audit records, without keys/tokens/paths |

Source paths and preparing work retain their workspace/file generation. Switching groups, cancelling, unsharing, and receiving revocation must never publish a file into a new group or restore a stopped share.

## Checks before distribution

Run core/integration tests, desktop build, dependency review, source secret scan, and real-machine gates from `docs/PLAN.md`. A clean scan of requirements or static review alone does not establish application security. Scan authored source separately from generated `target/` and `node_modules/`; dependency lockfile integrity hashes can be scanner false positives and still require review.

Protect the Windows profile: there is no v0.1 key recovery/export. Losing the owner identity prevents future member administration; existing members with intact identities and the last accepted roster can still transfer until they choose to stop.

## App updates

Update checks are explicitly requested by the user and fetch a public HTTPS manifest from this repository's `main` branch. Installers come from GitHub Releases. The native Tauri updater verifies the pinned public key and requires the signed version to match the manifest before installation; it rejects older/equal versions. The UI cannot provide an arbitrary update URL or key through IPC. Private signing keys stay outside Git and in repository Actions secrets. Updates wait for file preparation and active transfers to finish, then restart the application using the current-user NSIS installer. This signature does not supply a Windows Authenticode publisher certificate. LAN file sharing remains independent of update checks.

## Internet milestone boundaries

Stage 1 adds Identify and explicit QUIC dialing to the core. Identify reveals the public identity, implementation/protocol versions, supported protocols and the remote's observed network address. Interface listen addresses are hidden; automatic caching of remote addresses is disabled. The bounded application address book accepts only validated literal-IP QUIC addresses; private/local remote hints are not learned from Identify. Public observations remain unverified diagnostics and are not advertised. An authenticated transport connection grants no workspace access: catalog and file requests still require owner-signed membership on every request. Diagnostics do not place outsiders in the authorized online member list. No persistent state/signature schema migration is introduced.

Circuit transport and DCUtR are implemented in source; real NAT behavior remains unverified. Their threat model includes malicious forwarding peers that can observe IPs, public Peer IDs, circuit relationships, timing and approximate volume, and can drop/delay/interrupt delivery. The circuit authenticates and encrypts between the actual endpoints, independently of encrypted relay hops. It must preserve roster, manifest and SHA-256 checks, bounded resource use, opt-in hosting and zero relay payload storage. Signed contact locators must not imply authorization or reachability. Dump makes no anonymity claim. See `docs/PROTOCOL.md` for the contact decision and exact implemented/planned boundaries.

Stage 2 adds a client for AutoNAT v2, with a filter and fixed bound before its internal candidate cache. Only validated global-IP QUIC candidates can be submitted for probing. Confirmation requires an inbound nonce-bearing dial-back; confirmations are withdrawn after ten minutes or loss of the confirming server. A malicious probe server can still provide misleading availability information or deny service; the confirmation is no authorization guarantee and a later dial still authenticates the expected end peer. Only already connected peers can act as probe servers; LAN use never requires an Internet probe or a built-in public service. The shipping app has no AutoNAT server role yet. Probe traffic exposes the candidate address and public identity, never workspace data. Inconclusive tests stay unknown; a failed route is not proof of all NAT behavior. The current client resets its bounded completed-candidate cache after all connections close, so expired addresses may wait for reconnection before being advertised again.

### Circuit transport

Stage 3 enables a configurable relay client with no default relay; Stage 5 adds separately limited opt-in hosting. Circuit transport has a separate authenticated Noise session between the actual peers; endpoint-to-relay encryption is insufficient on its own. Relay identity and destination identity are validated separately. No address, relay connection or workspace secret bypasses the owner-signed membership checks. Both direct and relayed file completion retain manifest, byte-count and SHA-256 validation, cancellation cleanup and exclusive final commit. Private circuit addresses confirmed by libp2p are filtered out of Identify advertisements. New streams prefer direct without terminating an in-flight circuit transfer. The forwarding peer can still observe identities, circuit relationships, IPs, timing and approximate volume, deny delivery or exhaust its own quotas. The published LAN installer predates hosting; in source assistance is off by default and takes effect after restart. A separate forwarding swarm has no payload/disk API and enforces circuit, reservation, duration, byte, connection and rate ceilings plus a shared application-byte bandwidth bucket. Settings contain network locators only and grant no workspace access. Runtime protocol, authorization, relay-loss and direct-takeover tests passed locally, with no public relay or packet-capture proof of traffic confidentiality.


### Contact records and routing

Stage 6 adds short-lived Ed25519-signed public-IP routing hints through private invitations and `/dump/contact/1`. Records contain no workspace fields; they bind their public key to their Peer ID, sequence, expiry and bounded routes. Invitation parsing pins its record to the owner. The separate contact protocol authorizes every request before returning member-scoped records; it cannot turn an address, invitation token or signed contact into membership. Caches reject unsigned/changed records, private/DNS routes and sequence conflicts. Existing state loads new fields with safe defaults without changing identities or signed v1 payloads.

An unreachable or malicious endpoint may still cause delays or denial of service. Dial attempts, frame sizes/timeouts, connection counts, cached contacts and DCUtR negotiations are bounded. The normal app entry point always rejects loopback Internet contacts; isolated unit simulations enable a private test-only route exception. Local tests prove protocol/authentication/integrity behavior, including deliberately incorrect bytes from an approved provider and rejection of nonmember contact requests. They do not prove real NAT traversal or packet-capture confidentiality. No accounts, central directory, public workspace discovery or telemetry are introduced.

## Reporting

Report suspected vulnerabilities privately through [GitHub security advisories](https://github.com/Virakhit/Dump/security/advisories/new). Private vulnerability reporting is enabled for this repository. Do not publish real invite tokens, private keys, workspace secrets, user files, local paths, or personal network captures in an issue or report.
