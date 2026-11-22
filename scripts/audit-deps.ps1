<#
.SYNOPSIS
  Read-only dependency audit: Rust crates, UI packages and worker Python packages.

.DESCRIPTION
  Checks, in order:
    1. Rust. `cargo deny check` when cargo-deny is installed (uses deny.toml); otherwise the
       crates of the Windows build are looked up on osv.dev (which carries the RustSec
       advisories). `cargo audit` runs as well when it is installed. Always prints a licence
       summary taken from `cargo metadata` and fails on GPL-family or missing licences.
    2. UI. `npm audit --omit=dev --audit-level=high` (fails) and a full `npm audit`
       (informational, dev tooling only).
    3. Worker and benchmarks. `uv export` of the lock file, then `uvx pip-audit` on the
       pinned set. Advisories listed in $AcceptedPython are accepted, see
       docs/DEPENDENCY-AUDIT.md for the reasoning.

  Nothing in the repository is modified. Scratch files live under target/audit and the
  temporary and cache folders are removed at the end; the reports stay in target/audit.
  Only package names and versions are sent to the vulnerability databases.

.EXAMPLE
  pwsh scripts/audit-deps.ps1
  pwsh scripts/audit-deps.ps1 -SkipPython
#>
[CmdletBinding()]
param(
    [switch]$SkipRust,
    [switch]$SkipNpm,
    [switch]$SkipPython
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$work = Join-Path $root 'target/audit'
$tmp = Join-Path $work 'tmp'
$uvCache = Join-Path $work 'uv-cache'
$windowsTarget = 'x86_64-pc-windows-msvc'

# Advisories accepted for the worker environment (reasons in docs/DEPENDENCY-AUDIT.md).
$AcceptedPython = @('PYSEC-2026-3447', 'GHSA-h35f-9h28-mq5c', 'CVE-2026-59890', 'GHSA-xmj3-9gf7-h52w', 'PYSEC-2025-194')

$failures = New-Object System.Collections.Generic.List[string]
$saved = @{ TEMP = $env:TEMP; TMP = $env:TMP; UV_CACHE_DIR = $env:UV_CACHE_DIR }

function Write-Section([string]$title) { Write-Host ""; Write-Host "== $title" }

function Test-Tool([string]$name) { return [bool](Get-Command $name -ErrorAction SilentlyContinue) }

function Test-CargoSubcommand([string]$name) {
    $out = & cargo --list | Where-Object { $_ -match "^\s+$name(\s|$)" }
    return [bool]$out
}

function Invoke-RustAudit {
    Write-Section 'Rust: licences (cargo metadata, Windows target)'
    $json = & cargo metadata --format-version 1 --locked --filter-platform $windowsTarget | Out-String
    if ($LASTEXITCODE -ne 0) { throw 'cargo metadata failed' }
    $meta = $json | ConvertFrom-Json
    $pkgById = @{}
    foreach ($p in $meta.packages) { $pkgById[$p.id] = $p }
    $nodeById = @{}
    foreach ($n in $meta.resolve.nodes) { $nodeById[$n.id] = $n }
    $workspace = @{}
    foreach ($m in $meta.workspace_members) { $workspace[$m] = $true }

    # Crates linked into the application: everything reachable through normal dependencies.
    $seen = @{}
    $stack = New-Object System.Collections.Stack
    foreach ($m in $meta.workspace_members) { $stack.Push($m) }
    while ($stack.Count -gt 0) {
        $id = $stack.Pop()
        if ($seen.ContainsKey($id)) { continue }
        $seen[$id] = $true
        foreach ($d in $nodeById[$id].deps) {
            if (@($d.dep_kinds | Where-Object { $null -eq $_.kind }).Count -gt 0) { $stack.Push($d.pkg) }
        }
    }

    $licences = @{}
    $flagged = New-Object System.Collections.Generic.List[string]
    $lines = New-Object System.Collections.Generic.List[string]
    foreach ($id in ($seen.Keys | Sort-Object)) {
        if ($workspace.ContainsKey($id)) { continue }
        $p = $pkgById[$id]
        $lic = if ($p.license) { [string]$p.license } else { '(none)' }
        if (-not $licences.ContainsKey($lic)) { $licences[$lic] = 0 }
        $licences[$lic]++
        $lines.Add(('{0} {1} | {2}' -f $p.name, $p.version, $lic))
        $alts = $lic -split '\s+OR\s+|/' | ForEach-Object { $_.Trim('() ') }
        $allCopyleft = $true
        foreach ($a in $alts) { if ($a -notmatch '(^|[^A-Za-z])(A|L)?GPL') { $allCopyleft = $false } }
        if ($lic -eq '(none)' -or $allCopyleft) { $flagged.Add(('{0} {1} | {2}' -f $p.name, $p.version, $lic)) }
    }
    New-Item -ItemType Directory -Force $work | Out-Null
    $lines | Set-Content -Encoding utf8 (Join-Path $work 'rust-licenses.txt')
    Write-Host ("{0} third-party crates linked on {1}" -f $lines.Count, $windowsTarget)
    $licences.GetEnumerator() | Sort-Object Value -Descending | ForEach-Object { Write-Host ('{0,5}  {1}' -f $_.Value, $_.Key) }
    if ($flagged.Count -gt 0) {
        $flagged | ForEach-Object { Write-Host "DENIED licence: $_" }
        $failures.Add('rust: GPL-family or missing licence in the shipped set')
    }
    else { Write-Host 'No GPL-family or missing licence in the shipped set.' }

    Write-Section 'Rust: advisories'
    $ranTool = $false
    if (Test-CargoSubcommand 'deny') {
        $ranTool = $true
        & cargo deny check advisories bans licenses sources
        if ($LASTEXITCODE -ne 0) { $failures.Add('rust: cargo deny check failed') }
    }
    if (Test-CargoSubcommand 'audit') {
        $ranTool = $true
        & cargo audit
        if ($LASTEXITCODE -ne 0) { $failures.Add('rust: cargo audit reported advisories') }
    }
    if (-not $ranTool) {
        Write-Host 'cargo-deny and cargo-audit are not installed; querying osv.dev for the crates of the Windows build.'
        $queries = @()
        $rows = @()
        foreach ($id in $seen.Keys) {
            $p = $pkgById[$id]
            if ($workspace.ContainsKey($id) -or -not $p.source) { continue }
            $rows += $p
            $queries += @{ package = @{ name = $p.name; ecosystem = 'crates.io' }; version = $p.version }
        }
        $hits = @{}
        for ($i = 0; $i -lt $queries.Count; $i += 500) {
            $end = [Math]::Min($i + 499, $queries.Count - 1)
            $body = @{ queries = @($queries[$i..$end]) } | ConvertTo-Json -Depth 6 -Compress
            $res = Invoke-RestMethod -Method Post -Uri 'https://api.osv.dev/v1/querybatch' -ContentType 'application/json' -Body $body
            for ($k = 0; $k -lt $res.results.Count; $k++) {
                foreach ($v in @($res.results[$k].vulns)) {
                    if ($v) { $hits["$($rows[$i + $k].name) $($rows[$i + $k].version)|$($v.id)"] = $true }
                }
            }
        }
        if ($hits.Count -eq 0) { Write-Host 'No advisories for the crates of the Windows build.' }
        foreach ($h in ($hits.Keys | Sort-Object)) {
            $crate, $vid = $h -split '\|'
            $d = Invoke-RestMethod -Uri "https://api.osv.dev/v1/vulns/$vid"
            if ($d.summary -match 'unmaintained') { Write-Host "WARN (informational): $crate $vid $($d.summary)" }
            else { Write-Host "ADVISORY: $crate $vid $($d.summary)"; $failures.Add("rust: $crate $vid") }
        }
    }
}

function Invoke-NpmAudit {
    Write-Section 'UI: npm audit (production dependencies, high and above)'
    Push-Location (Join-Path $root 'ui')
    try {
        & npm audit --omit=dev --audit-level=high
        if ($LASTEXITCODE -ne 0) { $failures.Add('ui: npm audit --omit=dev found high or critical advisories') }
        Write-Section 'UI: npm audit (all dependencies, informational)'
        & npm audit --audit-level=high
        if ($LASTEXITCODE -ne 0) { Write-Host 'WARN (informational): advisories of high severity or above in dev tooling' }
    }
    finally { Pop-Location }
}

function Invoke-PythonAudit {
    if (-not (Test-Tool 'uv')) { Write-Host 'uv not found: skipping the Python audit.'; return }
    New-Item -ItemType Directory -Force $tmp, $uvCache | Out-Null
    $env:TEMP = $tmp; $env:TMP = $tmp; $env:UV_CACHE_DIR = $uvCache
    foreach ($project in @('worker', 'benchmarks')) {
        Write-Section "Python: pip-audit of $project/uv.lock"
        $req = Join-Path $work "$project-requirements.txt"
        Push-Location (Join-Path $root $project)
        try { & uv export --frozen --no-hashes --no-dev --no-emit-project -q -o $req }
        finally { Pop-Location }
        if ($LASTEXITCODE -ne 0) { $failures.Add("python: uv export failed for $project"); continue }
        # PyPI knows no local build tags such as +cu128.
        (Get-Content $req) -replace '\+cu\d+', '' | Set-Content -Encoding ascii $req
        $args = @('pip-audit', '-r', $req, '--no-deps', '--disable-pip', '--progress-spinner', 'off')
        if ($project -eq 'worker') { foreach ($id in $AcceptedPython) { $args += @('--ignore-vuln', $id) } }
        & uvx @args
        if ($LASTEXITCODE -ne 0) { $failures.Add("python: pip-audit found advisories in $project") }
    }
}

try {
    New-Item -ItemType Directory -Force $work | Out-Null
    Push-Location $root
    try {
        if (-not $SkipRust) { Invoke-RustAudit }
        if (-not $SkipNpm) { Invoke-NpmAudit }
        if (-not $SkipPython) { Invoke-PythonAudit }
    }
    finally { Pop-Location }
}
finally {
    foreach ($k in $saved.Keys) {
        if ($null -eq $saved[$k]) { Remove-Item "Env:$k" -ErrorAction SilentlyContinue }
        else { Set-Item "Env:$k" $saved[$k] }
    }
    Remove-Item -Recurse -Force -ErrorAction SilentlyContinue $tmp, $uvCache
}

Write-Section 'Result'
if ($failures.Count -gt 0) {
    $failures | ForEach-Object { Write-Host "FAIL: $_" }
    exit 1
}
Write-Host 'Audit clean (accepted advisories are listed in docs/DEPENDENCY-AUDIT.md).'
exit 0
