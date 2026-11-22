<#
.SYNOPSIS
  Stage 8 regression: runs the suites, optionally the GPU measurements, and writes the report
  with a go/no-go recommendation.

.DESCRIPTION
  Two parts, chosen with -Mode:

    cpu (default)  Every CPU suite once, through scripts\test-all.ps1: cargo fmt/clippy/test
                   (unit, contract and integration tests of the crates), worker pytest (unit and
                   contract tests), benchmarks pytest (metrics and this report), UI unit tests
                   and the end-to-end tests on the mock backend. No GPU, no models, no network
                   (apart from -Prepare).

    gpu            Opt-in. Real models on the GPU, one job after the other, each once, each with
                   its own timeout: worker tests with models, the supervisor pipeline with the
                   resilience cases (worker killed per stage, GPU occupied), speaker recognition,
                   the whole pipeline with extraction, and the measurements on the Polish,
                   English and mixed meetings (ASR, diarization, extraction, two hour timing)
                   that are compared with the recorded baselines. It refuses to start a job
                   unless scripts\gpu-preflight.ps1 passes (below 2 GB of GPU memory in use, no
                   llama-server or python of others). Fresh results go to
                   target\regression\gpu-results, never over the recorded baselines.

  Both parts end with the report (benchmarks\src\bbb_bench\regression.py): per-threshold
  verdicts, deltas against the baselines, the resilience matrix and a recommendation. The
  report is built from whatever logs and results exist, so run -Mode gpu after -Mode cpu (or
  the other way round) and the second report contains both. Everything runs with TEMP and TMP
  in target\test-tmp; a finally block cleans up and scripts\check-temp-hygiene.ps1 runs last.
  A hygiene count above 0 fails the run. No scheduled task or service is created.

.PARAMETER Mode
  cpu (default) or gpu.

.PARAMETER Skip
  CPU part: suites to leave out (rust, worker, benchmarks, ui).

.PARAMETER Suite
  CPU part: run only these suites.

.PARAMETER Only
  GPU part: run only these steps (ids as printed by -DryRun).

.PARAMETER ReportOnly
  Run nothing; build the report from the logs and results that exist.

.PARAMETER DryRun
  Print the plan (steps, commands, missing parameters) and touch neither the GPU nor any file.

.PARAMETER Prepare
  Before the run: create the environments that the part needs and that do not exist yet
  (worker environment, npm packages). Needs the network.

.PARAMETER WriteResults
  Write stage8-regression.json and .md to benchmarks\results instead of target\regression\report.

.PARAMETER Accepted
  JSON file: threshold id -> reason, for target misses the project owner has accepted.

.PARAMETER AudioDir, MiniDir, SlidesDir, VisionDir, ExtractionDir, LongAudio, LlamaServer, Model
  GPU part: the synthetic data sets, the llama-server executable and the language model (GGUF).
  A step whose input is missing is skipped and reported as skipped.

.PARAMETER StepTimeoutMinutes
  GPU part: longest a single step may run (default 120). A step over the limit is stopped (its
  own process tree) and counts as failed.

.PARAMETER TotalTimeoutMinutes
  GPU part: longest the whole part may take (default 0 = no limit). No step is started after
  the limit (it is reported as skipped), and a running step gets at most the time that is left.

.EXAMPLE
  scripts\regression.ps1 -Prepare

.EXAMPLE
  scripts\regression.ps1 -Mode gpu -DryRun -AudioDir D:\data\audio

.EXAMPLE
  scripts\regression.ps1 -Mode gpu -AudioDir D:\data\audio -MiniDir D:\data\mini -SlidesDir D:\data\slides `
      -VisionDir D:\models\vision -ExtractionDir D:\data\extraction -LongAudio D:\data\long\long-10h.wav `
      -LlamaServer D:\llama\llama-server.exe -Model D:\models\gemma.gguf
