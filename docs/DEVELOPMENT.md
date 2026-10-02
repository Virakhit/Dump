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
npm run tauri build
```

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

