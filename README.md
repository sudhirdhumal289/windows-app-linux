# Windows App (Windows 365 & AVD) for Linux

A native, high-performance Linux client for Microsoft Windows 365 Cloud PCs and Azure Virtual Desktop (AVD), re-engineered from the official Windows App MSIX package (`WindowsApp_x64_Release_2.0.1484.0.msix`) and built in Rust using GTK 4 (`gtk4-rs`), libadwaita (`libadwaita-rs`) and WebKitGTK (`webkit6`). The same Rust core also powers native Windows (WinUI 3) and macOS (AppKit) frontends, written in Rust as well.

---

## Architecture & Reverse Engineering

The application directly interoperates with Microsoft Cloud PC and Entra ID endpoints using reverse-engineered signatures and telemetry headers:

1. **Client Identity & Endpoints**:
   - **Client ID**: `4fb5cc57-dbbc-4cdc-9595-748adff5f414` (Official Microsoft Windows App PublicClientApplication)
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

- **Modern GTK 4 / libadwaita User Interface**:
  - Follows the GNOME HIG and the system light/dark theme and accent colour; no custom theming.
  - Grid and list layouts with persistent connection state across layout switches.
  - **Collapsible sections** in both layouts separate Cloud PCs, Azure Virtual Desktop resources and Remote PCs; collapsed sections are remembered (searching shows matches in every section).
  - **Quit confirmation**: closing the main window (or Ctrl+Q / **Quit**) while sessions are open asks whether to **Disconnect and Quit** or **Keep Open**, which closes only the main window; the app then exits after the last session window closes.
  - **About** dialog (main menu) with version, FreeRDP/GTK/libadwaita versions under Troubleshooting, credits, licence and issue link.
  - **Add Connection** button (Ctrl+N) in the header bar: add a Microsoft Cloud PC (signs in to Windows 365) or a Remote PC by address (`host`, `host:port`, `[ipv6]:port`).
  - An empty-state page when no connections exist yet.
  - Per-PC action menus (connect, restart, rename, connection info, details; edit/export/remove for Remote PCs).
  - **Search** (Ctrl+F or just type): filters all connections; Enter connects to the only match, or **quick-connects** to a typed address.
  - **Recent connections**: every card shows when it was last used.
  - **Groups**: Remote PCs can be given a group; they are sorted by group and the group is shown on the card.
  - Status badges with provisioning progress, power state, Frontline activation and grace-period days left; **View Details** adds licence, power, provisioning, grace-period end, last sign-in and the last remote action.

- **Cloud PC Management** (the Windows 365 end-user actions; unavailable items are disabled per PC):
  - **Start / Stop** for Windows 365 Frontline Cloud PCs. This uses Microsoft Graph and needs the delegated `CloudPC.ReadWrite.All` permission granted by your organization; connecting also starts the PC.
  - **Troubleshoot**: runs the service's connectivity check and repair and reports the outcome.
  - **Restore…** to a restore point and **Reset…** (reinstall, with typed confirmation).

- **Azure Virtual Desktop**: desktops and RemoteApps published to your account (RD Web feed) appear next to your Cloud PCs; RemoteApps open in a session window.

- **Remote PCs (direct RDP)**:
  - Domain, **RD Gateway** (optionally with separate credentials), **RemoteApp** (program and arguments) and **admin/console session**.
  - Security protocol (Automatic, NLA, TLS, RDP), certificate policy (trust on first use, ask, always accept) and authentication (Negotiate, NTLM only, Kerberos only; Kerberos uses your `kinit` ticket).
  - **Remember password**: stored in the system keyring (Secret Service, e.g. GNOME Keyring), never in files. Forget it from the PC's menu.
  - **`.rdp` files**: import (Ctrl+O or Add Connection → Import RDP File), export, or open one directly (`windows-app-linux file.rdp`, or from the file manager).
  - **Commands before connecting and after disconnecting** (set in Connection Settings, e.g. VPN or `kinit`). They run through `sh -c` with `WINDOWS_APP_NAME`, `WINDOWS_APP_ADDRESS`, `WINDOWS_APP_TARGET` and `WINDOWS_APP_PHASE` set. A failing (or over 60 s) pre-connect command cancels the connection.

