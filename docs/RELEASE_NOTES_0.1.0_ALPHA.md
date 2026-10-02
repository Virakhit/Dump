# Dump 0.1.0 Alpha — Windows Installer

Download **Dump_0.1.0_x64-setup.exe** from the assets below. The source-code archives are for developers.

Run the installer, follow the wizard, and leave **Run Dump** selected on the final screen. Later, launch Dump from the Windows Start Menu. No npm commands, Node.js, Rust, or development server are required. The installer includes the WebView2 offline installer and installs for the current Windows user.

## What you can try

Create a workspace, invite a trusted friend on the same LAN, compare device fingerprints, approve membership, and share or receive files directly. Transfers use authenticated QUIC and verify signed manifests and SHA-256. Approved members can transfer while the workspace creator is offline; each file owner still needs Dump open to serve their files.

## Verification and limits

The installer is for Windows x64 and the initial target is Windows 11. Local checks include compilation, core/integration tests, an 8 GiB authenticated loopback transfer, silent installation, Start Menu/protocol registration, and launch of the installed app without a development server.

This is an **unsigned alpha preview**, not a validated beta. Interactive wizard navigation, clean-machine installation/uninstallation, native drag/drop and DPI checks, multi-machine LAN/firewall checks, and the user pilot remain open. Windows may identify the installer as an unknown publisher. Internet/NAT/relay connections, resume, folders, auto-updates, and telemetry are not included. Removal propagates when peers receive updated membership; downloaded copies cannot be recalled.

Installer size: **219,279,799 bytes**. Verify it against the attached `.sha256` file:

```text
c35eb85ecf6df7e892cc95f83f6cf0d8a6b20fbaac2c983b2ba93fbfbc83b258  Dump_0.1.0_x64-setup.exe
```

Full usage and current verification details are in the repository README and `docs/VERIFICATION.md`.