#>
[CmdletBinding()]
param(
    [ValidateSet("cpu", "gpu")][string]$Mode = "cpu",
    [string[]]$Skip = @(),
    [string[]]$Suite = @(),
    [string[]]$Only = @(),
    [switch]$ReportOnly,
    [switch]$DryRun,
    [switch]$Prepare,
    [switch]$WriteResults,
    [string]$Accepted = "",
    [string]$AudioDir = "",
    [string]$MiniDir = "",
    [string]$SlidesDir = "",
    [string]$VisionDir = "",
    [string]$ExtractionDir = "",
    [string]$LongAudio = "",
    [string]$LlamaServer = "",
    [string]$Model = "",
    [int]$StepTimeoutMinutes = 120,
    [int]$TotalTimeoutMinutes = 0
)

$ErrorActionPreference = "Stop"
$Skip = @($Skip | ForEach-Object { $_ -split "," } | Where-Object { $_ })
$Suite = @($Suite | ForEach-Object { $_ -split "," } | Where-Object { $_ })
$Only = @($Only | ForEach-Object { $_ -split "," } | Where-Object { $_ })

$repo = Split-Path -Parent $PSScriptRoot
$testTmp = Join-Path $repo "target\test-tmp"
$regDir = Join-Path $repo "target\regression"
$logDir = Join-Path $regDir "logs"
$gpuDir = Join-Path $regDir "gpu-results"
$gpuLogDir = Join-Path $gpuDir "logs"
$reportDir = if ($WriteResults) { Join-Path $repo "benchmarks\results" } else { Join-Path $regDir "report" }
$workerPython = Join-Path $repo "worker\.venv\Scripts\python.exe"

function Q([string]$s) { '"' + $s + '"' }
function Join-Quoted([string]$base, [string[]]$names) {
    if (-not $base) { return "" }
    ($names | ForEach-Object { Q (Join-Path $base $_) }) -join " "
}

# The GPU steps. Keep the ids in sync with GPU_STEPS in benchmarks\src\bbb_bench\regression.py
# (a test compares the two lists).
$gpuSteps = @(
    @{ id = "worker-models"; needs = @("MiniDir", "SlidesDir", "VisionDir", "LlamaServer"); cwd = "worker"
       cmd = "uv run --frozen pytest -m models -q"
       env = @{ BBB_MINI_DIR = $MiniDir; BBB_SLIDES_DIR = $SlidesDir; BBB_VISION_DIR = $VisionDir; BBB_LLAMA_SERVER = $LlamaServer } },
    @{ id = "pipeline-models"; needs = @("MiniDir", "SlidesDir", "VisionDir", "LlamaServer"); cwd = "."
       cmd = "cargo test -p queue --test pipeline_models -- --ignored --test-threads=1"
       env = @{ BBB_MINI_DIR = $MiniDir; BBB_SLIDES_DIR = $SlidesDir; BBB_VISION_DIR = $VisionDir; BBB_LLAMA_SERVER = $LlamaServer } },
    @{ id = "speakers-models"; needs = @("AudioDir"); cwd = "."
       cmd = "cargo test -p queue --test speakers_models -- --ignored --test-threads=1 --nocapture"
       env = @{ BBB_AUDIO_DIR = $AudioDir; BBB_RESULTS_OUT = (Join-Path $gpuDir "stage3-speaker-recognition.json") } },
    @{ id = "stage4-models"; needs = @("AudioDir", "Model", "LlamaServer"); cwd = "."
       cmd = "cargo test -p queue --test stage4_models -- --ignored --test-threads=1 --nocapture"
       env = @{ BBB_AUDIO_DIR = $AudioDir; BBB_LLM_MODEL = $Model; BBB_LLAMA_SERVER = $LlamaServer } },
    @{ id = "asr"; needs = @("AudioDir"); cwd = "benchmarks"
       cmd = "uv run --group asr python -m bbb_bench.proto.p2_asr --model turbo --language auto --meetings " +
             (Join-Quoted $AudioDir @("pl-03", "en-03", "mixed-04"))
       env = @{} },
    @{ id = "diarization"; needs = @("AudioDir"); cwd = "benchmarks"
       cmd = "uv run --group diar python -m bbb_bench.proto.p3_diarize --meetings " +
             (Join-Quoted $AudioDir @("pl-03", "pl-08", "pl-16", "en-03", "mixed-04"))
       env = @{} },
    @{ id = "extraction"; needs = @("ExtractionDir", "Model", "LlamaServer"); cwd = "benchmarks"
       cmd = "uv run python -m bbb_bench.proto.stage4_extraction --model $(Q $Model) --name gemma-4-12b-q4 " +
             "--llama-server $(Q $LlamaServer) --worker-python $(Q $workerPython) --meetings @EXTRACTION@"
       env = @{} },
    @{ id = "extraction-2h"; needs = @("ExtractionDir", "Model", "LlamaServer"); cwd = "benchmarks"
       cmd = "uv run python -m bbb_bench.proto.stage4_extraction --model $(Q $Model) --name gemma-4-12b-q4 " +
             "--llama-server $(Q $LlamaServer) --worker-python $(Q $workerPython) --hours 2 --meetings @EXTRACTION@"
       env = @{} },
    @{ id = "pipeline-2h"; needs = @("LongAudio", "SlidesDir", "VisionDir", "LlamaServer"); cwd = "benchmarks"
       cmd = "uv run python -m bbb_bench.proto.stage2_pipeline --audio $(Q $LongAudio) --hours 2 --slides $(Q $SlidesDir) " +
             "--worker-python $(Q $workerPython) --work $(Q (Join-Path $regDir 'pipeline-2h-work'))"
       env = @{ BBB_LLAMA_SERVER = $LlamaServer; BBB_VISION_DIR = $VisionDir } }
)
if ($Only.Count -gt 0) {
    foreach ($id in $Only) { if ($id -notin $gpuSteps.id) { throw "unknown GPU step '$id' ($($gpuSteps.id -join ', '))" } }
    $gpuSteps = @($gpuSteps | Where-Object { $_.id -in $Only })
}

