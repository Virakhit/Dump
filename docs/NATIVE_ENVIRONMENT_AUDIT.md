# Native verification environment and CI audit — 2026-11-15

All activity stayed in the isolated dump-v0.1.1 worktree or read existing tools.
No install, firewall/network-policy change, app launch, commit, push, PR, workflow
invocation, package, release or deployment. Both Desktop Commander devices are
currently offline. WSL --list --quiet fails 'file cannot be accessed'; no usable
WSL environment is established. No accessible alternate remote runner was used.

## Existing tool discovery

MSVC Rust exists but vswhere reports no installed VC x86/x64 tools component.
GNU Rust includes rustc, gcc linker driver, ld, dlltool and Rust libraries, but no
complete native C compiler/assembler setup was found in the targeted existing
Rust/Cargo/Git/JetBrains/Visual Studio/workspace/download/cache paths. Exact
registry libp2p-stream =0.5.0-alpha is absent from Cargo registry cache/index.
A later vendored copy in the original checkout documents a connection-map cleanup
patch; it was not substituted into the real manifest or Cargo registry cache.

A previous cached-artifact link failed nanosleep64. Read-only inspection found
Rust's bundled libpthread.a/libwinpthread-1.dll lacks that symbol, while the
existing C:/Program Files/Git/mingw64/bin/libwinpthread-1.dll contains it.
The ignored .tools/check-cached-metadata.ps1 now has opt-in ExistingPthread and
RunTests options: pass the existing Git DLL as a linker input and put its existing
directory first in the child test PATH. No DLL was replaced/copied/installed,
no fake symbol shim was introduced and no system PATH/config changed. Native
test executables stay in ignored .tools/cached-tests. ReuseCore is only for
unchanged previously compiled core; CoreUnit compiles actual current unit sources.
These executables use cached compatible later GNU libraries including patched
libp2p-stream, NOT a resolved exact Cargo.lock build. This distinction remains a
release gate even when a test body passes.

## Native socket blocker

The three-node relay test reaches native execution but fails at initial owner
join: 'Failed to negotiate transport protocol(s)' and 'Handshake with the remote
timed out' for explicit 127.0.0.1 QUIC. Owner receives no request; no relay/file
success can be inferred. A separate Node dgram probe bound one loopback UDP
receiver on an OS-selected port and sent from another UDP socket in the SAME
process. Bind/send reported no errors, but no datagram arrived before 2500 ms.
This demonstrates a general loopback UDP failure in this execution context; it
does not identify firewall/security software as the cause. No bypass or setting
change was attempted. There is no physical LAN/Tailscale acceptance evidence.

Necessary next step: an authorized existing Windows execution environment where
ordinary loopback UDP is delivered, plus the original pinned registry dependency
and usable native toolchain. Re-run locked Cargo core/desktop/updater/Clippy there.
A fully provisioned CI runner is another option only after separately authorized
commit/push/PR/workflow action; do not claim this checkout passed CI now. If tools
or dependencies must be installed/restored/downloaded, coordinate that explicit
step rather than changing network/security settings or substituting versions.

## Read-only workflow audit

Isolated worktree: HEAD ffb6c1549528e1bf8c904363a55ee1ea285c6960, dirty sources.
Its .github/workflows/check.yml triggers push to main and pull_request, uses
contents:read, windows-latest, checkout/setup-node24, rustup installation/npm ci,
frontend/UI, locked core/updater/library tests, publisher test and Clippy. There
is NO installer-preview job, workflow_dispatch or release write in this check
workflow. Local main ref 5db96d9 also lacks that preview job.

The later codex/workspace-approval-hotfix ref 05e1569 includes a PR-only
installer-preview job after successful checks. That job builds a manually
installable NSIS preview with createUpdaterArtifacts=false and uploads installer
plus SHA-256 as a 14-day Actions artifact. It does not install the app or publish
a release, but creates a distributable artifact and does NOT disable its updater
client. It belongs to later source; it is not evidence for this worktree.

The isolated release.yml is a separate high-impact path: push tags v* builds
using signing secrets and tauri-action, creates draft release/assets with
contents:write; release published triggers announce job, checks out main and runs
publish-update.cjs to announce the signed channel. No such event was triggered.
A PR/check run must contain the exact reviewed commit/lockfiles and tests. Current
uncommitted source cannot be validated by unrelated main results. No commit,
push, PR, workflow run or release was created. CI cannot replace the three-device
LAN/Tailscale matrix or resolve updater policy by merely making an installer.

