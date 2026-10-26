#!/usr/bin/env bash
set -euo pipefail

# Build and package Windows App into a standard Debian (.deb) package

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"

# Verify required development dependencies
source "${SCRIPT_DIR}/check-deps.sh"
check_dev_dependencies

VERSION="$(grep '^version' "${ROOT_DIR}/Cargo.toml" | head -n 1 | cut -d '"' -f 2)"
ARCH="amd64"
PKG_NAME="windows-app_${VERSION}_${ARCH}"
BUILD_DIR="${ROOT_DIR}/target/package-deb/${PKG_NAME}"
OUTPUT_DIR="${ROOT_DIR}/target/packages"

echo "==> Building release binary..."
cd "${ROOT_DIR}"
cargo build --release

if [ -f "${ROOT_DIR}/target/release/windows-app" ]; then
    BIN_SRC="${ROOT_DIR}/target/release/windows-app"
elif [ -f "${ROOT_DIR}/target/release/windows-app-linux" ]; then
    BIN_SRC="${ROOT_DIR}/target/release/windows-app-linux"
else
    echo "Error: Release binary not found in target/release!"
    exit 1
fi

echo "==> Staging Debian package layout in ${BUILD_DIR}..."
rm -rf "${BUILD_DIR}"
mkdir -p "${BUILD_DIR}/DEBIAN"
mkdir -p "${BUILD_DIR}/usr/bin"
mkdir -p "${BUILD_DIR}/usr/share/applications"
mkdir -p "${BUILD_DIR}/usr/share/icons/hicolor/scalable/apps"

# Binary
cp "${BIN_SRC}" "${BUILD_DIR}/usr/bin/windows-app"
chmod 755 "${BUILD_DIR}/usr/bin/windows-app"
ln -sf windows-app "${BUILD_DIR}/usr/bin/windows-app-linux"

# Desktop entry
cp "${ROOT_DIR}/assets/com.microsoft.windowsapp.linux.desktop" "${BUILD_DIR}/usr/share/applications/"
chmod 644 "${BUILD_DIR}/usr/share/applications/com.microsoft.windowsapp.linux.desktop"

# Desktop search (GNOME Shell search provider, started through D-Bus activation)
mkdir -p "${BUILD_DIR}/usr/share/gnome-shell/search-providers" "${BUILD_DIR}/usr/share/dbus-1/services"
install -m 644 "${ROOT_DIR}/assets/com.microsoft.windowsapp.linux.search-provider.ini" "${BUILD_DIR}/usr/share/gnome-shell/search-providers/"
install -m 644 "${ROOT_DIR}/assets/com.microsoft.windowsapp.linux.service" "${BUILD_DIR}/usr/share/dbus-1/services/"

# Optional GNOME Shell helper for restoring workspaces (turned on in Preferences)
EXT_DIR="${BUILD_DIR}/usr/share/gnome-shell/extensions/windows-app-linux@sudhirdhumal289.github.io"
mkdir -p "${EXT_DIR}"
install -m 644 "${ROOT_DIR}/assets/gnome-shell-extension/windows-app-linux@sudhirdhumal289.github.io/"* "${EXT_DIR}/"

# Scalable vector icon
cp "${ROOT_DIR}/assets/com.microsoft.windowsapp.linux.svg" "${BUILD_DIR}/usr/share/icons/hicolor/scalable/apps/"
chmod 644 "${BUILD_DIR}/usr/share/icons/hicolor/scalable/apps/com.microsoft.windowsapp.linux.svg"

# Multi-resolution raster icons
SIZES=("16x16" "24x24" "32x32" "48x48" "64x64" "128x128" "256x256" "512x512")
for size in "${SIZES[@]}"; do
    SRC_PNG="${ROOT_DIR}/resources/icons/${size}/apps/com.microsoft.windowsapp.linux.png"
    if [ -f "${SRC_PNG}" ]; then
        DEST_DIR="${BUILD_DIR}/usr/share/icons/hicolor/${size}/apps"
        mkdir -p "${DEST_DIR}"
        cp "${SRC_PNG}" "${DEST_DIR}/"
        chmod 644 "${DEST_DIR}/com.microsoft.windowsapp.linux.png"
    fi
done

# Bundle FreeRDP 3.32+ if present so users are protected from the distro 3.15 camera regression
FREERDP_BUNDLE="${ROOT_DIR}/freerdp-3.32"
if [ -d "${FREERDP_BUNDLE}" ]; then
    echo "==> Bundling FreeRDP 3.32+ libraries..."
    LIB_DEST="${BUILD_DIR}/usr/lib/windows-app"
    mkdir -p "${LIB_DEST}"
    find "${FREERDP_BUNDLE}/lib" -maxdepth 2 -name "*.so*" -exec cp -a {} "${LIB_DEST}/" \;
fi

# DEBIAN/control - strictly specifies required system runtime dependencies
cat <<EOF > "${BUILD_DIR}/DEBIAN/control"
Package: windows-app
Provides: windows-app-linux
Replaces: windows-app-linux
Version: ${VERSION}
Section: utils
Priority: optional
Architecture: ${ARCH}
Maintainer: Windows App for Linux Contributors
Depends: libgtk-4-1 (>= 4.14), libadwaita-1-0 (>= 1.5), libwebkitgtk-6.0-4 (>= 2.42), libsoup-3.0-0 (>= 3.0), libfreerdp3-3 | libfreerdp3 | libc6, libwinpr3-3 | libwinpr3 | libc6
Description: Microsoft Windows 365 Cloud PC and Azure Virtual Desktop client for Linux
 Native Linux client written in Rust using GTK 4 and WebKitGTK 6 for connecting
 to Windows 365 Cloud PCs and AVD environments.
EOF

# DEBIAN/postinst
cat <<'EOF' > "${BUILD_DIR}/DEBIAN/postinst"
#!/bin/sh
set -e
if [ -x /usr/bin/update-desktop-database ]; then
    update-desktop-database -q /usr/share/applications || true
fi
if [ -x /usr/bin/gtk-update-icon-cache ]; then
    gtk-update-icon-cache -q -f -t /usr/share/icons/hicolor || true
fi
EOF
chmod 755 "${BUILD_DIR}/DEBIAN/postinst"

# DEBIAN/postrm
cat <<'EOF' > "${BUILD_DIR}/DEBIAN/postrm"
#!/bin/sh
set -e
if [ -x /usr/bin/update-desktop-database ]; then
    update-desktop-database -q /usr/share/applications || true
fi
if [ -x /usr/bin/gtk-update-icon-cache ]; then
    gtk-update-icon-cache -q -f -t /usr/share/icons/hicolor || true
fi
EOF
chmod 755 "${BUILD_DIR}/DEBIAN/postrm"

echo "==> Building binary package using dpkg-deb..."
mkdir -p "${OUTPUT_DIR}"
dpkg-deb --build --root-owner-group "${BUILD_DIR}" "${OUTPUT_DIR}/${PKG_NAME}.deb"

echo "==> Debian package generated successfully:"
echo "    ${OUTPUT_DIR}/${PKG_NAME}.deb"
ls -lh "${OUTPUT_DIR}/${PKG_NAME}.deb"
