# Dump — Requirements

## 1. Overview

**Dump** is an open-source desktop application for sharing files directly between trusted users.

Dump is **peer-to-peer (P2P)** by design:

- Dump does not provide central file storage.
- Files remain on the owner's computer until another user downloads them.
- Users download files directly from another user's device whenever possible.
- If the owner closes Dump or becomes unreachable, their shared files become unavailable.
- Dump should work without requiring a central Dump-owned backend.
- The project will be open source.

---

## 2. Product Goals

Dump should allow a small team or group of trusted users to:

1. Create a private workspace.
2. Invite another user into that workspace.
3. Discover other workspace members.
4. Drag and drop files into Dump.
5. See files currently shared by online workspace members.
6. Download files directly from the file owner's device.
7. Transfer files securely even when an untrusted relay is involved.
8. Use Dump on the same LAN without any external infrastructure.
9. Support users on different home or office networks.

---

## 3. Core Principles

### 3.1 No Central File Storage

Dump must never require files to be uploaded to a central Dump server.

The original file remains on the owner's filesystem.

Example:

```text
C:\project\.env
```

Dump stores a local reference to that path and exposes the file only while the owner is available.

### 3.2 Peer-to-Peer First

Preferred transfer path:

```text
User A <====================> User B
            Direct P2P
```

A direct encrypted connection should always be preferred.

### 3.3 Relay Is Untrusted

If a direct connection cannot be created, Dump may use another peer as a relay.

```text
User A =====> Relay =====> User B
```

The relay must be treated as fully untrusted.

A relay must not be able to:

- Read file contents.
- Modify a file without detection.
- Forge another user's identity.
- Produce a valid replacement file.
- Access workspace secrets.

### 3.4 Private Workspaces

Dump must not expose a global list of:

- Users.
- Workspaces.
- Shared files.
- File names.
- Workspace membership.

Only users possessing a valid workspace invitation/key should be able to participate in that workspace.

---

## 4. User Identity

Each Dump installation must generate its own cryptographic identity.

Recommended identity model:

```text
Private Key
    |
    +--> Public Key
             |
             +--> Peer ID
```

Requirements:

- Private keys must remain on the user's device.
- Private keys must never be sent to other peers.
- Connections must cryptographically authenticate peer identity.
- Workspace members should be represented by their peer identity.

---

## 5. Workspace

A user must be able to create a workspace locally without contacting a Dump server.

A workspace should contain at minimum:

```text
workspace_id
workspace_secret
members
```

Joining should be possible using an invitation such as:

```text
dump://join/<invite-data>
```

Future UI may also support QR codes.

The invitation must contain enough cryptographic information for an authorized user to join without requiring a central account system.

---

## 6. Peer Discovery

### 6.1 Local Network

For devices on the same LAN, Dump should discover peers automatically using mDNS or an equivalent local discovery mechanism.

Example:

```text
User A ----+
User B ----+---- LAN / mDNS
User C ----+
```

No external server should be required for LAN operation.

### 6.2 Internet

For peers on different networks, Dump should attempt connections in this order:

1. Direct connection.
2. NAT traversal / hole punching.
3. Community relay fallback.

Conceptually:

```text
Direct P2P
   |
   v
Hole Punching
   |
   v
Relay
```

The architecture should support peers behind NAT and CGNAT where technically possible.

---

## 7. Community Relay

Dump may allow users to voluntarily help the network.

Possible setting:

```text
[ ] Help the Dump network

Allow this device to:
- assist peer discovery
- assist NAT traversal
- relay encrypted traffic
```

Relay requirements:

- Relay participation must be optional.
- Relay traffic must be bandwidth-limited.
- Relay connections should support configurable limits.
- Relay must not store transferred files.
- Relay must not have access to plaintext file contents.
- Relay should only forward encrypted traffic.

A malicious relay must be considered part of the threat model.

---

## 8. File Sharing

When a user adds a file to Dump:

1. Dump must not copy the file into cloud storage.
2. Dump should keep a local reference to the original path.
3. Dump creates a file manifest.
4. The manifest is announced only to authorized workspace peers.

Example manifest:

```text
file_id
name
size
content_hash
owner_peer_id
workspace_id
signature
```

The local filesystem path must not be sent to other peers.

Example:

```text
C:\company\secret\.env
```

must remain local.

---

## 9. File Availability

A shared file is available only while its owner can serve it.

Example:

```text
User A online

.env
build.zip
design.fig
```

When User A disconnects:

```text
User A offline

.env       -> unavailable
build.zip  -> unavailable
design.fig -> unavailable
```

For the MVP, files belonging to offline peers may simply disappear from the active shared-file list.

Dump is not intended to provide permanent cloud availability.

---

## 10. File Transfer

A file should be streamed from disk instead of loaded entirely into memory.

Example:

```text
Owner SSD
   |
read chunk
   |
encrypted network stream
   |
receiver
   |
temporary .part file
   |
verification
   |
final file
```

