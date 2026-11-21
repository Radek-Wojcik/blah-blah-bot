<#
.SYNOPSIS
  Removes the scratch files that tests leave behind.

.DESCRIPTION
  Removes
    - the contents of <repo>\target\test-tmp and <repo>\target\e2e-tmp, and
    - the folders of the user's temp directory (and of $env:TEMP / $env:TMP when they differ)
      whose names match the ones the tests of this repository use: bbb-*, .tmpXXXXXX
      (the default prefix of the Rust tempfile crate) and ci-venv*.
  Nothing else in the temp directory is touched.

.PARAMETER Quiet
  Print only the summary line.
#>
[CmdletBinding()]
param([switch]$Quiet)

$ErrorActionPreference = "Continue"
$repo = Split-Path -Parent $PSScriptRoot
$pattern = '^(bbb-|\.tmp[A-Za-z0-9]{6}$|ci-venv)'

function Remove-Tree([string]$path) {
    for ($i = 0; $i -lt 3; $i++) {
        try {
            Remove-Item -LiteralPath $path -Recurse -Force -ErrorAction Stop
            return $true
        } catch {
            Start-Sleep -Milliseconds 500
        }
        if (-not (Test-Path -LiteralPath $path)) { return $true }
    }
    # Long paths and read-only files: let the command interpreter try as well.
    cmd.exe /c rd /s /q "`"$path`"" 2>$null | Out-Null
    return -not (Test-Path -LiteralPath $path)
}

$removed = 0
$failed = @()

# Folders of the repository that hold nothing but scratch data: empty them, keep the folder.
foreach ($name in "test-tmp", "e2e-tmp") {
    $dir = Join-Path $repo "target\$name"
    if (-not (Test-Path -LiteralPath $dir)) { continue }
    foreach ($item in Get-ChildItem -LiteralPath $dir -Force) {
        if (Remove-Tree $item.FullName) { $removed++ } else { $failed += $item.FullName }
    }
}

# The user's temp directory: only the names of this repository's tests.
$roots = @("$env:LOCALAPPDATA\Temp", $env:TEMP, $env:TMP) |
    Where-Object { $_ -and (Test-Path -LiteralPath $_) } |
    ForEach-Object { (Resolve-Path -LiteralPath $_).Path.TrimEnd('\') } |
    Sort-Object -Unique
$repoTarget = (Join-Path $repo "target").TrimEnd('\')
foreach ($root in $roots) {
    if ($root.StartsWith($repoTarget, [StringComparison]::OrdinalIgnoreCase)) { continue }
    foreach ($item in Get-ChildItem -LiteralPath $root -Force -ErrorAction SilentlyContinue |
        Where-Object { $_.Name -match $pattern }) {
        if (Remove-Tree $item.FullName) { $removed++ } else { $failed += $item.FullName }
    }
}

if (-not $Quiet) { foreach ($f in $failed) { Write-Warning "could not remove $f" } }
Write-Host "clean-test-tmp: removed $removed entries, $($failed.Count) could not be removed"
if ($failed.Count -gt 0) { exit 1 }
exit 0
