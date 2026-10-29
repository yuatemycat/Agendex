<#
.SYNOPSIS
    Builds, tests, formats, and packages Agendex release binaries for Windows.

.DESCRIPTION
    This script initializes the MSVC toolchain (if available), verifies formatting
    and unit tests, builds an optimized release binary, and packages the executable,
    README, and LICENSE into a versioned zip bundle with a SHA256 checksum in dist/.

.PARAMETER Version
    Explicit version tag (e.g. 0.1.0 or v0.1.0). If omitted, the version is resolved
    from git tags, crates/agendex-cli/Cargo.toml, or workspace Cargo.toml.

.PARAMETER OutputDir
    Destination directory for release bundles. Defaults to 'dist'.

.PARAMETER SkipTests
    Skip running cargo test prior to packaging.

.PARAMETER SkipFmt
    Skip running cargo fmt check prior to packaging.

.EXAMPLE
    .\scripts\build_release.ps1
    .\scripts\build_release.ps1 -SkipTests
    .\scripts\build_release.ps1 -Version 0.1.0 -OutputDir dist
#>

[CmdletBinding()]
param (
    [string]$Version,
    [string]$OutputDir = "dist",
    [switch]$SkipTests,
    [switch]$SkipFmt
)

$ErrorActionPreference = "Stop"

# Resolve repository root
$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$RepoRoot = (Resolve-Path "$ScriptDir\..").Path
Set-Location $RepoRoot

function Initialize-MsvcEnvironment {
    Write-Host "[Release] Checking MSVC build toolchain..." -ForegroundColor Cyan

    if ((Get-Command link.exe -ErrorAction SilentlyContinue) -and (Get-Command cl.exe -ErrorAction SilentlyContinue)) {
        Write-Host "[Release] MSVC toolchain is already active in current environment." -ForegroundColor Green
        return
    }

    # Attempt 1: Custom build environment if specified by DEVOPS_ROOT
    if ($env:DEVOPS_ROOT -and (Test-Path "$env:DEVOPS_ROOT\env.ps1")) {
        $devopsEnv = "$env:DEVOPS_ROOT\env.ps1"
        Write-Host "[Release] Loading build environment from $devopsEnv..." -ForegroundColor Cyan
        & $devopsEnv
        if ((Get-Command link.exe -ErrorAction SilentlyContinue) -and (Get-Command cl.exe -ErrorAction SilentlyContinue)) {
            Write-Host "[Release] MSVC toolchain loaded successfully via custom build environment." -ForegroundColor Green
            return
        }
    }

    # Attempt 2: Custom MSVC setup script if specified
    if ($env:DEVOPS_ROOT -and (Test-Path "$env:DEVOPS_ROOT\msvc\setup_x64.bat")) {
        $setupBat = "$env:DEVOPS_ROOT\msvc\setup_x64.bat"
        Write-Host "[Release] Activating MSVC from $setupBat..." -ForegroundColor Cyan
        $tempBat = [System.IO.Path]::GetTempFileName() + ".bat"
        @"
@echo off
call "$setupBat"
set
"@ | Set-Content -Path $tempBat
        cmd.exe /c $tempBat | ForEach-Object {
            if ($_ -match '^(.*?)=(.*)$') {
                $name = $matches[1]
                $val = $matches[2]
                if ($name -in @('PATH', 'INCLUDE', 'LIB', 'LIBPATH', 'VCToolsVersion', 'WindowsSDKVersion', 'VCToolsInstallDir', 'WindowsSdkBinPath')) {
                    Set-Item -Path "env:$name" -Value $val
                }
            }
        }
        Remove-Item $tempBat -Force -ErrorAction SilentlyContinue
        if (Get-Command link.exe -ErrorAction SilentlyContinue) {
            Write-Host "[Release] MSVC toolchain activated successfully." -ForegroundColor Green
            return
        }
    }

    # Attempt 3: Standard Visual Studio vswhere
    $vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
    if (Test-Path $vswhere) {
        $vsInstallPath = & $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
        if ($vsInstallPath -and (Test-Path "$vsInstallPath\VC\Auxiliary\Build\vcvars64.bat")) {
            $vcvars = "$vsInstallPath\VC\Auxiliary\Build\vcvars64.bat"
            Write-Host "[Release] Activating MSVC from $vcvars..." -ForegroundColor Cyan
            $tempBat = [System.IO.Path]::GetTempFileName() + ".bat"
            @"
@echo off
call "$vcvars"
set
"@ | Set-Content -Path $tempBat
            cmd.exe /c $tempBat | ForEach-Object {
                if ($_ -match '^(.*?)=(.*)$') {
                    $name = $matches[1]
                    $val = $matches[2]
                    if ($name -in @('PATH', 'INCLUDE', 'LIB', 'LIBPATH', 'VCToolsVersion', 'WindowsSDKVersion', 'VCToolsInstallDir', 'WindowsSdkBinPath')) {
                        Set-Item -Path "env:$name" -Value $val
                    }
                }
            }
            Remove-Item $tempBat -Force -ErrorAction SilentlyContinue
            if (Get-Command link.exe -ErrorAction SilentlyContinue) {
                Write-Host "[Release] MSVC toolchain activated successfully." -ForegroundColor Green
                return
            }
        }
    }

    Write-Warning "[Release] MSVC toolchain was not found. Build will continue with active cargo defaults."
}

