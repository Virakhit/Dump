# Physical-network acceptance plan

**All eight scenarios: NOT RUN.** This is a manual plan, not new verification evidence. The local Windows development checkout is available. The current device inventory contains only this same desktop, reported Offline by the remote-device connector; no second/third authorized Windows machine, reachable owned relay, or controlled remote network has been supplied. Loopback tests and CI do not satisfy these gates.

Source baseline when this plan was prepared: `c92f6c4614d24c4a8e8edea0bff642262f521e36`. At execution, use the same current source revision on every device and record the actual revision/build below. The published LAN-only 0.1.1 installer cannot test this Internet implementation.

## Equipment and boundaries

- A and B: two physical Windows devices, approved test profiles and networks, source-build prerequisites from [DEVELOPMENT.md](DEVELOPMENT.md), and space for source plus received files. Scenario 8 also needs a third member/provider C to observe propagated revocation.
- Two genuinely different networks for scenarios 2–7. Record the network path; two apps on one machine or two devices behind one router are not a cross-network test.
- R: an existing, authorized, reachable relay. It may be B in scenario 5. Use only its actual pinned **Relay Peer ID** and already approved reachable ports. No public third-party relay, new infrastructure, firewall/router changes, release, installer or version change is part of this plan.
- Scenario 4 needs an existing approved UDP-blocked network with a usable TCP route. A positive AutoNAT re-probe observation in scenario 7 additionally needs an owned/explicitly authorized compatible AutoNAT v2 server; Dump's forwarding host is not a probe server.
- Use synthetic, nonconfidential files and a new output folder per attempt. Keep invitations, keys, workspace secrets, full rosters/contact records and file contents out of evidence. Preserve existing device identities; do not copy DPAPI state between Windows users or silently replace remote relay pins.

Run each desktop from its source checkout using `npm ci` and `npm run tauri dev` as described in the development guide. These are operator steps, not commands executed by this plan. Do not build an installer or enable hosting unless that scenario uses an approved host. If migrating an older source profile, confirm its Device Peer ID is unchanged, its separate Relay Peer ID persists after restart, and previous hosting remains off; explicitly refresh old hosted locators/invites before use.

## Record before every attempt

| Field | Required evidence |
|---|---|
| Case/attempt/status/time | Scenario number, unique attempt, `NOT RUN` / `PASS` / `FAIL` / `IN PROGRESS` / `INCONCLUSIVE`, start/end with timezone; missing prerequisites remain NOT RUN with the reason |
| Each device/build | A/B/C/R role, Windows version, `git rev-parse HEAD`, `git status --short`, matching source-snapshot SHA-256 from the stability runner, actual running executable SHA-256, relevant toolchain versions; executable hashes may differ across machines |
| Identity | Device Peer ID/fingerprint compared through a trusted channel; host Relay Peer ID separately; no private key/state dump |
| Topology/settings | Which networks/router paths each role uses, transport addresses supplied, hosting opt-in, configured relay pins, existing UDP/TCP policy and reservation readiness |
| Observed NAT | Router/provider/authorized endpoint observations and their source, or `Unknown`; do not infer CGNAT/symmetric NAT from a failed connection or private LAN address |
| Route evidence | Actual direct or relayed route captured for this transfer, expected endpoint identity, and transport/relay identity when observable; a supplied locator or transport connection alone is insufficient |
| File | Synthetic filename, source byte count and SHA-256, receiver byte count and SHA-256, final/partial-file outcome; never include bytes |
| Steps/diagnostics | Reproducible ordered actions with elapsed stages; bounded connection/route/protocol outcome, reservation, join/transfer state, error and membership revision number when available |

For local fingerprints/hashes, operators can use `Get-FileHash -Algorithm SHA256 -LiteralPath <path>` and `(Get-Item -LiteralPath <file>).Length`. Record the executable actually running, not an old installed copy. Use UI state and approved targeted connection diagnostics; record unavailable fields as `NOT OBSERVED`. Do not invent transport, reservation, quota or membership-revision evidence from a generic Online label. Share only sanitized summaries, not full state payloads or unrelated packet captures.

## Common successful file loop

1. B creates/selects the workspace and gives A a fresh invitation through the approved private channel. For relay use, wait for a real reservation and ensure the invitation contains B's current circuit locator.
2. A requests to join. B compares A's device fingerprint and explicitly approves it. A must receive the signed membership before catalog/file access. A relay connection alone grants nothing.
3. B shares the synthetic file. A must see its signed manifest, choose its unique output folder and receive it. Record the transfer's actual route, not just the current peer route after an upgrade.
4. A must reach Completed with the exact source byte count/SHA-256 and no owned `.part` left. Repeating into the same destination must fail without overwrite. Record a failure at its actual stage rather than restarting silently.

## Eight scenarios

| # | Scenario | Status | Required equipment |
|---|---|---|---|
| 1 | LAN without Internet | NOT RUN | A/B on the same approved LAN with no external service |
| 2 | Different networks, direct | NOT RUN | A/B on different networks, reachable direct member route |
| 3 | Different networks, relay | NOT RUN | A/B on different networks, reachable authorized R |
| 4 | UDP blocked, TCP fallback or truthful failure | NOT RUN | Existing approved blocked-UDP network and controlled TCP endpoint |
| 5 | B is both relay host and workspace owner | NOT RUN | A/B on different networks; B has an approved reachable host route |
| 6 | Relay stops during receive | NOT RUN | A/B/R, actual active circuit transfer and relay operator |
| 7 | Long session, address change and reconnect | NOT RUN | A/B, approved alternate network; probe server for positive confirmation subcase |
| 8 | Member revoked during transfer | NOT RUN | Owner B, provider C and receiving member A |

### 1. LAN without Internet

