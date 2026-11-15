# Restart development from Dump 0.1.1

Prepared 15 November 2026. At worktree creation the application source was
unchanged from the selected baseline. Subsequent authorized phase-1 work is
recorded in JOIN_PHASE1_VERIFICATION.md and the workspace LAN/Tailscale plan;
the initial baseline checks below remain historical evidence for that snapshot.

## Exact starting point

- Branch: `codex/restart-v0.1.1`.
- Worktree: `C:\Users\<user>\Desktop\DUMP\Dump\.tools\worktrees\dump-v0.1.1`.
- Tag: `v0.1.1-alpha.2`.
- Commit: `ffb6c1549528e1bf8c904363a55ee1ea285c6960`.
- Package/Tauri/Rust application version: `0.1.1`.
- The local tag matches GitHub's current tag reference. The successful
  [Windows release run 36976427561](https://github.com/Virakhit/Dump/actions/runs/36976427561)
  used this exact commit and tag. The release record's `target_commitish` is a
  different historical commit (`efa5680`); the resolved tag and run head are the
  evidence used to choose the development baseline.
- `v0.1.1-alpha.1` resolves to `7f1f05e055607352a80df9b3be70e55dd9898127`.
  Alpha.2 changes only six workflow/documentation files; engine, network and UI
  behavior are identical to alpha.1. Alpha.2 is selected for its corrected release
  workflow, without importing later networking changes.

Original checkout remains on `codex/workspace-approval-hotfix` at `05e1569`.
Its user documents and investigation notes remain in place. The new worktree is
under the already ignored `.tools/` directory; no ignore-file change or commit
was needed. No reset, revert, clean, app installation, downgrade, push, release
or deployment was performed. No AGENTS.md was found in the new baseline.

## Executed baseline checks

| Check | Result |
| --- | --- |
| `git diff --exit-code v0.1.1-alpha.2 -- .` before adding this document | Pass: tracked baseline identical |
| `npm.cmd run build` | Pass: TypeScript and Vite, 37 modules |
| `cargo fmt --manifest-path src-tauri/Cargo.toml --check` | Pass |
| `node --test scripts/publish-update.test.cjs` | Pass: one test |
| `cargo test --offline --locked --manifest-path src-tauri/Cargo.toml --no-default-features` | Blocked: original registry `libp2p-stream` absent from local cache; no test bodies ran |

No dependencies were installed. Frontend tools/dependencies resolve from the
existing ancestor `node_modules`; this is a useful build check, not a clean
`npm ci` reproduction of the baseline lockfile. The source/package lockfiles
were not changed. No UI suite exists in 0.1.1. Historical verification in
VERIFICATION.md is not fresh evidence for this worktree.

Native execution also needs a complete Windows toolchain: existing MSVC lacks
`link.exe`/the VC workload; Rust GNU includes dlltool but its import-library step
fails CreateProcess with the available supporting tools. Do not install or
change machine configuration automatically. Once dependency retrieval and a
complete toolchain are separately coordinated, rerun the core suite, desktop
library/updater tests, cargo check and Clippy. Do not replace the pinned registry
stream crate with the newer vendored fork merely to make baseline compile.

## What restarting omits

The older source remains available, but later features/fixes are absent from
this development branch:

- Short `dump://lan/` invitations. Baseline uses full `dump://join/` links.
- Explicit direct-address routing, TCP/Noise, signed contact discovery,
  AutoNAT, relay assistance/hosting, hole punching, route diagnostics and
  Tailscale transport. Baseline provides local mDNS plus authenticated QUIC.
- Group chat and durable message receipts.
- Request notices across Files/Chat/dialog, direct approval in the invitation
  dialog, accurate pre-ACK progress, and consumed-dialog closing fixes. Baseline
  displays requests in Members and may say waiting before delivery.
- Later receive cleanup/error handling and stream lifecycle/security robustness
  changes, plus their expanded regression suites. Review these individually
  before choosing which to carry forward; they are not assumed unnecessary.
- Later CI checks, multiprocess invitation tests and stability runners.

Baseline retains signed workspace membership/manifests, device identity,
fingerprint-gated manual approval, protected Windows persistence, LAN file
transfer and signed in-app updater code. Returning to this source does not
establish that a current profile can safely be opened by the older executable.

## Development sequence

1. Freeze the baseline and restore reproducible checks first. Resolve only the
   required cached dependency/toolchain blocker through a separately coordinated
   environment; record source SHA, commands and results. Keep dependency versions
   and authentication unchanged while validating the baseline.
2. Make test isolation explicit before running a desktop development build.
   Existing desktop code uses `app.dump.desktop` and the normal per-user app data
   location; a second source directory alone DOES NOT isolate identity/state or
   single-instance/deep-link behavior. Prefer the existing core tests, which pass
   temporary stores explicitly, for the first local runtime checks.
3. Add a narrowly scoped invitation diagnostic/regression harness with separate
   temporary profiles, preserving owner/identity/token/expiry checks. Observe
   discovery, authenticated connection, received Join, pending request, Waiting
   ACK, manual fingerprint approval and persisted membership separately. Cover
   wrong selected workspace, restart, cancellation, rejection and replay. Avoid
   importing the entire post-0.1.1 network stack.
4. Verify the unchanged full-invite path on two physical Windows devices with
   disposable test profiles. Use an isolated portable/test build configuration
   with a distinct app identifier, data root and protocol/single-instance scope,
   or separately coordinated existing test users. Do not run the old installer
   against the current user's app or redirect real protected state. Do not alter
   firewall/security policy; if current policy blocks discovery, record it as a
   blocker. Record actual versions, route, owner request, ACK, fingerprint
   approval and a small file's matching SHA-256. Tokens stay out of reports.
5. Reintroduce only supported changes one at a time: truthful request progress
   and visibility first, then individually reviewed correctness/security fixes.
   Each change needs a failing regression, applicable core/frontend checks and
   the affected physical-device scenario. Consider chat and Internet/Tailscale
   features only after the LAN baseline has repeatable evidence.

No runtime LAN acceptance has been executed in this restart. Both available
Desktop Commander entries were Offline during the preceding inspection. A
loopback/core pass, when obtainable, cannot replace the physical two-device gate.
Version 0.1.1 here identifies the source baseline only; any later distributable
must use an appropriately newer app version under separately authorized release
work. Do not publish a rollback through the updater channel.
