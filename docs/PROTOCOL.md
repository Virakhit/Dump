# Dump protocol v1 with Internet connectivity

## Identity and trust

Each installation keeps its original libp2p Ed25519 device/member identity and a separate persisted Ed25519 relay-host identity. Their public keys derive the Device Peer ID and Relay Peer ID; both private keys are protected by Windows user-scoped DPAPI. The device key signs existing rosters, manifests and contacts. The forwarding swarm uses only the relay key. QUIC/TLS 1.3 or TCP/Noise authenticates the expected service endpoint's Peer ID; circuits authenticate the actual member endpoint through an additional end-peer Noise session. Display names and relay identities never grant membership.

Every workspace pins its creator's Peer ID and public key. Only that identity signs membership revisions. A workspace ID can never be rebound to a different creator through a join link or network message. The latest locally persisted signed roster determines access; a shared workspace secret alone grants no access. Workspace secrets are supplementary protected state in v0.1 and are not group-wide passwords or independent encryption keys.

## Discovery

libp2p mDNS advertises multiaddresses and Peer IDs. No workspace IDs, names, member lists, filenames, or hashes are put into discovery records. There is no DHT, rendezvous backend or public user list. The current core has an explicitly configured circuit relay client (Stage 3), with no default infrastructure; the opt-in host role is described below. Local observers can nevertheless correlate running devices and IPs.

The v0.2 Stage 1 core adds Identify and explicit literal-IP QUIC dialing. Identify exposes the device public key, protocol/agent versions, supported transport protocols and the address at which the remote peer was observed. It hides this device's interface listen addresses, and its automatic peer-address cache is disabled. LAN discovery remains independent. Public addresses learned from already known peers are bounded hints, never authorization. Observed public addresses are recorded in local diagnostics only: a peer's report is not evidence that a NAT permits inbound connections. No observed address is promoted to a confirmed external address or advertised in this stage.

Direct addresses must be exactly `/ip4/<IP>/udp/<port>/quic-v1` or `/ip6/<IP>/udp/<port>/quic-v1`, optionally followed by the expected `/p2p/<PeerID>`. Ports must be nonzero; unspecified, multicast and broadcast destinations are rejected. A conflicting suffix is rejected before dialing; the QUIC handshake checks the expected identity even without a suffix. Explicit local callers can dial LAN/loopback addresses. Remote Identify hints exclude private, shared/CGNAT, loopback, link-local, documentation and other reserved address ranges. Address records are capped at 256 peers and eight addresses per peer. `Node::connect` acknowledges registration with the actor, not a successful connection; its bounded diagnostic watch distinguishes connecting, direct, offline and dial failure. Raw addresses are exposed only in advanced network settings, outside normal sharing UI. UI connectivity reflects successful authorized catalog exchanges, rather than an outsider's transport connection.

Stage 2 adds an AutoNAT v2 client. Only validated global IP/QUIC or IP/TCP candidates are submitted; local/private addresses are never sent to an AutoNAT server. Successful confirmation requires an inbound authenticated dial-back bound to the candidate, pending nonce and request connection, not only a server's success response. The library then advertises the confirmed external address through Identify; Dump retains it for at most ten minutes and removes it when the confirming server disconnects. Results describe specific routes from that server, not universal Internet reachability, trustworthy infrastructure or membership. `Unknown` means no current confirmation (including no usable server or inconclusive timeout); `Public` means at least one current confirmed address; `Unreachable` means the reported test failed and no confirmed address remains, not proof that every possible NAT route is impossible.

Candidates are deduplicated and capped at eight, including pending probes. The narrowly vendored `libp2p-autonat` 0.16.0 patch makes completed probes eligible again after five minutes through the existing five-second timer; refreshed evidence expires after ten minutes. Live public listeners remain eligible even when a server was initially unavailable; observed-only candidates expire after ten minutes and expired listeners are removed. Revalidation retains the behaviour, connection handlers and pending nonce ownership; it does not close active transport connections or transfers. Failed/inconclusive/expired evidence withdraws its advertisement. Signed contacts separately carry public listener candidates and live circuit locators as routing hints, without promoting unverified observed addresses to confirmed Identify advertisements. LAN startup never dials an Internet service: probes use only already connected peers that advertise AutoNAT server support. Without a usable server, evidence expires to Unknown. Dump does not implement a probe-server role; a compatible generic AutoNAT server may supply that evidence. The opt-in Dump host supplies circuit forwarding. Tests enable loopback candidates only in their compiled test harness, never in shipping code.

