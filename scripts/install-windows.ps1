# Install Concierge on Windows: build release, install to %LOCALAPPDATA%\Concierge,
# add a launcher + Start Menu shortcut. No Nix. Re-runnable.
#   Run from the repo root:  powershell -ExecutionPolicy Bypass -File scripts\install-windows.ps1
$ErrorActionPreference = "Stop"
$Repo = (Resolve-Path "$PSScriptRoot\..").Path
$Dest = Join-Path $env:LOCALAPPDATA "Concierge"

Write-Host "==> building release (gui + cli + daemon)"
cargo build --release -p concierge-gui -p concierge -p concierge-daemon

Write-Host "==> installing to $Dest"
New-Item -ItemType Directory -Force -Path $Dest | Out-Null
# concierge.exe IS the GUI (double-click, Vortex-style); concierge-cli.exe is
# the agent CLI; the daemon rides along so the GUI can spawn it.
Copy-Item "$Repo\target\release\concierge.exe"        $Dest -Force
Copy-Item "$Repo\target\release\concierge-cli.exe"    $Dest -Force
Copy-Item "$Repo\target\release\concierge-daemon.exe" $Dest -Force

# A shortcut-launched app has no repo cwd, so a wrapper .cmd bakes CONCIERGE_REPO
# in. (Tools are found by concierge-platform::find_tool via PATH.)
$Wrapper = Join-Path $Dest "Concierge.cmd"
"@echo off`r`nset CONCIERGE_REPO=$Repo`r`nstart """" ""$Dest\concierge.exe"" %*" |
    Set-Content -Encoding ASCII $Wrapper

# Start Menu shortcut -> the wrapper
$Programs = [Environment]::GetFolderPath("Programs")
$Lnk = Join-Path $Programs "Concierge.lnk"
$WScript = New-Object -ComObject WScript.Shell
$Shortcut = $WScript.CreateShortcut($Lnk)
$Shortcut.TargetPath = $Wrapper
$Shortcut.WorkingDirectory = $Dest
$Shortcut.Save()

Write-Host "OK: Concierge installed to $Dest"
Write-Host "    launch from the Start Menu (Concierge), or run $Dest\Concierge.cmd"
Write-Host "    NOTE: put clickhouse.exe and 7zz.exe on PATH (or in $Dest) for the"
Write-Host "          mod catalog + archive extraction (concierge finds them via PATH)."
