<#
.SYNOPSIS
  Refuses (exit code 1) unless the GPU is idle enough for a measurement run.

.DESCRIPTION
  Passes only when all of this holds:
    - nvidia-smi answers;
    - the memory in use on the first GPU is below -MaxUsedMB (default 2048, that is 2 GB);
    - nvidia-smi lists no compute process;
    - no llama-server, llama-cli, python or pythonw process is running (they belong to somebody
      else when a run is about to start; stop them yourself, this script never does).
  Used by scripts\regression.ps1 -Mode gpu and scripts\ten-hour-run.ps1 before every GPU job.

.PARAMETER MaxUsedMB
  Largest GPU memory in use that still counts as idle.

.PARAMETER Quiet
  Print only the refusal reasons.
#>
[CmdletBinding()]
param([int]$MaxUsedMB = 2048, [switch]$Quiet)

$ErrorActionPreference = "Continue"
$reasons = New-Object System.Collections.Generic.List[string]

$smi = Get-Command nvidia-smi -ErrorAction SilentlyContinue
if (-not $smi) {
    $reasons.Add("nvidia-smi was not found")
} else {
    $used = & nvidia-smi --query-gpu=memory.used --format=csv,noheader,nounits --id=0 2>$null
    $usedMB = 0
    if ($LASTEXITCODE -ne 0 -or -not [int]::TryParse(("$used").Trim(), [ref]$usedMB)) {
        $reasons.Add("nvidia-smi did not report the memory in use")
    } elseif ($usedMB -ge $MaxUsedMB) {
        $reasons.Add("the GPU holds $usedMB MiB (limit: below $MaxUsedMB MiB)")
    } elseif (-not $Quiet) {
        Write-Host "gpu preflight: $usedMB MiB in use (limit: below $MaxUsedMB MiB)"
    }
    $apps = @(& nvidia-smi --query-compute-apps=pid,process_name,used_memory --format=csv,noheader 2>$null |
        Where-Object { $_ -and $_ -notmatch "No running|\[N/A\]\s*$" })
    if ($apps.Count -gt 0) { $reasons.Add("compute processes on the GPU: $($apps -join '; ')") }
}

$foreign = @(Get-Process -ErrorAction SilentlyContinue |
    Where-Object { $_.ProcessName -match '^(llama-server|llama-cli|python|pythonw)$' } |
    ForEach-Object { "$($_.ProcessName) (pid $($_.Id))" })
if ($foreign.Count -gt 0) { $reasons.Add("processes of others are running: $($foreign -join ', ')") }

if ($reasons.Count -gt 0) {
    Write-Host "GPU PREFLIGHT REFUSED:" -ForegroundColor Red
    foreach ($r in $reasons) { Write-Host "  - $r" }
    exit 1
}
if (-not $Quiet) { Write-Host "gpu preflight: ok" }
exit 0
