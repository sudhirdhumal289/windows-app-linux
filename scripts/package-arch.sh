#!/usr/bin/env bash
set -euo pipefail

# Build and package Windows App for Arch Linux / Manjaro (.pkg.tar.zst)

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"

# Verify required development dependencies
source "${SCRIPT_DIR}/check-deps.sh"
check_dev_dependencies

OUTPUT_DIR="${ROOT_DIR}/target/packages"
ARCH_DIR="${ROOT_DIR}/packaging/arch"

echo "==> Building release binary..."
cd "${ROOT_DIR}"
cargo build --release

mkdir -p "${OUTPUT_DIR}"

if command -v makepkg >/dev/null 2>&1; then
    echo "==> Packaging with makepkg..."
    cd "${ARCH_DIR}"
    PKGDEST="${OUTPUT_DIR}" makepkg -f --nodeps
    echo "==> Successfully created Arch package in ${OUTPUT_DIR}/"
else
    echo "==> Staged Arch PKGBUILD at ${ARCH_DIR}/PKGBUILD."
    echo "    On an Arch / Manjaro system, run:"
    echo "    cd ${ARCH_DIR} && makepkg -si"
fi
