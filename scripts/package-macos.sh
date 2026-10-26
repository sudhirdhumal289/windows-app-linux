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

echo "==> Building release binary (AppKit frontend)..."
cd "${ROOT_DIR}"
cargo build --release --no-default-features --features appkit

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

# App icon: packaging/macos/AppIcon.appiconset holds every size iconutil expects.
if command -v iconutil >/dev/null 2>&1; then
    ICONSET_DIR="${ROOT_DIR}/target/macos-bundle/AppIcon.iconset"
    rm -rf "${ICONSET_DIR}"
    mkdir -p "${ICONSET_DIR}"
    cp "${ROOT_DIR}"/packaging/macos/AppIcon.appiconset/icon_*.png "${ICONSET_DIR}/"
    iconutil -c icns "${ICONSET_DIR}" -o "${RESOURCES_DIR}/AppIcon.icns"
    rm -rf "${ICONSET_DIR}"
else
    cp "${ROOT_DIR}/packaging/macos/AppIcon.appiconset/icon_512x512@2x.png" "${RESOURCES_DIR}/AppIcon.png"
fi

# Ad-hoc signature with the app's entitlements (network client, keychain); replace "-"
# with a Developer ID identity for distribution.
if command -v codesign >/dev/null 2>&1; then
    codesign --force --deep --options runtime \
        --entitlements "${ROOT_DIR}/packaging/macos/WindowsApp.entitlements" \
        --sign "${CODESIGN_IDENTITY:--}" "${BUNDLE_DIR}"
fi

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
