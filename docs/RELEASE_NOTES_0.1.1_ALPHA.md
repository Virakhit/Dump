# Dump 0.1.1 Alpha — In-app updates

Download **Dump_0.1.1_x64-setup.exe**, run the installer, and launch Dump. No Node.js, npm commands, or Rust toolchain are needed.

This release adds **Check for updates** to the sidebar. When a newer published version exists, click **Update to …** to download, verify, and install it. Dump restarts afterward and keeps your device identity and workspaces. Finish or cancel transfers and file preparation first. Checks happen only when you click and require internet access; file sharing remains on the LAN.

The updater verifies the pinned signing key and the version bound into the signature. It refuses tampered downloads and older/equal versions. Published alpha releases are included in the update channel; drafts are excluded.

**Existing 0.1.0 users:** install this version manually once. That original release has no updater; subsequent updates can be installed through the app.

This is an **alpha preview**. The updater's signature is separate from Windows publisher signing: the installer still has no Authenticode certificate. Clean-machine installation, physical multi-machine LAN checks, and the user pilot remain pending. See `docs/VERIFICATION.md` for recorded checks and limits.
