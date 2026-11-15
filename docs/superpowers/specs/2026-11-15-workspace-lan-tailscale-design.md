# Workspace chat and one LAN/Tailscale invitation

User direction: restart from 0.1.1, workspace chat, one owner invitation usable
from LAN and Tailscale, and owner-host relay when two approved devices cannot
connect directly. Work stays on codex/restart-v0.1.1, separate from the original
checkout and installed application. This document records implementation choices
made under the user's instruction to plan and start in phases; it does not claim
the unimplemented phases work.

## Approach and boundaries

Retain the 0.1.1 authenticated QUIC LAN path and signed membership/manifest
contracts. Add narrowly scoped route/invitation, chat and relay modules in that
order. Do not merge/cherry-pick the entire later Internet or Tailscale branches.
The project remains Rust/Tauri/React. Tailscale must already be installed,
connected and authorized; Dump neither enrolls peers nor changes ACLs/firewall.

Alternatives considered by the implementer: requiring every peer to join one
tailnet fails the requested mixed LAN/Tailscale case; a public hosted service
adds an unrequested external dependency; an application plaintext proxy exposes
file contents to the host and duplicates transfer authorization. Prefer a bounded
libp2p circuit relay on the owner host with end-peer Noise/Yamux encryption,
while direct routes continue to use QUIC. This is a proposed implementation
choice, not a claim that the user chose specific cryptographic libraries.

Host must be reachable from each side (LAN interface and an allowed Tailscale
interface). A relay cannot bridge devices that cannot reach the host. Host online
is required for new admission and relayed traffic. Approved peers may use direct
routes without the host, subject to their latest verified membership. Offline
revocation cannot be instantaneous. A source device must remain online while
serving its file; the host streams bytes and does not store file copies.

## One invitation, preserved authorization

Keep the existing v1 Invitation and /dump/control/1 Join payload unchanged.
Introduce a separately versioned URL envelope containing that invitation and an
owner-signed bounded route record. Parser accepts legacy full invitations as
LAN-only and new envelopes for mixed routes; unsupported envelope versions fail
clearly. Older 0.1.1 apps need not parse new envelopes. No shortened-code protocol
is added. Cap the new URL at 8192 bytes and route records at eight literal-IP
addresses; bind signature to invitation digest, workspace, owner Peer ID, expiry
and route role. Never log tokens or workspace secrets. Never put real paths or
unvalidated remote error text in diagnostic exports.

Validate signature, route shape and owner identity before dialing. mDNS LAN
routes remain usable without Tailscale. Signed 100.64.0.0/10 Tailscale hints are
overlay candidates, not proof that a route is reachable or that its peer is a
member. No DNS/HTTP resolution or automatic public-address scan is introduced.
Deduplicate routes, try LAN first, then Tailscale, then host circuit with at most
two concurrent attempts and explicit bounded retry/backoff. Pin authenticated
Peer IDs on every route. A route change must not interrupt an active transfer.

Joining the owner is insufficient for member-to-member discovery. Add a separate
authenticated /dump/routes/1 exchange carrying each approved member's signed,
workspace-bound, expiring routes and the owner's signed host-relay locator.
Validate sender key/Peer ID, workspace, roster membership, sequence and expiry
for every record; authorize every page and recheck current roster after waits.
Bound to eight routes per member and the 128-member roster; purge removed/expired
records. Never announce routes to outsiders or globally. The owner's envelope
bootstraps this exchange, after approval, so members without mutual mDNS can
discover catalogs/files and try direct Tailscale routes before host fallback.

The requested same link is shareable with multiple friends on either network,
with a separate fingerprint approval for every device. Baseline v1 invitations
are one-device-only and MUST retain that legacy behavior. New shareable envelopes
use a separate additive invitation ledger: token hash, workspace, expiry, bounded
per-peer approvals and per-peer rejections, up to the existing 128-member limit.
Reuse never admits another peer automatically. Rejecting one device blocks that
device for the token; an explicit owner action revokes the whole shared link.
Approvals/grants are bound to the authenticated peer and live membership; reject
or replay cannot return another device's grant. Do not silently turn old v1
invites into reusable tokens. The v1 Join payload can remain unchanged because
the owner chooses the ledger by token hash; new token issuance is opt-in via the
new envelope command. Test the exact same URL with two independently approved
peers on different route types. Owner must select the invited workspace, compare
each requester fingerprint and approve. No shared account or automatic admission.
Report finding owner, connecting, sending request, acknowledged awaiting approval,
denied/expired and approved from actual local events, scoped to the exact current
join attempt. A late response must not revive a cancelled/replaced request.

## Chat

