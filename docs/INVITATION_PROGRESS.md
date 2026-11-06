# Invitation visibility and progress hotfix candidate

## 0.1.3 candidate (6 November 2026)

The published 0.1.2 installer predates the source fixes below. The 0.1.3
candidate includes those fixes and shows the requester's name, fingerprint,
Approve device and Reject controls directly inside the invitation dialog.
Approval still requires checking the fingerprint confirmation. The same
request card is used in Members. When no request has arrived, the owner's
Members panel explains that there is nothing to approve yet and asks both
devices to keep Dump open on the same LAN with the invited workspace selected.

Local TypeScript/Vite build and all seven React/JSDOM invitation cases pass.
The two new cases failed before implementation. Local Rust testing cannot
link because this machine's Build Tools installation lacks the C++ workload
(`link.exe`). The pull request's Windows checks run core, desktop and updater
tests; after they pass, the installer-preview job builds a manually installable
0.1.3 installer and SHA-256 checksum in the Dump-Windows-preview artifact.
That preview does not change the published updater feed and has no updater
signature. The normal release workflow still requires the original signing key.

Install the candidate on both devices, create a fresh LAN invitation, keep the
owner's invited workspace selected, then compare fingerprints and approve in
the invitation dialog or Members. Physical two-machine discovery and the
reported incident remain unverified until this flow succeeds on the affected
LAN. The historical results below describe the earlier source candidate.

The 0.1.2 interface says it is waiting for owner approval as soon as joining starts, before discovery or acknowledgment. An arriving owner request is visible only on Members and can be obscured by the invitation dialog. This candidate distinguishes discovery/contact/request/acknowledgment and adds an owner notice with a Review requests shortcut, including inside the invitation dialog. Requests are never approved automatically; fingerprint comparison remains required.

The user-reported absence of an owner request on a physical LAN is not claimed resolved by this UI change. The owner's screenshot was already on Members. Check that the owner keeps the invitation's workspace selected, the link is fresh and current, and approved network policy permits discovery and connections. A selected pre-existing workspace on the joining machine does not identify the invitation target: the new workspace becomes active after approval.

## Executed checks

- React/JSDOM regression: four cases failed on the original UI; five passed after the fix, covering Files, Chat, open invitation dialog, fingerprint gate, owner/room scope and accurate join-stage display. `npm run test:ui` runs these tests; Windows CI and `npm run check` include them. Tauri boundary calls are simulated, so this is not native IPC/OS URL-open acceptance.
- Rust stage regression failed before implementation (missing progress stage), then passed. Code-only ownership probes remain requesting, and a named authenticated owner acknowledgment advances the stage. Full-invite cancellation/replacement and the existing LAN approval/replay flow are covered.
- Windows GNU normal-parallel `cargo test --manifest-path src-tauri/Cargo.toml --target x86_64-pc-windows-gnu --no-default-features --locked`: 75 passed (56 unit, 3 chat, 4 connectivity, 4 LAN, 6 invite-format, 2 relay); one optional 8 GiB case ignored. Updater without its feature and doctests execute zero cases.
- Frontend TypeScript/Vite production build, update-publisher regression (one test), rustfmt, whitespace checks and Windows GNU core/all-target Clippy (`--no-default-features --all-targets -- -D warnings`) pass. An earlier frontend type-check failure was corrected before the final build.
- An initial network-restricted sandbox run had two LAN connection deadline failures. The unmodified baseline outside that sandbox passed all 11 focused LAN invitation cases; the candidate full run above used that same permitted loopback environment. No application timeout or authentication check was loosened.

## Limits and packaging

No physical two-machine acceptance, native installed-app interaction, fresh MSVC desktop/CI, clean-machine installer test, new installer or release was performed. Existing network/firewall settings and installed user data were not changed. The P2P wire protocol, persisted state and app version remain unchanged; the stage flag is transient and the View field is additive.

For a distributable hotfix, use a new version rather than replacing immutable published 0.1.2 assets, run exact-head Windows desktop/CI checks, build and verify the signed installer/updater artifacts, then obtain any required release authorization. Retry a fresh link on the actual approved LAN and verify both owner request and invitee acknowledgment before claiming this user incident closed.
