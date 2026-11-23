<#
.SYNOPSIS
  Ten hour end-to-end run: a synthetic ten hour meeting through the whole processing pipeline
  with the real worker, real models and a managed llama-server. Does nothing without -Run.

.DESCRIPTION
  The audio is synthetic. The reference meetings of -AudioDir are repeated until -Hours of audio
  exist and are cut into 60 second segment files like the recorder writes them (the test
  crates\queue\tests\long_e2e_models.rs does that and then runs the supervisor over the job).
  Recording itself is not repeated here: the ten hour recorder soak is a separate result
  (benchmarks\results\stage1-soak-10h.json).

  Without -Run the script validates its inputs and prints the plan (a dry run): it does not
  call nvidia-smi, does not start anything and writes nothing. With -Run it
    1. checks the inputs, the free disk space and the GPU (scripts\gpu-preflight.ps1 must pass:
       below 2 GB of GPU memory in use, no llama-server or python of others);
    2. builds the test, then runs it with TEMP and TMP in target\test-tmp and the work folder
       under target\e2e-tmp, with a time limit (-TimeoutMinutes);
    3. samples the machine every -SampleSeconds into target\ten-hour-run\samples.csv: GPU memory
       and load, free disk space, size of the work folder, working set of the worker and
       llama-server processes;
    4. in a finally block removes the scratch files (scripts\clean-test-tmp.ps1), runs
       scripts\check-temp-hygiene.ps1 and writes the result. A hygiene count above 0 fails.
  The test output goes to target\ten-hour-run\run.log. The result (stage times, peak VRAM, draft
  validity, item count, verdicts against the targets of docs\TESTING.md: 4 hours, 11.5 GB) goes
  to -ResultsOut. When -Hours is below 10 the result says full_length = false and the ten hour
  target is not claimed as met.

  The run may take hours (about 1.5 hours of processing were measured for the extraction of
  ten hours of text alone). It is meant for the reference machine, started by hand.

.PARAMETER AudioDir
  Folder with the synthetic meetings, one folder each (mixed.wav and annotation.json).

.PARAMETER Model
  GGUF file of the language model.

.PARAMETER LlamaServer
  llama-server executable.

.PARAMETER Hours
  Hours of audio to build and process (default 10).

.PARAMETER Run
  Actually run. Without it the script only validates and prints the plan.

.PARAMETER TimeoutMinutes
  Longest the test may run (default: the target time of 0.4 hours per hour of audio plus 2.5
  hours, that is 6.5 hours for -Hours 10). A test over the limit is stopped (its own process
  tree) and the run counts as failed.

.PARAMETER SampleSeconds
  Interval of the resource samples (default 60).

.PARAMETER MinFreeGB
  Free disk space required on the drive of the repository (default: 1.5 GB per hour of audio,
  at least 5, that is 15 GB for ten hours).

.PARAMETER ResultsOut
  Result file (default target\regression\gpu-results\stage8-e2e-10h.json, where
  scripts\regression.ps1 -ReportOnly picks it up).

.EXAMPLE
  scripts\ten-hour-run.ps1 -AudioDir D:\data\audio -Model D:\models\gemma.gguf -LlamaServer D:\llama\llama-server.exe

.EXAMPLE
  scripts\ten-hour-run.ps1 -Run -AudioDir D:\data\audio -Model D:\models\gemma.gguf -LlamaServer D:\llama\llama-server.exe
#>
[CmdletBinding()]
param(
    [string]$AudioDir = "",
    [string]$Model = "",
    [string]$LlamaServer = "",
    [double]$Hours = 10,
    [switch]$Run,
    [int]$TimeoutMinutes = 0,
    [int]$SampleSeconds = 60,
    [double]$MinFreeGB = 0,
    [string]$ResultsOut = ""
)