## Stage 3 circuit transport (implemented core API)

`Node::reserve_relay(expected_relay_peer_id, address)` registers a circuit listener at an explicitly supplied literal-IP QUIC/TCP endpoint. There are at most three concurrent relay reservations per node. libp2p handles reservation renewal, expiry and listener errors. `relay_addresses` contains currently accepted circuit listen locators; closed/expired listeners remove them. No relay is contacted by default, and no host payload handling is enabled. A relay server must advertise its reachable address for reservation responses; the local test relay advertises loopback explicitly. The source includes persisted advanced settings and an opt-in self-host role; the published 0.1.1 installer predates them.

`Node::connect(expected_peer_id, circuit_address)` validates a single relay hop and the destination identity. Shape: `<relay IP/transport>/p2p/<relay PeerID>/p2p-circuit[/p2p/<destination PeerID>]`. Reject missing relay identity, nested circuits, relay/destination identity substitution and conflicting destination suffixes. The outgoing dial always pins the destination Peer ID. Direct TCP uses Noise plus Yamux; direct QUIC still uses TLS 1.3. A relayed connection uses its own authenticated end-peer Noise session plus Yamux inside the circuit, in addition to any encrypted transport between each endpoint and the relay. Relay hop authentication alone never grants workspace access.

Direct and circuit connections have separate libp2p-stream controls and protocol handlers. The pinned library otherwise chooses a random connection for a peer. The independent persisted relay Peer ID prevents a direct forwarding-service connection from being selected as a member's application endpoint. New requests prefer the member's direct route; an existing circuit is retained so active file streams can finish. The same control/file requests, signed rosters/manifests and final SHA-256/no-overwrite rules run over both paths. If a relay-only connection disappears, transfer cancellation/error cleanup removes the recorded partial instead of committing it. Core diagnostics distinguish direct/circuit/offline and bounded protocol negotiation outcomes; successful authenticated catalog exchanges determine normal online status. Private relay locators are usable for explicit local configuration, but Identify filters them from external advertisements even when the relay client confirms them. Raw locators appear only in advanced network settings, outside normal sharing UI.

Stage 4 implements native DCUtR. Only bounded public literal-IP candidates are negotiated; invalid/private/DNS remote hole-punch routes are rejected before dialing. Failed upgrades retain circuits and active streams. Eight handler events per connection and 1024 per connected lifetime bound native attempt history; state resets after all connections close. Local protocol tests prove a direct upgrade and circuit fallback, not real NAT traversal. Stage 6 adds the automatic route scheduling and invitation contacts described below.

## Opt-in hosting (Stage 5)

**Network settings / Help the Dump network** is disabled by default. Changes apply after restarting Dump, avoiding live handler replacement during transfers. The forwarding role runs in a separate swarm with its independently persisted protected relay identity and owns no workspace or disk/payload API. It listens on TCP and UDP port 42042. Advanced settings accept at most two explicit public literal-IP hosting addresses and three relay locators ending in their Peer IDs; private and DNS routes are rejected there. Manual hosting addresses are operator-configured claims, not AutoNAT evidence.

Loading old protected state without a relay key generates and saves that key once using the existing atomic DPAPI save, leaving the original device key and signed workspace/file data intact. Previously enabled hosting is disabled and a migration notice remains until explicit valid reconfiguration. Existing hosting addresses and remote pinned relay locators are preserved for operator review, never silently rewritten. Hosting address suffixes naming the old Device Peer ID must be removed or explicitly replaced with the new Relay Peer ID. Previously distributed host locators and invitations containing those circuits require fresh locators/invites. Hosting is not enabled by migration.

