#!/usr/bin/env bash
set -euo pipefail

# Build and package Windows App into an RPM package (Fedora, RHEL, CentOS Stream)

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"

# Verify required development dependencies
source "${SCRIPT_DIR}/check-deps.sh"
check_dev_dependencies

VERSION="$(grep '^version' "${ROOT_DIR}/Cargo.toml" | head -n 1 | cut -d '"' -f 2)"
OUTPUT_DIR="${ROOT_DIR}/target/packages"
RPM_TOPDIR="${ROOT_DIR}/target/rpmbuild"

echo "==> Building release binary..."
cd "${ROOT_DIR}"
cargo build --release

BIN_SRC="${ROOT_DIR}/target/release/windows-app-linux"
if [ ! -f "${BIN_SRC}" ]; then
    echo "Error: Release binary ${BIN_SRC} not found!"
    exit 1
fi

rm -rf "${RPM_TOPDIR}"
mkdir -p "${RPM_TOPDIR}/SOURCES"
mkdir -p "${RPM_TOPDIR}/SPECS"
mkdir -p "${RPM_TOPDIR}/RPMS"
mkdir -p "${RPM_TOPDIR}/SRPMS"
mkdir -p "${RPM_TOPDIR}/BUILD"
mkdir -p "${RPM_TOPDIR}/BUILDROOT"
mkdir -p "${OUTPUT_DIR}"

cp "${BIN_SRC}" "${RPM_TOPDIR}/SOURCES/"
cp "${ROOT_DIR}/assets/com.microsoft.windowsapp.linux.desktop" "${RPM_TOPDIR}/SOURCES/"
cp "${ROOT_DIR}/assets/com.microsoft.windowsapp.linux.svg" "${RPM_TOPDIR}/SOURCES/"
cp "${ROOT_DIR}/assets/com.microsoft.windowsapp.linux.search-provider.ini" "${RPM_TOPDIR}/SOURCES/"
cp "${ROOT_DIR}/assets/com.microsoft.windowsapp.linux.service" "${RPM_TOPDIR}/SOURCES/"
cp -r "${ROOT_DIR}/resources/icons" "${RPM_TOPDIR}/SOURCES/"
cp "${ROOT_DIR}/packaging/rpm/windows-app-linux.spec" "${RPM_TOPDIR}/SPECS/"

if command -v rpmbuild >/dev/null 2>&1; then
    echo "==> Packaging RPM with rpmbuild..."
    rpmbuild --define "_topdir ${RPM_TOPDIR}" -bb "${RPM_TOPDIR}/SPECS/windows-app-linux.spec"
    cp "${RPM_TOPDIR}"/RPMS/*/*.rpm "${OUTPUT_DIR}/"
    echo "==> Successfully created RPM package(s) in ${OUTPUT_DIR}/"
else
    echo "==> Staged RPM build sources in ${RPM_TOPDIR}."
    echo "    On an RPM distribution (Fedora / RHEL), run:"
    echo "    rpmbuild --define \"_topdir ${RPM_TOPDIR}\" -bb ${RPM_TOPDIR}/SPECS/windows-app-linux.spec"
fi
