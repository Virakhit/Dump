# Dump

A Windows desktop app for sharing files directly with trusted people on the same local network.
Files stay on the owner's device until another member receives them. No account, cloud file storage, or Dump backend is required.

**Status:** v0.1.1 alpha. The initial target is Windows 11 x64 and small groups of friends or teammates. See [verification results](docs/VERIFICATION.md) for tested behavior and outstanding release checks.

## Install on Windows

**[Download the Windows installer](https://github.com/Virakhit/Dump/releases/download/v0.1.1-alpha.2/Dump_0.1.1_x64-setup.exe)** — Windows x64.

[Release notes and updater signature](https://github.com/Virakhit/Dump/releases/tag/v0.1.1-alpha.2). Download the `.exe` installer from the release assets; the source-code ZIP is for developers.

1. Download and run `Dump_0.1.1_x64-setup.exe` on each Windows device.
2. Click **Next**, choose an installation folder, and click **Install**. Dump installs for the current Windows user.
3. On the final wizard screen, leave **Run Dump** selected and click **Finish**.
4. Later, open **Dump** from the Windows Start Menu or the desktop shortcut if you chose to create one.

**No terminal commands are needed to use the installed app.** The installer includes the compiled application, its interface, and the WebView2 offline installer. End users do not need Node.js, npm, Rust, or a development server.

The current alpha installer is unsigned. Silent installation and launch from the installed folder have passed on the development Windows machine; a clean Windows machine and interactive wizard navigation still require verification. To uninstall, use Windows **Settings > Apps > Installed apps > Dump**.

## Update the app

Click **Check for updates** in the sidebar. If a newer release is available, finish any transfers and click **Update to …**. Dump downloads it from GitHub, verifies its signature and signed version, then closes and restarts through the installer. Your device identity and workspaces are kept. Checks happen only when you click the button and require internet access; file sharing stays on the LAN.

Updates follow published GitHub releases, including alpha previews. A draft release does not announce an update. Version 0.1.0 does not contain an updater: install 0.1.1 manually once to enable future in-app updates.

## Share your first file

1. Open Dump on both devices on the same local network.
2. Optionally set your device name from the identity panel.
3. Create a workspace and copy an invitation to your friend through your existing conversation.
4. Your friend opens the invite or pastes it into **Join with an invite**.
5. Compare the friend's fingerprint through a trusted conversation, then approve their device in **Members**.
6. Drop files into the workspace or choose them with the file picker.
7. Your friend selects **Receive** and chooses a destination folder.

The owner must be online with the relevant workspace selected to approve joins. Once approved, members can transfer to each other without the workspace owner online. Only the currently selected workspace is active. Switching workspaces cancels unfinished transfers after confirmation. Creating a workspace or accepting a join approval also selects that workspace and cancels unfinished transfers. Closing Dump stops all sharing. File drops are accepted only inside the drop zone in the Shared files view with no dialog open.

Sharing does not duplicate the original file. Files being hashed or served are temporarily opened read-only with write/delete sharing disabled. Changed files receive a new signed manifest; unavailable files disappear from the active catalog. Empty files and files larger than 4 GiB use the same streaming path.

## Privacy and limits

- QUIC authenticates device keys and encrypts traffic; workspace membership is checked separately on every request.
- Only authorized members receive catalogs. mDNS still exposes running devices' IP addresses and Peer IDs on the LAN.
- Invitations expire in 24 hours, approve one device, and do not grant membership without owner approval.
- Removal takes effect on a device when it receives the latest owner-signed membership. Isolated devices may continue granting old permissions. Downloaded copies cannot be recalled.
- Private keys, workspace secrets, invites awaiting approval, local paths, and persisted membership are protected by Windows user-scoped DPAPI. There is no recovery/export workflow in v0.1; protect your Windows profile. Moving the data to another Windows account is unsupported.
- Files are verified before completion and never overwrite an existing destination. Failed/cancelled downloads use only recorded Dump-owned `.part` files for cleanup.
- Two outgoing and two incoming transfers may run at once, at most one in each direction per peer. Outgoing receive requests queue locally; a busy serving peer rejects excess requests so control traffic remains available. Retry these requests after a transfer finishes.
- Protocol safety ceilings: 128 members per workspace, 10,000 advertised files, 100 queued/active transfers, 32 pending join requests, and 128 live invite records.
- The initial product target is small groups of up to ten devices. Protocol ceilings are defensive bounds, not tested capacity claims.
- LAN clients must be able to reach each other. Wi-Fi guest/client isolation or blocked UDP/multicast may prevent discovery. Use the network's approved firewall configuration; Dump does not disable protection or create port-forwarding rules.
- In the published 0.1.1 installer, Internet discovery, NAT traversal and relays are unavailable. Resume, folders, previews, background update checks and telemetry are also outside this milestone. App updates use GitHub only after you request a check.

## Troubleshooting

| Problem | What to check |
|---|---|
| A friend or file is missing | Keep Dump open on both devices, use the same LAN, and select the same workspace. Guest Wi-Fi and client isolation may prevent devices from reaching each other. |
| A join request is waiting | The owner must keep Dump open, select the invited workspace, and approve the device in **Members** after comparing fingerprints. |
| An invitation is rejected | Ask the owner for a new invite. Invites expire after 24 hours and are bound to one approved device. |
| A transfer fails because the sender is busy | Wait for an existing transfer to finish, then select **Receive** again. |
| A destination file already exists | Choose a different destination folder. Dump does not overwrite existing files. |
| A shared file changes or disappears | Wait for the owner to finish preparing the new version. The old manifest cannot be used to receive changed contents. |

Check the network's approved firewall configuration if discovery or connections fail. Internet connections between different networks are outside v0.1's scope.

## For developers

Building from source is optional. See the [development guide](docs/DEVELOPMENT.md) for prerequisites, development commands, installer builds, and tests.

The v0.2 Internet milestone is implemented in source and locally tested: Identify, bounded direct dialing, AutoNAT v2, authenticated TCP fallback, end-peer encrypted relay circuits, DCUtR, opt-in forwarding with resource ceilings, signed invitation contacts and authorized contact exchange. Known LAN/direct routes take priority; direct upgrades preserve active relay transfers. Legacy device identities, signed workspaces and LAN invitations remain usable; Internet invites with stale hosted-circuit locators require refresh. There is no default or mandatory Dump relay.

The stabilization source separates the **Device Peer ID** used for membership/file signatures from a persistent **Relay Peer ID** used by the forwarding host. Both private keys use the existing Windows user-scoped DPAPI state. Upgrading old source state creates the relay key once and leaves the device identity and signed workspaces unchanged. Previously enabled hosting is turned off until explicitly configured again. Share the new Relay Peer ID, update old hosting address suffixes and distributed relay locators, and create fresh invitations containing the new circuit route. Saved remote relay pins are preserved; operators must explicitly update a changed remote relay locator.

AutoNAT can revalidate candidates during an open session without disconnecting transfers. Evidence still expires; having no usable probe server reports Unknown and does not prevent LAN use. When a file uses a relay, the UI warns that circuit traffic and lifetime limits can interrupt it. The built-in host allows 256 MiB of combined circuit traffic and ten minutes per circuit; file bytes, other streams and encryption overhead share that budget. Remote limits and remaining capacity are not known reliably. A failed receive never commits partial bytes; try a direct route or a smaller file and start a new receive manually. There is no resume or automatic transfer retry.

To test different-network invitations, both devices need this source build and the owner needs a reachable direct route or a configured trusted relay. Open **Network settings > Advanced connectivity**, add a reachable relay locator, restart Dump, then create the invitation after its reservation is ready. The recipient opens it and follows the usual fingerprint/approval/share/receive flow. Raw addresses stay in advanced settings. For community hosting, see the [opt-in role and setup steps](docs/PROTOCOL.md#opt-in-hosting-stage-5).

The installer linked above remains the LAN-only 0.1.1 release. No new Internet installer has been built or published. The historical local matrix covers the twelve Internet regression cases. The stabilization evidence is recorded separately; local protocol exchanges do not establish success through actual NAT/CGNAT/firewalls. See the [verification matrix](docs/VERIFICATION.md) and [remaining acceptance gates](docs/PLAN.md). Before packaging this source, bump its app version as described in the development guide.

## Documentation

- [Product requirements](requirement.md).
- [Delivery plan and remaining release gates](docs/PLAN.md).
- [Protocol and trust model](docs/PROTOCOL.md).
- [Security model and reporting guidance](SECURITY.md).
- [Executed checks and known verification limits](docs/VERIFICATION.md).

## License

Source code is licensed under [MIT](LICENSE). Third-party components retain their own licenses.