Limits: 16 reservations, one per Peer ID, ten-minute reservations; eight circuits, two per source Peer ID, ten minutes and 256 MiB per circuit; 32 established/two per-peer connections, eight pending each way; 60 incoming connection attempts per rolling minute. Native per-IP/per-Peer-ID reservation and circuit request token buckets remain enabled. A shared 2 MiB/s read-plus-write application-byte bucket (64 KiB burst) wraps every host-role multiplexed stream, including forwarded encrypted bytes and host control traffic. This is not a wire-speed cap on ACKs/handshakes or a limit on ordinary workspace transfers. Forwarded payloads never go to disk. Requests require transport identity and capacity, but no account; quotas bound anonymous participation.

The circuit byte budget covers relayed traffic in both directions, including all streams, application framing and inner encryption overhead. Its lifetime starts with the circuit, not a particular file. Earlier catalog/contact/file traffic can consume budget before a receive begins. Therefore 256 MiB is not an exact maximum file size, and the client cannot report reliable remaining quota for arbitrary relays. The UI warns for actual relay routes; known direct routes are not blocked by relay limits. Remote relay limits can differ from the built-in host. A relay stream close/reset/stall is not sufficient evidence of quota exhaustion: the transfer reports that the relay connection closed or stalled and its limit may have been reached, with manual direct-route/operator guidance. Hash/length failures and disk errors keep their own causes. Incomplete files never commit; only recorded Dump-owned partials are cleaned, with failed cleanup retained for startup. There is no resume or automatic transfer retry/circuit churn to bypass limits.

The pinned stream dependency is locally patched to release closed connection senders, with a 4096-cycle regression check, preventing lifetime accumulation as public peers connect/disconnect. No protocol change is made by that patch.

To self-host on Windows from this source build: open Network settings, enable assistance, and enter your publicly reachable `/ip4/<public-IP>/udp/42042/quic-v1` and/or `/ip4/<public-IP>/tcp/42042` under Advanced connectivity. Restart Dump, allow those ports through the approved firewall, and map them on your router to this Windows device if needed. Copy the **Relay Peer ID** from Network settings and give trusted users the corresponding address with `/p2p/<RelayPeerID>` appended; the Device Peer ID still identifies the file-sharing member. A compatible circuit relay v2 implementation is an alternative. Hosting behind CGNAT without an inbound route is insufficient. The published LAN installer does not yet include these controls.

## Internet contact decision and implemented flow (Stage 6)

| Option | Decision |
|---|---|
| Relay address in invite | Use as a fallback locator, after the owner has a live reservation. The locator includes relay and owner Peer IDs. |
| Global bootstrap lookup | Omit. An invite already pins its owner, so global lookup adds infrastructure without removing the need for a usable locator. |
| Rendezvous | Omit. Public workspace namespaces would reveal group activity; generic Peer ID rendezvous adds a dependency and complexity unnecessary for the first milestone. |
| Signed contact/address record | Use a bounded, expiring owner-signed record with direct and relay locators, carried through the trusted invitation channel. Exchange member contact records only inside authenticated, authorized workspace connections. |

Chosen combination: invite-carried signed contact plus configurable, replaceable relay locators. No default Dump-operated relay, mandatory backend or public workspace announcement. No public filenames, hashes, workspace identifiers/names, member lists or secrets are used for routing. Old invitations without contacts continue to work on LAN. Existing v1 membership and manifest signature payloads, device identities and protected workspaces remain unchanged. An optional invitation contact carries the locators; Internet invites require the new build on both devices. Old invitations without that extension remain valid on LAN. Signed contacts prove identity and record integrity, not reachability or membership. Expired contacts or changed addresses require a fresh invite or an authenticated refresh; there is no global identity directory.

Automatic routing order: LAN direct, Internet direct, DCUtR upgrade, retained encrypted circuit fallback. DCUtR needs an initial relay connection to coordinate a direct attempt; this initial circuit is not a claim that relay transfers outrank direct transfers. QUIC remains the preferred direct transport. Relay circuits use an end-peer authenticated Noise session plus Yamux over circuit relay v2, independent of encryption on each endpoint-to-relay hop. Existing Dump application authorization applies unchanged inside that session. If the direct upgrade succeeds, new streams should use it without interrupting in-flight streams; failure retains the bounded circuit. Direct routes are tried before circuits, with bounded retry intervals; DCUtR runs when a circuit supplies coordination. New streams always prefer an established direct connection while active circuit streams can finish.

