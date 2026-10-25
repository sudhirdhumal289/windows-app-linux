# Windows App (Windows 365 & AVD) for Linux

A native, high-performance Linux client for Microsoft Windows 365 Cloud PCs and Azure Virtual Desktop (AVD), re-engineered from the official Windows App MSIX package (`WindowsApp_x64_Release_2.0.1484.0.msix`) and built in Rust using GTK 4 (`gtk4-rs`) and WebKitGTK (`webkit6`).

---

## Architecture & Reverse Engineering

The application directly interoperates with Microsoft Cloud PC and Entra ID endpoints using reverse-engineered signatures and telemetry headers:

1. **Client Identity & Endpoints**:
   - **Client ID**: `4fb5cc57-dbbc-4cdc-9595-748adff5f414` (Official Microsoft Windows App PublicClientApplication)
   - **Legacy Client ID**: `a85cf173-4192-42f8-81fa-777a763e6e2c` (Azure Virtual Desktop Client)
   - **Authority**: `https://login.microsoftonline.com/organizations` (Work or school accounts) / custom tenant routing
   - **Redirect URI**: `ms-appx-web://microsoft.aad.brokerplugin/4fb5cc57-dbbc-4cdc-9595-748adff5f414`
   - **Graph Inventory API**: `https://graph.microsoft.com/beta/me/cloudPCs`
   - **Gateway Brokering**: `rdgateway.wvd.microsoft.com` / `windows.cloud.microsoft`

2. **Required Telemetry & Request Headers**:
   - `client-request-id`: UUIDv4 per request
   - `cpc-data-boundary`: `"commercial"` or `"eu"`
   - `source-client`: `"windows-app"`
   - `client-version`: `"2.0.1484.0"`
   - `x-client-SKU`: `"MSAL.Desktop"`
   - `x-client-Ver`: `"4.88.0.0"`
   - `x-client-OS`: `"Microsoft Windows NT 10.0.22631.0"` (WAM broker emulation)

---

## Key Features

- **Modern GTK 4 User Interface**:
  - Compliant with GNOME Human Interface Guidelines with system light/dark theme synchronization.
  - Fluent Windows App branding with scalable vector icons in desktop launchers, dock, and taskbar.
  - Seamless Grid and List layout modes with persistent connection state (maintains active spinners and disabled cards across layout switches).
  - OS-specific graphic banners with click-to-connect integration and dedicated action menus.

- **Authentication Options**:
  - **In-App WebKitGTK 6 SSO (Default)**: Embedded browser supporting PKCE, device identity (PKeyAuth challenge signing), and 30-day persistent cookie caching for seamless SSO restarts.
  - **Browser SSO**: Authenticate via your default system browser (Edge / Chrome) with PKCE loopback redirect.
  - **Device Code Flow**: Authenticate via Microsoft `devicelogin` code entry.
  - **Token Paste**: Manual authorization token entry.

- **First-Class RDP Client Engine Support**:
  - **In-App Native Remote Desktop Window (Default / Recommended)**: Connects to Cloud PC and AVD sessions in-process using FreeRDP 3 C-FFI, streaming frames directly into a GTK 4 `GtkDrawingArea` via Cairo. Provides a 100% native GTK window with title bar, minimize/maximize/close, fullscreen dock, dynamic resolution resizing, and full keyboard/mouse/scroll forwarding—working seamlessly on GNOME Wayland, KDE, and X11 without requiring standalone external clients.
  - **Pure Rust IronRDP (`ironrdp-client`)**: In-process Rust RDP client engine for direct connections.
  - **Hardware Audio & Camera Redirection**: Bidirectional PulseAudio/PipeWire audio and low-latency webcam redirection (`MS-RDPECAM`) with VAAPI hardware acceleration.
  - **Dependency Detection & Resolution**: Automatic startup and launch checks with built-in installation dialogs across all Linux distributions, macOS, and Windows.

