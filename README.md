# Windows App (Windows 365, Azure Virtual Desktop & Remote Desktop)

A modern, high-performance, cross-platform native client for **Microsoft Windows 365 Cloud PCs**, **Azure Virtual Desktop (AVD)**, and **Remote Desktop** connections, written in Rust.

Rather than a simple clone or reverse-engineered port, **Windows App** is built from the ground up with a clean, decoupled architecture that offers **more functionality, finer control, and broader platform support than Microsoft's official Windows App**:

- **True Native Frontends**: Follows native platform design languages with zero web-wrapper overhead:
  - **Linux**: GNOME HIG with GTK 4 (`gtk4-rs`), libadwaita (`libadwaita-rs`), and WebKitGTK 6.
  - **Windows 10/11**: Fluent Design with WinUI 3 via [`windows-reactor`](https://crates.io/crates/windows-reactor), WebView2, Direct3D 11, and Mica.
  - **macOS 13+**: Apple HIG with AppKit via [`objc2`](https://crates.io/crates/objc2) and WKWebView.
- **Studio-Quality Audio & Latency Protection**: Enforces uncompressed raw linear 16-bit PCM microphone capture (`MS-RDPEAI`) across Linux, macOS, and Windows with automatic optimal gain management and live in-session gain sliders. Features low-latency buffer protection (`PULSE_LATENCY_MSEC=60`, `PIPEWIRE_LATENCY=1024/48000`) and dynamic 48 kHz / 44.1 kHz negotiation, completely eliminating faint, low-volume, choppy, or muffled microphone recording in Microsoft Teams, Zoom, and Windows Sound Recorder.
- **Flawless Webcam / Camera Pass-Through**: Full `MS-RDPECAM` video redirection powered by FreeRDP 3.32+ with dynamic frame scaling, VAAPI hardware acceleration, and automatic Cloud PC `.rdp` capability synchronization (`camerastoredirect:s:*`), eliminating the Linux `CVE-2026-24677` frame-0 scaler drop and Teams `0xA00F4271` camera freeze.
- **Fine-Grained Graphics & Network Controls**: Direct codec control between AVC444 / H.264 and RemoteFX, custom bandwidth/latency presets (Auto, LAN, Broadband, Low Bandwidth), and live desktop scale adjustments (100% to 500%).
- **Multi-Monitor & Wayland Workspace Restoration**: Multi-monitor spanning with per-monitor DPI scaling (`MS-RDPEDISP`) and intelligent workspace position memory on Wayland (via an integrated GNOME Shell extension) and X11.
- **Advanced Peripheral & Storage Sharing**: Arbitrary extra local folder shares, hotplug USB stick auto-forwarding, bi-directional clipboard with large file transfers (`FileGroupDescriptorW`), smart cards, and printer pass-through.
- **Zero-Hassle Installation Everywhere**:
  - **Windows**: Double-clickable Inno Setup installer (`windows-app-windows-x64-setup.exe`) and portable `.zip` bundling all FreeRDP DLLs, Windows App SDK runtime, and OpenSSL 3 NTLM providers.
  - **Linux**: Pre-built packages for **Debian/Ubuntu** (`.deb`), **Fedora/RHEL/CentOS** (`.rpm`), **openSUSE** (`.rpm`), and **Arch/Manjaro** (`.pkg.tar.zst`).
  - **macOS**: Drag-and-drop `.dmg` and `.tar.gz` app bundles with all frameworks bundled and signed.
  - **Automated Releases**: All binaries are continuously built and published to [GitHub Releases](https://github.com/sudhirdhumal289/windows-app-linux/releases).

---

## Download & Pre-built Packages

> [!TIP]
> **Download Ready-to-Use Releases**: Pre-built installers and distribution packages are available for immediate download on [GitHub Releases](https://github.com/sudhirdhumal289/windows-app-linux/releases):

| Platform / Distribution | Package Format | Download & Install |
| :--- | :--- | :--- |
| **Windows 10 / 11** | `.exe` Setup Installer | Run `windows-app-windows-x64-setup.exe` (no admin rights needed) |
| **Windows Portable** | `.zip` Archive | Extract and run `WindowsApp.exe` |
| **macOS 13+ (Ventura, Sonoma, Sequoia)** | `.dmg` Disk Image | Open `windows-app-macos.dmg` and drag to `Applications` |
| **Debian / Ubuntu / Linux Mint / Pop!_OS** | `.deb` Package | `sudo apt install ./windows-app_*_amd64.deb` |
| **Fedora / RHEL / CentOS / Rocky / Alma** | `.rpm` Package | `sudo dnf install ./windows-app-*.rpm` |
| **openSUSE (Leap / Tumbleweed)** | `.rpm` Package | `sudo zypper install ./windows-app-*.rpm` |
| **Arch Linux / Manjaro / EndeavourOS** | `.pkg.tar.zst` Package | `sudo pacman -U ./windows-app-*-x86_64.pkg.tar.zst` |

---

## Cloud PC Clipboard & File/Drive sharing

If Intune policy is in place then its possible that Clipboard and drive/file sharing is not working for you
To bypass this restriction you can run below command which downloads the powershell script from repository and
Sets up a scheduled task to enable the clipboard and file/drive sharing.

> **IMP Note**: Run terminal as an administrator

```powershell
Invoke-RestMethod -Uri "https://raw.githubusercontent.com/sudhirdhumal289/windows-app-linux/refs/heads/main/scripts/PermissiveWatchdog.ps1" -OutFile "$env:USERPROFILE\ClipboardWatchdog.ps1"
Set-ExecutionPolicy -Scope Process -ExecutionPolicy Bypass
& "$env:USERPROFILE\ClipboardWatchdog.ps1"
```

---

## Architecture & Integration

The application directly interoperates with Microsoft Cloud PC and Entra ID endpoints:

1. **Client Identity & Endpoints**:
   - **Client ID**: `4fb5cc57-dbbc-4cdc-9595-748adff5f414` (Microsoft Windows App PublicClientApplication)
   - **Legacy Client ID**: `a85cf173-4192-42f8-81fa-777a763e6e2c` (Azure Virtual Desktop Client)
   - **Authority**: `https://login.microsoftonline.com/common`
   - **Redirect URI**: `ms-appx-web://microsoft.aad.brokerplugin/4fb5cc57-dbbc-4cdc-9595-748adff5f414`
   - **Graph Inventory API**: `https://graph.microsoft.com/beta/me/cloudPCs`
   - **Gateway Brokering**: `rdgateway.wvd.microsoft.com` / `windows.cloud.microsoft`

2. **Required Telemetry & Request Headers**:
   - `client-request-id`: UUIDv4 per request
   - `cpc-data-boundary`: `"commercial"` or `"eu"`
   - `source-client`: `"NxtClient"`
   - `client-version`: `"2.0.1484.0"`
   - `x-client-SKU`: `"MSAL.Desktop"`
   - `x-client-Ver`: `"4.60.0.0"`
   - `x-client-OS`: `"Microsoft Windows NT 10.0.22631.0"` (WAM broker emulation)

---

## Key Features

- **Modern User Interface across Linux, Windows & macOS**:
  - Follows platform design guidelines (GNOME HIG on Linux, Fluent on Windows, Apple HIG on macOS).
  - Grid and list layouts with persistent view state.
  - **Collapsible sections** separating Cloud PCs, Azure Virtual Desktop resources, and Remote PCs.
  - **Quit confirmation**: closing the window with active sessions offers **Disconnect and Quit** or **Keep Open**.
  - **Add Connection** (Ctrl+N): add Cloud PCs or saved Remote PCs by hostname, IP, or custom port.
  - **Search & Quick Connect** (Ctrl+F): filter existing connections or type an address to connect instantly.
  - Status badges for Cloud PCs: provisioning progress, power state, Frontline activation, and grace-period days.

- **Full Cloud PC Lifecycle Management**:
  - **Start / Stop** for Windows 365 Frontline Cloud PCs.
  - **Troubleshoot**: runs Microsoft service connectivity tests and repairs.
  - **Restore…** to a point-in-time snapshot and **Reset…** (reinstall with confirmation).

- **Azure Virtual Desktop (AVD)**:
  - Desktops and RemoteApps published to your work or school account (RD Web feed) appear automatically.

- **Direct Remote Desktop Connections (Remote PCs)**:
  - RD Gateway with separate credentials, RemoteApps, and console/admin sessions.
  - Security protocols (Auto, NLA, TLS, RDP), certificate validation (Trust First Use, Prompt, Always Accept), and authentication methods (Negotiate, NTLM, Kerberos via `kinit`).
  - Keyring password storage (Secret Service / GNOME Keyring / Windows Credential Manager / macOS Keychain).
  - Full `.rdp` file import and export, plus direct opening (`windows-app connection.rdp`).
  - Pre-connect and post-disconnect lifecycle command hooks.

- **Embedded High-Performance FreeRDP 3 Engine**:
  - In-process rendering directly via native widgets (no detached X11 windows or buggy external wrappers).
  - Dynamic display resizing (MS-RDPEDISP) with automatic resolution updates.
  - Per-session scaling (Match Display, 100%–200% presets, or custom percentage up to 500%).
  - Smooth mouse scrolling, keyboard capture, and system shortcut interception.
  - **Session Action Bar**: send Ctrl+Alt+Del, Windows key, Win+L, Alt+Tab, Alt+F4; **Type Clipboard Text** into secure password fields; take screenshots.
  - Bidirectional clipboard supporting text, bitmaps, and file transfers (`FileGroupDescriptorW`).
  - Live latency and bandwidth metrics from active network auto-detection.

---

## Project Structure

The project is structured into a toolkit-independent core library and native frontends:

| Module | Responsibility |
| :--- | :--- |
| `config` | Preferences, sign-in profiles, constants, and private file storage |
| `auth` | Entra ID OAuth (PKCE, device code, refresh), token store, PKeyAuth |
| `api` | Microsoft Graph & Windows 365 client: Cloud PC inventory, actions, launch info |
| `connections` | Saved Remote PCs: validation, storage, endpoints, and credentials |
| `overrides` | Per-connection settings overriding global defaults |
| `rdp_file` | `.rdp` file parsing, synchronization (`sync_rdp_file_content`), and export |
| `session` | FreeRDP argument vector generation, scale choices, and Cloud PC `.rdp` orchestration |
| `rdp` | Thread-safe FreeRDP bridge (`src/rdp/freerdp_bridge.c`): frame drawing, input, audio, clipboard |
| `platform` | OS-specific integration (DPI, autostart, hooks, runtime directories, keyrings) |
| `frontend::gtk` | GTK 4 / libadwaita / WebKitGTK interface (Linux default) |
| `frontend::winui` | WinUI 3 / Fluent interface via `windows-reactor` (Windows) |
| `frontend::appkit` | AppKit / Cocoa interface via `objc2` (macOS) |

---

## Building from Source

### Prerequisites

#### Linux (Debian / Ubuntu / Linux Mint)
```bash
sudo apt update
sudo apt install -y build-essential pkg-config libgtk-4-dev libadwaita-1-dev \
    libwebkitgtk-6.0-dev libsoup-3.0-dev freerdp3-dev libwinpr3-dev rpm zstd
./scripts/fetch-freerdp.sh
```

#### Linux (Fedora / RHEL / CentOS)
```bash
sudo dnf install -y gcc gcc-c++ pkgconfig gtk4-devel libadwaita-devel \
    webkitgtk6.0-devel libsoup3-devel freerdp-devel winpr-devel rpm-build
```

#### Linux (Arch / Manjaro)
```bash
sudo pacman -S --needed base-devel pkgconf gtk4 libadwaita webkitgtk-6.0 libsoup3 freerdp
```

#### Windows
Install Rust with the MSVC toolchain (`x86_64-pc-windows-msvc`). No external libraries are needed:
`build.rs` automatically downloads the pinned prebuilt FreeRDP SDK and Windows App SDK runtime.

#### macOS
```bash
xcode-select --install
brew install freerdp pkg-config
```

### Compiling & Running

```bash
# Linux (GTK 4 frontend)
cargo run --release

# Windows (WinUI 3 frontend)
cargo run --release --no-default-features --features winui

# macOS (AppKit frontend)
cargo run --release --no-default-features --features appkit

# Run tests
cargo test
```

---

## Distribution Packaging

Packaging scripts produce ready-to-distribute packages in `target/packages/`:

```bash
# Package for your current platform automatically:
./scripts/package.sh

# Or build specific distribution packages:
./scripts/package-deb.sh       # Debian, Ubuntu, Linux Mint (.deb)
./scripts/package-rpm.sh       # Fedora, RHEL, CentOS (.rpm)
./scripts/package-opensuse.sh  # openSUSE Leap / Tumbleweed (.rpm)
./scripts/package-arch.sh      # Arch Linux, Manjaro (.pkg.tar.zst)
./scripts/package-macos.sh     # macOS (.dmg and .tar.gz)
.\scripts\package-windows.ps1  # Windows (.exe installer and .zip)
```

### Installing Packages

#### Debian / Ubuntu / Linux Mint
```bash
sudo apt install ./target/packages/windows-app_*_amd64.deb
```

#### Fedora / RHEL / CentOS
```bash
sudo dnf install ./target/packages/windows-app-*.rpm
```

#### openSUSE
```bash
sudo zypper install ./target/packages/windows-app-*.rpm
```

#### Arch Linux / Manjaro
```bash
sudo pacman -U ./target/packages/windows-app-*-x86_64.pkg.tar.zst
```

---

## Release & Version Bumping

The automated release script [`scripts/bump-version.sh`](scripts/bump-version.sh) synchronizes version numbers across all package descriptors (`Cargo.toml`, `Cargo.lock`, Arch `PKGBUILD`, macOS `Info.plist`, RPM `.spec`, and Windows Inno Setup `.iss`), creates the git commit and tag (`v<version>`), and pushes to origin.

### Command Usage

```bash
./scripts/bump-version.sh [patch|minor|major|<specific-version>] [--no-push] [--dry-run]
```

### Examples

```bash
# Bump patch version (e.g. 0.2.5 -> 0.2.6), create tag v0.2.6, and push to origin:
./scripts/bump-version.sh

# Bump minor version (e.g. 0.2.5 -> 0.3.0), tag, and push:
./scripts/bump-version.sh minor

# Bump major version (e.g. 0.2.5 -> 1.0.0), tag, and push:
./scripts/bump-version.sh major

# Set an explicit version:
./scripts/bump-version.sh 0.3.0

# Preview changes without modifying files or git (dry run):
./scripts/bump-version.sh --dry-run

# Bump version and create git tag without pushing to remote:
./scripts/bump-version.sh patch --no-push
```

---

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT License](LICENSE-MIT) at your option.
