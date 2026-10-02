# Dump protocol v1

## Identity and trust

Each installation generates one libp2p Ed25519 identity. Its public key derives its Peer ID; its private key is persisted under Windows user-scoped DPAPI. QUIC/TLS 1.3 authenticates the remote Peer ID. Display names never grant access.

Every workspace pins its creator's Peer ID and public key. Only that identity signs membership revisions. A workspace ID can never be rebound to a different creator through a join link or network message. The latest locally persisted signed roster determines access; a shared workspace secret alone grants no access. Workspace secrets are supplementary protected state in v0.1 and are not group-wide passwords or independent encryption keys.

## Discovery

libp2p mDNS advertises multiaddresses and Peer IDs. No workspace IDs, names, member lists, filenames, or hashes are put into discovery records. There is no DHT, rendezvous backend or public user list. The current core has an explicitly configured circuit relay client (Stage 3), with no default infrastructure; the opt-in host role is described below. Local observers can nevertheless correlate running devices and IPs.

The v0.2 Stage 1 core adds Identify and explicit literal-IP QUIC dialing. Identify exposes the device public key, protocol/agent versions, supported transport protocols and the address at which the remote peer was observed. It hides this device's interface listen addresses, and its automatic peer-address cache is disabled. LAN discovery remains independent. Public addresses learned from already known peers are bounded hints, never authorization. Observed public addresses are recorded in local diagnostics only: a peer's report is not evidence that a NAT permits inbound connections. No observed address is promoted to a confirmed external address or advertised in this stage.

Direct addresses must be exactly `/ip4/<IP>/udp/<port>/quic-v1` or `/ip6/<IP>/udp/<port>/quic-v1`, optionally followed by the expected `/p2p/<PeerID>`. Ports must be nonzero; unspecified, multicast and broadcast destinations are rejected. A conflicting suffix is rejected before dialing; the QUIC handshake checks the expected identity even without a suffix. Explicit local callers can dial LAN/loopback addresses. Remote Identify hints exclude private, shared/CGNAT, loopback, link-local, documentation and other reserved address ranges. Address records are capped at 256 peers and eight addresses per peer. `Node::connect` acknowledges registration with the actor, not a successful connection; its bounded diagnostic watch distinguishes connecting, direct, offline and dial failure. These raw addresses are not exposed through desktop IPC or the normal UI. UI connectivity reflects successful authorized catalog exchanges, rather than an outsider's transport connection.

Stage 2 adds an AutoNAT v2 client. Only validated global IP/QUIC candidates are submitted; local/private addresses are never sent to an AutoNAT server. Successful confirmation requires an inbound authenticated dial-back with the pending nonce, not only a server's success response. The library then advertises the confirmed external address through Identify; Dump retains it for at most ten minutes and removes it when the confirming server disconnects. Results describe specific routes from that server, not universal Internet reachability, trustworthy infrastructure or membership. `Unknown` means no current confirmation (including no connected server or inconclusive timeout); `Public` means at least one current confirmed address; `Unreachable` means the reported test failed and no confirmed address remains, not proof that every possible NAT route is impossible.

Candidates are deduplicated and capped at eight per connected lifetime before AutoNAT receives them; its own configuration only limits each probe batch. The client cache resets once all transport connections close. AutoNAT v2 currently does not re-probe a completed candidate until that reset; expired confirmations are withdrawn rather than advertised indefinitely. This conservative limit can reduce advertised direct routes in a long-lived session, and will be revisited during contact/reconnection integration. LAN startup never dials an Internet service: probes use only already connected peers that advertise AutoNAT server support. Dump does not implement a probe-server role; a compatible generic AutoNAT server may supply that evidence. The opt-in Dump host supplies circuit forwarding. Tests enable loopback candidates only in their compiled test harness, never in shipping code.

## Stage 3 circuit transport (implemented core API)

`Node::reserve_relay(expected_relay_peer_id, address)` registers a circuit listener at an explicitly supplied literal-IP QUIC/TCP endpoint. There are at most three concurrent relay reservations per node. libp2p handles reservation renewal, expiry and listener errors. `relay_addresses` contains currently accepted circuit listen locators; closed/expired listeners remove them. No relay is contacted by default, and no host payload handling is enabled. A relay server must advertise its reachable address for reservation responses; the local test relay advertises loopback explicitly. The source includes persisted advanced settings and an opt-in self-host role; the published 0.1.1 installer predates them.

