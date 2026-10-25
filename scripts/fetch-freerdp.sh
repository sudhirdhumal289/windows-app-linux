#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(dirname "$SCRIPT_DIR")"
TARGET_DIR="${ROOT_DIR}/freerdp-3.32"

echo "=== Fetching FreeRDP 3.32+ Bundle for Windows App Linux ==="

# Check if apt-get is available
if ! command -v apt-get &>/dev/null; then
    echo "Note: apt-get not found. On non-Debian distributions (Fedora, Arch), ensure system FreeRDP >= 3.30 is installed."
    exit 0
fi

TEMP_DL="$(mktemp -d /tmp/freerdp-fetch-XXXXXX)"
trap 'rm -rf "${TEMP_DL}"' EXIT

cd "${TEMP_DL}"
echo "Downloading FreeRDP 3.32 and dependencies from trixie-backports..."

apt-get download \
    freerdp3-dev=3.32.1+dfsg-1~bpo13+1 \
    libfreerdp-client3-3=3.32.1+dfsg-1~bpo13+1 \
    libfreerdp3-3=3.32.1+dfsg-1~bpo13+1 \
    libwinpr3-3=3.32.1+dfsg-1~bpo13+1 \
    libwinpr3-dev=3.32.1+dfsg-1~bpo13+1 \
    libwinpr-tools3-3=3.32.1+dfsg-1~bpo13+1 \
    libsndio7.0 \
    libsso-mib0=0.10.2-1~bpo13+1 2>/dev/null || {
    echo "Warning: Direct version download failed, attempting general package download..."
    apt-get download -t trixie-backports freerdp3-dev libfreerdp-client3-3 libfreerdp3-3 libwinpr3-3 libwinpr3-dev libwinpr-tools3-3 libsndio7.0 libsso-mib0 2>/dev/null || true
}

mkdir -p "${TARGET_DIR}"
echo "Extracting packages into ${TARGET_DIR}..."

for deb in *.deb; do
    if [ -f "$deb" ]; then
        dpkg -x "$deb" "${TEMP_DL}/extracted"
    fi
done

if [ -d "${TEMP_DL}/extracted/usr" ]; then
    cp -r "${TEMP_DL}/extracted/usr/"* "${TARGET_DIR}/"
    echo "FreeRDP 3.32+ bundle successfully installed into ${TARGET_DIR}"
    echo "Build script (build.rs) will automatically detect and link this bundle."
else
    echo "Error: No packages extracted successfully."
    exit 1
fi
