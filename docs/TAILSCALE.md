# Sharing through Tailscale

Dump can use a separately installed Tailscale client to connect approved workspace
members across networks. Tailscale supplies the network path; Dump still checks
device identity, workspace membership, signed file manifests and SHA-256 before
completing a receive.

This feature is in the current source build. The older 0.1.3 installer does not
include it. Both devices need a Dump build containing the Tailscale changes;
the version number alone does not identify those changes. See
[Developing Dump](DEVELOPMENT.md) for Windows prerequisites and run the current
checkout with `npm ci` followed by `npm run tauri dev`. A browser-only preview
cannot connect devices or transfer files. A manually installable local
**0.1.4-alpha.1** preview and its checksum are recorded in
[VERIFICATION.md](VERIFICATION.md). It has not been published to the updater
channel or accepted on a clean second machine.

## Prepare both devices

1. Install [Tailscale](https://tailscale.com/download/windows) separately on each
   Windows device and sign in through the Tailscale app. Keep its service running.
2. Put the devices in the same tailnet, or configure and accept a machine share
   through Tailscale. The receiving device must be able to see the intended
   shared machine in its local Tailscale peer list.
3. Have the network administrator allow **TCP and UDP port 42043** to the intended
   devices in Tailscale grants and Windows Firewall. Dump binds this fixed port
   to the local Tailscale addresses, using QUIC first with authenticated TCP
   fallback. It does not edit firewall rules, grants or sharing settings.
4. Open the supporting Dump build on both devices. In **Network settings**, select
   **Use installed Tailscale**, then **Save settings**. Select **Check Tailscale**
   to refresh the status.
5. Wait for Tailscale to be ready and for the Dump Tailscale listener to be ready.
   If the listener is unavailable, check whether another process is already using
   port 42043. Dump does not silently choose a different overlay port.

Dump reads the installed client through its fixed `tailscale status --json`
command. It does not install Tailscale, sign in, accept machine shares, manage
accounts, or change Tailscale policy. Status reads have a three-second timeout
and bounded output. Readiness means the local adapter has valid status and
addresses; it does not prove that a remote Dump device or its port is reachable.

## Choose the workspace network policy

The adapter switch and the workspace policy have separate purposes. Enabling
**Use installed Tailscale** makes the adapter available. For the selected
workspace, choose **Network settings > Workspace network policy > Tailscale
only** to use it for that workspace. Existing workspaces initially keep
**LAN / Internet** until explicitly changed.

**Tailscale only** applies to joining, contact exchange, catalogs, files and chat.
Dump must use a validated Tailscale connection for those operations, including
when another connection already exists to the same Device Peer ID. If Tailscale
becomes unavailable, the workspace does not silently fall back to a LAN address,
public address or Dump relay. A Tailscale tunnel using DERP remains a Tailscale
path.

Finish or cancel unfinished transfers and wait for file preparation to finish
before changing the adapter or workspace policy. The controls are disabled while
that work is active. Policy changes close the previous connections; they do not
move an in-progress transfer to another network. Turning the adapter off does
not convert a **Tailscale only** workspace to **LAN / Internet**.

## Invite a new member

1. The owner selects the workspace and chooses **Invite a friend > Invite via
   Tailscale**. Use the Tailscale workspace policy and keep Dump open with that
   workspace selected while the friend joins.
2. Select **Copy Tailscale invite** and send the link through an existing trusted
   conversation. The link starts with `dump://tailscale/`.
3. The recipient enables the installed Tailscale adapter, then opens the link or
   pastes it into **Join with an invite**. This link selects the Tailscale route
   policy for the join and resulting workspace.
4. Compare device fingerprints through a trusted conversation before the owner
   approves the request in **Members**. A matching Tailscale name or account does
   not replace this check.
5. After approval, select the same workspace on both devices, share a file and
   choose **Receive**, or open **Group chat**.

Tailscale visibility alone grants no Dump membership. Connections check the
expected **Device Peer ID**, while workspace requests still require approved
membership. Use the device fingerprint for approval, not a Relay Peer ID. Ask for
a fresh invitation if its signed contact expires or the node identity changes.

## Reconnect existing members

An approved member can select **Network settings > Copy Tailscale connection
link** and send it through the existing conversation. The other member opens it
or pastes it into **Join with an invite**. It uses the same `dump://tailscale/`
prefix but carries a connection contact rather than a new-member invitation.

Both devices must already have the workspace and a signed membership roster
that recognizes the other member. The connection link does not add members,
reset device identity, replace the roster or grant access. The owner can be
offline. Both members still need Tailscale visibility and a supporting Dump
build. Review and select **Tailscale only** for the workspace before using this
connection path.

## Shared machines and groups

In the same tailnet, each intended member pair needs a route allowed by the
tailnet policy. Machine sharing across tailnets has additional limits:

- With a one-way share, the recipient initiates the connection to the shared
  machine. Tailscale's sharing quarantine limits connections initiated in the
  opposite direction. Once Dump has an authenticated connection, it is intended
  to carry authorized file and chat traffic in both directions. The recipient
  must initiate again after that connection is lost.
- Sharing A with B and C does not make B and C visible to each other. Dump does
  not forward B's files or messages through A. Configure permitted sharing or
  other Tailscale visibility for every member pair that needs to communicate;
  mutual sharing may be needed when either side must initiate.
- A shared machine's address can be remapped in the recipient's tailnet. Dump
  matches the signed node ID and full DNS name against the receiver's local
  Tailscale peer map, then uses that map's addresses. It does not trust an IP
  copied from the sender or guess from a short machine name.

See Tailscale's [machine sharing documentation](https://tailscale.com/docs/features/sharing)
and [grants documentation](https://tailscale.com/docs/reference/syntax/grants)
when configuring these permissions. A full mesh requires pairwise visibility;
sharing a single machine does not create one.

## Understand status and failures

| Dump status or symptom | What to check |
|---|---|
| Tailscale is not installed | Install the Windows client in its normal installation location, then check again. Dump does not search arbitrary executable paths. |
| Tailscale service is unavailable | Start the separate Tailscale app/service and check again. A status timeout can also produce this result. |
| Tailscale needs login | Sign in or complete device authorization through Tailscale. |
| Unsupported status | Update Tailscale and check again. Dump rejects malformed, oversized or unsafe status data. |
| Tailscale is ready, but the listener is not | Check local TCP/UDP port 42043 availability and refresh the status. |
| Peer missing, offline, or node/name mismatch | Confirm the intended share was accepted, both devices are online, and the local peer list identifies the intended node. Refresh the contact or request a new link when needed. |
| Connection times out | Check the remote Dump process, selected workspace, peer visibility, port, grants and Windows Firewall. A timeout does not prove an ACL or firewall denial. |
| Device identity or signature mismatch | Stop and verify the expected Device Peer ID and fingerprint through a trusted conversation. Do not substitute another peer with a similar name. |
| Adapter or policy controls are disabled | Finish or cancel transfers and wait for file preparation before changing them. |

Transfers using the overlay display **Via Tailscale**. This label does not promise
a direct tunnel: Tailscale may use a direct path, DERP or another supported relay
path, and Dump does not currently identify that internal tunnel route. Tailscale
relays are separate from Dump's forwarding host. The Dump relay's 256 MiB circuit
budget is not a Tailscale file-size limit; network throughput and other Tailscale
conditions still affect transfers.

A failed or cancelled receive does not commit an incomplete destination file,
and Dump does not overwrite an existing file. There is no file resume or
automatic file-transfer retry. After fixing connectivity, start a new receive
manually. Chat also has no offline delivery or history synchronization; see the
[group chat guide](GROUP_CHAT.md).

## Verification boundary

Automated tests exercise signed links, remapped peer-map fixtures, bounded
contact exchange, policy isolation and cold-start links. Actual authenticated
loopback sockets cover recipient-initiated QUIC and TCP/Noise invite/approval,
hashed files and chat in both directions, logout cancellation/reconnect,
occupied-port recovery and returning to LAN. These tests use independent
identities with test-only locator translation, rather than a real tailnet.

Inspection against the installed client returned **Ready** and resolved one
visible online peer using its locally reported addresses. A separate local
smoke test starts the production adapter, opens actual Tailscale TCP/QUIC
listeners on port 42043 and issues a valid signed link without changing the
temporary profile's device identity.

These checks do not establish an end-to-end Dump transfer through Tailscale.
Real two-device invite/approval/file/chat tests, one-way sharing, shared-IP
remapping, reconnects, IPv6, firewall/grants failures and large-file/cancellation
cases still need their own recorded evidence. Integration verification
records the completed checks and outstanding cases in
[VERIFICATION.md](VERIFICATION.md) and
[NETWORK_ACCEPTANCE.md](NETWORK_ACCEPTANCE.md). Loopback fixtures and CLI peer
reachability do not substitute for those physical-network acceptance gates.
