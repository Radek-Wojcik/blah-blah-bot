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

.EXAMPLE
  scripts\test-all.ps1 -Skip ui
#>
[CmdletBinding()]
param([string[]]$Skip = @())

$ErrorActionPreference = "Stop"
# `-File` passes "a,b" as one string: accept both forms.
$Skip = @($Skip | ForEach-Object { $_ -split "," } | Where-Object { $_ })
foreach ($name in $Skip) {
    if ($name -notin "rust", "worker", "benchmarks", "ui") { throw "unknown suite '$name' (rust, worker, benchmarks, ui)" }
}
$repo = Split-Path -Parent $PSScriptRoot
$testTmp = Join-Path $repo "target\test-tmp"
$failures = New-Object System.Collections.Generic.List[string]

function Invoke-Step([string]$title, [string]$dir, [scriptblock]$command) {
    Write-Host ""
    Write-Host "=== $title" -ForegroundColor Cyan
    Push-Location (Join-Path $repo $dir)
    try {
        & $command
        if ($LASTEXITCODE -ne 0) { throw "exit code $LASTEXITCODE" }
    } catch {
        Write-Host "FAILED: $title ($_)" -ForegroundColor Red
        $failures.Add($title)
    } finally {
        Pop-Location
    }
}

$savedTemp = $env:TEMP
$savedTmp = $env:TMP
$hygiene = 0
try {
    New-Item -ItemType Directory -Force -Path $testTmp | Out-Null
    $env:TEMP = $testTmp
    $env:TMP = $testTmp

    if ($Skip -notcontains "rust") {
        Invoke-Step "cargo fmt --check" "." { cargo fmt --all --check }
        Invoke-Step "cargo clippy" "." { cargo clippy --workspace --all-targets -- -D warnings }
        Invoke-Step "cargo test" "." { cargo test --workspace }
    }
    if ($Skip -notcontains "worker") {
        Invoke-Step "worker pytest" "worker" { uv run --no-sync pytest -q }
    }
    if ($Skip -notcontains "benchmarks") {
        Invoke-Step "benchmarks pytest" "benchmarks" { uv run pytest -q }
    }
    if ($Skip -notcontains "ui") {
        Invoke-Step "ui typecheck" "ui" { npm run typecheck }
        Invoke-Step "ui lint" "ui" { npm run lint }
        Invoke-Step "ui format:check" "ui" { npm run format:check }
        Invoke-Step "ui unit tests" "ui" { npm test }
        Invoke-Step "ui mock end-to-end tests" "ui" { npm run test:e2e }
    }
} finally {
    $env:TEMP = $savedTemp
    $env:TMP = $savedTmp
    Write-Host ""
    Write-Host "=== cleaning scratch files" -ForegroundColor Cyan
    & (Join-Path $PSScriptRoot "clean-test-tmp.ps1")
    & (Join-Path $PSScriptRoot "check-temp-hygiene.ps1")
    $hygiene = $LASTEXITCODE
}

if ($failures.Count -gt 0) {
    Write-Host ""
    Write-Host "Failed steps: $($failures -join ', ')" -ForegroundColor Red
}
if ($failures.Count -gt 0 -or $hygiene -ne 0) { exit 1 }
Write-Host "All steps passed." -ForegroundColor Green
exit 0
