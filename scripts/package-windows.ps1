# Builds the WinUI 3 frontend and stages a portable folder, a .zip, and an Inno Setup installer .exe in target\packages.
#
# The package is self-contained: FreeRDP, the Windows App SDK runtime and the Visual C++
# runtime ship next to WindowsApp.exe, so users install nothing (WebView2, used for
# sign-in, is part of Windows 11 and current Windows 10).
#
# Requirements: Rust (MSVC toolchain) with the Visual Studio C++ build tools, and git.
# Optional: Inno Setup 6 (winget install JRSoftware.InnoSetup or choco install innosetup)
# to build the double-clickable setup installer (windows-app-windows-x64-setup.exe).
#
# Usage: .\scripts\package-windows.ps1

$ErrorActionPreference = "Stop"
$Root = Resolve-Path (Join-Path $PSScriptRoot "..")
Set-Location $Root

# Read version from Cargo.toml
$CargoToml = Get-Content (Join-Path $Root "Cargo.toml") -Raw
$Version = "0.1.0"
if ($CargoToml -match '(?m)^version\s*=\s*"([^"]+)"') {
    $Version = $Matches[1]
}

Write-Host "==> Building release binary (WinUI frontend v$Version)..."
cargo build --release --no-default-features --features winui
if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }

$Release = Join-Path $Root "target\release"
$Stage = Join-Path $Root "target\windows-package\Windows App"
$Output = Join-Path $Root "target\packages"
Remove-Item -Recurse -Force $Stage -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force $Stage, $Output | Out-Null

$ExeSrc = if (Test-Path (Join-Path $Release "windows-app.exe")) { Join-Path $Release "windows-app.exe" } else { Join-Path $Release "windows-app-linux.exe" }
Copy-Item $ExeSrc (Join-Path $Stage "WindowsApp.exe")
Copy-Item "packaging\windows\AppIcon.ico" $Stage

# Copy runtime binaries and resources, explicitly omitting cargo internal build folders
Get-ChildItem $Release -File | Where-Object { $_.Extension -in ".dll", ".pri" } |
    Copy-Item -Destination $Stage

# Windows App SDK, OpenSSL modules, and resource subdirectories
Get-ChildItem $Release -Directory |
    Where-Object { $_.Name -notin "build", "deps", "examples", "incremental", ".fingerprint" } |
    Copy-Item -Destination $Stage -Recurse

# Visual C++ runtime (app-local deployment), for PCs without the redistributable.
$VsWhere = Join-Path ${env:ProgramFiles(x86)} "Microsoft Visual Studio\Installer\vswhere.exe"
if (Test-Path $VsWhere) {
    $Vs = & $VsWhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
    $Crt = Get-ChildItem (Join-Path $Vs "VC\Redist\MSVC\*\x64\Microsoft.VC14*.CRT") -Directory -ErrorAction SilentlyContinue |
        Sort-Object FullName | Select-Object -Last 1
    if ($Crt) {
        Copy-Item (Join-Path $Crt.FullName "*.dll") $Stage
    } else {
        Write-Warning "Visual C++ runtime DLLs not found; users may need the VC++ redistributable."
    }
}

# Validate critical runtime DLLs
$RequiredPatterns = @("Microsoft.WindowsAppRuntime.dll", "freerdp3*.dll", "freerdp-client3*.dll", "winpr3*.dll")
foreach ($Required in $RequiredPatterns) {
    if (-not (Get-ChildItem -Path $Stage -Filter $Required -ErrorAction SilentlyContinue)) {
        throw "$Required is missing from the package; it would not run on a clean PC."
    }
}

# 1. Create portable .zip package
$Zip = Join-Path $Output "windows-app-windows-x64.zip"
Remove-Item -Force $Zip -ErrorAction SilentlyContinue
Compress-Archive -Path $Stage -DestinationPath $Zip
Write-Host "==> Created portable zip: $Zip"

# 2. Build Inno Setup double-clickable installer (.exe) if Inno Setup is available
$Iscc = Get-Command iscc.exe -ErrorAction SilentlyContinue | Select-Object -ExpandProperty Source
if (-not $Iscc) {
    $Candidates = @(
        (Join-Path ${env:ProgramFiles(x86)} "Inno Setup 6\ISCC.exe"),
        (Join-Path $env:ProgramFiles "Inno Setup 6\ISCC.exe"),
        (Join-Path ${env:LOCALAPPDATA} "Programs\Inno Setup 6\ISCC.exe")
    )
    foreach ($Candidate in $Candidates) {
        if (Test-Path $Candidate) {
            $Iscc = $Candidate
            break
        }
    }
}

if ($Iscc) {
    Write-Host "==> Building Windows Installer (.exe) with Inno Setup..."
    $IssFile = Join-Path $Root "packaging\windows\installer.iss"
    & $Iscc "/O$Output" "/DAppVersion=$Version" $IssFile
    if ($LASTEXITCODE -ne 0) {
        Write-Warning "Inno Setup failed to build installer."
    } else {
        $SetupExe = Join-Path $Output "windows-app-windows-x64-setup.exe"
        Write-Host "==> Created double-clickable installer: $SetupExe"
    }
} else {
    Write-Host "Note: Inno Setup (ISCC.exe) not found. To build the double-clickable setup installer (.exe),"
    Write-Host "install Inno Setup with: winget install JRSoftware.InnoSetup or choco install innosetup"
}