- **Preferences & Display Scaling**:
  - Engine switcher (In-App Native Window [Recommended] vs IronRDP) with dirty-state save button tracking.
  - Configurable display scaling (Auto-detected from desktop scale, 100%, 125%, 150%, 175%, 200%).

- **Security & Privacy**:
  - PII and authorization tokens are redacted from default logs; verbose debugging is restricted to debug builds.

---

## Prerequisites & Installation

### Required Development Packages

The project requires GTK 4, WebKitGTK 6, libsoup 3, and FreeRDP 3 development libraries for in-process C-FFI rendering and AVD Gateway connections.

#### Debian / Ubuntu / Linux Mint
```bash
sudo apt update
sudo apt install -y \
    build-essential \
    pkg-config \
    libgtk-4-dev \
    libwebkitgtk-6.0-dev \
    libsoup-3.0-dev \
    freerdp3-dev \
    libwinpr3-dev
```

#### Fedora / RHEL / CentOS Stream
```bash
sudo dnf install -y \
    gcc \
    gcc-c++ \
    pkgconfig \
    gtk4-devel \
    webkitgtk6.0-devel \
    libsoup3-devel \
    freerdp-devel \
    winpr-devel \
    freerdp
```

#### Arch Linux / Manjaro
```bash
sudo pacman -S --needed \
    base-devel \
    pkgconf \
    gtk4 \
    webkitgtk-6.0 \
    libsoup3 \
    freerdp
```

#### openSUSE (Tumbleweed / Leap)
```bash
sudo zypper install -y \
    patterns-devel-base-devel_basis \
    pkg-config \
    gtk4-devel \
    webkit2gtk-6_0-devel \
    libsoup-devel \
    freerdp-devel \
    freerdp
```

#### macOS (Homebrew)
```bash
brew install gtk4 webkit2png freerdp pkg-config
```

---

### FreeRDP 3.32+ Requirement & Camera Redirection Fix

> [!IMPORTANT]
> **Debian / Ubuntu FreeRDP 3.15 Camera Redirection Bug**:
> In Debian 13 (Trixie) and Ubuntu packages shipping FreeRDP 3.15 (`3.15.0+dfsg-2.1+deb13u3`), the security patch for `CVE-2026-24677` introduced a regression in webcam redirection (`channels/rdpecam/client/encoding.c`).
> - When apps like Microsoft Teams, the Windows Camera app, or a web browser open the webcam, the encoder checks uninitialized scaler contexts on frame 0, returning `Frame drop or error in ecam_encoder_compress` on every single frame.
> - The frame capture thread then hangs in an unhandled wait loop, causing Windows Media Foundation to time out after 5 seconds with error `"0xA00F4271 Camera is in use by another app"` and turn off the camera.
>
> Upstream FreeRDP completely redesigned the `rdpecam` video pipeline in **FreeRDP >= 3.32** (`freerdp_video_sample_convert`), resolving the issue with full VAAPI hardware encoding support.

#### Pulling the FreeRDP 3.32+ Bundle for the Build (Debian / Ubuntu)

To build against FreeRDP 3.32+ without replacing your system packages or requiring root privileges:

```bash
# Pull FreeRDP 3.32+ backport libraries into ./freerdp-3.32
./scripts/fetch-freerdp.sh
```

The Cargo build script (`build.rs`) dynamically detects `./freerdp-3.32` relative to the workspace, automatically points `PKG_CONFIG_PATH` to it, and configures dynamic RPATHs (`$ORIGIN` and local paths) so the compiled binary uses FreeRDP 3.32+ seamlessly without machine-specific hardcoded paths.

---

## Building & Running

Once the development packages are installed (and `./scripts/fetch-freerdp.sh` has been run on Debian/Ubuntu), compile directly with standard Cargo commands:

```bash
# Compile release binary
cargo build --release

# Run tests
cargo test

# Run the application
cargo run --release
```

---

## Distribution Packaging

Platform packages bundle the executable binary, the `.desktop` launcher, and multi-resolution vector/raster icons directly into system paths (`/usr/bin`, `/usr/share/applications`, `/usr/share/icons/hicolor`).

