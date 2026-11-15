# Rebuild updater gate: 15 November 2026

**Development/preview updater is now disabled in source, as authorized.**
Locked native desktop build/tests and optimized policy tests now pass. Packaging
and physical acceptance remain unperformed; no app was handed out.
See UPDATER_POLICY_VERIFICATION.md for current code/tests. The risk evidence below
describes the original configuration, which remains unchanged.
This is a verified configuration/channel risk, not evidence that an update was
offered, downloaded or installed by the real app. No updater/feed/signing/version
configuration was changed in this investigation.

The worktree's package/Tauri/Cargo versions remain 0.1.1. Tauri config points to
https://raw.githubusercontent.com/Virakhit/Dump/main/releases/latest.json with
the original pinned public key and requireSignedVersion=true. A read-only fetch
of that feed returned version **0.1.3**, pointing to the published 0.1.3 alpha
installer. Alpha is the release tag; the updater manifest's version is numeric.

Source evidence: desktop::check_update uses the configured updater without a
custom version comparator/channel restriction. The pinned tauri-plugin-updater
2.13.1 source defaults to versions newer than the running package. Its download
verification checks installer signature and the version in the authenticated
trusted comment. The publisher script requires complete signed assets and a
strictly increasing numeric version; it does not identify this rebuild branch.
Existing updater fixtures cover older/equal rejection, tampering and inflated
version paired with an older signed artifact. Those native tests were not rerun.

Read-only signature check against the configured public key:

- Minisign packet key ID matches the pinned key.
- Ed25519 global/trusted-metadata signature verifies.
- Authenticated comment binds file Dump_0.1.3_x64-setup.exe and version 0.1.3.
- Installer bytes were NOT downloaded; the installer-content/prehash signature
  was NOT verified in this round. Metadata verification cannot replace that step.

Therefore a rebuilt app still identified as 0.1.1 can legitimately regard the old
0.1.3 line as newer. Publisher authenticity and version binding prevent tampering
and version substitution; they do not separate development branches. The UI
checks only on explicit user action (no startup auto-check), and installation is
a further explicit action; neither was invoked here.

The chosen authorized mitigation is now to disable the rebuild updater entirely
in compiled source. Full native desktop/packaging validation is still required.
A future enabled channel/endpoint needs an approved version and signing policy.
A version bump alone does not prevent a later legacy release from crossing back
into this branch. Preserve the old channel for its users. Do not replace public
assets/feed, change the pinned key or assume signatures distinguish source lines.
Test the chosen policy against the existing signed old release plus older/equal,
tampered and wrong-channel fixtures before approving a distributable build.

Public read evidence: [current feed](https://github.com/Virakhit/Dump/blob/main/releases/latest.json).
Local read snapshot is .tools/updater-feed-read.json (ignored). No release/feed
write, installer fetch, signature/key mutation or application update occurred.

## Development-build policy (implemented; native desktop validation pending)

The application check/install commands unconditionally deny through the compiled
update_policy module. Production registers no updater plugin and has no cached
Update object or network/download/install implementation. Default desktop feature
no longer enables the updater dependency; updater-tests enables that dependency
only for isolated plugin fixtures. It does not enable application update policy.
There is no environment, feature or runtime setting to turn this policy on.
The frontend hides updater controls and displays Development / preview build,
Updates disabled; missing policy information also hides the controls.

This applies in optimized release mode as well as debug. Feed, public key, signed
version requirement, publisher/signing workflow and installed app are unchanged.
A future enabled updater needs a separately reviewed source/channel change; do not
restore the legacy client or toggle ENABLED without that migration decision.
Tests and remaining build limitations are in UPDATER_POLICY_VERIFICATION.md.

The later preview workflow's createUpdaterArtifacts=false only suppresses
creation of updater bundles/signatures. It does NOT disable the updater client
already compiled into a preview or change which feed it accepts. Consequently an
unsigned/manual preview installer is not, by itself, protection from this risk.
Only local application command/plugin registration and dependency feature policy changed.
No public feed, key, signing setting, installed app or release was changed.
