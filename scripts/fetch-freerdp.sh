#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(dirname "$SCRIPT_DIR")"
TARGET_DIR="${ROOT_DIR}/freerdp-3.32"

echo "=== Fetching FreeRDP 3.32+ Bundle for Windows App Linux ==="

# Check if target already exists and contains pkgconfig
if [ -f "${TARGET_DIR}/lib/x86_64-linux-gnu/pkgconfig/freerdp3.pc" ] || [ -f "${TARGET_DIR}/lib/pkgconfig/freerdp3.pc" ]; then
    echo "FreeRDP 3.32+ bundle already present in ${TARGET_DIR}."
    exit 0
fi

TEMP_DL="$(mktemp -d /tmp/freerdp-fetch-XXXXXX)"
trap 'rm -rf "${TEMP_DL}"' EXIT

cd "${TEMP_DL}"
echo "Downloading FreeRDP 3.32 and dependencies..."

DOWNLOAD_SUCCESS=0

# Attempt 1: apt-get download if apt-get is available
if command -v apt-get &>/dev/null; then
    if apt-get download \
        freerdp3-dev=3.32.1+dfsg-1~bpo13+1 \
        libfreerdp-client3-3=3.32.1+dfsg-1~bpo13+1 \
        libfreerdp3-3=3.32.1+dfsg-1~bpo13+1 \
        libwinpr3-3=3.32.1+dfsg-1~bpo13+1 \
        libwinpr3-dev=3.32.1+dfsg-1~bpo13+1 \
        libwinpr-tools3-3=3.32.1+dfsg-1~bpo13+1 \
        libsndio7.0 \
        libsso-mib0=0.10.2-1~bpo13+1 \
        libfuse3-4=3.17.2-3 2>/dev/null; then
        DOWNLOAD_SUCCESS=1
    elif apt-get download -t trixie-backports \
        freerdp3-dev libfreerdp-client3-3 libfreerdp3-3 libwinpr3-3 libwinpr3-dev libwinpr-tools3-3 libsndio7.0 libsso-mib0 libfuse3-4 2>/dev/null; then
        DOWNLOAD_SUCCESS=1
    fi
fi

# Attempt 2: Direct HTTP download from Debian archive mirror (works on any Linux distribution)
if [ "${DOWNLOAD_SUCCESS}" -eq 0 ]; then
    echo "Downloading packages directly from Debian mirror..."
    DEB_BASE="http://deb.debian.org/debian/pool/main"
    URLS=(
        "${DEB_BASE}/f/freerdp3/freerdp3-dev_3.32.1+dfsg-1~bpo13+1_amd64.deb"
        "${DEB_BASE}/f/freerdp3/libfreerdp-client3-3_3.32.1+dfsg-1~bpo13+1_amd64.deb"
        "${DEB_BASE}/f/freerdp3/libfreerdp3-3_3.32.1+dfsg-1~bpo13+1_amd64.deb"
        "${DEB_BASE}/f/freerdp3/libwinpr3-3_3.32.1+dfsg-1~bpo13+1_amd64.deb"
        "${DEB_BASE}/f/freerdp3/libwinpr3-dev_3.32.1+dfsg-1~bpo13+1_amd64.deb"
        "${DEB_BASE}/f/freerdp3/libwinpr-tools3-3_3.32.1+dfsg-1~bpo13+1_amd64.deb"
        "${DEB_BASE}/s/sndio/libsndio7.0_1.10.0-0.1_amd64.deb"
        "${DEB_BASE}/s/sso-mib/libsso-mib0_0.10.2-1~bpo13+1_amd64.deb"
        "${DEB_BASE}/f/fuse3/libfuse3-4_3.17.2-3_amd64.deb"
    )

    for url in "${URLS[@]}"; do
        filename="$(basename "$url")"
        echo "Fetching ${filename}..."
        if command -v curl &>/dev/null; then
            curl -fsSL --retry 3 -o "${filename}" "${url}"
        elif command -v wget &>/dev/null; then
            wget -q -O "${filename}" "${url}"
        else
            echo "Error: Neither curl nor wget found."
            exit 1
        fi
    done
    DOWNLOAD_SUCCESS=1
fi

mkdir -p "${TARGET_DIR}"
echo "Extracting packages into ${TARGET_DIR}..."

for deb in *.deb; do
    if [ -f "$deb" ]; then
        if command -v dpkg-deb &>/dev/null; then
            dpkg-deb -x "$deb" "${TEMP_DL}/extracted"
        elif command -v dpkg &>/dev/null; then
            dpkg -x "$deb" "${TEMP_DL}/extracted"
        elif command -v ar &>/dev/null; then
            ar x "$deb" data.tar.xz 2>/dev/null || ar x "$deb" data.tar.gz 2>/dev/null || ar x "$deb" data.tar.zst 2>/dev/null
            if [ -f data.tar.xz ]; then
                tar -xf data.tar.xz -C "${TEMP_DL}/extracted"
                rm -f data.tar.xz
            elif [ -f data.tar.gz ]; then
                tar -xf data.tar.gz -C "${TEMP_DL}/extracted"
                rm -f data.tar.gz
            fi
        fi
    fi
done

if [ -d "${TEMP_DL}/extracted/usr" ]; then
    cp -r "${TEMP_DL}/extracted/usr/"* "${TARGET_DIR}/"
    # Merged-usr fallback in case libraries were placed in /lib
    if [ -d "${TEMP_DL}/extracted/lib" ]; then
        cp -r "${TEMP_DL}/extracted/lib/"* "${TARGET_DIR}/lib/" 2>/dev/null || true
    fi
    echo "FreeRDP 3.32+ bundle successfully installed into ${TARGET_DIR}"
    echo "Build script (build.rs) will automatically detect and link this bundle."
else
    echo "Error: No packages extracted successfully."
    exit 1
fi