`Node::connect(expected_peer_id, circuit_address)` validates a single relay hop and the destination identity. Shape: `<relay IP/transport>/p2p/<relay PeerID>/p2p-circuit[/p2p/<destination PeerID>]`. Reject missing relay identity, nested circuits, relay/destination identity substitution and conflicting destination suffixes. The outgoing dial always pins the destination Peer ID. Direct TCP uses Noise plus Yamux; direct QUIC still uses TLS 1.3. A relayed connection uses its own authenticated end-peer Noise session plus Yamux inside the circuit, in addition to any encrypted transport between each endpoint and the relay. Relay hop authentication alone never grants workspace access.

Direct and circuit connections have separate libp2p-stream controls and protocol handlers. The pinned library otherwise chooses a random connection for a peer. New requests select direct whenever one exists; an existing circuit is retained so active file streams can finish. The same control/file requests, signed rosters/manifests and final SHA-256/no-overwrite rules run over both paths. If a relay-only connection disappears, transfer cancellation/error cleanup removes the recorded partial instead of committing it. Core diagnostics distinguish direct/circuit/offline; successful authenticated catalog exchanges determine normal online status. Private relay locators are usable for explicit local configuration, but Identify filters them from external advertisements even when the relay client confirms them. Raw locators appear only in advanced network settings, outside normal sharing UI.

Stage 4 implements native DCUtR. Only bounded public literal-IP candidates are negotiated; invalid/private/DNS remote hole-punch routes are rejected before dialing. Failed upgrades retain circuits and active streams. Eight handler events per connection and 1024 per connected lifetime bound native attempt history; state resets after all connections close. Local protocol tests prove a direct upgrade and circuit fallback, not real NAT traversal. Automatic route scheduling and invite-carried locators remain Stage 6 work.

## Opt-in hosting (Stage 5)

**Network settings / Help the Dump network** is disabled by default. Changes apply after restarting Dump, avoiding live handler replacement during transfers. The forwarding role runs in a separate swarm with the existing protected identity and owns no workspace or disk/payload API. It listens on TCP and UDP port 42042. Advanced settings accept at most two explicit public literal-IP hosting addresses and three relay locators ending in their Peer IDs; private and DNS routes are rejected there. Manual hosting addresses are operator-configured claims, not AutoNAT evidence.

Limits: 16 reservations, one per Peer ID, ten-minute reservations; eight circuits, two per source Peer ID, ten minutes and 256 MiB per circuit; 32 established/two per-peer connections, eight pending each way; 60 incoming connection attempts per rolling minute. Native per-IP/per-Peer-ID reservation and circuit request token buckets remain enabled. A shared 2 MiB/s read-plus-write application-byte bucket (64 KiB burst) wraps every host-role multiplexed stream, including forwarded encrypted bytes and host control traffic. This is not a wire-speed cap on ACKs/handshakes or a limit on ordinary workspace transfers. Forwarded payloads never go to disk. Requests require transport identity and capacity, but no account; quotas bound anonymous participation.

The pinned stream dependency is locally patched to release closed connection senders, with a 4096-cycle regression check, preventing lifetime accumulation as public peers connect/disconnect. No protocol change is made by that patch.

To self-host on Windows from this source build: open Network settings, enable assistance, and enter your publicly reachable `/ip4/<public-IP>/udp/42042/quic-v1` and/or `/ip4/<public-IP>/tcp/42042` under Advanced connectivity. Restart Dump, allow those ports through the approved firewall, and map them on your router to this Windows device if needed. Copy the Peer ID from the identity panel and give trusted users the corresponding address with `/p2p/<PeerID>` appended. A compatible circuit relay v2 implementation is an alternative. Hosting behind CGNAT without an inbound route is insufficient. The published LAN installer does not yet include these controls.

## v0.2 Internet contact decision (design, later stages)

| Option | Decision |
|---|---|
| Relay address in invite | Use as a fallback locator, after the owner has a live reservation. The locator includes relay and owner Peer IDs. |
| Global bootstrap lookup | Omit. An invite already pins its owner, so global lookup adds infrastructure without removing the need for a usable locator. |
| Rendezvous | Omit. Public workspace namespaces would reveal group activity; generic Peer ID rendezvous adds a dependency and complexity unnecessary for the first milestone. |
| Signed contact/address record | Use a bounded, expiring owner-signed record with direct and relay locators, carried through the trusted invitation channel. Exchange member contact records only inside authenticated, authorized workspace connections. |

