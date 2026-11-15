# Standalone development build verification — 2026-11-15

Prepared locally in the isolated v0.1.1 worktree. This is a Windows x64 development
executable with embedded frontend, not an installer/release. No feed/signing change,
installed-app replacement, commit, push, deployment or release occurred.

## Deliverables

Folder: .tools/standalone-dev
Executable: .tools/standalone-dev/Dump-Restart-Dev.exe
Archive: .tools/Dump-Restart-Dev-20261115a.zip
EXE SHA-256: 298a74fbcc6213d3c679d216c02e9b9967dfa340a0bd7e941590ad9e95813c39
ZIP SHA-256: 030ba98219953767bce10f9c49059a4c0f5f2ab8de1ce40cb7529bc1882fb416

ZIP has exactly six files: executable, build-config.json, Launch-Dev.ps1,
README.txt, manifest.json, SHA256SUMS.txt. No state.bin, WebView folder, identity,
profile, proof snapshot, actual invite or peer/address inventory is included.
These are checksums for integrity, not an Authenticode publisher signature.
The EXE is unsigned; stop if host execution policy blocks it, without bypassing
or changing protection. WebView2 runtime must already be available on test devices.

## Isolation verified before launch

New override src-tauri/tauri.standalone-dev.conf.json sets:
- identifier app.dump.restart.dev20261115a (old identifier app.dump.desktop);
- product/window name Dump Restart Dev / isolated profile;
- empty deep-link schemes (no installed dump:// handler registration);
- bundle inactive; no installer, updater artifact or signing action;
- no devUrl; built with tauri/custom-protocol to embed frontend.

Base config's feed, public key, signed-version requirement and signing/release
workflow were not changed. Updater check/install remain hard-disabled compiled
code; standalone production dependency graph contains no updater plugin.

Tauri 2.12.1 path source uses config identifier for app_local_data_dir; Windows
WebView defaults to that same directory. Single-instance Windows mutex/class/window
names derive from config identifier. Source has no register_all call. No app data
or window data override is present in base/standalone config. The compiled EXE
contains the new identifier/title and not app.dump.desktop.

Before launch the new profile did not exist. Process/user/machine WebView data
and profile-argument overrides were absent, and WebView policy roots were absent.
All legacy files and dump URI command values were read-only snapshotted locally.
Launch was performed only after those checks. The portable launcher's positive
CheckOnly preflight and a temporary process-only override negative case both pass;
the latter stops before launch and restores the process environment. No persistent
setting or policy was changed. On other devices, run that preflight first and
stop if isolation cannot be established; do not assume their policy matches here.

## Actual smoke result

- GUI opened at http://tauri.localhost/ without a development server.
- Correct separate title; initially no workspaces; Updates disabled v0.1.1 label.
- Created only Standalone smoke 2026-11-15 in the development profile.
- Host relay remained disabled by default; online-only/no-cache limits visible.
- Observed six WebView subprocesses using the new identifier's data directory.
- New state.bin and EBWebView were created at:
  C:/Users/verak/AppData/Local/app.dump.restart.dev20261115a
- Closed the development app normally; no standalone process remains.
- All 298 legacy Dump files (content hashes/file list) and installed dump:// command
  values matched both during the smoke and after close. Installed EXE SHA-256
  remains 9194219176a37bac64a025e82069bf909d5f4d0403b9fe720eaefdb0bd78d50f.
- Existing native locked validation remains in LOCKED_NATIVE_VERIFICATION.md;
  standalone config regression test and full current UI suite pass.

Private local evidence is under .tools/standalone-proof (not in the ZIP). Build
log is .tools/standalone-build.log; .tools/build-standalone.ps1 records the local
MSVC/TAURI_CONFIG build. The build reuses ignored native target outputs; neither
base checkout nor installed executable is used as a launch target.

## Three-device procedure

The full procedure is README.txt inside the ZIP. Summary:
1. H has existing LAN and Tailscale reachability; A is LAN member; B is Tailscale
   source on another existing network. Use existing policy permitting H on both.
   Run preflight then launch the SAME standalone build on all, separate generated
   device profiles. Never copy profiles/keys between devices or overwrite old app.
2. H creates a NEW test workspace, enables its host relay, and shares ONE fresh
   full dump://connect invitation to A and B. PASTE it in this dev app's Join;
   clicking the URL may open the old installed app, whose handler stays unchanged.
3. Compare both device fingerprints via trusted conversation/display; H approves
   each device separately with this test workspace selected.
4. B shares a disposable small file. A receives into a fresh test folder. Require
   actual Via workspace host route and independent source/destination SHA-256
   match. Direct completion is not relay evidence. Use an existing topology where
   A/B cannot directly connect; do not change firewall/ACL/VPN/router to force it.
5. Test source/host app closed, cancellation, and revocation only in the disposable
   development workspace. No corrupt/completed output should be accepted. Larger
   than 256 MiB / 8 GiB tests require coordinated disk/time; 8 GiB remains unrun.

BOTH SOURCE AND HOST MUST STAY ONLINE. There is no host file cache, no download
from an offline source, and no offline chat queue. Real physical LAN/Tailscale
acceptance is still pending. No network-security setting was changed to obtain
these results; a blocked prompt or route must be reported, not bypassed.

