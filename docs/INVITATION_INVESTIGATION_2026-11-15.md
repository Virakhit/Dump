# LAN invitation investigation, 15 November 2026

Baseline: `codex/workspace-approval-hotfix`, HEAD
`05e156978dfaaebbd9ac2081d02c8b64606fd7b0`, source version 0.1.3.
No new source defect was reproduced and no application code was changed.
The physical LAN incident remains unresolved.

## Version and flow comparison

- `v0.1.1-alpha.1` to `v0.1.1-alpha.2` changes only workflow and documentation;
  engine/network behavior is identical.
- 0.1.1 full invitations use mDNS routes and open the control stream to the
  invitation's owner every four seconds. The owner already requires the invited
  workspace to be selected. A valid authenticated request inserts a pending
  approval and returns Waiting; manual approval returns the signed workspace.
- 0.1.2 adds explicit route dialing, TCP, relay/contact support, and short LAN
  invitations. Full joins now dial learned routes before polling an established
  connection. mDNS still adds routes independently of Identify's hidden listener
  addresses. This comparison alone does not prove a broken discovery/dial path.
- Short invitations use a separate LAN protocol and live mDNS provenance, private
  QUIC routes, a code-only owner commitment probe, then a named request after
  authenticating the committed owner. A probe deliberately creates no pending
  approval. Short links expire after ten minutes and are lost on owner restart
  or workspace switch. These are distinct from full invitations.
- Both handlers enforce owner identity, selected workspace, token and expiry.
  Pending requests are scoped again in Engine::view and the React active-owner
  filter. These restrictions were not relaxed.
- Current source advances to awaiting approval only after the named owner ACK.
  Existing fixes expose requests on Files, Chat, Members and the invitation
  dialog, retain fingerprint confirmation and close consumed invitation dialogs.
  They do not prove that a missing request ever reached the historical owner.

## Read-only local artifact inspection

The Start Menu shortcut resolves to
`C:\Users\<user>\AppData\Local\Dump\dump.exe`. Windows version metadata reports
**0.1.4-alpha.1**, not 0.1.2 or this checkout's 0.1.3. File size: 577036891 bytes.
SHA-256:
`9194219176a37bac64a025e82069bf909d5f4d0403b9fe720eaefdb0bd78d50f`.
No 0.1.4 commit was found by version search in local Git history. Binary metadata
does not establish its source provenance or whether it contains these fixes.
The installed executable was neither launched nor changed.

The existing `releases/0.1.3-preview/Dump_0.1.3_x64-setup.exe` matches its adjacent
checksum:
`db07258aa6b1cfaf00e676a6ff0af670307f53571728b0efd234a4e684b1fd99`.
This is integrity evidence against the local checksum, not publisher verification
or a fresh build. The checkout's local updater manifest still names 0.1.2; no
public update service was queried.

## Checks executed

- `npm.cmd run test:ui`: all nine tests passed (mocked Tauri boundary).
- `npm.cmd run build`: TypeScript and Vite passed.
- `cargo fmt --manifest-path src-tauri/Cargo.toml --check`: passed.
- `node --test scripts/publish-update.test.cjs`: one passed.
- `git diff --check`: passed before this documentation addition.
- MSVC isolated-process command from INVITATION_TESTING.md: failed during
  compilation because `link.exe` is missing; no test bodies executed.
- Same focused test command using `+stable-x86_64-pc-windows-gnu` and
  `--target x86_64-pc-windows-gnu`: failed during compilation because
  `dlltool.exe` is missing; no test bodies executed.

Core/desktop execution and Clippy validation remain blocked by the native
toolchain. Historical test results in INVITATION_PROGRESS.md are not reruns.
No AGENTS.md or local .agents/.codex skill directory was found in this checkout
or the checked ancestor paths. The local Codex memory summary had no Dump entry.

## Next verification

First identify the installed 0.1.4 source/build and the exact versions on both
affected machines. Do not silently downgrade it or reuse its real profile for
an older candidate. Once coordinated, use the checksum-verified 0.1.3 preview
on separate test Windows users/devices, preserving existing profiles. Keep the
owner's invited workspace selected and both apps open. Test a fresh short link
and then a fresh full link separately; record actual version, invite type,
invitee progress stage, owner pending card, fingerprint approval and completed
membership. Never record invitation tokens in diagnostic reports.

Finding owner with no pending card points to discovery/route/probe work still
needed. Awaiting approval means the named owner request was acknowledged; if
the owner still has no visible card, capture selected workspace and request/UI
state. Compare fingerprints before approving. Confirm on two physical devices;
loopback tests cannot validate multicast, firewall policy or the affected LAN.
No firewall/network changes, user data reset, installation, external services,
commit, push, release, deploy or WisePOS actions were performed.

## Follow-up evidence and investigation stopped

Read-only follow-up found the installed executable contains, byte-for-byte at
offset 52283383, the 88078-byte Brotli asset
`src-tauri/target/debug/build/dump-1ef0b9ab74add961/out/tauri-codegen-assets/cf64bf3b79b2d7f43f96ec8375b99eb8bd272cc5b224616fa22c1bac8d08fed2.js`.
Asset SHA-256 is
`0661d80ecbc1a55e73610e87a9884a304e31321f445419ff20965e803bfcca7e`.
Decompressing it produces the request notice, Review requests, empty Members
guidance, authenticated-owner progress title, fingerprint confirmation and
Tailscale UI. Thus the installed frontend DOES contain the visibility fix;
plain-text searching the compressed executable initially missed it.

The existing build dependency file identifies a `dump-tailscale` source path;
existing build output records the GNU target and the local target directory
contains a 0.1.4-alpha.1 debug installer. The local Tailscale commit `8182935`
descends from `05e1569`. These support local Tailscale-build provenance, but do
not establish an exact source tree for the installed binary. The installed
binary and current target/debug/dump.exe have different hashes. Authenticode
reports NotSigned. Neither executable was run.

GitHub read-only release, branch and recent Actions inspection showed 0.1.3
as the latest published release, with successful Windows checks on exact hotfix
HEAD 05e1569 (run 42177050460). No 0.1.4 release appeared in that inventory.
These remote reads supersede the earlier statement that no external services
were queried; no remote mutations or workflow dispatch occurred.

The Rust GNU toolchain DOES include self-contained dlltool.exe. Adding only
its existing directory to the child shell PATH gets past lookup, but dlltool
then fails CreateProcess while making bcryptprimitives import libraries (a
supporting GNU tool is missing); focused test bodies still did not execute.
vswhere found Build Tools without the VC x64 component; targeted searches found
no complete alternative compiler/toolchain. Git's usr/bin/link.exe is not the
MSVC linker. Both Desktop Commander devices were Offline; no other machine
was contacted or altered.

Investigation stopped on the user's decision to restart development from 0.1.1.
No downgrade, install, firewall/security change or real-profile test occurred.
