#!/usr/bin/env bash
set -euo pipefail

# Build and package Windows App into an RPM package (Fedora, RHEL, CentOS Stream, Rocky, Alma)

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"

# Verify required development dependencies
if [ -f "${SCRIPT_DIR}/check-deps.sh" ]; then
    source "${SCRIPT_DIR}/check-deps.sh"
    check_dev_dependencies || true
fi

VERSION="$(grep '^version' "${ROOT_DIR}/Cargo.toml" | head -n 1 | cut -d '"' -f 2)"
OUTPUT_DIR="${ROOT_DIR}/target/packages"
RPM_TOPDIR="${ROOT_DIR}/target/rpmbuild"

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

rm -rf "${RPM_TOPDIR}"
mkdir -p "${RPM_TOPDIR}/SOURCES"
mkdir -p "${RPM_TOPDIR}/SPECS"
mkdir -p "${RPM_TOPDIR}/RPMS"
mkdir -p "${RPM_TOPDIR}/SRPMS"
mkdir -p "${RPM_TOPDIR}/BUILD"
mkdir -p "${RPM_TOPDIR}/BUILDROOT"
mkdir -p "${OUTPUT_DIR}"

cp "${BIN_SRC}" "${RPM_TOPDIR}/SOURCES/windows-app"
cp "${ROOT_DIR}/assets/com.microsoft.windowsapp.linux.desktop" "${RPM_TOPDIR}/SOURCES/"
cp "${ROOT_DIR}/assets/com.microsoft.windowsapp.linux.svg" "${RPM_TOPDIR}/SOURCES/"
cp "${ROOT_DIR}/assets/com.microsoft.windowsapp.linux.search-provider.ini" "${RPM_TOPDIR}/SOURCES/"
cp "${ROOT_DIR}/assets/com.microsoft.windowsapp.linux.service" "${RPM_TOPDIR}/SOURCES/"
cp -r "${ROOT_DIR}/resources/icons" "${RPM_TOPDIR}/SOURCES/"
cp -r "${ROOT_DIR}/assets/gnome-shell-extension" "${RPM_TOPDIR}/SOURCES/"

SPEC_SRC="${ROOT_DIR}/packaging/rpm/windows-app.spec"
[ -f "${SPEC_SRC}" ] || SPEC_SRC="${ROOT_DIR}/packaging/rpm/windows-app-linux.spec"
cp "${SPEC_SRC}" "${RPM_TOPDIR}/SPECS/windows-app.spec"

if command -v rpmbuild >/dev/null 2>&1; then
    echo "==> Packaging RPM with rpmbuild..."
    rpmbuild --define "_topdir ${RPM_TOPDIR}" -bb "${RPM_TOPDIR}/SPECS/windows-app.spec"
    cp "${RPM_TOPDIR}"/RPMS/*/*.rpm "${OUTPUT_DIR}/"
    echo "==> Successfully created RPM package(s) in ${OUTPUT_DIR}/"
else
    echo "==> Staged RPM build sources in ${RPM_TOPDIR}."
    echo "    To build, install rpmbuild (e.g. sudo apt install rpm or sudo dnf install rpm-build):"
    echo "    rpmbuild --define \"_topdir ${RPM_TOPDIR}\" -bb ${RPM_TOPDIR}/SPECS/windows-app.spec"
fi