## Reproduction commands (PowerShell, from isolated worktree)

The helper is deliberately ignored/local; it is not a CI substitute. No network
fetch or install is performed by these cached-artifact commands:

```powershell
& .tools/check-cached-metadata.ps1 -RunTests -ExistingPthread -Tests @('join_contract','invite_routes','route_selection')
& .tools/check-cached-metadata.ps1 -RunTests -ExistingPthread -CoreUnit -Tests @()
& .tools/check-cached-metadata.ps1 -RunTests -ExistingPthread -Tests @('host_relay')
```

The final command currently returns failure (4 passed / 1 failed), not green.
Latest logs: .tools/native-contracts.log, .tools/native-bridge-unit.log,
.tools/host-native-cached.log; exact offline Cargo failure: .tools/host-locked.log.
Passing native count is 18 distinct tests across those suites, excluding repeats.
The new tracked test-only module is src-tauri/src/network_host_tests.rs. It uses
bounded channels, real Noise upgrades and real serve_file_request; it supplies
approved signed fixtures without a network approval exchange. It proves neither
QUIC delivery, host control admission over sockets nor destination commit.

Current public main was also fetched read-only from
https://raw.githubusercontent.com/Virakhit/Dump/main/.github/workflows/check.yml .
It DOES contain installer-preview (PR-only, needs check, 14-day artifact) with the
behavior described above for local 05e1569. Local main=5db96d9 is stale relative
to that public file; it must not be described as current remote main. The remote
file check is source evidence only, not a workflow run or a passing build result.

Follow-up: the authorized updater guard is now implemented. Production no longer
registers or compiles the updater client under default desktop, IPC always denies,
and optimized native policy tests pass. See UPDATER_POLICY_VERIFICATION.md. Earlier
sections recording the proposal describe the investigation snapshot, not a still
open decision about enabling the legacy feed.

## Selected-computer clarification after connected-environment report

The parent reported both Virakhit computer and Cloud computer connected. That is
compatible with the following narrower tool observations; Desktop Commander is a
separate connector and its offline result must NOT be treated as proof that the
native Virakhit computer or Cloud connection is offline.

Direct exec identity in this subthread was verified:
- MachineName: VIRAKHIT; user verak; Windows kernel 10.0.26300; PowerShell Core.
- cwd C:/Users/verak/Desktop/DUMP/Dump; tests executed in its isolated .tools worktree.
- Computer Use inventory is also Windows with native Windows applications. No
  cloud/Linux desktop surface was offered in that inventory. No app was controlled.
- Desktop Commander separately lists Virakhit ed6b0295-90a3-49c4-89af-d21cafb06135
  and DESKTOP-3QDCK86 offline. That connector report does not describe the native
  exec route, which is demonstrably working on VIRAKHIT.

The Cloud environment runtime skill was read. It requests environment_status if
available; exhaustive available-tool name/description search in THIS delegated
turn exposes neither cloud_environment.environment_status nor a cloud shell or
machine/environment selector. exec_command exposes only the current shell/cwd,
not an environment_id selection argument. Therefore no Cloud environment ID,
installed toolchain, dependency cache or UDP result was verified. The user-facing
connected indicator must not be contradicted by claiming Cloud is offline.

What is needed: expose/attach the already connected Cloud computer's shell to the
active task/subthread (or let the parent with that route run the same checks).
Then first verify its environment_id/hostname/OS, existing checkout/revision and
dirty state, installed Rust/native linker and original locked registry dependency,
and a bounded same-process UDP loopback probe. Do not install, transfer source,
clone, alter network policy or assume it has Windows tooling before inspecting.
Linux alone cannot validate this Windows-only core/runtime or installer; a generic
Linux UDP success would only prove that environment's UDP. The exact dirty source
must be available before any claimed locked build of this worktree.

Existing accessible route remains restricted Windows exec on VIRAKHIT, with the
already documented locked dependency/toolchain and loopback UDP blockers. No new
software, policy change, UI shell workaround, push, CI or release was performed.

Authorization update: official build prerequisites and dependency downloads were
subsequently approved. Build Tools installation, successful locked fetch and UDP
host-context follow-up are recorded in LOCKED_NATIVE_VERIFICATION.md. Earlier
missing-tool/no-install paragraphs are historical snapshots, not current status.