$ErrorActionPreference = "Stop"
$repo = Split-Path -Parent $PSScriptRoot
$testTmp = Join-Path $repo "target\test-tmp"
$e2eTmp = Join-Path $repo "target\e2e-tmp"
$runDir = Join-Path $repo "target\ten-hour-run"
$logPath = Join-Path $runDir "run.log"
$samplesPath = Join-Path $runDir "samples.csv"
$testResult = Join-Path $runDir "test-result.json"
if (-not $ResultsOut) { $ResultsOut = Join-Path $repo "target\regression\gpu-results\stage8-e2e-10h.json" }
if ($TimeoutMinutes -le 0) { $TimeoutMinutes = [int]($Hours * 24 + 150) }
if ($MinFreeGB -le 0) { $MinFreeGB = [math]::Max(5, 1.5 * $Hours) }
$workerDir = Join-Path $repo "worker"
$envDir = if ($env:BBB_WORKER_ENV_DIR) { $env:BBB_WORKER_ENV_DIR } else { Join-Path $workerDir ".venv" }

function ConvertTo-Count($text) {
    $n = 0
    if ([int]::TryParse(("$text").Trim(), [ref]$n)) { $n } else { 0 }
}

# ---- validation (no GPU access) ----------------------------------------------------------------
$problems = New-Object System.Collections.Generic.List[string]
if ($Hours -le 0) { $problems.Add("-Hours must be positive") }
if (-not $AudioDir -or -not (Test-Path -LiteralPath $AudioDir -PathType Container)) {
    $problems.Add("-AudioDir is not a folder: '$AudioDir'")
    $meetings = @()
} else {
    $meetings = @(Get-ChildItem -LiteralPath $AudioDir -Directory | Where-Object {
            (Test-Path (Join-Path $_.FullName "mixed.wav")) -and (Test-Path (Join-Path $_.FullName "annotation.json")) } |
        Sort-Object Name)
    if ($meetings.Count -eq 0) { $problems.Add("no meeting folder with mixed.wav and annotation.json in $AudioDir") }
}
if (-not $Model -or -not (Test-Path -LiteralPath $Model -PathType Leaf)) { $problems.Add("-Model is not a file: '$Model'") }
if (-not $LlamaServer -or -not (Test-Path -LiteralPath $LlamaServer -PathType Leaf)) { $problems.Add("-LlamaServer is not a file: '$LlamaServer'") }
$drive = (Get-Item -LiteralPath $repo).PSDrive
$freeGB = [math]::Round($drive.Free / 1GB, 1)
if ($freeGB -lt $MinFreeGB) { $problems.Add("only $freeGB GB free on drive $($drive.Name):, $MinFreeGB GB needed") }
if (-not (Test-Path -LiteralPath (Join-Path $envDir "Scripts\python.exe"))) {
    $problems.Add("no worker environment at $envDir (run: cd worker; uv sync --frozen)")
}
if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) { $problems.Add("cargo was not found") }

$plan = @(
    "ten hour end-to-end run",
    "  audio:      $Hours h of synthetic audio from $($meetings.Count) meeting(s) in '$AudioDir', repeated and cut into 60 s segments",
    "  model:      '$Model'",
    "  llama:      '$LlamaServer'",
    "  disk:       $freeGB GB free on $($drive.Name):, $MinFreeGB GB required",
    "  work files: $e2eTmp (removed at the end)",
    "  log:        $logPath",
    "  samples:    $samplesPath every $SampleSeconds s",
    "  result:     $ResultsOut",
    "  time limit: $TimeoutMinutes min (target for 10 h of audio: 4 h; extraction alone took about 1.5 h)",
    "  GPU:        scripts\gpu-preflight.ps1 must pass (below 2 GB in use, no llama-server or python of others)",
    "  command:    cargo test -p queue --test long_e2e_models -- --ignored --test-threads=1 --nocapture"
)
$plan | ForEach-Object { Write-Host $_ }
if ($Hours -lt 10) {
    Write-Host "  NOTE: $Hours h is shorter than 10 h. The result will say full_length = false; the ten hour target is not met by it." -ForegroundColor Yellow
}
if ($problems.Count -gt 0) {
    Write-Host "PROBLEMS:" -ForegroundColor Red
    $problems | ForEach-Object { Write-Host "  - $_" }
}
if (-not $Run) {
    Write-Host "Dry run: nothing was started. Add -Run to execute." -ForegroundColor Cyan
    if ($problems.Count -gt 0) { exit 1 }
    exit 0
}
if ($problems.Count -gt 0) { exit 1 }

