<#
.SYNOPSIS
  Builds the Windows installer, verifies what is inside it and writes SHA256SUMS.txt.

.DESCRIPTION
  Steps (any failure stops the script with a non-zero exit code):
    1. Checks that Cargo, Tauri and the UI package agree on the version (and on -Tag, if given).
    2. Builds the UI (npm ci, npm run build).
    3. Stages the resources of the installer in target\installer-stage: the worker sources
       (no tests, no caches), the JSON schemas, NOTICE, LICENSE and a pinned uv.exe. The uv
       release is downloaded and its SHA256 is compared with the pinned value below; -UvExe
       takes an existing uv.exe of the pinned version instead (no download).
    4. Runs `tauri build` for the NSIS target with crates\app\tauri.bundle.conf.json, with source
       paths removed from the binary (--remap-path-prefix).
    5. Copies the installer to target\installer\Blah-Blah-Bot_<version>_x64-setup.exe (no spaces,
       so that the name survives a GitHub release upload) and extracts it with 7-Zip into
       target\installer-verify to check the contents: the required files exist; no model files,
       caches, environment files, keys or tokens; no absolute paths of this machine in the
       files or in the executable. The verification folder is removed afterwards.
    6. Writes target\installer\CONTENTS.txt (the file list) and target\installer\SHA256SUMS.txt.

  Everything is written under target\. The temp directory is redirected to target\test-tmp for
  the duration of the build and cleaned afterwards; the hygiene check runs last.

  Signing is off by default (docs\decisions\0024-installer-signing.md). When the environment
  variable BBB_SIGN_COMMAND is set, it is passed to Tauri as bundle.windows.signCommand
  ("%1" stands for the file to sign) and the checksum is computed after signing.

.PARAMETER Tag
  Release tag (for example v1.0.0) that the version in the files must match.

.PARAMETER UvExe
  Path of a uv.exe of the pinned version to bundle instead of downloading the pinned release.

.PARAMETER SkipUiBuild
  Use the existing ui\dist and ui\node_modules.

.PARAMETER Scan
  Run Windows Defender (MpCmdRun.exe -Scan -ScanType 3 -DisableRemediation) on the built installer.

.EXAMPLE
  scripts\build-installer.ps1
#>
[CmdletBinding()]
param(
    [string]$Tag,
    [string]$UvExe,
    [switch]$SkipUiBuild,
    [switch]$SkipBuild,
    [switch]$Scan
)

$ErrorActionPreference = "Stop"

# The uv release that is bundled. To change it, take the checksum from the .sha256 file
# next to the asset on https://github.com/astral-sh/uv/releases.
$UvVersion = "0.11.15"
$UvZipSha256 = "04b98d414a9000e25e5e0e7c9f53749e66b790cdaffc582829e6f58c544ee11c"
$UvZipUrl = "https://github.com/astral-sh/uv/releases/download/$UvVersion/uv-x86_64-pc-windows-msvc.zip"

$repo = Split-Path -Parent $PSScriptRoot
$target = Join-Path $repo "target"
$testTmp = Join-Path $target "test-tmp"
$stage = Join-Path $target "installer-stage"
$outDir = Join-Path $target "installer"
$verifyDir = Join-Path $target "installer-verify"
$downloadDir = Join-Path $target "installer-download"

$savedTemp = $env:TEMP
$savedTmp = $env:TMP
$savedRustflags = $env:RUSTFLAGS
$savedInstallerBuild = $env:BBB_INSTALLER_BUILD
$failed = $false

function Invoke-Native([string]$title, [scriptblock]$command) {
    Write-Host ""
    Write-Host "=== $title" -ForegroundColor Cyan
    & $command
    if ($LASTEXITCODE -ne 0) { throw "$title failed (exit code $LASTEXITCODE)" }
}

