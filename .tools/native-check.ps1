param([ValidateSet('core','relay','desktop','updater','clippy','policy-release','build')][string]$Check = 'core')
$ErrorActionPreference = 'Stop'
$taskRoot = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '..')).Path
Set-Location -LiteralPath $taskRoot
$envLines = & cmd.exe /d /c (Join-Path $PSScriptRoot 'native-env.cmd')
if ($LASTEXITCODE -ne 0) { throw 'Microsoft compiler environment setup failed' }
$names = @('PATH','INCLUDE','LIB','LIBPATH','VCToolsInstallDir','VCToolsVersion','WindowsSdkDir','WindowsSDKVersion','UCRTVersion','UniversalCRTSdkDir','VSINSTALLDIR','VCINSTALLDIR')
foreach ($line in $envLines) { $parts = $line.Split('=',2); if ($parts.Length -eq 2 -and $names -contains $parts[0]) { Set-Item -LiteralPath ('Env:'+$parts[0]) -Value $parts[1] } }
$env:PATH = "$env:USERPROFILE\.cargo\bin;C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\Common7\IDE\CommonExtensions\Microsoft\CMake\CMake\bin;" + $env:PATH
$env:CARGO_TARGET_DIR = Join-Path $PSScriptRoot 'native-msvc'
$env:CARGO_NET_OFFLINE = 'true'
$env:CARGO_BUILD_JOBS = '2'
$argsList = @('test','--manifest-path','src-tauri/Cargo.toml','--locked')
switch ($Check) {
  'build' { $argsList = @('build','--manifest-path','src-tauri/Cargo.toml','--locked') }
  'core' { $argsList += '--no-default-features' }
  'relay' { $argsList += @('--no-default-features','--test','host_relay','--','--nocapture') }
  'desktop' { $argsList += '--lib' }
  'updater' { $argsList += @('--features','updater-tests','--test','updater') }
  'policy-release' { $argsList += @('--release','--no-default-features','--test','update_policy') }
  'clippy' { $argsList = @('clippy','--manifest-path','src-tauri/Cargo.toml','--all-targets','--locked','--','-D','warnings') }
}
& cargo @argsList
exit $LASTEXITCODE
