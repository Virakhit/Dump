#requires -Version 7.0
# Synthetic checks of the verification runner. No Cargo/npm or network tests run.
$ErrorActionPreference = 'Stop'
if (-not $IsWindows) { throw 'This runner self-check requires Windows filesystem events.' }
$repo = Split-Path -Parent $PSScriptRoot
$fixtureRoot = Join-Path $repo ".tools/stability-self-check/$([Guid]::NewGuid().ToString('N'))"
New-Item -ItemType Directory -Path $fixtureRoot | Out-Null
$originalThreadsPresent = Test-Path Env:RUST_TEST_THREADS
$originalThreads = [Environment]::GetEnvironmentVariable('RUST_TEST_THREADS', 'Process')
try {
    foreach ($scenario in @('success', 'absent-success', 'empty-success', 'full-success', 'failure', 'restored-drift', 'incomplete', 'failure-incomplete', 'zero-tests')) {
        $expectedThreadsPresent = $scenario -ne 'absent-success'
        $expectedThreadsValue = if ($scenario -eq 'empty-success') { '' } else { '1' }
        if ($expectedThreadsPresent) { Set-Item Env:RUST_TEST_THREADS -Value $expectedThreadsValue }
        else { Remove-Item Env:RUST_TEST_THREADS -ErrorAction SilentlyContinue }
        $fixture = Join-Path $fixtureRoot $scenario
        New-Item -ItemType Directory -Path (Join-Path $fixture 'src'), (Join-Path $fixture 'scripts') | Out-Null
        Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'verify-stability.ps1') -Destination (Join-Path $fixture 'scripts/verify-stability.ps1')
        Copy-Item -LiteralPath $PSCommandPath -Destination (Join-Path $fixture 'scripts/verify-stability.test.ps1')
        [IO.File]::WriteAllText((Join-Path $fixture 'src/input.rs'), 'initial source')
        [IO.File]::WriteAllText((Join-Path $fixture 'README.md'), 'documentation')
        [IO.File]::WriteAllText((Join-Path $fixture '.gitignore'), ".tools/`n")
        $state = @{ attempts = 0; parallel = $true; arguments = [Collections.Generic.List[string]]::new() }
        $runner = {
            param($Program, [string[]] $Arguments)
            $command = $Arguments -join ' '
            $result = @{ ExitCode = 0; Stdout = ''; Stderr = '' }
            if ($command -eq '--version' -or $command -eq '-Vv') { $result.Stdout = 'synthetic version'; return $result }
            if ($Program -eq 'git') {
                switch -Wildcard ($command) {
                    'rev-parse HEAD' { $result.Stdout = ('c' * 40) }
                    'branch --show-current' { $result.Stdout = 'synthetic-fixture' }
                    'status *' { $result.Stdout = '' }
                    'ls-files --cached -z' { $result.Stdout = @('.gitignore', 'README.md', 'scripts/verify-stability.ps1', 'scripts/verify-stability.test.ps1', 'src/input.rs') -join [char] 0 }
                    'ls-files --others --exclude-standard -z' { $result.Stdout = '' }
                    'check-ignore *' { $result.ExitCode = if ($Arguments[-1] -like '.tools/*') { 0 } else { 1 } }
                    'diff --check' { return $result }
                    default { throw "Unexpected synthetic Git command: $command" }
                }
                return $result
            }
            if ($Program -notin @('cargo', 'npm.cmd', 'node')) { throw "Unexpected validation command: $Program $command" }
            $state.attempts++
            $state.arguments.Add($command)
            if (Test-Path Env:RUST_TEST_THREADS) { $state.parallel = $false }
            if ($state.attempts -eq 1 -and $scenario -in @('success', 'absent-success', 'empty-success')) {
                # Probe a real child environment without running Rust/npm tests.
                $child = & (Join-Path $PSHOME 'pwsh.exe') -NoProfile -Command '[pscustomobject]@{present=(Test-Path Env:RUST_TEST_THREADS);value=[Environment]::GetEnvironmentVariable("RUST_TEST_THREADS","Process")} | ConvertTo-Json -Compress' | ConvertFrom-Json
                if ($LASTEXITCODE -ne 0 -or $child.present -or $null -ne $child.value) { throw 'Actual child inherited RUST_TEST_THREADS; empty is not absent.' }
            }
            $result.Stdout = if ($Program -eq 'cargo' -and $Arguments[0] -eq 'test') { 'test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s' } else { 'synthetic command output' }
            if ($state.attempts -eq 1) {
                switch ($scenario) {
                    'success' { [IO.File]::WriteAllText((Join-Path $fixture 'README.md'), 'docs may record results') }
                    'full-success' { [IO.File]::WriteAllText((Join-Path $fixture 'README.md'), 'docs may record results') }
                    'failure' { $result.ExitCode = 17; $result.Stdout = 'test result: FAILED. 0 passed; 1 failed; 0 ignored;'; $result.Stderr = 'synthetic failure' }
                    'failure-incomplete' { $result.ExitCode = 17; $result.Stdout = 'test result: FAILED. 0 passed; 1 failed; 0 ignored;'; $result.Stderr = 'synthetic failure' }
                    'restored-drift' {
                        $source = Join-Path $fixture 'src/input.rs'
                        $bytes = [IO.File]::ReadAllBytes($source)
                        [IO.File]::WriteAllText($source, 'edited during attempt')
                        [IO.File]::WriteAllBytes($source, $bytes)
                        # Wait for the actual watcher event, retaining it for the runner.
                        if (-not (Wait-Event -Timeout 5)) { throw 'No source-write event was observed in synthetic fixture.' }
                    }
                    'incomplete' { throw [OperationCanceledException]::new('Synthetic interruption; runtime batch incomplete.') }
                    'zero-tests' { $result.Stdout = 'test result: ok. 0 passed; 0 failed; 0 ignored;' }
                }
            }
            if ($scenario -eq 'failure-incomplete' -and $state.attempts -eq 2) { throw [OperationCanceledException]::new('Synthetic interruption after a failure.') }
            return $result
        }.GetNewClosure()
        & (Join-Path $fixture 'scripts/verify-stability.ps1') -RepositoryRoot $fixture -CommandRunner $runner -FullValidation:($scenario -eq 'full-success')
        $run = Get-ChildItem -LiteralPath (Join-Path $fixture '.tools/stability') -Directory | Select-Object -First 1
        $summary = Get-Content -LiteralPath (Join-Path $run.FullName 'summary.json') -Raw | ConvertFrom-Json
        $expected = switch ($scenario) { { $_ -in @('success', 'absent-success', 'empty-success', 'full-success') } { 'SIMULATED_PASS' }; 'restored-drift' { 'SOURCE_DRIFT' }; 'incomplete' { 'INCOMPLETE' }; default { 'FAIL' } }
        if ($summary.status -ne $expected) { throw "$scenario expected $expected, got $($summary.status)" }
        if ($summary.runtime_evidence -or $summary.execution_mode -ne 'synthetic-self-check') { throw 'Synthetic outputs were mislabeled as runtime evidence.' }
        if ($scenario -in @('success', 'absent-success', 'empty-success', 'failure', 'zero-tests') -and $summary.completed_attempts -ne 13) { throw "$scenario did not run exactly 13 independent fake attempts." }
        if ($scenario -eq 'full-success' -and ($summary.completed_attempts -ne 23 -or $summary.focused_attempts -ne 10 -or $summary.core_attempts -ne 3)) { throw 'Full validation did not include fixed 10+3 and all ten other checks.' }
        if ($scenario -eq 'failure' -and $summary.attempts[0].exit_code -ne 17) { throw 'Initial failure was lost after later successes.' }
        if ($scenario -eq 'restored-drift' -and ($summary.source_change_count -lt 1 -or $summary.completed_attempts -ge 13)) { throw 'Edit-and-restore did not invalidate and stop the batch.' }
        if ($scenario -eq 'incomplete' -and $summary.completed_attempts -ne 0) { throw 'Interrupted attempt was counted as complete.' }
        if ($scenario -eq 'failure-incomplete' -and ($summary.complete -or $summary.completed_attempts -ne 1 -or $summary.attempts[0].exit_code -ne 17)) { throw 'Failure followed by interruption was mislabeled complete or lost the failure.' }
        if (-not $state.parallel -or @($state.arguments | Where-Object { $_ -match '--test-threads' }).Count -gt 0) { throw 'Default test parallelism was overridden.' }
        if ((Test-Path Env:RUST_TEST_THREADS) -ne $expectedThreadsPresent -or ($expectedThreadsPresent -and [Environment]::GetEnvironmentVariable('RUST_TEST_THREADS', 'Process') -cne $expectedThreadsValue)) { throw 'Original test-thread environment presence/value was not restored.' }
        if (($summary.status -eq 'SIMULATED_PASS') -ne ($LASTEXITCODE -eq 0)) { throw 'Runner exit status disagrees with aggregate status.' }
        Write-Host "Synthetic runner scenario ${scenario}: expected $expected confirmed."
    }
}
finally {
    if ($originalThreadsPresent) { Set-Item Env:RUST_TEST_THREADS -Value $originalThreads }
    else { Remove-Item Env:RUST_TEST_THREADS -ErrorAction SilentlyContinue }
}
# A rejected output directory must never be written by the runner.
$existing = Join-Path $fixture '.tools/stability/preexisting'
New-Item -ItemType Directory -Path $existing | Out-Null
$sentinel = Join-Path $existing 'summary.json'
[IO.File]::WriteAllText($sentinel, 'sentinel from an older run')
& (Join-Path $fixture 'scripts/verify-stability.ps1') -RepositoryRoot $fixture -OutputDirectory $existing -CommandRunner $runner
if ($LASTEXITCODE -eq 0 -or [IO.File]::ReadAllText($sentinel) -ne 'sentinel from an older run') { throw 'Rejected output directory was modified or accepted.' }
# Exercise the real command-not-found branch, without invoking any installed tool.
$ast = [Management.Automation.Language.Parser]::ParseFile((Join-Path $PSScriptRoot 'verify-stability.ps1'), [ref] $null, [ref] $null)
$function = $ast.Find({ param($node) $node -is [Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -eq 'Invoke-Captured' }, $true)
. ([scriptblock]::Create($function.Extent.Text))
$synthetic = $false
$missingExit = Invoke-Captured "dump-nonexistent-selfcheck-$([Guid]::NewGuid().ToString('N'))" @() (Join-Path $fixtureRoot 'missing.stdout.log') (Join-Path $fixtureRoot 'missing.stderr.log')
if ($missingExit -eq 0) { throw 'Missing actual program was reported successful.' }
Write-Host "Synthetic runner self-check PASS (9 batch scenarios, actual child environment absence, original absent/empty/value restoration, rejected output preservation, missing command). No Rust/npm/runtime tests executed. Fixtures: $fixtureRoot"