function Get-RelativeFiles([string]$root) {
    $base = (Resolve-Path -LiteralPath $root).Path.TrimEnd('\')
    Get-ChildItem -LiteralPath $base -Recurse -File -Force | ForEach-Object {
        [pscustomobject]@{
            Path = $_.FullName.Substring($base.Length + 1).Replace('\', '/')
            Full = $_.FullName
            Size = $_.Length
        }
    }
}

function Get-RequiredVersion {
    (Get-Content -LiteralPath (Join-Path $repo "crates\app\tauri.conf.json") -Raw | ConvertFrom-Json).version
}

function Resolve-SignCommand([string]$raw) {
    $trimmed = $raw.Trim()
    if ($trimmed.StartsWith("{")) {
        try {
            $parsed = $trimmed | ConvertFrom-Json
        } catch {
            throw "BBB_SIGN_COMMAND looks like JSON but failed to parse: $_"
        }
        if (-not $parsed.cmd -or -not ($parsed.cmd -is [string]) -or [string]::IsNullOrWhiteSpace($parsed.cmd)) {
            throw "BBB_SIGN_COMMAND JSON must contain a non-empty 'cmd' string"
        }
        $hasPlaceholder = $false
        if ($null -ne $parsed.args -and -not ($parsed.args -is [System.Collections.IList])) {
            throw "BBB_SIGN_COMMAND JSON 'args' must be an array of strings"
        }
        if ($parsed.args) {
            foreach ($arg in $parsed.args) {
                if ("$arg" -like "*%1*") { $hasPlaceholder = $true; break }
            }
        }
        if (-not $hasPlaceholder) {
            throw "BBB_SIGN_COMMAND JSON must contain 'args' with at least one '%1' placeholder"
        }
        return $parsed
    }
    if ($trimmed -notlike "*%1*") {
        throw "BBB_SIGN_COMMAND must contain '%1' as the file placeholder (or be JSON with 'cmd' and 'args' containing '%1')"
    }
    return $trimmed
}

# ----- staging -------------------------------------------------------------------------------

function Initialize-Stage {
    if (Test-Path -LiteralPath $stage) { Remove-Item -LiteralPath $stage -Recurse -Force }
    New-Item -ItemType Directory -Force -Path $stage | Out-Null

    # Worker: what `uv sync --frozen` and the worker need, without tests and caches.
    $worker = Join-Path $stage "worker"
    New-Item -ItemType Directory -Force -Path $worker | Out-Null
    foreach ($name in "pyproject.toml", "uv.lock", ".python-version") {
        Copy-Item -LiteralPath (Join-Path $repo "worker\$name") -Destination $worker
    }
    $src = Join-Path $repo "worker\src"
    $srcOut = Join-Path $worker "src"
    foreach ($f in Get-ChildItem -LiteralPath $src -Recurse -File -Force) {
        $rel = $f.FullName.Substring($src.Length + 1)
        if ($rel -match '(^|\\)__pycache__(\\|$)' -or $f.Extension -in ".pyc", ".pyo") { continue }
        $dest = Join-Path $srcOut $rel
        New-Item -ItemType Directory -Force -Path (Split-Path -Parent $dest) | Out-Null
        Copy-Item -LiteralPath $f.FullName -Destination $dest
    }

    # Schemas sit next to worker\ (the worker finds them from its own location).
    $schemas = Join-Path $stage "schemas"
    New-Item -ItemType Directory -Force -Path $schemas | Out-Null
    Copy-Item -Path (Join-Path $repo "schemas\*.json") -Destination $schemas

    Copy-Item -LiteralPath (Join-Path $repo "NOTICE") -Destination (Join-Path $stage "NOTICE.txt")
    Copy-Item -LiteralPath (Join-Path $repo "LICENSE") -Destination (Join-Path $stage "LICENSE.txt")

    # Generated list of third-party licenses (optional, produced by the audit tooling).
    $licenses = Join-Path $target "release-licenses\third-party-licenses.html"
    if (Test-Path -LiteralPath $licenses) {
        Copy-Item -LiteralPath $licenses -Destination (Join-Path $stage "third-party-licenses.html")
    }

    Initialize-Uv
}

function Initialize-Uv {
    $bin = Join-Path $stage "bin"
    New-Item -ItemType Directory -Force -Path $bin | Out-Null
    $dest = Join-Path $bin "uv.exe"
    if ($UvExe) {
        Copy-Item -LiteralPath $UvExe -Destination $dest
        Write-Host "uv.exe taken from -UvExe; its SHA256 is $((Get-FileHash -LiteralPath $dest -Algorithm SHA256).Hash.ToLower())"
    } else {
        New-Item -ItemType Directory -Force -Path $downloadDir | Out-Null
        $zip = Join-Path $downloadDir "uv.zip"
        Write-Host "downloading uv $UvVersion from the release page of astral-sh/uv"
        Invoke-WebRequest -Uri $UvZipUrl -OutFile $zip -UseBasicParsing
        $actual = (Get-FileHash -LiteralPath $zip -Algorithm SHA256).Hash.ToLower()
        if ($actual -ne $UvZipSha256) {
            Remove-Item -LiteralPath $zip -Force
            throw "the downloaded uv archive has SHA256 $actual, expected $UvZipSha256"
        }
        Write-Host "uv archive SHA256 matches the pinned value"
        $unzipped = Join-Path $downloadDir "uv"
        Expand-Archive -LiteralPath $zip -DestinationPath $unzipped -Force
        Copy-Item -LiteralPath (Join-Path $unzipped "uv.exe") -Destination $dest
    }
    $reported = (& $dest --version) -join " "
    if ($reported -notmatch [regex]::Escape($UvVersion)) {
        throw "the bundled uv reports '$reported', expected version $UvVersion"
    }
    Write-Host "bundled $reported"
}

# ----- verification --------------------------------------------------------------------------

$Required = @(
    '^worker/pyproject\.toml$', '^worker/uv\.lock$', '^worker/\.python-version$',
    '^worker/src/bbb_worker/__init__\.py$', '^worker/src/bbb_worker/__main__\.py$',
    '^schemas/transcript\.schema\.json$', '^schemas/job-manifest\.schema\.json$',
    '^bin/uv\.exe$', '^NOTICE\.txt$', '^LICENSE\.txt$',
    '^app\.exe$'
)
$ForbiddenName = '(\.(gguf|onnx|safetensors|bin|pt|pth|ckpt|npy|npz|wav|flac|mp3|ogg|m4a|mp4|mkv|db|sqlite|log|pem|pfx|p12|key|pyc|pyo)$|(^|/)(\.env[^/]*|\.venv|__pycache__|\.git|node_modules|tests?|models)(/|$)|(^|/)(id_rsa|id_ed25519)$)'
$SecretPatterns = [ordered]@{
    "Hugging Face token"  = 'hf_[A-Za-z0-9]{30,}'
    "GitHub token"        = '(ghp|gho|ghu|ghs|ghr)_[A-Za-z0-9]{30,}|github_pat_[A-Za-z0-9_]{30,}'
    "API key"             = 'sk-[A-Za-z0-9_-]{24,}|AKIA[0-9A-Z]{16}|xox[abprs]-[A-Za-z0-9-]{10,}'
    "private key"         = '-----BEGIN [A-Z ]*PRIVATE KEY-----\s*(?:[A-Za-z-]+:[^\r\n]*\s*)*(?:[A-Za-z0-9+/=]\s*){64,}'
    "Discord webhook URL" = 'discord(app)?\.com/api/webhooks/\d+/[A-Za-z0-9_-]+'
}
$AbsolutePath = '(?i)[A-Za-z]:[\\/]+(Users|Documents and Settings)[\\/]+[^\\/\s"<>|*?]+|(?<![A-Za-z0-9_.-])(?!/users/me/)/(home|Users)/[A-Za-z0-9._-]+/'

function Test-Installer([string]$installer) {
    $problems = New-Object System.Collections.Generic.List[string]
    $sevenZip = (Get-Command 7z -ErrorAction SilentlyContinue).Source
    if (-not $sevenZip) { $sevenZip = "C:\Program Files\7-Zip\7z.exe" }
    if (-not (Test-Path -LiteralPath $sevenZip)) { throw "7-Zip is needed to inspect the installer (7z not found)" }

    if (Test-Path -LiteralPath $verifyDir) { Remove-Item -LiteralPath $verifyDir -Recurse -Force }
    New-Item -ItemType Directory -Force -Path $verifyDir | Out-Null
    Invoke-Native "extract the installer payload with 7-Zip (nothing is installed or run)" {
        & $sevenZip x -y "-o$verifyDir" $installer | Out-Null
    }
    # The NSIS payload also holds the installer's own plug-ins and script.
    $files = @(Get-RelativeFiles $verifyDir | Where-Object { $_.Path -notmatch '^(\$PLUGINSDIR|\[NSIS\]\.nsi)' })
    Write-Host "$($files.Count) files in the payload"

    $listing = $files | Sort-Object Path | ForEach-Object { "{0,10}  {1}" -f $_.Size, $_.Path }
    Set-Content -LiteralPath (Join-Path $outDir "CONTENTS.txt") -Value $listing -Encoding utf8

    # The main program is named after the product; find it by the Tauri product name.
    $names = $files.Path
    foreach ($pattern in $Required) {
        $hit = $names | Where-Object { $_ -match $pattern }
        if (-not $hit) { $problems.Add("missing from the installer: $pattern") }
    }
    foreach ($f in $files) {
        if ($f.Path -match $ForbiddenName) { $problems.Add("forbidden file in the installer: $($f.Path)") }
    }
    $worker = @($files | Where-Object { $_.Path -like 'worker/*' })
    if (-not ($worker | Where-Object { $_.Path -like 'worker/src/bbb_worker/steps/*' })) {
        $problems.Add("worker/src/bbb_worker/steps is missing")
    }

    # Text files: no absolute paths, no secrets.
    $textExt = ".py", ".toml", ".lock", ".json", ".txt", ".html", ".md", ".cfg", ".ini", ".yaml", ".yml"
    foreach ($f in $files) {
        $ext = [IO.Path]::GetExtension($f.Path).ToLower()
        if (($ext -notin $textExt) -and ($f.Path -notmatch '(^|/)\.python-version$')) { continue }
        $text = [IO.File]::ReadAllText($f.Full)
        # uv.lock and pyproject name public URLs only; any machine path is a defect.
        $m = [regex]::Match($text, $AbsolutePath)
        if ($m.Success) { $problems.Add("absolute path in $($f.Path): $($m.Value)") }
        foreach ($kind in $SecretPatterns.Keys) {
            if ([regex]::IsMatch($text, $SecretPatterns[$kind])) { $problems.Add("$kind found in $($f.Path)") }
        }
    }

    # The executables: this machine's user name and checkout path must not be inside.
    $needles = @($env:USERNAME, $repo, $env:USERPROFILE, $env:LOCALAPPDATA) |
        Where-Object { $_ -and $_.Length -ge 4 } | Sort-Object -Unique
    foreach ($f in $files | Where-Object { $_.Path -match '\.(exe|dll)$' -and $_.Path -ne 'bin/uv.exe' -and $_.Path -ne 'uninstall.exe' }) {
        $bytes = [IO.File]::ReadAllBytes($f.Full)
        $ascii = [Text.Encoding]::GetEncoding(28591).GetString($bytes)
        $utf16 = [Text.Encoding]::Unicode.GetString($bytes)
        $utf16odd = [Text.Encoding]::Unicode.GetString($bytes, 1, $bytes.Length - 1 - (($bytes.Length - 1) % 2))
        foreach ($needle in $needles) {
            foreach ($hay in $ascii, $utf16, $utf16odd) {
                if ($hay.IndexOf($needle, [StringComparison]::OrdinalIgnoreCase) -ge 0) {
                    $problems.Add("$($f.Path) contains a path or name of this machine (length $($needle.Length), starts '$($needle.Substring(0, 3))...')")
                    break
                }
            }
        }
        $hits = [regex]::Matches($ascii, $AbsolutePath) | ForEach-Object { $_.Value } | Sort-Object -Unique
        foreach ($h in $hits) { $problems.Add("absolute user path in $($f.Path): $h") }
        foreach ($kind in $SecretPatterns.Keys) {
            if ([regex]::IsMatch($ascii, $SecretPatterns[$kind])) { $problems.Add("$kind found in $($f.Path)") }
        }
    }
    Remove-Item -LiteralPath $verifyDir -Recurse -Force
    return $problems
}

# ----- main ----------------------------------------------------------------------------------

try {
    Set-Location $repo
    New-Item -ItemType Directory -Force -Path $testTmp, $outDir | Out-Null

    Write-Host "=== version consistency" -ForegroundColor Cyan
    if ($Tag) { python scripts/check_versions.py --tag $Tag } else { python scripts/check_versions.py }
    if ($LASTEXITCODE -ne 0) { throw "the version differs between Cargo, Tauri and the UI package" }
    $version = Get-RequiredVersion
    if (-not (Test-Path -LiteralPath (Join-Path $repo "CHANGELOG.md"))) { throw "CHANGELOG.md is missing" }

    $signCommandValue = $null
    if ($env:BBB_SIGN_COMMAND) {
        $signCommandValue = Resolve-SignCommand $env:BBB_SIGN_COMMAND
    }

    $env:TEMP = $testTmp
    $env:TMP = $testTmp

    if (-not $SkipBuild) {
        if (-not $SkipUiBuild) {
            Push-Location (Join-Path $repo "ui")
            try {
                Invoke-Native "npm ci" { npm ci }
                Invoke-Native "npm run build" { npm run build }
            } finally { Pop-Location }
        }
        if (-not (Test-Path -LiteralPath (Join-Path $repo "ui\dist\index.html"))) { throw "ui\dist\index.html is missing; build the UI first" }

        Write-Host ""
        Write-Host "=== stage the resources" -ForegroundColor Cyan
        Initialize-Stage

        # Source paths must not end up in the executable (panic messages and debug paths).
        $cargoHome = if ($env:CARGO_HOME) { $env:CARGO_HOME } else { Join-Path $env:USERPROFILE ".cargo" }
        $rustupHome = if ($env:RUSTUP_HOME) { $env:RUSTUP_HOME } else { Join-Path $env:USERPROFILE ".rustup" }
        $flags = @("--remap-path-prefix=$repo=/src", "--remap-path-prefix=$cargoHome=/cargo", "--remap-path-prefix=$rustupHome=/rustup")
        # CARGO_ENCODED_RUSTFLAGS splits on 0x1f, so paths with spaces stay intact.
        $env:CARGO_ENCODED_RUSTFLAGS = $flags -join [char]0x1f

        $configs = @("--config", "tauri.bundle.conf.json")
        if ($null -ne $signCommandValue) {
            $signConfig = Join-Path $stage "tauri.sign.conf.json"
            $json = @{ bundle = @{ windows = @{ signCommand = $signCommandValue } } } | ConvertTo-Json -Depth 5
            [IO.File]::WriteAllText($signConfig, $json, (New-Object Text.UTF8Encoding($false)))
            $configs += @("--config", $signConfig)
            Write-Host "signing is on (BBB_SIGN_COMMAND is set)"
        }

        # Turns on the bundle resource guard in crates/app/build.rs.
        $env:BBB_INSTALLER_BUILD = "1"
        Push-Location (Join-Path $repo "crates\app")
        try {
            $tauri = Join-Path $repo "ui\node_modules\.bin\tauri.cmd"
            Invoke-Native "tauri build (NSIS)" { & $tauri build --ci --bundles nsis @configs }
        } finally {
            $env:BBB_INSTALLER_BUILD = $savedInstallerBuild
            Pop-Location
        }
        $env:RUSTFLAGS = $savedRustflags
        $env:CARGO_ENCODED_RUSTFLAGS = $null

    }

    $built = @(Get-ChildItem -LiteralPath (Join-Path $target "release\bundle\nsis") -Filter "*_${version}_x64-setup.exe")
    if ($built.Count -ne 1) { throw "expected exactly one *_${version}_x64-setup.exe in target\release\bundle\nsis, found $($built.Count)" }

    Get-ChildItem -LiteralPath $outDir -Force | Remove-Item -Recurse -Force
    $installer = Join-Path $outDir "Blah-Blah-Bot_${version}_x64-setup.exe"
    Copy-Item -LiteralPath $built[0].FullName -Destination $installer

    Write-Host ""
    Write-Host "=== verify the installer contents" -ForegroundColor Cyan
    $problems = Test-Installer $installer
    if ($problems.Count -gt 0) {
        $problems | ForEach-Object { Write-Host "  PROBLEM: $_" -ForegroundColor Red }
        throw "the installer did not pass the content check ($($problems.Count) problems)"
    }
    Write-Host "content check passed"

    Write-Host ""
    Write-Host "=== signature check" -ForegroundColor Cyan
    $sig = Get-AuthenticodeSignature -LiteralPath $installer
    Write-Host "signature status: $($sig.Status)"
    if ($sig.SignerCertificate) {
        Write-Host "signer: $($sig.SignerCertificate.Subject) ($($sig.SignerCertificate.Thumbprint))"
    }
    if ($env:BBB_SIGN_COMMAND -and $sig.Status -ne "Valid") {
        throw "BBB_SIGN_COMMAND was set, but the installer signature status is $($sig.Status): $($sig.StatusMessage)"
    }

    Write-Host ""
    Write-Host "=== checksums" -ForegroundColor Cyan
    $sums = foreach ($f in Get-ChildItem -LiteralPath $outDir -Filter "*-setup.exe") {
        "{0}  {1}" -f (Get-FileHash -LiteralPath $f.FullName -Algorithm SHA256).Hash.ToLower(), $f.Name
    }
    # LF line endings and no BOM: `sha256sum -c` reads the file as it is.
    [IO.File]::WriteAllText((Join-Path $outDir "SHA256SUMS.txt"), (($sums -join "`n") + "`n"), (New-Object Text.UTF8Encoding($false)))
    $sums | ForEach-Object { Write-Host $_ }

    if ($Scan) {
        $mpCmd = (Get-Command MpCmdRun.exe -ErrorAction SilentlyContinue).Source
        if (-not $mpCmd) { $mpCmd = Join-Path $env:ProgramFiles "Windows Defender\MpCmdRun.exe" }
        if (-not (Test-Path -LiteralPath $mpCmd)) { throw "MpCmdRun.exe was not found for -Scan" }
        Invoke-Native "Windows Defender scan" {
            & $mpCmd -Scan -ScanType 3 -File $installer -DisableRemediation
        }
    }

    Write-Host ""
    Write-Host ("installer: {0} ({1:N1} MB)" -f $installer, ((Get-Item -LiteralPath $installer).Length / 1MB))
} catch {
    $failed = $true
    Write-Host "FAILED: $_" -ForegroundColor Red
} finally {
    $env:BBB_INSTALLER_BUILD = $savedInstallerBuild
    $env:RUSTFLAGS = $savedRustflags
    $env:CARGO_ENCODED_RUSTFLAGS = $null
    $env:TEMP = $savedTemp
    $env:TMP = $savedTmp
    foreach ($dir in $verifyDir, $downloadDir) {
        if (Test-Path -LiteralPath $dir) { Remove-Item -LiteralPath $dir -Recurse -Force -ErrorAction SilentlyContinue }
    }
    Write-Host ""
    Write-Host "=== cleaning scratch files" -ForegroundColor Cyan
    & (Join-Path $PSScriptRoot "clean-test-tmp.ps1") -Quiet
    & (Join-Path $PSScriptRoot "check-temp-hygiene.ps1")
    if ($LASTEXITCODE -ne 0) { $failed = $true }
}
if ($failed) { exit 1 }
exit 0
