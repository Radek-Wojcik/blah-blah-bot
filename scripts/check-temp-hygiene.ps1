<#
.SYNOPSIS
  Fails when tests have left scratch folders behind.

.DESCRIPTION
  Counts the directories, files and megabytes that match the scratch names of this
  repository's tests:
    - in the temp directory (the user's %LOCALAPPDATA%\Temp, and $env:TEMP / $env:TMP when they
      differ): names bbb-*, .tmpXXXXXX (default prefix of the Rust tempfile crate), ci-venv*;
    - in <repo>\target\test-tmp and <repo>\target\e2e-tmp: everything.
  Exit code 1 and a message when any matching directory exists, or when the matching files
  exceed -MaxFiles or -MaxMB. Remove what it finds with scripts\clean-test-tmp.ps1 and fix
  the test that left it.

.PARAMETER MaxFiles
  Most files that matching entries may hold (default 500).

.PARAMETER MaxMB
  Most megabytes that matching entries may hold (default 50).

.PARAMETER Quiet
  Print nothing when the check passes.
#>
[CmdletBinding()]
param([int]$MaxFiles = 500, [int]$MaxMB = 50, [switch]$Quiet)

$ErrorActionPreference = "Continue"
$repo = Split-Path -Parent $PSScriptRoot
$pattern = '^(bbb-|\.tmp[A-Za-z0-9]{6}$|ci-venv)'

function Measure-Entry($entry) {
    if ($entry.PSIsContainer) {
        $files = @(Get-ChildItem -LiteralPath $entry.FullName -Force -Recurse -File -ErrorAction SilentlyContinue)
        [pscustomobject]@{
            Dirs  = 1 + @(Get-ChildItem -LiteralPath $entry.FullName -Force -Recurse -Directory -ErrorAction SilentlyContinue).Count
            Files = $files.Count
            Bytes = [int64](($files | Measure-Object -Property Length -Sum).Sum)
        }
    } else {
        [pscustomobject]@{ Dirs = 0; Files = 1; Bytes = [int64]$entry.Length }
    }
}

$found = @()

$tempRoots = @("$env:LOCALAPPDATA\Temp", $env:TEMP, $env:TMP) |
    Where-Object { $_ -and (Test-Path -LiteralPath $_) } |
    ForEach-Object { (Resolve-Path -LiteralPath $_).Path.TrimEnd('\') } |
    Sort-Object -Unique
$repoTarget = (Join-Path $repo "target").TrimEnd('\')
foreach ($root in $tempRoots) {
    # A temp directory that the wrapper moved into target\ is reported as test-tmp below.
    if ($root.StartsWith($repoTarget, [StringComparison]::OrdinalIgnoreCase)) { continue }
    foreach ($e in Get-ChildItem -LiteralPath $root -Force -ErrorAction SilentlyContinue |
        Where-Object { $_.Name -match $pattern }) {
        $m = Measure-Entry $e
        $found += [pscustomobject]@{ Where = $root; Name = $e.Name; Dirs = $m.Dirs; Files = $m.Files; MB = [math]::Round($m.Bytes / 1MB, 1) }
    }
}
foreach ($name in "test-tmp", "e2e-tmp") {
    $dir = Join-Path $repo "target\$name"
    if (-not (Test-Path -LiteralPath $dir)) { continue }
    foreach ($e in Get-ChildItem -LiteralPath $dir -Force -ErrorAction SilentlyContinue) {
        $m = Measure-Entry $e
        $found += [pscustomobject]@{ Where = $dir; Name = $e.Name; Dirs = $m.Dirs; Files = $m.Files; MB = [math]::Round($m.Bytes / 1MB, 1) }
    }
}

$dirs = [int](($found | Measure-Object -Property Dirs -Sum).Sum)
$files = [int](($found | Measure-Object -Property Files -Sum).Sum)
$mb = [math]::Round([double](($found | Measure-Object -Property MB -Sum).Sum), 1)
$summary = "temp hygiene: $dirs directories, $files files, $mb MB left by tests"

$bad = ($dirs -gt 0) -or ($files -gt $MaxFiles) -or ($mb -gt $MaxMB)
if (-not $bad) {
    if (-not $Quiet) { Write-Host "$summary (ok)" }
    exit 0
}

Write-Host "FAILED - $summary (limits: no directories, at most $MaxFiles files and $MaxMB MB)" -ForegroundColor Red
$found | Sort-Object Files -Descending | Select-Object -First 25 | Format-Table Where, Name, Dirs, Files, MB -AutoSize | Out-String | Write-Host
Write-Host "Remove them with scripts\clean-test-tmp.ps1 and fix the test that left them behind."
exit 1
