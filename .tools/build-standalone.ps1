$ErrorActionPreference='Stop'
$taskRoot=(Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '..')).Path
Set-Location -LiteralPath $taskRoot
$envLines=& cmd.exe /d /c (Join-Path $PSScriptRoot 'native-env.cmd')
if($LASTEXITCODE -ne 0){throw 'Compiler environment failed'}
$names=@('PATH','INCLUDE','LIB','LIBPATH','VCToolsInstallDir','VCToolsVersion','WindowsSdkDir','WindowsSDKVersion','UCRTVersion','UniversalCRTSdkDir','VSINSTALLDIR','VCINSTALLDIR')
foreach($line in $envLines){$parts=$line.Split('=',2);if($parts.Length -eq 2 -and $names -contains $parts[0]){Set-Item -LiteralPath ('Env:'+$parts[0]) -Value $parts[1]}}
$env:PATH="$env:USERPROFILE\.cargo\bin;C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\Common7\IDE\CommonExtensions\Microsoft\CMake\CMake\bin;"+$env:PATH
$env:CARGO_TARGET_DIR=Join-Path $PSScriptRoot 'native-msvc'
$env:CARGO_BUILD_JOBS='2'
$env:CARGO_NET_OFFLINE='true'
& node --test scripts/standalone-config.test.cjs
if($LASTEXITCODE -ne 0){exit $LASTEXITCODE}
$env:TAURI_CONFIG=Get-Content -LiteralPath src-tauri/tauri.standalone-dev.conf.json -Raw
$devConfig=$env:TAURI_CONFIG | ConvertFrom-Json
if($devConfig.identifier -ne 'app.dump.restart.dev20261115a' -or $devConfig.identifier -eq 'app.dump.desktop' -or $devConfig.plugins.'deep-link'.desktop.schemes.Count -ne 0 -or $devConfig.build.devUrl){throw 'Isolation configuration is invalid'}
& cargo build --locked --manifest-path src-tauri/Cargo.toml --features tauri/custom-protocol
if($LASTEXITCODE -ne 0){exit $LASTEXITCODE}
$output=Join-Path $PSScriptRoot 'standalone-dev'
New-Item -ItemType Directory -Path $output -Force | Out-Null
Copy-Item -LiteralPath (Join-Path $env:CARGO_TARGET_DIR 'debug/dump.exe') -Destination (Join-Path $output 'Dump-Restart-Dev.exe')
Copy-Item -LiteralPath src-tauri/tauri.standalone-dev.conf.json -Destination (Join-Path $output 'build-config.json')
Get-FileHash -LiteralPath (Join-Path $output 'Dump-Restart-Dev.exe') -Algorithm SHA256 | Select-Object Hash,Path | ConvertTo-Json