Chosen combination: invite-carried signed contact plus configurable, replaceable relay locators. No default Dump-operated relay, mandatory backend or public workspace announcement. No public filenames, hashes, workspace identifiers/names, member lists or secrets are used for routing. Old invitations without contacts continue to work on LAN. Existing v1 membership and manifest signature payloads, device identities and protected workspaces remain unchanged. A later additive invite extension carries the contact; older app versions may require a legacy LAN invite rather than understanding the new extension. Signed contacts prove identity and record integrity, not reachability or membership. Expired contacts or changed addresses require a fresh invite or an authenticated refresh; there is no global identity directory.

Planned transport order: LAN direct, Internet direct, DCUtR upgrade, retained encrypted circuit fallback. DCUtR needs an initial relay connection to coordinate a direct attempt; this initial circuit is not a claim that relay transfers outrank direct transfers. QUIC remains the preferred direct transport. Relay circuits use an end-peer authenticated Noise session plus Yamux over circuit relay v2, independent of encryption on each endpoint-to-relay hop. Existing Dump application authorization applies unchanged inside that session. If the direct upgrade succeeds, new streams should use it without interrupting in-flight streams; failure retains the bounded circuit. This transport and its scheduling are not implemented in Stage 1.

Relays may learn their connected peers' IP addresses, public Peer IDs, requested circuit endpoints, reservation lifetimes, connection timing and approximate traffic volume; they may correlate relationships. E2E session encryption must protect workspace metadata, membership, secrets, filenames, hashes, signed manifests and file bytes. Relays may deny, delay or interrupt delivery. Authentication, signed manifests and receiver length/hash checks must turn tampering into failure, never a silently altered completed file. No anonymity guarantee is made.

Self-hosting will use a generic opt-in peer role with reservation, circuit, bandwidth, session-byte and connection-rate limits; no payload persistence. A host needs a stable reachable transport address and Peer ID. Users must be able to replace configured relays; a private relay shared through an invite is sufficient, with no bootstrap service. Stage 5 will supply executable setup steps and defaults after the role is implemented and tested. Stage 1 has no relay host/client command. Symmetric NAT/CGNAT, UDP blocking and restrictive firewalls may prevent direct traversal and require a reachable relay; relay quotas can prevent large transfers. With all configured relays unavailable, try known direct routes and LAN mDNS, then show offline/failure with no cloud fallback. LAN requires no Internet probe or infrastructure connection.

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

The URI is `dump://join/<base64url(JSON invitation)>`. Invitation fields: `version`, `workspace_id`, `owner_peer_id`, and a 256-bit random `token`. The owner stores the token's SHA-256 digest, expiry, and the transport Peer ID to which approval is bound. The raw token never authorizes catalogs or downloads.

The requester pins the owner from the invite, then sends a bounded join request. The owner compares the displayed authenticated device fingerprint through a trusted channel and explicitly approves it. Approval atomically binds/consumes the token and persists a new signed roster. The same approved Peer ID can retrieve its grant until expiry; another identity cannot reuse that token. Rejecting a request invalidates its invite. The owner must remain in the selected workspace during this process.

## Membership propagation

Members periodically request catalogs from discovered roster peers and exchange owner-signed snapshots inside the encrypted connection. A newcomer can present a newer genuine snapshot so a stale member can establish its authorization. A lower revision cannot roll state back. An equal revision with different signed contents is a conflict. The owner key must always match the locally pinned key.

Persist accepted membership before granting access; stop removed identities' transfers and clear their manifests. A peer that has not received the update may still authorize using its older roster. No bounded revocation time is promised during partitions. A removed peer receives no new catalogs; it may retain historical metadata and downloaded bytes.

## Local completion and resource bounds

Source paths are resolved only through local opaque share IDs. Native file pickers and native drag/drop are the only desktop sources of paths. Rehash work retains its original workspace and file generation; stale results cannot share into another workspace or undo Stop sharing.

Downloads use exclusively created `.dump-<UUID>.part` files in the selected destination folder. DPAPI-protected state records the partial before creation. Flush and close before a Windows no-replace, write-through move. Failed cleanup remains recorded for a later startup retry. Never auto-open received files.

Connection limits, 32 control tasks, 8 file-header tasks, per-peer request rate of 10/second, and transfer semaphores bound resource use. Serving transfers are admitted without an unbounded queue; excess inbound transfers fail. There are two send slots and two receive slots, and one per direction per peer. Availability expires after 15 seconds without a successful authenticated catalog exchange.
