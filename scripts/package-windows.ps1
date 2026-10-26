# Builds the WinUI 3 frontend and stages a portable folder plus a .zip in target\packages.
#
# Requirements: Rust (MSVC toolchain), FreeRDP 3 from vcpkg
# (`vcpkg install "freerdp[client]:x64-windows" pkgconf:x64-windows`) with PKG_CONFIG set, and the Windows App SDK runtime installed on the machine that runs the
# app (https://learn.microsoft.com/windows/apps/windows-app-sdk/downloads).
#
# Usage: .\scripts\package-windows.ps1 [-VcpkgRoot C:\vcpkg]

param(
    [string]$VcpkgRoot = $env:VCPKG_ROOT,
    [string]$Triplet = "x64-windows"
)

$ErrorActionPreference = "Stop"
$Root = Resolve-Path (Join-Path $PSScriptRoot "..")
Set-Location $Root

if (-not $VcpkgRoot) {
    throw "Set VCPKG_ROOT or pass -VcpkgRoot (FreeRDP is taken from vcpkg)."
}
$Installed = Join-Path $VcpkgRoot "installed\$Triplet"
$env:PKG_CONFIG_PATH = Join-Path $Installed "lib\pkgconfig"

Write-Host "==> Building release binary (WinUI frontend)..."
cargo build --release --no-default-features --features winui
if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }

$Stage = Join-Path $Root "target\windows-package\Windows App"
$Output = Join-Path $Root "target\packages"
Remove-Item -Recurse -Force $Stage -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force $Stage, $Output | Out-Null

Copy-Item "target\release\windows-app-linux.exe" (Join-Path $Stage "WindowsApp.exe")
Copy-Item "packaging\windows\AppIcon.ico" $Stage
# Windows has no rpath: FreeRDP and its dependencies sit next to the executable.
Copy-Item (Join-Path $Installed "bin\*.dll") $Stage

$Zip = Join-Path $Output "windows-app-windows-x64.zip"
Remove-Item -Force $Zip -ErrorAction SilentlyContinue
Compress-Archive -Path $Stage -DestinationPath $Zip
Write-Host "==> Created $Zip"
