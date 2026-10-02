#requires -Version 7.0
<#
Run on Windows with the repository's source frozen:
  pwsh -NoProfile -File scripts/verify-stability.ps1 [-FullValidation]
Each Cargo invocation is a fresh process. Test binaries retain normal parallelism.
Outputs are local, ignored evidence; this script does not publish or dispatch CI.
CommandRunner and RepositoryRoot are hooks for the synthetic script self-check.
#>
[CmdletBinding()]
param(
    [switch] $FullValidation,
    [string] $OutputDirectory,
    [string] $RepositoryRoot = (Split-Path -Parent $PSScriptRoot),
    [scriptblock] $CommandRunner
)

$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $false
$root = [IO.Path]::GetFullPath($RepositoryRoot).TrimEnd('\', '/')
$synthetic = $null -ne $CommandRunner
$watchers = [Collections.Generic.List[object]]::new()
$eventIds = [Collections.Generic.List[string]]::new()
$attempts = [Collections.Generic.List[object]]::new()
$changes = [Collections.Generic.List[object]]::new()
$changeCount = 0
$failureReasons = [Collections.Generic.List[string]]::new()
$baseline = $null
$output = $null
$outputCreated = $false
$status = 'INCOMPLETE'
$originalTestThreadsPresent = Test-Path Env:RUST_TEST_THREADS
$originalTestThreads = [Environment]::GetEnvironmentVariable('RUST_TEST_THREADS', 'Process')
$oldLocation = Get-Location
$startedAt = [DateTime]::UtcNow.ToString('o')
$runId = [Guid]::NewGuid().ToString('N')
$checks = [Collections.Generic.List[object]]::new()

function Add-Check($Name, $Program, [string[]] $Arguments, $Evidence = '') {
    $checks.Add([ordered]@{ name = $Name; program = $Program; arguments = $Arguments; evidence = $Evidence })
}
if ($FullValidation) {
    Add-Check 'npm-ci' 'npm.cmd' @('ci')
    Add-Check 'frontend-build' 'npm.cmd' @('run', 'build')
    Add-Check 'format' 'cargo' @('fmt', '--manifest-path', 'src-tauri/Cargo.toml', '--check')
}
for ($round = 1; $round -le 10; $round++) {
    Add-Check "focused-$round" 'cargo' @('test', '--manifest-path', 'src-tauri/Cargo.toml', '--no-default-features', '--locked', '--lib', 'network::internet_tests::invite_locator_automatically_joins_and_transfers_and_contacts_stay_private', '--', '--exact', '--nocapture') 'focused'
}
for ($round = 1; $round -le 3; $round++) {
    Add-Check "core-$round" 'cargo' @('test', '--manifest-path', 'src-tauri/Cargo.toml', '--no-default-features', '--locked') 'rust-tests'
}
if ($FullValidation) {
    Add-Check 'streams' 'cargo' @('test', '--manifest-path', 'src-tauri/Cargo.toml', '--locked', '-p', 'libp2p-stream', '--lib') 'rust-tests'
    Add-Check 'autonat' 'cargo' @('test', '--manifest-path', 'src-tauri/Cargo.toml', '--locked', '-p', 'libp2p-autonat', '--lib') 'rust-tests'
    Add-Check 'updater' 'cargo' @('test', '--manifest-path', 'src-tauri/Cargo.toml', '--features', 'updater-tests', '--test', 'updater', '--locked') 'rust-tests'
    Add-Check 'desktop-library' 'cargo' @('test', '--manifest-path', 'src-tauri/Cargo.toml', '--lib', '--locked') 'rust-tests'
    Add-Check 'publisher' 'node' @('--test', 'scripts/publish-update.test.cjs')
    Add-Check 'clippy' 'cargo' @('clippy', '--manifest-path', 'src-tauri/Cargo.toml', '--all-targets', '--locked', '--', '-D', 'warnings')
    Add-Check 'diff' 'git' @('diff', '--check')
}

function Write-Json($Path, $Value) {
    $Value | ConvertTo-Json -Depth 15 | Set-Content -LiteralPath $Path -Encoding utf8
}
function Invoke-Captured($Program, [string[]] $Arguments, $Stdout, $Stderr) {
    [IO.File]::WriteAllText($Stdout, '')
    [IO.File]::WriteAllText($Stderr, '')
    if ($synthetic) {
        $result = & $CommandRunner $Program $Arguments
        if ($null -eq $result -or $null -eq $result.ExitCode) { throw 'Synthetic runner must return ExitCode, Stdout and Stderr.' }
        [IO.File]::WriteAllText($Stdout, [string] $result.Stdout)
        [IO.File]::WriteAllText($Stderr, [string] $result.Stderr)
        return [int] $result.ExitCode
    }
    try {
        # Native nonzero exits are evidence, not terminating PowerShell errors.
        $ErrorActionPreference = 'Stop'
        $null = Get-Command -Name $Program -ErrorAction Stop
        $global:LASTEXITCODE = 0
        & $Program @Arguments 1> $Stdout 2> $Stderr
        return [int] $LASTEXITCODE
    }
    catch {
        $_ | Out-String | Set-Content -LiteralPath $Stderr -Encoding utf8
        return 127
    }
}
function Read-Tool($Tag, $Program, [string[]] $Arguments, [int[]] $Allowed = @(0)) {
    $out = Join-Path $internalDirectory "$Tag.stdout.log"
    $err = Join-Path $internalDirectory "$Tag.stderr.log"
    $code = Invoke-Captured $Program $Arguments $out $err
    if ($code -notin $Allowed) { throw "$Program $($Arguments -join ' ') exited $code; see $err" }
    return [IO.File]::ReadAllText($out)
}
function Test-InputPath([string] $RelativePath) {
    $path = $RelativePath.Replace('\', '/').TrimStart('/')
    if ($path -match '^(\.git|\.tools|node_modules|dist|docs|releases)(/|$)' -or $path -match '^src-tauri/(target|gen)(/|$)') { return $false }
    # Documentation may record results after a batch without changing its inputs.
    if ($path -notmatch '/' -and ($path -match '\.md$' -or $path -match '^LICENSE(?:\..*)?$')) { return $false }
    return $true
}
function Get-Snapshot {
    $head = (Read-Tool 'head' 'git' @('rev-parse', 'HEAD')).Trim()
    $tracked = Read-Tool 'tracked' 'git' @('ls-files', '--cached', '-z')
    $untracked = Read-Tool 'untracked' 'git' @('ls-files', '--others', '--exclude-standard', '-z')
    $kinds = @{}
    foreach ($path in $tracked.Split([char] 0, [StringSplitOptions]::RemoveEmptyEntries)) { if (Test-InputPath $path) { $kinds[$path] = 'tracked' } }
    foreach ($path in $untracked.Split([char] 0, [StringSplitOptions]::RemoveEmptyEntries)) { if (Test-InputPath $path) { $kinds[$path] = 'untracked' } }
    $paths = [string[]] @($kinds.Keys)
    [Array]::Sort($paths, [StringComparer]::Ordinal)
    $files = @(
        foreach ($path in $paths) {
            $fullPath = Join-Path $root $path
            if (-not [IO.File]::Exists($fullPath)) {
                [ordered]@{ path = $path; kind = $kinds[$path]; sha256 = 'MISSING'; bytes = $null }
            }
            else {
                $item = Get-Item -LiteralPath $fullPath
                [ordered]@{ path = $path; kind = $kinds[$path]; sha256 = (Get-FileHash -LiteralPath $fullPath -Algorithm SHA256).Hash.ToLowerInvariant(); bytes = $item.Length }
            }
        }
    )
    $canonical = ConvertTo-Json -InputObject $files -Compress -Depth 5
    $hash = [Convert]::ToHexString([Security.Cryptography.SHA256]::HashData([Text.Encoding]::UTF8.GetBytes($canonical))).ToLowerInvariant()
    return [ordered]@{ head = $head; sha256 = $hash; files = $files }
}
function Add-Change($Kind, $Path) {
    $script:changeCount++
    if ($changes.Count -lt 50) { $changes.Add([ordered]@{ kind = $Kind; path = $Path; observed_utc = [DateTime]::UtcNow.ToString('o') }) }
}
function Read-Changes {
    foreach ($id in $eventIds) {
        foreach ($event in @(Get-Event -SourceIdentifier $id -ErrorAction SilentlyContinue)) {
            try {
                if ($event.SourceEventArgs -is [IO.ErrorEventArgs]) {
                    Add-Change 'watcher-error' $event.SourceEventArgs.GetException().Message
                    continue
                }
                $paths = @($event.SourceEventArgs.FullPath)
                if ($event.SourceEventArgs -is [IO.RenamedEventArgs]) { $paths += $event.SourceEventArgs.OldFullPath }
                foreach ($fullPath in $paths) {
                    # Directory metadata notifications can follow reads. Actual
                    # tree creation/deletion/renames and file writes are watched.
                    if ($event.SourceEventArgs.ChangeType -eq [IO.WatcherChangeTypes]::Changed -and [IO.Directory]::Exists($fullPath)) { continue }
                    $path = [IO.Path]::GetRelativePath($root, $fullPath).Replace('\', '/')
                    if (-not (Test-InputPath $path)) { continue }
                    if (@($baseline.files.path) -notcontains $path) {
                        $out = Join-Path $internalDirectory 'ignore.stdout.log'
                        $err = Join-Path $internalDirectory 'ignore.stderr.log'
                        $code = Invoke-Captured 'git' @('check-ignore', '--quiet', '--', $path) $out $err
                        if ($code -eq 0) { continue }
                        if ($code -ne 1) { Add-Change 'ignore-check-error' $path; continue }
                    }
                    Add-Change ([string] $event.SourceEventArgs.ChangeType) $path
                }
            }
            finally { Remove-Event -EventIdentifier $event.EventIdentifier -ErrorAction SilentlyContinue }
        }
    }
}
function Start-InputWatchers {
    # Avoid recursive watches over Cargo/npm outputs. Watch root and src-tauri
    # shallowly; watch existing input subtrees recursively. Root directory events
    # detect creation or renaming of a new input tree during the batch.
    $directories = @{ $root = $false }
    foreach ($file in $baseline.files) {
        $segments = $file.path.Split('/')
        if ($segments.Length -lt 2) { continue }
        if ($segments[0] -eq 'src-tauri') {
            $directories[(Join-Path $root 'src-tauri')] = $false
            if ($segments.Length -gt 2) { $directories[(Join-Path $root "$($segments[0])/$($segments[1])")] = $true }
        }
        else { $directories[(Join-Path $root $segments[0])] = $true }
    }
    foreach ($directory in $directories.Keys) {
        if (-not [IO.Directory]::Exists($directory)) { continue }
        $watcher = [IO.FileSystemWatcher]::new($directory)
        $watcher.IncludeSubdirectories = $directories[$directory]
        $watcher.NotifyFilter = [IO.NotifyFilters]::FileName -bor [IO.NotifyFilters]::DirectoryName -bor [IO.NotifyFilters]::LastWrite -bor [IO.NotifyFilters]::Size -bor [IO.NotifyFilters]::Attributes
        $watcher.InternalBufferSize = 65536
        foreach ($kind in @('Changed', 'Created', 'Deleted', 'Renamed', 'Error')) {
            $id = "stability-$runId-$($eventIds.Count)"
            Register-ObjectEvent -InputObject $watcher -EventName $kind -SourceIdentifier $id | Out-Null
            $eventIds.Add($id)
        }
        $watcher.EnableRaisingEvents = $true
        $watchers.Add($watcher)
    }
}
function Assert-Frozen {
    $snapshot = Get-Snapshot
    Read-Changes
    if ($snapshot.head -ne $baseline.head -or $snapshot.sha256 -ne $baseline.sha256 -or $changeCount -gt 0) {
        Write-Json (Join-Path $output 'drift-snapshot.json') $snapshot
        throw [InvalidOperationException]::new('SOURCE_DRIFT: HEAD, input contents/inventory or a watched input changed. Stop and start a new batch after freezing source.')
    }
    return $snapshot
}
function Write-Summary {
    Write-Json (Join-Path $output 'summary.json') ([ordered]@{
        status = $status; execution_mode = $(if ($synthetic) { 'synthetic-self-check' } else { 'windows-runtime' }); runtime_evidence = (-not $synthetic)
        started_utc = $startedAt; updated_utc = [DateTime]::UtcNow.ToString('o'); output_directory = $output
        head = $(if ($baseline) { $baseline.head } else { $null }); source_sha256 = $(if ($baseline) { $baseline.sha256 } else { $null })
        expected_attempts = $checks.Count; completed_attempts = @($attempts | Where-Object { $null -ne $_.exit_code }).Count
        complete = ($attempts.Count -eq $checks.Count -and @($attempts | Where-Object { $null -eq $_.exit_code }).Count -eq 0)
        focused_attempts = @($attempts | Where-Object { $_.name -like 'focused-*' }).Count; core_attempts = @($attempts | Where-Object { $_.name -like 'core-*' }).Count
        failure_reasons = @($failureReasons); source_change_count = $changeCount; first_source_changes = @($changes); attempts = @($attempts)
    })
}

try {
    $allowedOutputRoot = Join-Path $root '.tools/stability'
    $output = if ($OutputDirectory) { [IO.Path]::GetFullPath($OutputDirectory) } else { Join-Path $allowedOutputRoot "$([DateTime]::UtcNow.ToString('yyyyMMddTHHmmssZ', [Globalization.CultureInfo]::InvariantCulture))-$($runId.Substring(0, 8))" }
    $allowedPrefix = [IO.Path]::GetFullPath($allowedOutputRoot).TrimEnd('\', '/') + [IO.Path]::DirectorySeparatorChar
    if (-not $output.StartsWith($allowedPrefix, [StringComparison]::OrdinalIgnoreCase)) { throw "Output directory must be a fresh child of $allowedOutputRoot" }
    if (Test-Path -LiteralPath $output) { throw "Output directory already exists: $output. Each batch requires a fresh directory." }
    New-Item -ItemType Directory -Path $output | Out-Null
    $outputCreated = $true
    $internalDirectory = Join-Path $output 'internal'
    New-Item -ItemType Directory -Path $internalDirectory | Out-Null
    Write-Summary
    if (-not $IsWindows) { throw 'Windows runtime evidence is required. This batch is INCOMPLETE; no tests were run.' }
    Set-Location -LiteralPath $root
    $outputRelative = [IO.Path]::GetRelativePath($root, (Join-Path $output 'summary.json')).Replace('\', '/')
    $ignored = Invoke-Captured 'git' @('check-ignore', '--quiet', '--', $outputRelative) (Join-Path $internalDirectory 'output-ignore.stdout.log') (Join-Path $internalDirectory 'output-ignore.stderr.log')
    if ($ignored -ne 0) { throw 'Evidence output is not ignored by Git. No tests were run.' }
    $baseline = Get-Snapshot
    Write-Json (Join-Path $output 'source-snapshot.json') $baseline
    Start-InputWatchers
    # A second snapshot closes the gap between enumeration and watcher setup.
    $null = Assert-Frozen
    $versions = [ordered]@{}
    foreach ($tool in @(@('rustc', '-Vv'), @('cargo', '--version'), @('node', '--version'), @('npm.cmd', '--version'), @('git', '--version'))) {
        $program = $tool[0]
        $out = Join-Path $internalDirectory "$program-version.stdout.log"
        $err = Join-Path $internalDirectory "$program-version.stderr.log"
        $code = Invoke-Captured $program @($tool[1..($tool.Length - 1)]) $out $err
        $versions[$program] = [ordered]@{ exit_code = $code; stdout = [IO.File]::ReadAllText($out).Trim(); stderr = [IO.File]::ReadAllText($err).Trim() }
        if ($code -ne 0) { $failureReasons.Add("Version check for $program exited $code.") }
    }
    # On this Windows/.NET host SetEnvironmentVariable(name, $null) retains an
    # empty variable, which Rust rejects. Remove the environment entry itself.
    Remove-Item Env:RUST_TEST_THREADS -ErrorAction SilentlyContinue
    Write-Json (Join-Path $output 'metadata.json') ([ordered]@{
        execution_mode = $(if ($synthetic) { 'synthetic-self-check' } else { 'windows-runtime' }); started_utc = $startedAt
        repository_root = $root; head = $baseline.head; branch = (Read-Tool 'branch' 'git' @('branch', '--show-current')).Trim()
        git_status = (Read-Tool 'status' 'git' @('status', '--porcelain=v1', '--untracked-files=all')).TrimEnd()
        source_sha256 = $baseline.sha256; source_file_count = $baseline.files.Count
        fingerprint_scope = 'All Git-tracked and nonignored untracked files, including source, tests, vendor, build/package configuration, scripts (this runner/self-check), workflow and .gitignore. Excludes docs/, root Markdown/LICENSE, .git/, .tools/, node_modules/, dist/, releases/, src-tauri/target/ and src-tauri/gen/. Snapshot rows record path, tracked/untracked kind, byte count and SHA256; missing tracked inputs are recorded as MISSING.'
        drift_detection = 'HEAD and source inventory/content hashes before and after every command; input filesystem write/rename/create/delete events also invalidate edit-and-restore. Watcher errors invalidate the batch. First 50 events are recorded.'
        windows = [Environment]::OSVersion.VersionString; powershell = $PSVersionTable.PSVersion.ToString(); tools = $versions
        rust_test_threads_original_present = $originalTestThreadsPresent; rust_test_threads_original = $originalTestThreads
        rust_test_threads_effective_present = (Test-Path Env:RUST_TEST_THREADS); rust_test_threads_effective = [Environment]::GetEnvironmentVariable('RUST_TEST_THREADS', 'Process')
        cargo_processes = 'Sequential fresh processes; test binaries use default parallelism. No --ignored/--include-ignored and no public relay service.'
        checks = @($checks)
    })
    Write-Summary
    foreach ($check in $checks) {
        $before = Assert-Frozen
        $stdout = Join-Path $output "$($check.name).stdout.log"
        $stderr = Join-Path $output "$($check.name).stderr.log"
        $entry = [ordered]@{ name = $check.name; command = "$($check.program) $($check.arguments -join ' ')"; program = $check.program; arguments = $check.arguments; started_utc = [DateTime]::UtcNow.ToString('o'); finished_utc = $null; seconds = $null; exit_code = $null; stdout = [IO.Path]::GetFileName($stdout); stderr = [IO.Path]::GetFileName($stderr); before_head = $before.head; before_sha256 = $before.sha256; after_head = $null; after_sha256 = $null; rust_results = @(); evidence_error = $null }
        $attempts.Add($entry)
        Write-Summary
        Write-Host "Starting $($check.name): $($entry.command)"
        $timer = [Diagnostics.Stopwatch]::StartNew()
        try { $entry.exit_code = Invoke-Captured $check.program $check.arguments $stdout $stderr }
        catch { $_ | Out-String | Add-Content -LiteralPath $stderr -Encoding utf8; throw }
        finally {
            $timer.Stop(); $entry.seconds = [Math]::Round($timer.Elapsed.TotalSeconds, 3); $entry.finished_utc = [DateTime]::UtcNow.ToString('o')
            if ($null -eq $entry.exit_code) {
                $entry | ConvertTo-Json -Depth 10 -Compress | Add-Content -LiteralPath (Join-Path $output 'attempts.jsonl') -Encoding utf8
                Write-Summary
            }
        }
        if ($entry.exit_code -ne 0) { $failureReasons.Add("$($check.name) exited $($entry.exit_code).") }
        if ($check.evidence) {
            $results = @(
                foreach ($line in @(Select-String -LiteralPath $stdout -Pattern 'test result: .*? (\d+) passed; (\d+) failed; (\d+) ignored;' -ErrorAction SilentlyContinue)) {
                    if ($line.Line -match 'test result: .*? (\d+) passed; (\d+) failed; (\d+) ignored;') { [pscustomobject][ordered]@{ passed = [int] $Matches[1]; failed = [int] $Matches[2]; ignored = [int] $Matches[3]; summary = $line.Line.Trim() } }
                }
            )
            $entry.rust_results = $results
            $passed = ($results | Measure-Object -Property passed -Sum).Sum
            $failed = ($results | Measure-Object -Property failed -Sum).Sum
            if ($results.Count -eq 0 -or $passed -lt 1 -or $failed -gt 0 -or ($check.evidence -eq 'focused' -and ($results.Count -ne 1 -or $passed -ne 1 -or $results[0].ignored -ne 0))) {
                $entry.evidence_error = 'Expected executed Rust tests were not proven by the output; skipped/zero tests do not count as passed.'
                $failureReasons.Add("$($check.name): $($entry.evidence_error)")
            }
        }
        $entry | ConvertTo-Json -Depth 10 -Compress | Add-Content -LiteralPath (Join-Path $output 'attempts.jsonl') -Encoding utf8
        $after = Get-Snapshot
        $entry.after_head = $after.head
        $entry.after_sha256 = $after.sha256
        Write-Summary
        $null = Assert-Frozen
        Write-Host "$($check.name): exit $($entry.exit_code), $($entry.seconds)s"
    }
    $null = Assert-Frozen
    if ($attempts.Count -ne $checks.Count -or @($attempts | Where-Object { $null -eq $_.exit_code }).Count -gt 0) { throw 'Batch stopped before every required attempt completed.' }
    $status = if ($failureReasons.Count -gt 0) { 'FAIL' } elseif ($synthetic) { 'SIMULATED_PASS' } else { 'PASS' }
}
catch {
    $failureReasons.Add($_.Exception.Message)
    if ($_.Exception.Message.StartsWith('SOURCE_DRIFT:')) { $status = 'SOURCE_DRIFT' }
    elseif ($failureReasons.Count -gt 1 -or @($attempts | Where-Object { $null -ne $_.exit_code -and $_.exit_code -ne 0 }).Count -gt 0) { $status = 'FAIL' }
    else { $status = 'INCOMPLETE' }
    Write-Warning $_.Exception.Message
}
finally {
    foreach ($watcher in $watchers) { $watcher.EnableRaisingEvents = $false; $watcher.Dispose() }
    foreach ($id in $eventIds) { Unregister-Event -SourceIdentifier $id -ErrorAction SilentlyContinue; Get-Event -SourceIdentifier $id -ErrorAction SilentlyContinue | Remove-Event -ErrorAction SilentlyContinue }
    if ($originalTestThreadsPresent) { Set-Item Env:RUST_TEST_THREADS -Value $originalTestThreads }
    else { Remove-Item Env:RUST_TEST_THREADS -ErrorAction SilentlyContinue }
    Set-Location -LiteralPath $oldLocation.Path
    if ($outputCreated) { Write-Summary }
}
$global:LASTEXITCODE = if ($status -in @('PASS', 'SIMULATED_PASS')) { 0 } else { 1 }
Write-Host "Stability verification: $status. Evidence: $output"
if (-not $synthetic) { exit $LASTEXITCODE }