Each package automatically specifies **runtime dependencies** so the distribution package manager automatically installs all required shared libraries (`gtk4`, `webkitgtk-6.0`, `libsoup-3.0`, `libfreerdp3`, `libwinpr3`) when installing the package.

### 1. Generating Packages

A unified packaging entrypoint is provided in `scripts/package.sh`, which automatically verifies development headers and delegates to the appropriate packaging script:

```bash
# Auto-detect your distribution and build its native package
./scripts/package.sh

# Or build for a specific target:
./scripts/package.sh deb       # Debian, Ubuntu, Linux Mint (.deb)
./scripts/package.sh rpm       # Fedora, RHEL, CentOS (.rpm)
./scripts/package.sh opensuse  # openSUSE Tumbleweed / Leap (.rpm)
./scripts/package.sh arch      # Arch Linux, Manjaro (.pkg.tar.zst)
./scripts/package.sh macos     # macOS App Bundle (.app, .dmg, .tar.gz)
./scripts/package.sh all       # Package all supported platforms
```

Output artifacts are generated in `target/packages/`:
- **Debian / Ubuntu**: `target/packages/windows-app-linux_<version>_amd64.deb`
- **Fedora / RHEL**: `target/packages/windows-app-linux-<version>-1.fc*.x86_64.rpm`
- **openSUSE**: `target/packages/windows-app-linux-<version>-1.suse*.x86_64.rpm`
- **Arch / Manjaro**: `target/packages/windows-app-linux-<version>-1-x86_64.pkg.tar.zst`
- **macOS**: `target/packages/windows-app-macos.dmg` / `windows-app-macos.tar.gz`

### 2. Runtime Package Dependencies

When packages are installed, the respective package managers resolve and install these runtime libraries:

| Distribution | Required Runtime Dependencies | Recommended RDP Clients (First-Class) |
| :--- | :--- | :--- |
| **Debian / Ubuntu** | `libgtk-4-1 (>= 4.12)`, `libwebkitgtk-6.0-4 (>= 2.40)`, `libsoup-3.0-0 (>= 3.0)`, `libfreerdp3-3`, `libwinpr3-3` | `freerdp3-x11`, `freerdp3-wayland` |
| **Fedora / RHEL** | `gtk4 >= 4.12`, `webkitgtk6.0 >= 2.40`, `libsoup3 >= 3.0`, `freerdp-libs >= 3.0` | `freerdp >= 3.0` |
| **openSUSE** | `gtk4 >= 4.12`, `libwebkit2gtk-6_0-4 >= 2.40`, `libsoup-3_0-0 >= 3.0`, `libfreerdp3 >= 3.0` | `freerdp >= 3.0` |
| **Arch Linux / Manjaro** | `gtk4`, `webkitgtk-6.0`, `libsoup3`, `freerdp` | `freerdp` |
| **macOS** | `gtk4`, `freerdp` | `freerdp` |

### 3. Installing Generated Packages

#### Debian / Ubuntu / Linux Mint
```bash
sudo apt install ./target/packages/windows-app-linux_*_amd64.deb
```

#### Fedora / RHEL
```bash
sudo dnf install ./target/packages/windows-app-linux-*.rpm
```

#### openSUSE
```bash
sudo zypper install ./target/packages/windows-app-linux-*.rpm
```

#### Arch Linux / Manjaro
```bash
sudo pacman -U ./target/packages/windows-app-linux-*-x86_64.pkg.tar.zst
```

---

## Verifying Desktop Integration

After installing a package, verify desktop integration with:

```bash
# Verify desktop database registration
update-desktop-database /usr/share/applications

# Verify icon cache contains com.microsoft.windowsapp.linux
gtk-update-icon-cache -f -t /usr/share/icons/hicolor

# Launch via Application launcher or terminal:
windows-app-linux
```

---

## License

This project is licensed under the MIT License. See [LICENSE](LICENSE) for details.