Requirements:

- Large files must be supported.
- Transfer progress must be visible.
- Transfers must be cancellable.
- Partial transfers should use a temporary file.
- A file must not be marked complete until integrity verification succeeds.

---

## 11. Encryption

All communication between workspace peers must be encrypted end-to-end.

Preferred design:

```text
User A
  |
  | authenticated encrypted channel
  |
Relay (untrusted)
  |
  | encrypted data
  |
User B
```

The encryption layer must provide:

- Confidentiality.
- Integrity.
- Peer authentication.

A relay altering encrypted packets must cause the connection or affected data to fail verification.

---

## 12. File Integrity

Every shared file must have a cryptographic content hash.

Recommended algorithms:

- BLAKE3, or
- SHA-256.

Example:

```text
file.zip
hash = 8fa291...
```

After receiving the file:

```text
expected_hash == received_hash
```

must be true before the transfer is accepted.

If verification fails:

```text
TRANSFER FAILED
FILE CORRUPTED
```

Dump must not expose the corrupted temporary file as a successfully downloaded file.

---

## 13. Signed File Manifest

The file owner should cryptographically sign the file manifest.

Example:

```text
Sign(
    file_id,
    name,
    size,
    content_hash,
    workspace_id
)
```

The receiver must verify the signature against the owner's public identity.

This prevents an untrusted relay from replacing:

```text
original.zip
```

with:

```text
malicious.zip
```

while pretending that the replacement came from the original owner.

---

## 14. File Changes

Dump must handle cases where a shared file changes on disk.

For the MVP:

```text
old file changed
      |
      v
invalidate old manifest
      |
      v
create new manifest + hash
```

Dump should never silently treat modified contents as the same immutable file version.

---

## 15. Security Threat Model

Dump must assume that the following can be malicious:

- Internet peers outside the workspace.
- Community relays.
- Public networks.
- Network intermediaries.
- Attackers capable of observing traffic.

Dump should protect against:

- Man-in-the-middle attacks.
- File tampering.
- Peer impersonation.
- Unauthorized workspace access.
- Unauthorized file downloads.
- Malicious relay modification.
- Replay of invalid transfer metadata where practical.

Dump does **not** guarantee protection if:

- A user's private key is stolen.
- The user's operating system is compromised.
- Malware can read the original shared file directly from disk.

---

## 16. Privacy Model

Users outside a workspace should not learn:

- Workspace name where avoidable.
- Workspace membership.
- Shared filenames.
- File contents.
- File hashes where avoidable.
- File metadata.

A relay may still be able to observe limited network metadata such as:

- Source IP.
- Destination/peer connection information.
- Timing.
- Approximate traffic volume.

Hiding this metadata completely is not an MVP requirement.

Onion routing / Tor-like anonymity is explicitly out of scope for the initial versions.

---

## 17. MVP Scope

### Dump v0.1

Target:

- Windows first.
- Tauri desktop application.
- Rust core.
- LAN operation.
- Local peer discovery.
- Private workspace creation.
- Workspace invitation.
- Drag-and-drop file sharing.
- Shared file list.
- Direct peer-to-peer file transfer.
- Transfer progress.
- End-to-end encryption.
- Peer authentication.
- Content hashing.
- Signed file manifests.
- File integrity verification.

Suggested stack:

```text
Desktop UI
    Tauri
      |
Frontend
    React / Svelte
      |
Rust Core
      |
    libp2p
      |
 QUIC / secure transport
```

### Later Versions

Possible progression:

```text
v0.2
Internet direct connections

v0.3
NAT traversal / hole punching

v0.4
Community relay network

v0.5+
Resume transfers
Multiple sources
Cross-platform support
```

---

## 18. Non-Goals for MVP

Do not include initially:

- Central user accounts.
- Cloud file storage.
- Web dashboard.
- Chat.
- Comments.
- File previews.
- Automatic folder synchronization.
- Automatic cloud backup.
- File version history.
- Permanent file hosting.
- Complex per-file permissions.
- Global public user discovery.
- Global public workspace discovery.

---

## 19. UX Terminology

Because files are not uploaded to Dump infrastructure, the UI should prefer:

```text
Sharing
Receiving
Available
Unavailable
```

instead of:

```text
Uploading to Dump
Stored in Dump cloud
```

Example:

```text
Receiving from Tle-PC
██████████████░░ 82%
```

---

## 20. Success Criteria

The initial concept is successful when the following scenario works:

```text
1. User A installs Dump.
2. User A creates a workspace.
3. User A sends an invite to User B.
4. User B joins.
5. Both devices discover/authenticate each other.
6. User A drags file.zip into Dump.
7. User B immediately sees file.zip.
8. User B clicks Download.
9. file.zip transfers directly from A to B.
10. B verifies the sender signature and file hash.
11. The received file matches the original exactly.
12. User A closes Dump.
13. User A's remaining shared files become unavailable.
```

No Dump-owned server should be required to store the file at any point.