# ---- the run ----------------------------------------------------------------------------------
& (Join-Path $PSScriptRoot "gpu-preflight.ps1")
if ($LASTEXITCODE -ne 0) { exit 1 }

$savedTemp = $env:TEMP
$savedTmp = $env:TMP
$proc = $null
$timedOut = $false
$exitCode = -1
$started = Get-Date
$samples = New-Object System.Collections.Generic.List[object]
$hygieneCount = $null
$status = "failed"
try {
    New-Item -ItemType Directory -Force -Path $runDir, $testTmp, $e2eTmp | Out-Null
    $env:TEMP = $testTmp
    $env:TMP = $testTmp
    Remove-Item -LiteralPath $testResult -ErrorAction SilentlyContinue

    Write-Host "=== building the test" -ForegroundColor Cyan
    Push-Location $repo
    try { cargo test -p queue --test long_e2e_models --no-run } finally { Pop-Location }
    if ($LASTEXITCODE -ne 0) { throw "the test did not build" }

    Write-Host "=== running (log: $logPath)" -ForegroundColor Cyan
    $psi = New-Object System.Diagnostics.ProcessStartInfo
    $psi.FileName = "cmd.exe"
    $psi.Arguments = "/d /s /c `"cargo test -p queue --test long_e2e_models -- --ignored --test-threads=1 --nocapture > `"$logPath`" 2>&1`""
    $psi.WorkingDirectory = $repo
    $psi.UseShellExecute = $false
    foreach ($pair in @{ TEMP = $testTmp; TMP = $testTmp; BBB_AUDIO_DIR = $AudioDir; BBB_LLM_MODEL = $Model
            BBB_LLAMA_SERVER = $LlamaServer; BBB_LONG_HOURS = "$Hours"; BBB_RESULTS_OUT = $testResult
            BBB_E2E_WORK = $e2eTmp; BBB_WORKER_ENV_DIR = $envDir }.GetEnumerator()) {
        $psi.EnvironmentVariables[$pair.Key] = [string]$pair.Value
    }
    "time,gpu_used_mib,gpu_util_pct,free_disk_gb,work_dir_mb,worker_ws_mb" | Set-Content -LiteralPath $samplesPath -Encoding ASCII
    $proc = [System.Diagnostics.Process]::Start($psi)
    $deadline = $started.AddMinutes($TimeoutMinutes)
    while (-not $proc.WaitForExit($SampleSeconds * 1000)) {
        $drive = (Get-Item -LiteralPath $repo).PSDrive
        $gpu = (& nvidia-smi --query-gpu=memory.used,utilization.gpu --format=csv,noheader,nounits --id=0 2>$null) -split ","
        $work = (Get-ChildItem -LiteralPath $e2eTmp -Recurse -File -ErrorAction SilentlyContinue | Measure-Object -Property Length -Sum).Sum
        $ws = (Get-Process -ErrorAction SilentlyContinue | Where-Object { $_.ProcessName -match '^(python|llama-server)$' } |
            Measure-Object -Property WorkingSet64 -Sum).Sum
        $s = [ordered]@{
            time = (Get-Date).ToString("s")
            gpu_used_mib = ConvertTo-Count $gpu[0]
            gpu_util_pct = ConvertTo-Count $gpu[1]
            free_disk_gb = [math]::Round($drive.Free / 1GB, 1)
            work_dir_mb = [math]::Round([double]$work / 1MB)
            worker_ws_mb = [math]::Round([double]$ws / 1MB)
        }
        # Property objects (Measure-Object cannot read an ordered dictionary), and a culture-free
        # CSV line: on a comma-decimal locale 110.7 would be written as "110,7" and split the column.
        $samples.Add([pscustomobject]$s)
        (($s.Values | ForEach-Object { [string]::Format([cultureinfo]::InvariantCulture, "{0}", $_) }) -join ",") |
            Add-Content -LiteralPath $samplesPath -Encoding ASCII
        if ((Get-Date) -gt $deadline) {
            $timedOut = $true
            Write-Host "time limit of $TimeoutMinutes min reached, stopping the test" -ForegroundColor Red
            & taskkill.exe /PID $proc.Id /T /F | Out-Null
            $proc.WaitForExit(30000) | Out-Null
            break
        }
    }
    $exitCode = if ($proc.HasExited) { $proc.ExitCode } else { -1 }
    $status = if ($exitCode -eq 0 -and -not $timedOut) { "passed" } else { "failed" }
    Write-Host "test finished: exit code $exitCode$(if ($timedOut) { ' (time limit)' })"
} catch {
    Write-Host "FAILED: $_" -ForegroundColor Red
} finally {
    # An interrupt must not leave a ten hour GPU job behind: stop our own test process tree.
    if ($proc -and -not $proc.HasExited) { & taskkill.exe /PID $proc.Id /T /F | Out-Null }
    $env:TEMP = $savedTemp
    $env:TMP = $savedTmp
    Write-Host "=== cleaning scratch files" -ForegroundColor Cyan
    & (Join-Path $PSScriptRoot "clean-test-tmp.ps1")
    $hygieneText = (& (Join-Path $PSScriptRoot "check-temp-hygiene.ps1") *>&1 | Out-String)
    $hygieneExit = $LASTEXITCODE
    Write-Host $hygieneText.TrimEnd()
    $hygieneCount = 0
    if ($hygieneExit -ne 0) {
        $m = [regex]::Match($hygieneText, '(\d+) directories')
        $hygieneCount = [math]::Max(1, $(if ($m.Success) { [int]$m.Groups[1].Value } else { 1 }))
        $status = "failed"
    }

    # The result: the test's own file plus what this script observed.
    $ended = Get-Date
    $doc = [ordered]@{}
    if (Test-Path -LiteralPath $testResult) {
        $parsed = Get-Content -LiteralPath $testResult -Raw | ConvertFrom-Json
        foreach ($p in $parsed.PSObject.Properties) { $doc[$p.Name] = $p.Value }
    } else {
        $doc["prototype"] = "stage8-e2e-10h"
        $doc["status"] = "error"
        $doc["error"] = "the test wrote no result (see $logPath)"
        $doc["hours_of_audio"] = $Hours
        $doc["full_length"] = ($Hours -ge 10)
        $doc["passed"] = $false
    }
    $peak = if ($samples.Count -gt 0) { ($samples | Measure-Object -Property gpu_used_mib -Maximum).Maximum } else { $null }
    $doc["script"] = [ordered]@{
        hours_requested = $Hours
        started = $started.ToString("s")
        ended = $ended.ToString("s")
        wall_minutes = [math]::Round(($ended - $started).TotalMinutes, 1)
        timed_out = $timedOut
        time_limit_minutes = $TimeoutMinutes
        test_exit_code = $exitCode
        hygiene_count = $hygieneCount
        samples = $samples.Count
        sampled_peak_gpu_used_mib = $peak
        sampled_min_free_disk_gb = if ($samples.Count -gt 0) { ($samples | Measure-Object -Property free_disk_gb -Minimum).Minimum } else { $null }
        sampled_peak_work_dir_mb = if ($samples.Count -gt 0) { ($samples | Measure-Object -Property work_dir_mb -Maximum).Maximum } else { $null }
        sampled_peak_worker_ws_mb = if ($samples.Count -gt 0) { ($samples | Measure-Object -Property worker_ws_mb -Maximum).Maximum } else { $null }
        samples_file = "target/ten-hour-run/samples.csv"
        log_file = "target/ten-hour-run/run.log"
    }
    if ($status -ne "passed") { $doc["passed"] = $false }
    New-Item -ItemType Directory -Force -Path (Split-Path -Parent $ResultsOut) | Out-Null
    ConvertTo-Json -InputObject $doc -Depth 8 | Set-Content -LiteralPath $ResultsOut -Encoding UTF8
    Write-Host "result: $ResultsOut"
}

if ($status -ne "passed") { Write-Host "RUN FAILED" -ForegroundColor Red; exit 1 }
if ($Hours -lt 10) { Write-Host "Run passed, but with $Hours h of audio: the ten hour target is NOT demonstrated." -ForegroundColor Yellow }
else { Write-Host "Run passed." -ForegroundColor Green }
exit 0
