# Developing Dump

## Prerequisites

- Windows 11 x64.
- Node.js 24 and npm.
- Rust with the MSVC toolchain, plus Cargo, Rustfmt, and Clippy.
- Microsoft C++ Build Tools with the **Desktop development with C++** workload.
- Microsoft Edge WebView2 Runtime.

Run these commands from the project directory:

```powershell
npm ci
npm run tauri dev
```

For an interface-only browser preview, run `npm run dev`. Workspace actions and file transfers require the desktop app.

## Build the installer

```powershell
$env:TAURI_SIGNING_PRIVATE_KEY = "C:\path\to\dump-updater.key"
$env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD = ""
npm run tauri build
```

Signed updater artifacts require the original private signing key. This checkout contains only its public key. The local key created for this project is stored outside the repository at `%USERPROFILE%\.codex\keys\dump-updater.key`, with access restricted to the Windows user. Back it up securely; losing it prevents updates for existing installations. Never commit the private key or place it in release assets. The updater signature is separate from Windows Authenticode publisher signing.

The Windows x64 NSIS installer is generated in `src-tauri/target/release/bundle/nsis/`. The build includes the WebView2 offline installer; its first download requires an Internet connection. Copy the generated installer into the ignored local `releases/` directory when preparing a distributable build, and generate a matching SHA-256 checksum. Upload the installer and checksum as GitHub Release assets, labelling previews as alpha prereleases until the release gates pass; do not commit installer executables. The checksum file in this checkout applies to the existing local alpha installer, not to later rebuilds.

## Run checks

```powershell
npm run build
npm run test:core
cargo check --manifest-path src-tauri/Cargo.toml
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
```

The Windows core tests run real authenticated QUIC connections on loopback. They do not establish multi-machine discovery, firewall behavior, installer behavior on a clean machine, or user validation. See [SECURITY.md](../SECURITY.md) and [docs/PLAN.md](PLAN.md) for the remaining release gates.

The optional large-file check writes and transfers 8 GiB and requires at least 18 GiB of free temporary disk space:

```powershell
cargo test --release --manifest-path src-tauri/Cargo.toml --no-default-features --test lan eight_gib_streaming_transfer -- --ignored --nocapture
```

Current local evidence is recorded in [docs/VERIFICATION.md](VERIFICATION.md). This is an alpha build; review those limits before distributing it.

## Publish an update on GitHub

The repository's `TAURI_SIGNING_PRIVATE_KEY` Actions secret holds the updater signing key. `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` is optional for an encrypted key. Use the same key for every release.

1. Increase the numeric `major.minor.patch` app version in `package.json`/`package-lock.json`, `src-tauri/Cargo.toml`/`Cargo.lock`, and `src-tauri/tauri.conf.json`. Each release must increase the app version, even for alpha previews.
2. Commit and push, then push a matching tag (for example `v0.1.2-alpha.1` for app version `0.1.2`, or `v0.1.2` for a validated stable release).
3. The **Windows release** workflow builds a draft GitHub release containing the installer, `.exe.sig`, and `latest.json`. Source tests run separately in **Windows checks** on `main`.
4. Verify that **Windows checks** passed for the release's source, review the notes/assets, and **Publish release** on GitHub. The announce job copies the manifest to `releases/latest.json` on `main`, including for alpha releases. Existing users can now click **Check for updates** and install it.

GitHub Actions needs permission to write repository contents; branch protection must permit the manifest commit. An older or equal app version never replaces the update channel. If publishing manually, include the signed installer, its signature, and Tauri's generated `latest.json` before publishing; uploading source code alone does not update installed apps.

```powershell
cargo test --manifest-path src-tauri/Cargo.toml --features updater-tests --test updater --locked
cargo test --manifest-path src-tauri/Cargo.toml --lib --locked
node --test scripts/publish-update.test.cjs
```

Updater checks exercise the real plugin's version comparison and signature verification over loopback, including tampered bytes and an inflated manifest version paired with an older signed artifact. Test fixtures are signed data and public signatures; they contain no private key. HTTP is allowed only in the loopback test context; shipping updates require HTTPS.

