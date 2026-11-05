# Invitation regression tests

Run the frontend and invitation UI checks:

```sh
npm ci
npm run build
npm run test:ui
```

The UI suite renders the real React components in JSDOM, mocking the Tauri
boundary. It covers request notices on Files, Chat and the invitation dialog,
the Members shortcut, active-owner filtering, fingerprint confirmation and
truthful progress before an authenticated owner acknowledgment.

On Windows, test independent owner/invitee processes with the real core:

```sh
cargo test --manifest-path src-tauri/Cargo.toml --no-default-features --locked --lib multiprocess_ -- --nocapture
```

Seven cases cover full invitations over QUIC and TCP/Noise, short LAN
invitations, wrong selected workspace and recovery, cancellation before a late
grant, explicit rejection, and owner restart before approval. Each uses fresh
identities, separate temporary protected stores and real loopback sockets.
Successful joins are reloaded by a fresh process to check persistence. Parent
tests serialize children to bound resource use. The ignored `peer_worker`
entry point is invoked by those parents, not a skipped acceptance scenario.
The normal Windows library suite includes the parent tests automatically.

LAN discovery is injected by the private test harness. The restart case also
supplies the new listener route; it does not exercise multicast rediscovery.
Its separate 75-second observation budget allows existing connection expiry
and per-route retry timers. Recorded local reconnect observations took about
16 and 28 seconds; no immediate reconnect guarantee is implied. Short LAN
invitations remain memory-only and do not survive owner restart. Cancellation
does not withdraw the old request from the owner.

All process harness code is Windows `cfg(test)` only. It does not change the
desktop application's profile, transports, permissions or approval flow.
These tests do not validate native WebView/Tauri interaction, installer
installation, OS deep links, firewall policy, multicast or two physical LAN
machines. Physical owner/invitee acceptance remains required.
