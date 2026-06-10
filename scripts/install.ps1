# Miru installer for Windows
# Usage: irm https://raw.githubusercontent.com/shizukutanaka/miru/main/scripts/install.ps1 | iex

param(
    [string]$Version = "latest",
    [string]$InstallDir = "$env:ProgramFiles\Miru"
)

$ErrorActionPreference = "Stop"
$Repo = "shizukutanaka/miru"

# ─── Arch detection ───────────────────────────────────────────────────────────
$Arch = (Get-WmiObject -Class Win32_Processor).Architecture
$Target = switch ($Arch) {
    9  { "x86_64-pc-windows-msvc" }     # AMD64
    12 { "aarch64-pc-windows-msvc" }    # ARM64
    default { throw "Unsupported arch: $Arch" }
}

# ─── Resolve version ──────────────────────────────────────────────────────────
if ($Version -eq "latest") {
    $Release = Invoke-RestMethod "https://api.github.com/repos/$Repo/releases/latest"
    $Version = $Release.tag_name
}

Write-Host "Installing Miru $Version for $Target" -ForegroundColor Cyan

# ─── Download ─────────────────────────────────────────────────────────────────
$TmpDir = New-TemporaryFile | ForEach-Object { Remove-Item $_; New-Item -ItemType Directory -Path $_ }
$ZipPath = Join-Path $TmpDir "miru.zip"
$Url = "https://github.com/$Repo/releases/download/$Version/miru-$Target.zip"

Write-Host "Downloading: $Url"
Invoke-WebRequest -Uri $Url -OutFile $ZipPath -UseBasicParsing

Expand-Archive -Path $ZipPath -DestinationPath $TmpDir -Force

# ─── Install ──────────────────────────────────────────────────────────────────
if (-not (Test-Path $InstallDir)) {
    New-Item -ItemType Directory -Path $InstallDir | Out-Null
}

Copy-Item "$TmpDir\miru-host.exe"   "$InstallDir\miru-host.exe"   -Force
Copy-Item "$TmpDir\miru-signal.exe" "$InstallDir\miru-signal.exe" -Force

# Add to PATH if not present
$CurrentPath = [Environment]::GetEnvironmentVariable("Path", "Machine")
if (-not $CurrentPath.Contains($InstallDir)) {
    [Environment]::SetEnvironmentVariable("Path", "$CurrentPath;$InstallDir", "Machine")
    Write-Host "Added $InstallDir to system PATH"
}

# ─── Windows service ──────────────────────────────────────────────────────────
$SvcName = "MiruHost"
$ExePath = "$InstallDir\miru-host.exe"

if (Get-Service -Name $SvcName -ErrorAction SilentlyContinue) {
    Stop-Service -Name $SvcName -Force
    sc.exe delete $SvcName | Out-Null
}

New-Service -Name $SvcName `
    -BinaryPathName $ExePath `
    -DisplayName "Miru Remote Desktop Host" `
    -StartupType Automatic `
    -Description "Miru peer-to-peer remote desktop host daemon"

Write-Host ""
Write-Host "Miru $Version installed to $InstallDir" -ForegroundColor Green
Write-Host ""
Write-Host "Start the host service:"
Write-Host "  Start-Service MiruHost"
Write-Host ""
Write-Host "Or run manually:"
Write-Host "  miru-host"

# Cleanup
Remove-Item $TmpDir -Recurse -Force