Relays may learn their connected peers' IP addresses, public Peer IDs, requested circuit endpoints, reservation lifetimes, connection timing and approximate traffic volume; they may correlate relationships. E2E session encryption must protect workspace metadata, membership, secrets, filenames, hashes, signed manifests and file bytes. Relays may deny, delay or interrupt delivery. Authentication, signed manifests and receiver length/hash checks must turn tampering into failure, never a silently altered completed file. No anonymity guarantee is made.

Contacts use an Ed25519 signature over the CBOR tuple `("dump/contact/1", peer_id, public_key, sequence, issued_at, expires_at, addresses)`. They contain no workspace data. Each contact has at most eight literal-IP direct/circuit routes (512 characters each), a positive monotonic sequence and a lifetime of at most 24 hours. The signing key must derive the named Peer ID; addresses must be public and identity suffixes must match. Future issuance has at most five minutes of clock tolerance. Creating an invitation renews its owner's contact for the invite's full 24-hour window. No contact or token grants catalog/file access.

At startup, explicitly configured relays are reserved and retried every 15 seconds. Alternatives for the same Peer ID rotate, allowing TCP fallback when QUIC fails. No default relay is contacted. Contact generation includes current confirmed/public listener candidates and accepted public circuit locators; three route slots are kept for relay fallback. Observed addresses supplied by other peers remain unverified diagnostics. Signed listener candidates are hints, not promises about firewall/NAT reachability. Loss/expiry of a reservation withdraws its locator. Changed locators are signed with a higher sequence.

An invitation with a contact automatically dials its expected owner. LAN mDNS still works with no contact or external service. Routing tries known LAN addresses, then public direct addresses, then circuits, at most eight routes and one attempt per peer per ten seconds. While a circuit is live, direct candidates are retried at most every 30 seconds. Failed DCUtR retains relay service. Offline circuits retry against the configured replaceable infrastructure; no central lookup occurs.

`/dump/contact/1` uses the same 64 KiB/five-second frame rules and shares control admission limits. Request: `{ snapshot, contact, offset }`; response: `{ contacts, total }`, at most eight contacts per page and 128 per roster. Every request authorizes the transport Peer ID against the pinned owner-signed current roster before sharing anything. Incoming self-contact must name that authenticated sender. Forwarded records must verify and belong to that roster. Stale sequences are ignored and equal-sequence conflicts rejected. The protected cache holds at most 256 contacts; routes are used only for current members or the explicitly invited owner. Authorized members can exchange and retain each other's contact records without an online owner; revocation remains eventual as for catalogs. The extension is negotiated through Identify, so older peers without it still use the original control/file protocols.

With all configured relays unavailable, known direct routes and LAN discovery remain usable. If neither can reach a peer, it becomes offline and any lost transfer cleans up rather than committing partial bytes. There is no cloud fallback. Symmetric NAT/CGNAT, UDP blocking or restrictive firewalls can defeat direct traversal; a reachable relay is then needed. Relay quotas can stop large transfers. Address changes may require a fresh invite while the owner is unreachable; cached contacts expire after a day and are not a global directory.

Existing protected v1 device/member identities, workspace data and membership/manifest signature payloads remain unchanged. Optional settings/contact fields default safely when reading v0.1.x state. Normal UI shows authorized direct/relay/connecting/offline status and keeps routing syntax in Advanced connectivity. The published 0.1.1 installer predates this source milestone.

## Frames and signatures

Protocols `/dump/control/1` and `/dump/file/1` carry a big-endian four-byte frame length followed by a Serde CBOR message. Frames are limited to 64 KiB and reject trailing bytes/unknown fields. Reads and writes of control frames time out after five seconds.

Signed membership payload is the fixed tuple:

```
("dump/membership/1", version, workspace_id, name, owner_peer_id,
 owner_public_key, revision, members)
```

Members are sorted by Peer ID with no duplicates; member structs serialize `peer_id` then `name`. Signed file payload is:

