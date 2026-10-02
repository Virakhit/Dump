# Security model

Dump v0.1 assumes untrusted networks, nonmember peers, and eventually untrusted relays. Authorized members are trusted to receive shared files: they can redistribute downloaded copies. Compromised Windows accounts/OS or stolen private keys are outside the guarantee.

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

## Reporting

Report suspected vulnerabilities privately through [GitHub security advisories](https://github.com/Virakhit/Dump/security/advisories/new). Private vulnerability reporting is enabled for this repository. Do not publish real invite tokens, private keys, workspace secrets, user files, local paths, or personal network captures in an issue or report.