function Get-Missing($step) {
    @($step.needs | Where-Object { -not (Get-Variable -Name $_ -ValueOnly) })
}

function Invoke-Timed($step, [string]$logPath, [int]$timeoutMinutes) {
    # cmd.exe does the redirection, so the log fills while the step runs. The step's process tree
    # is stopped only when it is over its own time limit.
    $psi = New-Object System.Diagnostics.ProcessStartInfo
    $psi.FileName = "cmd.exe"
    $psi.Arguments = "/d /s /c `"$($step.cmd) > $(Q $logPath) 2>&1`""
    $psi.WorkingDirectory = Join-Path $repo $step.cwd
    $psi.UseShellExecute = $false
    $psi.EnvironmentVariables["TEMP"] = $testTmp
    $psi.EnvironmentVariables["TMP"] = $testTmp
    $psi.EnvironmentVariables["BBB_RESULTS_DIR"] = $gpuDir
    $psi.EnvironmentVariables["BBB_E2E_WORK"] = Join-Path $repo "target\e2e-tmp"
    foreach ($k in $step.env.Keys) { if ($step.env[$k]) { $psi.EnvironmentVariables[$k] = [string]$step.env[$k] } }
    $watch = [System.Diagnostics.Stopwatch]::StartNew()
    $p = [System.Diagnostics.Process]::Start($psi)
    $finished = $p.WaitForExit([int]($timeoutMinutes * 60 * 1000))
    if (-not $finished) {
        & taskkill.exe /PID $p.Id /T /F | Out-Null
        $p.WaitForExit(30000) | Out-Null
        return @{ status = "failed"; seconds = [math]::Round($watch.Elapsed.TotalSeconds, 1); note = "stopped after the time limit of $timeoutMinutes min" }
    }
    $code = $p.ExitCode
    return @{ status = $(if ($code -eq 0) { "passed" } else { "failed" }); seconds = [math]::Round($watch.Elapsed.TotalSeconds, 1); note = $(if ($code -eq 0) { "" } else { "exit code $code" }) }
}

# ---- plan only ---------------------------------------------------------------------------------
if ($DryRun) {
    Write-Host "regression plan (dry run, nothing is executed)" -ForegroundColor Cyan
    if ($Mode -eq "cpu") {
        Write-Host "CPU part: scripts\test-all.ps1 -LogDir target\regression\logs, steps:"
        $listArgs = @{ List = $true }
        if ($Skip.Count -gt 0) { $listArgs.Skip = $Skip }
        if ($Suite.Count -gt 0) { $listArgs.Suite = $Suite }
        & (Join-Path $PSScriptRoot "test-all.ps1") @listArgs
    } else {
        Write-Host "GPU part (opt-in): scripts\gpu-preflight.ps1 before every step, timeout $StepTimeoutMinutes min each, results in target\regression\gpu-results"
        foreach ($step in $gpuSteps) {
            $missing = Get-Missing $step
            $state = if ($missing.Count -gt 0) { "SKIPPED, missing: " + (($missing | ForEach-Object { "-$_" }) -join ", ") } else { "would run" }
            Write-Host ("  {0,-16} {1}" -f $step.id, $state)
            Write-Host ("  {0,-16}   (in {1}) {2}" -f "", $step.cwd, $step.cmd)
        }
    }
    Write-Host "report: benchmarks\src\bbb_bench\regression.py -> $reportDir"
    Write-Host "hygiene: scripts\check-temp-hygiene.ps1 at the end (exit code 1 when anything is left)"
    exit 0
}

# ---- run -----------------------------------------------------------------------------------------
$savedTemp = $env:TEMP
$savedTmp = $env:TMP
$savedResults = $env:BBB_RESULTS_DIR
$failed = New-Object System.Collections.Generic.List[string]
$hygieneCount = $null
$reportExit = 0
try {
    New-Item -ItemType Directory -Force -Path $testTmp | Out-Null
    $env:TEMP = $testTmp
    $env:TMP = $testTmp

    if (-not $ReportOnly -and $Prepare) {
        Write-Host "=== preparing the environments" -ForegroundColor Cyan
        if ($Mode -eq "cpu") {
            Push-Location (Join-Path $repo "worker"); try { uv sync --frozen --only-group dev } finally { Pop-Location }
            if ($LASTEXITCODE -ne 0) { throw "uv sync (worker) failed" }
            if (-not (Test-Path (Join-Path $repo "ui\node_modules"))) {
                Push-Location (Join-Path $repo "ui"); try { npm ci } finally { Pop-Location }
                if ($LASTEXITCODE -ne 0) { throw "npm ci failed" }
            }
        } else {
            Push-Location (Join-Path $repo "worker"); try { uv sync --frozen } finally { Pop-Location }
            if ($LASTEXITCODE -ne 0) { throw "uv sync (worker, full) failed" }
        }
    }

    if (-not $ReportOnly -and $Mode -eq "cpu") {
        Write-Host "=== CPU part: every suite once" -ForegroundColor Cyan
        # A full run starts clean; a partial one (-Suite, -Skip) adds to the logs that exist.
        if (($Skip.Count -eq 0) -and ($Suite.Count -eq 0) -and (Test-Path $logDir)) { Remove-Item -LiteralPath $logDir -Recurse -Force }
        $cpuArgs = @{ LogDir = $logDir }
        if ($Skip.Count -gt 0) { $cpuArgs.Skip = $Skip }
        if ($Suite.Count -gt 0) { $cpuArgs.Suite = $Suite }
        & (Join-Path $PSScriptRoot "test-all.ps1") @cpuArgs
        if ($LASTEXITCODE -ne 0) { $failed.Add("CPU suites") }
    }

    if (-not $ReportOnly -and $Mode -eq "gpu") {
        Write-Host "=== GPU part (opt-in): one step after the other" -ForegroundColor Cyan
        New-Item -ItemType Directory -Force -Path $gpuLogDir | Out-Null
        $extractionDirs = @()
        if ($ExtractionDir -and (Test-Path -LiteralPath $ExtractionDir)) {
            $extractionDirs = @(Get-ChildItem -LiteralPath $ExtractionDir -Directory | Sort-Object Name | ForEach-Object { Q $_.FullName })
        }
        $rows = New-Object System.Collections.Generic.List[object]
        $env:BBB_RESULTS_DIR = $gpuDir
        $partWatch = [System.Diagnostics.Stopwatch]::StartNew()
        foreach ($step in $gpuSteps) {
            $row = [ordered]@{ id = $step.id; status = "skipped"; seconds = $null; reason = ""; log = "logs/$($step.id).log" }
            $missing = Get-Missing $step
            $leftMinutes = if ($TotalTimeoutMinutes -gt 0) { $TotalTimeoutMinutes - $partWatch.Elapsed.TotalMinutes } else { [double]::MaxValue }
            if ($leftMinutes -le 1) {
                $row.reason = "total time limit of $TotalTimeoutMinutes min reached"
            } elseif ($missing.Count -gt 0) {
                $row.reason = "missing parameter: " + (($missing | ForEach-Object { "-$_" }) -join ", ")
            } elseif ($step.cmd -match "@EXTRACTION@" -and $extractionDirs.Count -eq 0) {
                $row.reason = "no meeting folders in -ExtractionDir"
            } else {
                & (Join-Path $PSScriptRoot "gpu-preflight.ps1") -Quiet | Out-Host
                if ($LASTEXITCODE -ne 0) {
                    $row.reason = "GPU preflight refused"
                } else {
                    $step.cmd = $step.cmd.Replace("@EXTRACTION@", ($extractionDirs -join " "))
                    Write-Host "--- $($step.id)" -ForegroundColor Cyan
                    $limit = [int][math]::Max(1, [math]::Min($StepTimeoutMinutes, [math]::Floor($leftMinutes)))
                    $result = Invoke-Timed $step (Join-Path $gpuLogDir "$($step.id).log") $limit
                    $row.status = $result.status
                    $row.seconds = $result.seconds
                    $row.reason = $result.note
                    Write-Host ("    {0} in {1} s {2}" -f $result.status, $result.seconds, $result.note)
                }
            }
            if ($row.status -eq "failed") { $failed.Add("GPU step $($step.id)") }
            $rows.Add($row)
        }
        ConvertTo-Json -InputObject @($rows.ToArray()) -Depth 4 | Set-Content -LiteralPath (Join-Path $gpuDir "gpu-steps.json") -Encoding UTF8
        # The two hour timing run leaves a work folder of segment files behind.
        $pipelineWork = Join-Path $regDir "pipeline-2h-work"
        if (Test-Path -LiteralPath $pipelineWork) { Remove-Item -LiteralPath $pipelineWork -Recurse -Force }
    }
} catch {
    Write-Host "FAILED: $_" -ForegroundColor Red
    $failed.Add("$_")
} finally {
    $env:BBB_RESULTS_DIR = $savedResults
    if (-not $ReportOnly) {
        Write-Host ""
        Write-Host "=== cleaning scratch files" -ForegroundColor Cyan
        & (Join-Path $PSScriptRoot "clean-test-tmp.ps1")
    }
    $hygieneText = (& (Join-Path $PSScriptRoot "check-temp-hygiene.ps1") *>&1 | Out-String)
    $hygieneExit = $LASTEXITCODE
    Write-Host $hygieneText.TrimEnd()
    $hygieneCount = 0
    if ($hygieneExit -ne 0) {
        $m = [regex]::Match($hygieneText, '(\d+) directories')
        $hygieneCount = [math]::Max(1, $(if ($m.Success) { [int]$m.Groups[1].Value } else { 1 }))
        $failed.Add("temp hygiene")
    }

    # The report, from whatever logs and results exist.
    Write-Host ""
    Write-Host "=== report" -ForegroundColor Cyan
    $reportArgs = @("run", "python", "-m", "bbb_bench.regression", "--out", $reportDir, "--hygiene-count", "$hygieneCount")
    if (Test-Path -LiteralPath (Join-Path $logDir "steps.json")) { $reportArgs += @("--logs", $logDir) }
    if (Test-Path -LiteralPath $gpuDir) { $reportArgs += @("--gpu-results", $gpuDir) }
    if ($Accepted) { $reportArgs += @("--accepted", $Accepted) }
    Push-Location (Join-Path $repo "benchmarks")
    try { & uv @reportArgs; $reportExit = $LASTEXITCODE } catch { $reportExit = 1 } finally { Pop-Location }
    if ($reportExit -ne 0) { $failed.Add("report generator") }
    # uv leaves a lock file in the redirected temp folder: remove it again.
    & (Join-Path $PSScriptRoot "clean-test-tmp.ps1") -Quiet | Out-Null
    $env:TEMP = $savedTemp
    $env:TMP = $savedTmp
}

if ($failed.Count -gt 0) {
    Write-Host ""
    Write-Host "Failed: $($failed -join '; ')" -ForegroundColor Red
    exit 1
}
Write-Host "Done. Report: $reportDir" -ForegroundColor Green
exit 0