```
("dump/file/1", version, file_id, workspace_id, name, size, sha256,
 owner_peer_id, owner_public_key)
```

Both use Ed25519 over CBOR bytes. Public keys are base64url-without-padding encoded libp2p protobuf keys; signatures use the same base64 variant. The verifier derives the claimed Peer ID from the key. SHA-256 is lowercase hex. File IDs identify immutable versions, never filesystem paths.

## Requests and responses

Serde externally tagged enums define the wire messages:

| Protocol | Request | Response |
|---|---|---|
| control | `Join { invitation, name }` | `Waiting`, `Approved { workspace }`, or `Denied` |
| control | `Catalog { snapshot, offset }` | `Catalog { snapshot, files, total }` or `Denied` |
| file | `File { snapshot, manifest }` | `File { manifest }`, raw file bytes, then a framed `Done`; or `Denied`/stream reset |

Catalog pages contain at most 32 manifests. Totals cannot exceed 10,000, nonterminal pages must advance, and changes in total cause a retry. Every manifest must belong to the transport peer and requested workspace. Partial inventory refreshes are discarded rather than merged into a completed catalog.

The serving peer checks its current share map as well as the signature: an old signed manifest cannot resurrect an unshared file. Raw file bytes are streamed in blocks up to 1 MiB, with a 30-second network inactivity timeout. Receiver disk operations finish before cancellation cleanup rather than abandoning pending writes. Sender and receiver calculate SHA-256 independently. Sender emits `Done` only if the source matches its advertised version; the receiver commits only after exact byte-count/hash validation and a final membership/cancellation check.

## Joining

The URI is `dump://join/<base64url(JSON invitation)>`. Invitation fields: `version`, `workspace_id`, `owner_peer_id`, a 256-bit random `token`, and an optional signed `contact`. URLs are limited to 8192 characters; parsing pins the contact to the invitation owner and validates its signature/routes/lifetime. The owner stores the token's SHA-256 digest, expiry, and the transport Peer ID to which approval is bound. The raw token never authorizes catalogs or downloads.

The requester pins the owner from the invite, then sends a bounded join request. The owner compares the displayed authenticated device fingerprint through a trusted channel and explicitly approves it. Approval atomically binds/consumes the token and persists a new signed roster. The same approved Peer ID can retrieve its grant until expiry; another identity cannot reuse that token. Rejecting a request invalidates its invite. The owner must remain in the selected workspace during this process.

## Membership propagation

Members periodically request catalogs from reachable roster peers and exchange owner-signed snapshots inside the encrypted connection. A newcomer can present a newer genuine snapshot so a stale member can establish its authorization. A lower revision cannot roll state back. An equal revision with different signed contents is a conflict. The owner key must always match the locally pinned key.

Persist accepted membership before granting access; stop removed identities' transfers and clear their manifests. A peer that has not received the update may still authorize using its older roster. No bounded revocation time is promised during partitions. A removed peer receives no new catalogs; it may retain historical metadata and downloaded bytes.

## Local completion and resource bounds

Source paths are resolved only through local opaque share IDs. Native file pickers and native drag/drop are the only desktop sources of paths. Rehash work retains its original workspace and file generation; stale results cannot share into another workspace or undo Stop sharing.

Downloads use exclusively created `.dump-<UUID>.part` files in the selected destination folder. DPAPI-protected state records only a successfully created, owned partial before payload writes; a colliding existing file is neither deleted nor recorded for startup cleanup. Flush and close before a Windows no-replace, write-through move. Failed deletion retains the owned path for a later startup retry when state persistence succeeds; receive and startup cleanup display guidance to close programs using the temporary download and restart Dump, preserving the original transfer error. Storage failures are surfaced. A crash between exclusive creation and recording may leave an empty unrecorded partial, which is never swept automatically. Never auto-open received files.

Connection limits, 32 control tasks, 8 file-header tasks, per-peer request rate of 10/second, and transfer semaphores bound resource use. Serving transfers are admitted without an unbounded queue; excess inbound transfers fail. There are two send slots and two receive slots, and one per direction per peer. Availability expires after 15 seconds without a successful authenticated catalog exchange.
