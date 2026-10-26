#!/usr/bin/env bash
set -euo pipefail

# Build and package Windows App for Arch Linux / Manjaro (.pkg.tar.zst)

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"

VERSION="$(grep '^version' "${ROOT_DIR}/Cargo.toml" | head -n 1 | cut -d '"' -f 2)"
PKG_REL="1"
ARCH="x86_64"
OUTPUT_DIR="${ROOT_DIR}/target/packages"
ARCH_DIR="${ROOT_DIR}/packaging/arch"
PKG_NAME="windows-app-${VERSION}-${PKG_REL}-${ARCH}.pkg.tar.zst"
STAGE_DIR="${ROOT_DIR}/target/package-arch"

mkdir -p "${OUTPUT_DIR}"

BIN_SRC="${ROOT_DIR}/target/release/windows-app"
if [ ! -f "${BIN_SRC}" ]; then
    if [ -f "${ROOT_DIR}/target/release/windows-app-linux" ]; then
        BIN_SRC="${ROOT_DIR}/target/release/windows-app-linux"
    else
        echo "==> Building release binary..."
        cd "${ROOT_DIR}"
        cargo build --release
    fi
fi

if command -v makepkg >/dev/null 2>&1; then
    echo "==> Packaging with makepkg..."
    cd "${ARCH_DIR}"
    PKGDEST="${OUTPUT_DIR}" makepkg -f --nodeps
    echo "==> Successfully created Arch package in ${OUTPUT_DIR}/"
else
    echo "==> Packaging Arch / Manjaro package (.pkg.tar.zst)..."
    rm -rf "${STAGE_DIR}"
    mkdir -p "${STAGE_DIR}/pkg/usr/bin"
    mkdir -p "${STAGE_DIR}/pkg/usr/share/applications"
    mkdir -p "${STAGE_DIR}/pkg/usr/share/icons/hicolor/scalable/apps"
    mkdir -p "${STAGE_DIR}/pkg/usr/share/gnome-shell/search-providers"
    mkdir -p "${STAGE_DIR}/pkg/usr/share/dbus-1/services"
    mkdir -p "${STAGE_DIR}/pkg/usr/share/gnome-shell/extensions/windows-app-linux@sudhirdhumal289.github.io"

    # Binaries
    cp "${BIN_SRC}" "${STAGE_DIR}/pkg/usr/bin/windows-app"
    chmod 755 "${STAGE_DIR}/pkg/usr/bin/windows-app"
    ln -sf windows-app "${STAGE_DIR}/pkg/usr/bin/windows-app-linux"

    # Desktop entry & DBus service
    cp "${ROOT_DIR}/assets/com.microsoft.windowsapp.linux.desktop" "${STAGE_DIR}/pkg/usr/share/applications/"
    cp "${ROOT_DIR}/assets/com.microsoft.windowsapp.linux.search-provider.ini" "${STAGE_DIR}/pkg/usr/share/gnome-shell/search-providers/"
    cp "${ROOT_DIR}/assets/com.microsoft.windowsapp.linux.service" "${STAGE_DIR}/pkg/usr/share/dbus-1/services/"
    cp "${ROOT_DIR}/assets/gnome-shell-extension/windows-app-linux@sudhirdhumal289.github.io/"* "${STAGE_DIR}/pkg/usr/share/gnome-shell/extensions/windows-app-linux@sudhirdhumal289.github.io/"
    cp "${ROOT_DIR}/assets/com.microsoft.windowsapp.linux.svg" "${STAGE_DIR}/pkg/usr/share/icons/hicolor/scalable/apps/"

    # Icons
    SIZES=("16x16" "24x24" "32x32" "48x48" "64x64" "128x128" "256x256" "512x512")
    for size in "${SIZES[@]}"; do
        SRC_PNG="${ROOT_DIR}/resources/icons/${size}/apps/com.microsoft.windowsapp.linux.png"
        if [ -f "${SRC_PNG}" ]; then
            DEST_DIR="${STAGE_DIR}/pkg/usr/share/icons/hicolor/${size}/apps"
            mkdir -p "${DEST_DIR}"
            cp "${SRC_PNG}" "${DEST_DIR}/"
        fi
    done

    # .PKGINFO metadata
    BUILD_DATE="$(date +%s)"
    PKG_SIZE="$(du -sb "${STAGE_DIR}/pkg" 2>/dev/null | cut -f1 || echo 10485760)"
    cat <<EOF > "${STAGE_DIR}/pkg/.PKGINFO"
pkgname = windows-app
pkgver = ${VERSION}-${PKG_REL}
pkgdesc = Microsoft Windows 365 Cloud PC and Azure Virtual Desktop client
url = https://github.com/sudhirdhumal289/windows-app-linux
builddate = ${BUILD_DATE}
packager = Windows App Contributors
size = ${PKG_SIZE}
arch = ${ARCH}
license = MIT
license = Apache-2.0
depend = gtk4
depend = libadwaita
depend = webkitgtk-6.0
depend = libsoup3
depend = freerdp
provides = windows-app-linux=${VERSION}
conflicts = windows-app-linux
EOF

    cd "${STAGE_DIR}/pkg"
    if tar --help | grep -q -- '--zstd'; then
        tar --zstd -cf "${OUTPUT_DIR}/${PKG_NAME}" .PKGINFO usr
    else
        tar -cf - .PKGINFO usr | zstd -c -T0 > "${OUTPUT_DIR}/${PKG_NAME}"
    fi
    echo "==> Successfully created Arch package: ${OUTPUT_DIR}/${PKG_NAME}"
fi