Use an already offline LAN; leave relay configuration/hosting off and supply no Internet locator. Start both desktops, run the common invite/approval/file loop through mDNS, and record direct application/file success. The owner must receive the join request without a public service. Unknown public reachability is valid; it must not stop LAN sharing. If discovery fails, preserve listener/connection/join evidence and mark FAIL without altering firewall policy.

### 2. Different networks, direct

Use an approved reachable public member route tied to B's **Device Peer ID**, with no relay involved in the receive. Run the common loop. PASS requires the expected authenticated B application endpoint, authorized catalog and an actual direct verified file; connection success or a public-address hint is insufficient. Record the observed transport. If the available UI cannot supply/confirm a direct locator for this topology, record that prerequisite as NOT RUN with its reason; do not edit protected state or use a test-only route exception.

### 3. Different networks, relay

Configure R's trusted public locator/pinned Relay Peer ID, restart as required, and wait for B's reservation before creating a fresh invite. Run the common loop. PASS requires an actual encrypted circuit transfer to B's Device Peer ID, manual approval, signed catalog and verified file. Record reservation/circuit and transfer-route evidence. If a direct upgrade occurs, record it; a file that actually used direct does not count as this relay-transfer gate. Unknown remaining relay quota is expected.

### 4. UDP blocked, TCP fallback or truthful failure

Use the supplied network's existing UDP restriction; do not add firewall rules. Configure the authorized endpoint's TCP locator (the relay host uses TCP 42042) and its correct Peer ID, then run the common loop. Record whether TCP actually carried the circuit/direct application exchange and verified file. A correctly reported inability to connect is acceptable failure handling, but does not establish working TCP fallback. A hang, false Connected/Completed state, wrong endpoint acceptance, hidden retry loop or partial commit is FAIL. Missing TCP transport evidence is NOT OBSERVED, not PASS for fallback.

### 5. B is both relay host and workspace owner

B explicitly enables its host using its separate Relay Peer ID R and keeps the owner Device Peer ID B unchanged. Confirm both IDs and their persistence after restart. B's app may reserve its co-hosted relay using R; a distributed circuit names R and destination B.

Run two separate attempts: **relay-first**, where A establishes B's forwarding-service reservation before opening B's invitation; and **app-first**, where A completes the app join/approval before connecting to B's relay service. In each, require authorized catalog and a verified file, with the actual route recorded. The forwarding connection must not substitute for `/dump/control/1` or `/dump/file/1`; R must not appear as a member merely because it hosts the relay. Use a fresh controlled attempt for each order and preserve every failure.

Record the observed connection order within one desktop session. Network setting changes apply only after restart; restarting after an app join does not establish the same-session app-first gate. If the available operator controls cannot arrange and observe an order, mark that subcase NOT RUN with its reason. Do not edit protected state or imply that the UI exposes the local regression's `Node` controls.

### 6. Relay stops during receive

Start a synthetic file large enough to observe real receiving progress (for example 128 MiB, subject to available disk/budget) and confirm this transfer is actually relayed. After A has written some bytes but before completion, the authorized relay operator closes R. Record the exact progress/time and ensure A terminates as Failed/Cancelled with actionable relay-interruption wording. No incomplete final file may exist; an existing destination must remain intact. Inspect the attempt's output folder for owned partials and, if cleanup fails, record the storage/cleanup error and later startup cleanup result. Never delete or sweep unrecorded files. Observe for another 60 seconds: no automatic receive restart/new transfer should occur. A relay reset does not by itself prove quota exhaustion.

### 7. Long session, address change and reconnect

Keep both desktops and an authenticated application connection open for at least 15 minutes. With an authorized AutoNAT v2 server, record bounded repeat confirmation/withdrawal evidence across the five-minute retry and ten-minute TTL; if a transfer overlaps, require it to remain active and later verify its bytes/hash. Without such a server, record Unknown and confirm LAN/known routes remain usable; the positive re-probe subcase remains NOT RUN.

Move one test device between the supplied approved networks or perform their ordinary disconnect/reconnect while leaving the desktop session open. Record the actual address change, stale route withdrawal, offline/connecting state and eventual fresh authenticated application route. Run a new verified receive after recovery. A fresh invite may be required when the old locator is unreachable; record that manual step. A broken transfer must terminate/clean safely and must not silently resume. Do not count old advertisements, remote success claims or a cached catalog alone as reconfirmation.

### 8. Member revoked during transfer

Approve A and C in B's workspace; C shares a sufficiently long synthetic file, and A starts a verified-protocol receive from C. While receiving is active, B removes A. Record B's signed revision number and when C accepts that newer genuine revision (bounded revision evidence only). Once C applies it, its authorization must stop A's active transfer and reject new catalog/file access; no incomplete file may commit on A. Check cleanup and absence of automatic retry. If the file completed before revocation reached C, record the ordering and repeat as a separate attempt; completed copies cannot be recalled.

If C is isolated from B and still has an older signed roster, continued old permission before the update is an eventual-revocation limitation, not proof of a bypass. Record the partition, revision arrival and enforcement time; there is no universal revocation deadline during isolation. Missing revision-arrival evidence leaves that enforcement checkpoint NOT OBSERVED. A two-device owner/provider test can be recorded separately but cannot replace the third-device propagation gate.

## Results and missing equipment

No physical-network attempt has been executed by creating this document. All scenario statuses remain NOT RUN. Supply the authorized device/network/relay inventory and operator availability before execution; no public or third-party endpoints are assumed. After each attempt, keep its sanitized record and add actual outcomes to [VERIFICATION.md](VERIFICATION.md), separately from historical loopback/CI evidence. A PASS applies only to that recorded topology, route and build; it is not universal NAT support or release acceptance.