Separate /dump/chat/2 stream, leaving v1 control/file enums unchanged. The later
existing /dump/chat/1 payload is incompatible with this new sequence/epoch
contract; never advertise that old protocol for these messages. Messages
contain version, UUID, workspace ID, authenticated sender, UTF-8 body, timestamp
and signature over a chat-specific domain. Proposed bounds: 4096 body bytes,
500 locally retained messages per room, four concurrent sends and 128 members.
Use current signed roster authorization before and after async operations; deny
nonmembers, wrong workspace/sender, invalid signatures and stale roster changes.
Deduplicate by workspace/sender/message ID with conflicting content rejected.
Persist locally before ACK. ACK means stored by that device, not read by a person.
Replay state is independent of the 500-message display/history retention. Signed
messages also bind a monotonic sender sequence and signed-roster revision.
Persist outgoing sequence/message atomically before sending. For each current
workspace/sender/roster epoch retain a durable 64-sequence sliding window of
message IDs and signed-content digests. Exact known duplicates ACK without
appending; conflicting sequence/ID/digest is denied; sequences below the window
are rejected as too old. Commit replay state with received history before ACK.
Reject messages from older than the latest known membership revision, including
old signed replays after removal/re-addition. Test pruning, restart, reordered
delivery, sender counter rollback and membership epoch change. This is a proposed
bounded online-only policy, not an offline history synchronization protocol.
Initial chat delivery is online-only, with local history and explicit per-device
failed/offline outcomes, not an automatic offline queue. No server-side history
or guaranteed offline delivery is implied. A future retry must reauthorize against current
membership and preserve the original signed message ID. Render plain text only.

## Host relay and file lifecycle

Relay is opt-in per owner workspace. Authenticate source and destination and
check both against that workspace's current verified roster before reservation
and circuit creation. Deny cross-workspace forwarding and nonmembers, including
authenticated outsiders. No generic open proxy, filesystem API or arbitrary
destination dialing. Membership revocation closes affected reservations/circuits.

Use one active workspace per relay instance; bind its allow-list, reservations
and circuits to that workspace. Stop all reservations/circuits before a workspace
switch and rebuild authorization for the new workspace, even when a peer belongs
to both rooms. A bare libp2p circuit request carries Peer IDs, not a workspace;
do not infer room scope from that request alone. Use a separate protected relay
identity bound by the owner's signed workspace locator; it is infrastructure,
never a workspace member or a substitute for an endpoint identity. Reject old
locators/reservations after switch/revocation and test overlapping memberships.

Initial proposed policy: four concurrent circuits per host, two per peer, bounded
16 MiB/s aggregate forwarding and 64 KiB buffered bytes per stream. Backpressure
must stall upstream reads instead of buffering the file. Reserve 15 seconds for
route establishment and 30 seconds of transfer inactivity; progressing streams
do not expire solely because a large file takes longer. Enforce admission limits
before allocating buffers and bounded reservation lifetime/renewal. These policy
values require measured implementation tests before shipping.

Do not inherit the older relay's 256 MiB circuit budget or a fixed short total
duration as an undocumented file-size limit. The implementation must expose any
configured byte/lifetime ceiling before promising a transfer, account for framing
and multiple streams, and test a progressing file larger than 256 MiB plus a slow
receiver. The optional 8 GiB test requires separately coordinated disk capacity;
no such file is created during this initial phase.

Endpoints retain signed manifest checks, exact byte count, final SHA-256,
no-overwrite destination commit, cancellation and owned-partial cleanup. Corrupt
relay/provider bytes, host loss, source loss and timeouts fail without exposing a
completed file. No automatic full-file retry or unlimited relay cycling. Never
advertise a source file as available merely because the host remembers a manifest.
Distinguish Direct LAN, Tailscale, Via host, Connecting, Unavailable/source offline
and Host unavailable from measured reachability. Cached data cannot prove online.

## Isolation and gates

Core tests use fresh temporary Engine stores and loopback ports. Before native
GUI testing, create a development-only app identifier, app-data directory,
single-instance scope and deep-link scope, so launching a test build cannot open
the real profile or replace dump:// registration. Do not install a test build
over the existing app. Preserve the original checkout and all user documents.

Phase 1 changes only request diagnostics/UI and adds baseline contract tests.
Initially production networking was held pending native tests. The user then
explicitly authorized implementing the chat phase while that environment remains
blocked. Its source stays isolated and unshipped, with type-checking and runtime
evidence distinguished; this exception does not satisfy the native/LAN gates.
Then implement one module at a time with failing regressions and standalone
review. Require real two-device LAN, two-device Tailscale and three-device mixed
topology tests; record actual routes and independently compared file SHA-256.
Loopback and mocked Tauri tests cannot satisfy those gates. No merge to main,
installed-app replacement, commit/push, deployment or release without coordination.
