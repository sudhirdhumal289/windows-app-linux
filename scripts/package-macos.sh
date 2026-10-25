#!/usr/bin/env bash
set -euo pipefail

# Build and package Windows App for macOS (.app bundle and .dmg / .tar.gz)

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"

OUTPUT_DIR="${ROOT_DIR}/target/packages"
BUNDLE_DIR="${ROOT_DIR}/target/macos-bundle/Windows App.app"
CONTENTS_DIR="${BUNDLE_DIR}/Contents"
MACOS_DIR="${CONTENTS_DIR}/MacOS"
RESOURCES_DIR="${CONTENTS_DIR}/Resources"

echo "==> Building release binary..."
cd "${ROOT_DIR}"
cargo build --release

BIN_SRC="${ROOT_DIR}/target/release/windows-app-linux"
if [ ! -f "${BIN_SRC}" ]; then
    echo "Error: Release binary ${BIN_SRC} not found!"
    exit 1
fi

echo "==> Staging macOS App Bundle at ${BUNDLE_DIR}..."
rm -rf "${BUNDLE_DIR}"
mkdir -p "${MACOS_DIR}"
mkdir -p "${RESOURCES_DIR}"
mkdir -p "${OUTPUT_DIR}"

# Binary and Info.plist
cp "${BIN_SRC}" "${MACOS_DIR}/windows-app-linux"
chmod 755 "${MACOS_DIR}/windows-app-linux"
cp "${ROOT_DIR}/packaging/macos/Info.plist" "${CONTENTS_DIR}/"

# App Icons
if command -v iconutil >/dev/null 2>&1; then
    ICONSET_DIR="${ROOT_DIR}/target/macos-bundle/AppIcon.iconset"
    rm -rf "${ICONSET_DIR}"
    mkdir -p "${ICONSET_DIR}"

    [ -f "${ROOT_DIR}/resources/icons/16x16/apps/com.microsoft.windowsapp.linux.png" ] && cp "${ROOT_DIR}/resources/icons/16x16/apps/com.microsoft.windowsapp.linux.png" "${ICONSET_DIR}/icon_16x16.png"
    [ -f "${ROOT_DIR}/resources/icons/32x32/apps/com.microsoft.windowsapp.linux.png" ] && cp "${ROOT_DIR}/resources/icons/32x32/apps/com.microsoft.windowsapp.linux.png" "${ICONSET_DIR}/icon_16x16@2x.png"
    [ -f "${ROOT_DIR}/resources/icons/32x32/apps/com.microsoft.windowsapp.linux.png" ] && cp "${ROOT_DIR}/resources/icons/32x32/apps/com.microsoft.windowsapp.linux.png" "${ICONSET_DIR}/icon_32x32.png"
    [ -f "${ROOT_DIR}/resources/icons/64x64/apps/com.microsoft.windowsapp.linux.png" ] && cp "${ROOT_DIR}/resources/icons/64x64/apps/com.microsoft.windowsapp.linux.png" "${ICONSET_DIR}/icon_32x32@2x.png"
    [ -f "${ROOT_DIR}/resources/icons/128x128/apps/com.microsoft.windowsapp.linux.png" ] && cp "${ROOT_DIR}/resources/icons/128x128/apps/com.microsoft.windowsapp.linux.png" "${ICONSET_DIR}/icon_128x128.png"
    [ -f "${ROOT_DIR}/resources/icons/256x256/apps/com.microsoft.windowsapp.linux.png" ] && cp "${ROOT_DIR}/resources/icons/256x256/apps/com.microsoft.windowsapp.linux.png" "${ICONSET_DIR}/icon_128x128@2x.png"
    [ -f "${ROOT_DIR}/resources/icons/256x256/apps/com.microsoft.windowsapp.linux.png" ] && cp "${ROOT_DIR}/resources/icons/256x256/apps/com.microsoft.windowsapp.linux.png" "${ICONSET_DIR}/icon_256x256.png"
    [ -f "${ROOT_DIR}/resources/icons/512x512/apps/com.microsoft.windowsapp.linux.png" ] && cp "${ROOT_DIR}/resources/icons/512x512/apps/com.microsoft.windowsapp.linux.png" "${ICONSET_DIR}/icon_256x256@2x.png"
    [ -f "${ROOT_DIR}/resources/icons/512x512/apps/com.microsoft.windowsapp.linux.png" ] && cp "${ROOT_DIR}/resources/icons/512x512/apps/com.microsoft.windowsapp.linux.png" "${ICONSET_DIR}/icon_512x512.png"

    iconutil -c icns "${ICONSET_DIR}" -o "${RESOURCES_DIR}/AppIcon.icns"
    rm -rf "${ICONSET_DIR}"
fi

# Fallback icons & resources
cp "${ROOT_DIR}/resources/icons/512x512/apps/com.microsoft.windowsapp.linux.png" "${RESOURCES_DIR}/AppIcon.png"
cp "${ROOT_DIR}/assets/com.microsoft.windowsapp.linux.svg" "${RESOURCES_DIR}/"

# DMG or Tarball packaging
if command -v hdiutil >/dev/null 2>&1; then
    echo "==> Creating macOS DMG with hdiutil..."
    DMG_PATH="${OUTPUT_DIR}/windows-app-macos.dmg"
    rm -f "${DMG_PATH}"
    hdiutil create -volname "Windows App" -srcfolder "${ROOT_DIR}/target/macos-bundle" -ov -format UDZO "${DMG_PATH}"
    echo "==> Successfully created ${DMG_PATH}"
else
    echo "==> Creating macOS App tarball..."
    TAR_PATH="${OUTPUT_DIR}/windows-app-macos.tar.gz"
    tar -czf "${TAR_PATH}" -C "${ROOT_DIR}/target/macos-bundle" "Windows App.app"
    echo "==> Staged macOS App Bundle and created ${TAR_PATH}"
fi