function Resolve-VersionString {
    param ([string]$ExplicitVersion, [string]$Root)

    if ($ExplicitVersion) {
        return $ExplicitVersion.TrimStart('v')
    }

    # 1. Try git exact tag
    try {
        $gitTag = git -C $Root describe --tags --exact-match 2>$null
        if ($gitTag -and ($gitTag -match '^v?([0-9]+\.[0-9]+\.[0-9]+.*)$')) {
            return $matches[1]
        }
    } catch {
        # Fall back to cargo manifest
    }

    # 2. Try crates/agendex-cli/Cargo.toml
    $cliToml = Join-Path $Root "crates\agendex-cli\Cargo.toml"
    if (Test-Path $cliToml) {
        $content = Get-Content $cliToml -Raw
        if ($content -match '(?m)^version\s*=\s*"([^"]+)"') {
            return $matches[1]
        }
    }

    # 3. Try root Cargo.toml
    $rootToml = Join-Path $Root "Cargo.toml"
    if (Test-Path $rootToml) {
        $content = Get-Content $rootToml -Raw
        if ($content -match '(?m)^version\s*=\s*"([^"]+)"') {
            return $matches[1]
        }
    }

    return "0.1.0"
}

function Find-CompiledBinary {
    param ([string]$Root)

    $candidates = @(
        (Join-Path $Root "target\release\agendex.exe"),
        (Join-Path $Root "target\release\agendex-cli.exe"),
        (Join-Path $Root "target\x86_64-pc-windows-msvc\release\agendex.exe"),
        (Join-Path $Root "target\x86_64-pc-windows-msvc\release\agendex-cli.exe")
    )

    foreach ($cand in $candidates) {
        if (Test-Path $cand) {
            return $cand
        }
    }
    return $null
}

# --- 1. Toolchain Activation ---
Initialize-MsvcEnvironment

if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    Write-Error "[Release] cargo command not found in PATH. Please verify Rust installation."
    exit 1
}

# --- 2. Verification Gate ---
if (-not $SkipFmt) {
    Write-Host "[Release] Checking code formatting (cargo fmt --all -- --check)..." -ForegroundColor Cyan
    & cargo fmt --all -- --check
    if ($LASTEXITCODE -ne 0) {
        Write-Error "[Release] Code formatting check failed. Run 'cargo fmt --all' to fix formatting issues."
        exit $LASTEXITCODE
    }
    Write-Host "[Release] Code formatting clean." -ForegroundColor Green
} else {
    Write-Host "[Release] Code formatting check skipped." -ForegroundColor Yellow
}

