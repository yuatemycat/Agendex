<#
.SYNOPSIS
    Authoritative LDPlayer 9/4 CLI Management Utility for PowerShell.
.DESCRIPTION
    Wraps ldconsole.exe (LDPlayer 9) and dnconsole.exe (LDPlayer 3/4/5) CLI commands
    with automatic path discovery, structured output, and error handling.
.PARAMETER Action
    The action to perform: Launch, Reboot, Quit, QuitAll, Status, List, RunApp, KillApp, Modify.
.PARAMETER Index
    Zero-based VM index (e.g. 0 for the first instance). Default: 0.
.PARAMETER Name
    Instance display title/name (alternative to Index).
.PARAMETER PackageName
    Target Android package name for Launch (launchex), RunApp, or KillApp.
.PARAMETER LdPath
    Explicit path to ldconsole.exe or dnconsole.exe if installed in non-standard path.
.EXAMPLE
    .\ldplayer_control.ps1 -Action List
    .\ldplayer_control.ps1 -Action Launch -Index 0 -PackageName com.supercell.hayday
    .\ldplayer_control.ps1 -Action Status -Index 0
    .\ldplayer_control.ps1 -Action Reboot -Index 0
    .\ldplayer_control.ps1 -Action Quit -Index 0
#>

[CmdletBinding()]
param (
    [Parameter(Position = 0)]
    [ValidateSet("Launch", "Reboot", "Quit", "QuitAll", "Status", "List", "RunApp", "KillApp", "Modify")]
    [string]$Action = "Status",

    [Parameter(Position = 1)]
    [int]$Index = 0,

    [Parameter()]
    [string]$Name,

    [Parameter()]
    [string]$PackageName,

    [Parameter()]
    [string]$Resolution,

    [Parameter()]
    [int]$Cpu,

    [Parameter()]
    [int]$Memory,

    [Parameter()]
    [string]$LdPath
)

# 1. Automatic Discovery of LDPlayer CLI binary location
function Get-LdConsolePath {
    param ([string]$ExplicitPath)

    if ($ExplicitPath -and (Test-Path $ExplicitPath)) {
        return (Resolve-Path $ExplicitPath).Path
    }

    $candidates = @(
        "E:\LDPlayer\LDPlayer9\ldconsole.exe",
        "C:\LDPlayer\LDPlayer9\ldconsole.exe",
        "D:\LDPlayer\LDPlayer9\ldconsole.exe",
        "E:\LDPlayer\LDPlayer4.0\dnconsole.exe",
        "C:\LDPlayer\LDPlayer4.0\dnconsole.exe",
        "D:\LDPlayer\LDPlayer4.0\dnconsole.exe",
        "C:\ChangPeng\LDPlayer9\ldconsole.exe",
        "D:\ChangPeng\LDPlayer9\ldconsole.exe",
        "E:\ChangPeng\LDPlayer9\ldconsole.exe"
    )

    foreach ($path in $candidates) {
        if (Test-Path $path) {
            return $path
        }
    }

    throw "LDPlayer console executable (ldconsole.exe / dnconsole.exe) not found. Specify via -LdPath."
}

$consoleExe = Get-LdConsolePath -ExplicitPath $LdPath

# 2. Build target identifier arguments (--name or --index)
$targetArgs = if ($PSBoundParameters.ContainsKey('Name') -and -not [string]::IsNullOrWhiteSpace($Name)) {
    @("--name", $Name)
} else {
    @("--index", $Index.ToString())
}

# 3. Action Dispatcher
switch ($Action) {
    "Launch" {
        if ($PackageName) {
            Write-Host "[*] Launching LDPlayer ($targetArgs) directly into $PackageName..." -ForegroundColor Cyan
            & $consoleExe launchex @targetArgs --packagename $PackageName
        } else {
            Write-Host "[*] Launching LDPlayer ($targetArgs)..." -ForegroundColor Cyan
            & $consoleExe launch @targetArgs
        }
    }
    "Reboot" {
        Write-Host "[*] Rebooting LDPlayer ($targetArgs)..." -ForegroundColor Yellow
        & $consoleExe reboot @targetArgs
    }
    "Quit" {
        Write-Host "[*] Quitting LDPlayer ($targetArgs)..." -ForegroundColor Magenta
        & $consoleExe quit @targetArgs
    }
    "QuitAll" {
        Write-Host "[*] Quitting all running LDPlayer instances..." -ForegroundColor Magenta
        & $consoleExe quitall
    }
    "Status" {
        $rawStatus = (& $consoleExe isrunning @targetArgs).Trim()
        $isRunning = ($rawStatus -eq "running")
        $color = if ($isRunning) { "Green" } else { "DarkGray" }
        Write-Host "[*] Instance Status ($targetArgs): $rawStatus" -ForegroundColor $color
        [PSCustomObject]@{
            Target    = ($targetArgs -join " ")
            Status    = $rawStatus
            IsRunning = $isRunning
        }
    }
    "List" {
        Write-Host "[*] Querying registered LDPlayer instances..." -ForegroundColor Cyan
        $rawLines = & $consoleExe list2
        $instances = @()
        foreach ($line in $rawLines) {
            $parts = $line.Split(',')
            if ($parts.Count -ge 7) {
                $idx = [int]$parts[0]
                $instName = $parts[1]
                $topHwnd = $parts[2]
                $bindHwnd = $parts[3]
                $androidPid = [int]$parts[4]
                $vboxPid = [int]$parts[5]
                $width = if ($parts.Count -ge 8) { [int]$parts[7] } else { 0 }
                $height = if ($parts.Count -ge 9) { [int]$parts[8] } else { 0 }
                $dpi = if ($parts.Count -ge 10) { [int]$parts[9] } else { 0 }
                
                $status = if ($vboxPid -gt 0 -or $androidPid -gt 0) { "running" } else { "stop" }

                $instances += [PSCustomObject]@{
                    Index      = $idx
                    Name       = $instName
                    Status     = $status
                    VBoxPID    = $vboxPid
                    AndroidPID = $androidPid
                    Resolution = if ($width -gt 0) { "${width}x${height}@${dpi}dpi" } else { "N/A" }
                }
            }
        }
        $instances | Format-Table -AutoSize
        return $instances
    }
    "RunApp" {
        if (-not $PackageName) {
            throw "Action RunApp requires -PackageName <package>"
        }
        Write-Host "[*] Starting app $PackageName on ($targetArgs)..." -ForegroundColor Cyan
        & $consoleExe runapp @targetArgs --packagename $PackageName
    }
    "KillApp" {
        if (-not $PackageName) {
            throw "Action KillApp requires -PackageName <package>"
        }
        Write-Host "[*] Force-stopping app $PackageName on ($targetArgs)..." -ForegroundColor Yellow
        & $consoleExe killapp @targetArgs --packagename $PackageName
    }
    "Modify" {
        $modArgs = @()
        if ($Resolution) { $modArgs += @("--resolution", $Resolution) }
        if ($Cpu -gt 0)   { $modArgs += @("--cpu", $Cpu.ToString()) }
        if ($Memory -gt 0){ $modArgs += @("--memory", $Memory.ToString()) }
        if ($modArgs.Count -eq 0) {
            throw "Action Modify requires at least one setting (-Resolution, -Cpu, -Memory)"
        }
        Write-Host "[*] Modifying VM configuration ($targetArgs) with $modArgs..." -ForegroundColor Yellow
        & $consoleExe modify @targetArgs @modArgs
    }
}
