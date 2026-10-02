# Dump protocol v1

## Identity and trust

Each installation generates one libp2p Ed25519 identity. Its public key derives its Peer ID; its private key is persisted under Windows user-scoped DPAPI. QUIC/TLS 1.3 authenticates the remote Peer ID. Display names never grant access.

Every workspace pins its creator's Peer ID and public key. Only that identity signs membership revisions. A workspace ID can never be rebound to a different creator through a join link or network message. The latest locally persisted signed roster determines access; a shared workspace secret alone grants no access. Workspace secrets are supplementary protected state in v0.1 and are not group-wide passwords or independent encryption keys.

## Discovery

libp2p mDNS advertises multiaddresses and Peer IDs. No workspace IDs, names, member lists, filenames, or hashes are put into discovery records. There is no DHT, rendezvous backend, public user list, or relay protocol. Local observers can nevertheless correlate running devices and IPs.

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