- **Per-Connection Settings** (Connection Settings… in each card's menu): override resolution (fit window, fixed presets or custom), graphics mode, connection quality, monitors and device redirection for one Cloud PC, AVD resource or Remote PC; everything else follows Preferences.

- **Desktop Integration (Linux)**:
  - **Activities search**: Cloud PCs, AVD resources and Remote PCs show up in GNOME Shell search and connect when chosen (search provider over D-Bus; results come from a local index, so searching never contacts Microsoft).
  - **Show in App Launcher**: adds a launcher entry per connection (`~/.local/share/applications`).
  - **Connect at Login**: XDG autostart entry per connection.
  - Command line: `windows-app-linux --connect cloudpc:<id>|avd:<id>|remote:<id>` or `windows-app-linux <file.rdp>`.
  - **Multiple accounts**: every signed-in work or school account is remembered; switch from the main menu or **Add Another Account…**.

- **Authentication Options** (switchable in the sign-in window):
  - **In-app browser (default)**: embedded WebKitGTK sign-in with PKCE, PKeyAuth device challenges and persistent SSO cookies.
  - **Device code**: approve a code in another (e.g. Intune-managed Edge) browser.
  - **Token paste**: last resort when Conditional Access blocks the other methods.

- **Embedded FreeRDP 3 Session Window**:
  - FreeRDP runs in-process; frames are drawn by a GTK widget, so sessions work on Wayland and X11 without external clients.
  - Dynamic resolution follows the window (MS-RDPEDISP), with automatic reconnect.
  - Per-session display scale (Match Display, 100–200 % presets, or a custom value up to 200 %) from the session header bar or fullscreen dock; applied live and never saved.
  - Full keyboard (including system shortcuts while focused; **Capture System Shortcuts** can be turned off), mouse and smooth scrolling.
  - **Session menu** (header bar and fullscreen dock): send Ctrl+Alt+Del, Ctrl+Shift+Esc, Windows, Win+L, Alt+Tab, Alt+F4 and Print Screen; **Type Clipboard Text** as keystrokes (for sign-in screens and consoles without clipboard); **Save Screenshot** (to Pictures); **View Only** (input is not sent).
  - Clipboard: text, images and files in both directions (files copied in the session are downloaded when pasted).
  - Server certificates you have not seen before can be reviewed and accepted once or permanently (per the connection's certificate policy).
  - Fullscreen with a slide-down dock (Ctrl+Alt+Enter toggles fullscreen).
  - **Remembers where each session window was**: reopens fullscreen on the same monitor (matched by make/model, then connector). The workspace is restored too: on X11 desktops directly, and on GNOME (Wayland) after turning on **Preferences → Session Windows → Restore Workspaces**, which turns on a small GNOME Shell extension installed with the app packages (Wayland gives apps no way to read or choose a window's workspace; the extension only lets an app query and move its own windows). GNOME Shell discovers extensions only at login, so log out and in once after first installing the package; after that the switch applies immediately. Stored in `~/.config/windows-app-linux/window-placement.json`.
  - Audio, microphone, camera and shared-folder redirection, configured in Preferences: the home or a chosen folder, **more shared folders**, and **removable drives** (USB sticks plugged in during the session appear too).
  - **Connection quality** presets (Automatic, LAN, Broadband, Low Bandwidth) in Preferences or per connection.
  - **Use All Monitors** in fullscreen (one remote monitor per local monitor, MS-RDPEDISP layout).
  - Graphics pipeline with H.264 (AVC444/AVC420) and RemoteFX fallback, selectable in Preferences. Cloud PC and AVD sessions use RemoteFX when FreeRDP is older than 3.27, which cannot display hosts with screen-capture protection.
  - USB device (`/usb`, needs a udev rule granting access to the device node), smart card and printer redirection.
  - Live connection-quality indicator (round-trip time and bandwidth from network auto-detect) in the session header and dock.
  - Session-host sign-in reuses the app's SSO silently and only shows a window when interaction is needed.

- **Security & Privacy**:
  - Tokens are stored with owner-only permissions; tokens, passwords and personal data are redacted from logs.
  - Remote PC passwords are asked for on every connection unless you choose to remember them; remembered passwords live only in the system keyring.
  - Unknown server certificates of Remote PCs are shown for review by default (or trusted on first use, per connection); a changed certificate is never accepted silently.

---

## Project Structure

The crate is split into a toolkit-independent **core library** and **frontends**, all in
Rust. Only a frontend touches a UI toolkit; each follows its platform's design guidelines.

| Module | Responsibility |
| :--- | :--- |
| `config` | Preferences, sign-in profiles, constants and private file storage |
| `auth` | Entra ID OAuth (PKCE, device code, refresh), token store, PKeyAuth, account info |
| `api` | Microsoft Graph / Windows 365 client: Cloud PC listing, actions, RDP launch details |
| `connections` | Saved Remote PCs (direct RDP by address, gateway, RemoteApp, security): validation and storage |
| `overrides` | Per-connection settings that override the preferences |
| `history` | When each connection was last used |
| `placement` | Which monitor and workspace each connection's session window was last on |
| `rdp_file` | `.rdp` file import and export |
| `launcher` | Launch targets (`--connect`), launcher/autostart entries and the desktop search index |
| `session` | Builds the private `.rdp` file and FreeRDP arguments for a session; per-session scale choices, key combinations and typed text |
| `rdp` | Safe wrapper around the embedded FreeRDP engine (C bridge in `src/rdp/freerdp_bridge.c`): events, frames, input, clipboard, keymaps for Linux, Windows and macOS |
| `platform` | Host-OS integration (desktop scale, USB devices, launcher/autostart entries, runtime directories, keyring secrets, connection commands) per OS |
| `frontend::common` | Toolkit-neutral view models shared by every frontend: the connection library (sections, cards, menus, search), connection preparation, the session controller, web-view sign-in and all user-facing text |
| `frontend::gtk` | GTK 4 / libadwaita / WebKitGTK interface for Linux (cargo feature `gtk`, default) |
| `frontend::winui` | WinUI 3 interface for Windows through [`windows-reactor`](https://crates.io/crates/windows-reactor), WebView2 sign-in and a Win32 session view (feature `winui`) |
| `frontend::appkit` | AppKit interface for macOS through [`objc2`](https://crates.io/crates/objc2), WKWebView sign-in (feature `appkit`) |

The core builds without any frontend: `cargo check --lib --no-default-features`.

| Platform | Design language | Build |
| :--- | :--- | :--- |
| Linux | GNOME HIG (libadwaita) | `cargo build --release` |
| Windows 10/11 | Fluent (Mica, title bar, NavigationView, ContentDialog) | `cargo build --release --no-default-features --features winui` |
| macOS 13+ | Apple HIG (unified toolbar, sidebar, sheets, SF Symbols) | `cargo build --release --no-default-features --features appkit` |

### Adding a frontend for another OS

1. Add a cargo feature and a `src/frontend/<name>/` module gated on it (and on the target
   OS), then call its `run()` from `src/main.rs`.
2. Build the screens from `frontend::common`: turn `library::SectionView`s into native
   cards or rows and `library::MenuItem`s into native menus, run the `launch::prepare_*`
   functions to connect, drive sign-in web views with `signin::BrowserSignIn`, and take
   every string from `text`.
3. For a session, create a `session::SessionController`; its `Wake` callback must schedule
   `drain_events()` on the UI thread. Draw the BGRX `rdp::Frame` and forward native input
   (`rdp::keymap` maps native key codes to RDP scancodes).
4. Add the OS-specific parts to `platform` and make `build.rs` locate FreeRDP on that OS.

---

## Prerequisites & Installation

### Required Development Packages

The project requires GTK 4.14+, libadwaita 1.5+, WebKitGTK 6 (2.42+), libsoup 3, and FreeRDP 3 development libraries for in-process C-FFI rendering and AVD Gateway connections.

#### Debian / Ubuntu / Linux Mint
```bash
sudo apt update
sudo apt install -y \
    build-essential \
    pkg-config \
    libgtk-4-dev \
    libadwaita-1-dev \
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
    libadwaita-devel \
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
    libadwaita \
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
    libadwaita-devel \
    webkit2gtk-6_0-devel \
    libsoup-devel \
    freerdp-devel \
    freerdp
```

#### macOS (Homebrew)
The AppKit frontend only needs FreeRDP and the Xcode command line tools:
```bash
xcode-select --install
brew install freerdp pkg-config
```

#### Windows
Install Rust with the MSVC toolchain, [vcpkg](https://vcpkg.io) and pkgconf, then FreeRDP:
```powershell
vcpkg install freerdp:x64-windows pkgconf
$env:PKG_CONFIG_PATH = "$env:VCPKG_ROOT\installed\x64-windows\lib\pkgconfig"
```
The app needs the [Windows App SDK runtime](https://learn.microsoft.com/windows/apps/windows-app-sdk/downloads)
and the WebView2 runtime (preinstalled on Windows 11).

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

# Windows (WinUI 3) and macOS (AppKit) frontends
cargo run --release --no-default-features --features winui
cargo run --release --no-default-features --features appkit
```

The non-Linux frontends can be type-checked from Linux with a cross target, e.g.
`cargo check --no-default-features --features appkit --target aarch64-apple-darwin`
(FreeRDP is skipped when cross-compiling, so only checking works, not linking).

Real Windows and macOS builds need their own OS: the Apple SDK (AppKit, WebKit) may only be
used on Apple hardware, and FreeRDP must be built for each target. The
[`Build` workflow](.github/workflows/build.yml) builds, tests and packages all three
frontends on GitHub's Linux, Windows and macOS runners and uploads the `.deb`, `.zip` and
`.dmg` as artifacts.

---

## Distribution Packaging

Platform packages bundle the executable binary, the `.desktop` launcher, and multi-resolution vector/raster icons directly into system paths (`/usr/bin`, `/usr/share/applications`, `/usr/share/icons/hicolor`).

The app icon (a white monitor with two-way connection arrows on a blue rounded tile, for Cloud PCs and any RDP host) is drawn in `assets/com.microsoft.windowsapp.linux.svg`. After editing it, run `scripts/render-icons.py` to regenerate the PNG sizes in `resources/icons/`.

Each package automatically specifies **runtime dependencies** so the distribution package manager automatically installs all required shared libraries (`gtk4`, `libadwaita`, `webkitgtk-6.0`, `libsoup-3.0`, `libfreerdp3`, `libwinpr3`) when installing the package.

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
./scripts/package.sh macos     # macOS App Bundle with the AppKit frontend (.app, .dmg, .tar.gz)
./scripts/package.sh all       # Package all supported platforms
```

Output artifacts are generated in `target/packages/`:
- **Debian / Ubuntu**: `target/packages/windows-app-linux_<version>_amd64.deb`
- **Fedora / RHEL**: `target/packages/windows-app-linux-<version>-1.fc*.x86_64.rpm`
- **openSUSE**: `target/packages/windows-app-linux-<version>-1.suse*.x86_64.rpm`
- **Arch / Manjaro**: `target/packages/windows-app-linux-<version>-1-x86_64.pkg.tar.zst`
- **macOS**: `target/packages/windows-app-macos.dmg` / `windows-app-macos.tar.gz`
- **Windows** (run `.\scripts\package-windows.ps1` on Windows): `target\packages\windows-app-windows-x64.zip`

### 2. Runtime Package Dependencies

When packages are installed, the respective package managers resolve and install these runtime libraries:

| Distribution | Required Runtime Dependencies |
| :--- | :--- |
| **Debian / Ubuntu** | `libgtk-4-1 (>= 4.14)`, `libadwaita-1-0 (>= 1.5)`, `libwebkitgtk-6.0-4 (>= 2.42)`, `libsoup-3.0-0 (>= 3.0)`, `libfreerdp3-3`, `libwinpr3-3` |
| **Fedora / RHEL** | `gtk4 >= 4.14`, `libadwaita >= 1.5`, `webkitgtk6.0 >= 2.42`, `libsoup3 >= 3.0`, `freerdp-libs >= 3.0` |
| **openSUSE** | `gtk4 >= 4.14`, `libadwaita-1-0 >= 1.5`, `libwebkit2gtk-6_0-4 >= 2.42`, `libsoup-3_0-0 >= 3.0`, `freerdp >= 3.0` |
| **Arch Linux / Manjaro** | `gtk4`, `libadwaita`, `webkitgtk-6.0`, `libsoup3`, `freerdp` |
| **macOS** | `freerdp` (Homebrew) |
| **Windows** | Windows App SDK runtime, WebView2 runtime (FreeRDP DLLs are bundled) |

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

# Desktop search: the provider is registered by these files (log out and in once
# after installing so GNOME Shell picks it up)
ls /usr/share/gnome-shell/search-providers/com.microsoft.windowsapp.linux.search-provider.ini \
   /usr/share/dbus-1/services/com.microsoft.windowsapp.linux.service

# Workspace restore helper on GNOME (turned on in Preferences)
gnome-extensions info windows-app-linux@sudhirdhumal289.github.io
```

When running from a source build instead of a package, install the helper for your user
(then log out and in once):

```bash
gnome-extensions pack --force assets/gnome-shell-extension/windows-app-linux@sudhirdhumal289.github.io
gnome-extensions install --force windows-app-linux@sudhirdhumal289.github.io.shell-extension.zip
```

To let USB redirection open a device, grant your user access to it, e.g.
`/etc/udev/rules.d/70-windows-app-usb.rules`:

```
SUBSYSTEM=="usb", ATTRS{idVendor}=="046d", ATTRS{idProduct}=="c52b", TAG+="uaccess"
```

---

## License

This project is licensed under the MIT License. See [LICENSE](LICENSE) for details.
