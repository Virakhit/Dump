# Local workspace group chat

## Intent

Add group chat to Dump for local use. Each existing workspace is a chat room;
members use their existing approved device identities and peer connections.
The implementation choices below are defaults selected for this request.

## Approach

Reuse the authenticated libp2p transport with a separate `/dump/chat/1` protocol.
This keeps file sharing compatible with older clients and requires no account,
new service, database server, or cloud deployment. A central WebSocket server
would add setup and another identity system; gossip/history replication would
add forwarding and offline-delivery semantics that are unnecessary here.

## Behavior and boundaries

- One Group chat tab per workspace, with plain-text messages, sender names,
  timestamps, a multiline composer, and per-message delivery counts.
- Send to approved peers known online when the message is submitted. Both ends
  must have the same workspace active. The owner's presence is not required
  once membership is approved.
- Save locally before sending; acknowledge at the recipient only after saving.
  Delivery means the recipient device saved the message, not that a person read it.
- Offline members do not receive missed messages. There is no offline queue,
  history synchronization, automatic retry, edit/delete, or typing indicator.
- Preserve received/sent history in the existing Windows DPAPI-protected state.
  Default missing chat state for older installations. Keep the latest 500
  messages per workspace and at most 2,000 across the device, in local arrival order.
- Limit each message to 4,096 UTF-8 bytes. Allow newlines and tabs, reject blank
  text and other control characters. Render text without interpreting HTML.
- Authenticate membership using the existing owner-signed roster and verify a
  sender-signed message. The authenticated remote peer must be the author.
  Duplicate deliveries of the same signed message are acknowledged once stored;
  conflicting message IDs are rejected.
- Include the intended workspace in the send command. Recheck membership and
  active workspace before each outgoing transmission. A later workspace switch
  must not redirect messages or delivery updates into the new room.
- Persist delivery status; unfinished sends become interrupted on restart.
  Bound incoming streams and concurrent fan-out, using the existing request budget.
- Keep existing optional connectivity settings; normal LAN use requires no
  Internet service. This change creates no release or deployment.

## Verification

Exercise message validation/signatures, backward-compatible state, bounded
history, durable acknowledgments, duplicate delivery, unauthorized/revoked peers,
workspace changes, and real multi-peer QUIC delivery. Build the frontend and check
composer behavior including Thai/IME input. Windows persistence and desktop
integration require Windows; distinguish CI/loopback evidence from a physical
multi-device LAN test.