if (-not $SkipTests) {
    Write-Host "[Release] Running workspace test suite (cargo test --workspace --all-targets)..." -ForegroundColor Cyan
    & cargo test --workspace --all-targets
    if ($LASTEXITCODE -ne 0) {
        Write-Error "[Release] Test suite failed. Please resolve test failures before packaging."
        exit $LASTEXITCODE
    }
    Write-Host "[Release] Test suite passed." -ForegroundColor Green
} else {
    Write-Host "[Release] Test suite skipped." -ForegroundColor Yellow
}

# --- 3. Build Release Binary ---
Write-Host "[Release] Building optimized release binary (cargo build --release -p agendex-cli)..." -ForegroundColor Cyan
& cargo build --release -p agendex-cli
if ($LASTEXITCODE -ne 0) {
    Write-Error "[Release] cargo build --release failed with exit code $LASTEXITCODE"
    exit $LASTEXITCODE
}

$binaryPath = Find-CompiledBinary -Root $RepoRoot
if (-not $binaryPath) {
    Write-Error "[Release] Could not locate compiled release binary in target/release/"
    exit 1
}

$binaryInfo = Get-Item $binaryPath
$binarySizeMB = [math]::Round($binaryInfo.Length / 1MB, 2)
Write-Host "[Release] Compiled binary located: $($binaryInfo.FullName) ($binarySizeMB MB)" -ForegroundColor Green

# --- 4. Package Artifacts into dist/ ---
$versionString = Resolve-VersionString -ExplicitVersion $Version -Root $RepoRoot
$bundleName = "agendex-v$versionString-x86_64-pc-windows-msvc.zip"

$distDir = Join-Path $RepoRoot $OutputDir
if (-not (Test-Path $distDir)) {
    New-Item -ItemType Directory -Force -Path $distDir | Out-Null
}

$stagingDir = Join-Path $distDir "staging_agendex"
if (Test-Path $stagingDir) {
    Remove-Item -Recurse -Force $stagingDir
}
New-Item -ItemType Directory -Force -Path $stagingDir | Out-Null

# Copy binary as agendex.exe
Copy-Item -Path $binaryPath -Destination (Join-Path $stagingDir "agendex.exe") -Force

# Copy documentation and license if available
$readmePath = Join-Path $RepoRoot "README.md"
if (Test-Path $readmePath) {
    Copy-Item -Path $readmePath -Destination (Join-Path $stagingDir "README.md") -Force
}

$licensePath = Join-Path $RepoRoot "LICENSE"
if (Test-Path $licensePath) {
    Copy-Item -Path $licensePath -Destination (Join-Path $stagingDir "LICENSE") -Force
}

# Create zip archive
$bundlePath = Join-Path $distDir $bundleName
if (Test-Path $bundlePath) {
    Remove-Item -Force $bundlePath
}

Write-Host "[Release] Packaging artifacts into $bundlePath..." -ForegroundColor Cyan
Compress-Archive -Path "$stagingDir\*" -DestinationPath $bundlePath -Force

# Generate SHA256 checksum file
$hash = (Get-FileHash -Path $bundlePath -Algorithm SHA256).Hash.ToLower()
$checksumPath = "$bundlePath.sha256"
"$hash  $bundleName" | Set-Content -Path $checksumPath -Encoding ascii

# Clean up staging folder
Remove-Item -Recurse -Force $stagingDir

$bundleInfo = Get-Item $bundlePath
$bundleSizeMB = [math]::Round($bundleInfo.Length / 1MB, 2)

Write-Host "=================================================================" -ForegroundColor Cyan
Write-Host "Agendex Release Package Ready" -ForegroundColor Green
Write-Host "Version:    v$versionString"
Write-Host "Target:     x86_64-pc-windows-msvc"
Write-Host "Bundle:     $bundlePath ($bundleSizeMB MB)"
Write-Host "Checksum:   $checksumPath"
Write-Host "SHA256:     $hash"
Write-Host "=================================================================" -ForegroundColor Cyan
