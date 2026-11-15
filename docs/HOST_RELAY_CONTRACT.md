# Host relay contract — implementation candidate

Source implementation now passes exact Cargo.lock native core tests, including
three-node QUIC loopback relay. Physical LAN/Tailscale acceptance and complete
desktop validation are still separate gates; see LOCKED_NATIVE_VERIFICATION.md.
No host file cache is implemented: source and owner host must both remain online.
This is a file/catalog fallback; chat retains its separate direct transport and
receipt semantics. Development/preview updater is compiled disabled.

## Transport refinement

The earlier proposed generic circuit relay, separate infrastructure key and
Noise/Yamux circuit locator are replaced here by a narrower application bridge.
This needs no public reservation server, generic socket forwarding, relay address
in invitations, or additional relay identity. Owner QUIC authenticates each outer
hop; a new libp2p Noise session inside the bridge authenticates the actual source
and receiver to each other. Host forwards opaque encrypted bytes and never acts
as source. One stream carries one authorized catalog page or file, so no inner
multiplexer is needed. Existing direct QUIC, v1 file/catalog messages and signed
manifests are unchanged. Old clients continue using direct connections; these new
protocols require a capable host and endpoints.

## Admission and scope

/dump/host-relay/1 supports member status, fetch and source prepare;
/dump/host-source/1 accepts the source callback. Only the active workspace owner
can explicitly enable the service (off by default, persisted per owned room).
Status uses authenticated owner identity and current signed membership. Direct
stream open is attempted first with a five-second bound; failure can try host.
No invitation join, chat or arbitrary destination traffic is accepted by a grant.

The owner signs a 15-second grant binding version/domain, random nonce, generation,
exact signed roster, receiver, source, purpose and full source-signed manifest or
catalog offset. Both endpoints must be distinct approved members; host self-loops
and cross-room files are denied. Fetch uses the authenticated receiver identity.
Source prepare must come from the authenticated owner. Source callback must come
from the signed source and claim an exact pending grant once. Invalid sources do
not consume that grant. Noise must authenticate the receiver/source identities,
and source rechecks current membership and the exact encrypted request before
serving. Grants never authorize paths or a file other than the exact manifest.
Workspace/roster change, disable and restart invalidate generation and cancel
bridges. Endpoint revocation still requires receipt of the newer signed roster;
there is no claim of instantaneous revocation on a disconnected source.

## Bounds and lifecycle

Four host sessions, two per endpoint, two outgoing client fallbacks, two prepared
source streams; incoming protocol handlers are bounded separately. Forwarding
uses one 64 KiB buffer per direction with writer backpressure, a shared aggregate
16 MiB/s schedule, 30-second read/write/close inactivity bounds and cancellation.
File fallback ceiling is 8 GiB; larger files require direct access. Byte budget is
signed size + 1% + 512 KiB + two maximum control frames, shared both directions,
including Noise bytes. Catalog budget has the same fixed overhead and one page.
Progressing transfers do not expire at 15 seconds: that bound is establishment.

The existing endpoint path enforces manifest signature/identity, exact size,
SHA-256, final source response, no-overwrite destination and temporary cleanup.
Relay stream loss follows that path; direct connection loss no longer cancels an
independent relay stream. Explicit cancel and changed membership stop transfers.
Host stores neither file bytes nor partial files. UI distinguishes actual relay
transfers, last confirmed relay status and online-only availability; no delivered
or offline-cache guarantee is inferred from a grant or connection.

## Verification still required

See HOST_RELAY_VERIFICATION.md. Native three-profile tests must run first, followed
by three physical Windows devices: LAN member, Tailscale member and host reachable
by both. Confirm no direct member route, actual relay usage, separate approvals,
independent SHA-256, revocation, direct disconnect during bridge, host/source loss,
cancel, corruption, busy/slow receiver, file beyond 256 MiB and ceiling rejection.
Use fresh explicit data/download directories and ports. Do not change firewall,
ACLs, installed app, feed or signing policy. No public services are required.
