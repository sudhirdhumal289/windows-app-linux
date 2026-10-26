# Builds the WinUI 3 frontend and stages a portable folder plus a .zip in target\packages.
#
# The folder is self-contained: FreeRDP, the Windows App SDK runtime and the Visual C++
# runtime ship next to WindowsApp.exe, so users install nothing (WebView2, used for
# sign-in, is part of Windows 11 and current Windows 10).
#
# Requirements: Rust (MSVC toolchain) with the Visual Studio C++ build tools, and git.
# The first build fetches vcpkg and compiles FreeRDP from vcpkg.json (see build.rs).
#
# Usage: .\scripts\package-windows.ps1

$ErrorActionPreference = "Stop"
$Root = Resolve-Path (Join-Path $PSScriptRoot "..")
Set-Location $Root

Write-Host "==> Building release binary (WinUI frontend)..."
cargo build --release --no-default-features --features winui
if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }

$Release = Join-Path $Root "target\release"
$Stage = Join-Path $Root "target\windows-package\Windows App"
$Output = Join-Path $Root "target\packages"
Remove-Item -Recurse -Force $Stage -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force $Stage, $Output | Out-Null

Copy-Item (Join-Path $Release "windows-app-linux.exe") (Join-Path $Stage "WindowsApp.exe")
Copy-Item "packaging\windows\AppIcon.ico" $Stage
# build.rs put the FreeRDP DLLs and the Windows App SDK runtime (DLLs, .pri resources,
# XAML and language folders) next to the executable; take all of it, minus cargo's dirs.
Get-ChildItem $Release -File | Where-Object { $_.Extension -in ".dll", ".pri" } |
    Copy-Item -Destination $Stage
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

foreach ($Required in "Microsoft.WindowsAppRuntime.dll", "freerdp3*.dll", "freerdp-client3*.dll", "winpr3*.dll") {
    if (-not (Test-Path (Join-Path $Stage $Required))) {
        throw "$Required is missing from the package; it would not run on a clean PC."
    }
}

$Zip = Join-Path $Output "windows-app-windows-x64.zip"
Remove-Item -Force $Zip -ErrorAction SilentlyContinue
Compress-Archive -Path $Stage -DestinationPath $Zip
Write-Host "==> Created $Zip"
