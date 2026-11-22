<#
.SYNOPSIS
  Runs the checks and the tests of every part of the repository, one suite after the other,
  and leaves nothing behind in the temp directory.

.DESCRIPTION
  The test processes get TEMP and TMP pointed at <repo>\target\test-tmp (this process only, the
  user's environment and .cargo\config.toml are not changed). Whatever happens - a failure, an
  interrupt - the scratch folders are removed in a finally block and the hygiene check runs
  last; its result decides the exit code together with the suites.

  Suites: cargo fmt/clippy/test, worker pytest, benchmarks pytest, UI typecheck/lint/format/
  unit tests/mock end-to-end tests. The end-to-end tests against the real application
  (npm run test:real) and anything that needs a GPU or models are not part of this script.

.PARAMETER Skip
  Suites to leave out: rust, worker, benchmarks, ui.

.PARAMETER Suite
  Run only these suites (same names as -Skip). Default: all of them.

.PARAMETER List
  Print the steps of the selected suites and exit without running anything.

.PARAMETER LogDir
  Write the output of every step to <LogDir>\<step>.log (pytest also writes a JUnit file next
  to it) and a summary of the steps to <LogDir>\steps.json. A later partial run (-Suite, -Skip)
  into the same folder replaces only the steps it ran. scripts\regression.ps1 reads these files
  to build its report.

.EXAMPLE
  scripts\test-all.ps1 -Skip ui

.EXAMPLE
  scripts\test-all.ps1 -Suite worker,benchmarks -LogDir target\test-logs
#>
[CmdletBinding()]
param([string[]]$Skip = @(), [string[]]$Suite = @(), [switch]$List, [string]$LogDir = "")

$ErrorActionPreference = "Stop"
# `-File` passes "a,b" as one string: accept both forms.
$Skip = @($Skip | ForEach-Object { $_ -split "," } | Where-Object { $_ })
$Suite = @($Suite | ForEach-Object { $_ -split "," } | Where-Object { $_ })
foreach ($name in $Skip + $Suite) {
    if ($name -notin "rust", "worker", "benchmarks", "ui") { throw "unknown suite '$name' (rust, worker, benchmarks, ui)" }
}
# -Suite narrows the selection; -Skip removes from what is left.
if ($Suite.Count -gt 0) {
    $Skip = @("rust", "worker", "benchmarks", "ui" | Where-Object { $_ -notin $Suite -or $_ -in $Skip })
}
$repo = Split-Path -Parent $PSScriptRoot
$testTmp = Join-Path $repo "target\test-tmp"
$failures = New-Object System.Collections.Generic.List[string]
$steps = New-Object System.Collections.Generic.List[object]
$logRoot = ""
if ($LogDir) {
    $logRoot = if ([System.IO.Path]::IsPathRooted($LogDir)) { $LogDir } else { Join-Path $repo $LogDir }
}

function Invoke-Step([string]$group, [string]$title, [string]$dir, [scriptblock]$command) {
    if ($List) {
        Write-Host ("{0,-11} {1}   (in {2})" -f $group, $title, $dir)
        return
    }
    Write-Host ""
    Write-Host "=== $title" -ForegroundColor Cyan
    $slug = ($title.ToLowerInvariant() -replace '[^a-z0-9]+', '-').Trim('-')
    $logName = "$slug.log"
    $writer = $null
    $savedAddOpts = $env:PYTEST_ADDOPTS
    $savedEap = $ErrorActionPreference
    $status = "passed"
    $watch = [System.Diagnostics.Stopwatch]::StartNew()
    Push-Location (Join-Path $repo $dir)
    try {
        if ($logRoot) {
            New-Item -ItemType Directory -Force -Path $logRoot | Out-Null
            $writer = New-Object System.IO.StreamWriter((Join-Path $logRoot $logName), $false, (New-Object System.Text.UTF8Encoding($false)))
            if ($title -like "*pytest") {
                # pytest splits the variable like a shell: quote the path and use forward slashes.
                $junit = (Join-Path $logRoot ($logName -replace '\.log$', '.junit.xml')) -replace '\\', '/'
                $env:PYTEST_ADDOPTS = "--junitxml=`"$junit`""
            }
            # Merged stderr must not turn into a terminating error in Windows PowerShell 5.1.
            $ErrorActionPreference = "Continue"
            & $command 2>&1 | ForEach-Object {
                $line = if ($_ -is [System.Management.Automation.ErrorRecord]) { $_.Exception.Message } else { "$_" }
                $writer.WriteLine($line)
                $line
            }
        } else {
            & $command
        }
        if ($LASTEXITCODE -ne 0) { throw "exit code $LASTEXITCODE" }
    } catch {
        $ErrorActionPreference = $savedEap
        Write-Host "FAILED: $title ($_)" -ForegroundColor Red
        $status = "failed"
        $failures.Add($title)
    } finally {
        $ErrorActionPreference = $savedEap
        $env:PYTEST_ADDOPTS = $savedAddOpts
        if ($writer) { $writer.Dispose() }
        Pop-Location
        $watch.Stop()
        if ($logRoot) {
            $steps.Add([ordered]@{ group = $group; name = $title; status = $status; seconds = [math]::Round($watch.Elapsed.TotalSeconds, 1); log = $logName })
        }
    }
}

$savedTemp = $env:TEMP
$savedTmp = $env:TMP
$hygiene = 0
try {
    if (-not $List) {
        New-Item -ItemType Directory -Force -Path $testTmp | Out-Null
        $env:TEMP = $testTmp
        $env:TMP = $testTmp
    }

    if ($Skip -notcontains "rust") {
        Invoke-Step "rust" "cargo fmt --check" "." { cargo fmt --all --check }
        Invoke-Step "rust" "cargo clippy" "." { cargo clippy --workspace --all-targets -- -D warnings }
        Invoke-Step "rust" "cargo test" "." { cargo test --workspace }
    }
    if ($Skip -notcontains "worker") {
        Invoke-Step "worker" "worker pytest" "worker" { uv run --no-sync pytest -q }
    }
    if ($Skip -notcontains "benchmarks") {
        Invoke-Step "benchmarks" "benchmarks pytest" "benchmarks" { uv run pytest -q }
    }
    if ($Skip -notcontains "ui") {
        Invoke-Step "ui" "ui typecheck" "ui" { npm run typecheck }
        Invoke-Step "ui" "ui lint" "ui" { npm run lint }
        Invoke-Step "ui" "ui format:check" "ui" { npm run format:check }
        Invoke-Step "ui" "ui unit tests" "ui" { npm test }
        Invoke-Step "ui" "ui mock end-to-end tests" "ui" { npm run test:e2e }
    }
    if ($List) { exit 0 }
} finally {
    $env:TEMP = $savedTemp
    $env:TMP = $savedTmp
    if (-not $List) {
        if ($logRoot) {
            New-Item -ItemType Directory -Force -Path $logRoot | Out-Null
            # Steps of an earlier partial run (-Suite, -Skip) stay in the summary unless they ran again.
            $stepsFile = Join-Path $logRoot "steps.json"
            $kept = @()
            if (Test-Path -LiteralPath $stepsFile) {
                $ran = @($steps | ForEach-Object { $_.name })
                $kept = @(Get-Content -LiteralPath $stepsFile -Raw | ConvertFrom-Json | Where-Object { $_.name -notin $ran })
            }
            ConvertTo-Json -InputObject (@($kept) + @($steps.ToArray())) -Depth 4 | Set-Content -LiteralPath $stepsFile -Encoding UTF8
        }
        Write-Host ""
        Write-Host "=== cleaning scratch files" -ForegroundColor Cyan
        & (Join-Path $PSScriptRoot "clean-test-tmp.ps1")
        & (Join-Path $PSScriptRoot "check-temp-hygiene.ps1")
        $hygiene = $LASTEXITCODE
    }
}

if ($failures.Count -gt 0) {
    Write-Host ""
    Write-Host "Failed steps: $($failures -join ', ')" -ForegroundColor Red
}
if ($failures.Count -gt 0 -or $hygiene -ne 0) { exit 1 }
Write-Host "All steps passed." -ForegroundColor Green
exit 0
