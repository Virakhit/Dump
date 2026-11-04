# Local group chat

## Run it

Use Windows with the prerequisites in [DEVELOPMENT.md](DEVELOPMENT.md). From a
checkout containing the group-chat changes:

```powershell
npm ci
npm run tauri dev
```

Use the same source build on each device. `npm run dev` alone only previews the
interface; it cannot access the desktop identity, protected storage, or network.
The existing published installer does not gain chat until a new release is built
and published separately.

1. Open Dump on devices connected to the same LAN.
2. Create a workspace, share its invite, and approve each device after comparing
   fingerprints using the existing Members flow.
3. Select that workspace on every participating device and open **Group chat**.
4. Enter a message and select **Send**, or press **Enter**. Use **Shift+Enter**
   for a new line. Text composition with an IME does not send prematurely.

No additional service, account, port-forwarding rule, or database setup is needed.
Chat uses Dump's existing authenticated, encrypted device connections. Existing
optional connectivity settings also apply; this feature adds no hosted service.

## Delivery and local history

The sender saves the message locally before attempting delivery to approved
devices known online. Each recipient checks its active workspace and current
membership, verifies the author's signature, and saves before acknowledging.
The interface shows how many targeted devices acknowledged. An acknowledgment
does not establish that a person read the message.

If no other members are online, the message is saved only on the sender. Offline
members, devices with another workspace selected, and older builds do not receive
it. There is no offline queue, history exchange, automatic retry, or forwarding
through another member. If an acknowledgment is lost, a recipient may have saved
the message even though the sender cannot confirm delivery. A send interrupted by
closing/restarting the app remains visibly unconfirmed.

Switching workspaces keeps each room's history separate. A message submitted for
one workspace is never redirected into another; pending delivery results update
the original message. Members can chat without the workspace owner online once
they have the signed membership. As with file sharing, a removal takes effect on
each device after it learns the updated membership. Already received history
stays on the receiving device.

Messages are plain text, including Thai and other Unicode text. Each message can
contain at most 4,096 UTF-8 bytes; a Thai character or emoji may use several bytes.
Whitespace-only messages and control characters other than line breaks and tabs
are rejected. HTML-like text is displayed as text.

The existing Windows user-scoped DPAPI storage protects history together with
device state. The app retains the latest 500 messages in each workspace and at
most 2,000 across the device, removing the oldest locally stored entries when a
limit is reached. Arrival order is local to each device; displayed timestamps
come from senders' clocks. There is no export, backup, or cross-device recovery
feature in this change. Use the updated build consistently: older builds do not
understand chat history and can discard it when rewriting the shared local state.

## Check on your LAN

This is a manual acceptance checklist, not a claim that these physical-device
checks have already run:

- With three approved devices in the same workspace, send messages from each and
  verify sender names, multiline Thai text, and receipt counts on the others.
- Close the owner and confirm the two remaining approved devices can still chat.
- Close/reopen a participant and confirm its own saved history remains. Confirm
  that messages sent while it was offline are not silently backfilled.
- Select another workspace on one device and verify that it neither displays nor
  accepts messages intended for the first workspace.
- Remove a member, let the updated membership propagate, and verify that new
  messages from that device are rejected.
- Receive a file while chatting to check that both functions remain usable.

For automated checks use `npm run build` and `npm run test:core` on Windows.
The focused chat command is `cargo test --manifest-path src-tauri/Cargo.toml --locked --test chat`;
it also compiles the desktop feature used by the application.
The chat integration tests use real authenticated QUIC over loopback. They do not
replace multi-device LAN discovery or Windows firewall acceptance checks.

## Recorded verification

The [Windows run for a0e9087](https://github.com/Virakhit/Dump/actions/runs/41792942659)
passed frontend build, Rust formatting, all three chat integration tests, all 37
core unit tests (including four new chat tests), and all four connectivity tests.
That run then stopped at the existing LAN file-transfer test's source-file edit
with Windows sharing violation 32; it was not a fully green suite. The subsequent
test adjustment waits for the serving side's source handle to be released before
the edit, preserving the stale-manifest assertions. The focused chat CI step also
enables the desktop feature to verify the Tauri command integration.

Local frontend checks exercised UTF-8 boundaries, receipt labels, literal text
rendering, IME input, duplicate submission, workspace-switch draft isolation, and
draft preservation after a failed send command in a React/JSDOM harness. The native
Windows interface and physical multi-device LAN checklist above remain manual
acceptance checks. See the current checks on [PR #2](https://github.com/Virakhit/Dump/pull/2)
for the result on the latest revision.
