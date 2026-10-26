#!/usr/bin/env bash
set -euo pipefail

# Checks for required build and development dependencies across supported distributions

check_dev_dependencies() {
    # Check pkg-config availability
    if ! command -v pkg-config >/dev/null 2>&1; then
        echo "Error: pkg-config is not installed."
        echo "Please install pkg-config using your package manager:"
        echo "  - Debian/Ubuntu:     sudo apt install -y pkg-config"
        echo "  - Fedora/RHEL:       sudo dnf install -y pkgconfig"
        echo "  - Arch/Manjaro:      sudo pacman -S --needed pkgconf"
        echo "  - openSUSE:          sudo zypper install -y pkg-config"
        echo "  - macOS (Homebrew):  brew install pkg-config"
        exit 1
    fi

    local missing=()
    for pkg in gtk4 libadwaita-1 webkitgtk-6.0 libsoup-3.0 freerdp3 freerdp-client3 winpr3; do
        if ! pkg-config --exists "$pkg" 2>/dev/null; then
            missing+=("$pkg")
        fi
    done

    if [ ${#missing[@]} -ne 0 ]; then
        echo "=========================================================================="
        echo "Error: Missing required development libraries: ${missing[*]}"
        echo "=========================================================================="
        echo "Please install development headers on your system before building packages:"
        echo ""
        echo "  Debian / Ubuntu / Linux Mint:"
        echo "    sudo apt install -y build-essential pkg-config libgtk-4-dev libadwaita-1-dev libwebkitgtk-6.0-dev libsoup-3.0-dev freerdp3-dev libwinpr3-dev libfuse3-dev"
        echo ""
        echo "  Fedora / RHEL / CentOS Stream:"
        echo "    sudo dnf install -y gcc gcc-c++ pkgconfig gtk4-devel libadwaita-devel webkitgtk6.0-devel libsoup3-devel freerdp-devel winpr-devel"
        echo ""
        echo "  Arch Linux / Manjaro:"
        echo "    sudo pacman -S --needed base-devel pkgconf gtk4 libadwaita webkitgtk-6.0 libsoup3 freerdp"
        echo ""
        echo "  openSUSE (Tumbleweed / Leap):"
        echo "    sudo zypper install -y patterns-devel-base-devel_basis pkg-config gtk4-devel libadwaita-devel webkit2gtk-6_0-devel libsoup-devel freerdp-devel"
        echo ""
        echo "  macOS (Homebrew):"
        echo "    brew install gtk4 libadwaita webkit2png pkg-config freerdp"
        echo "=========================================================================="
        exit 1
    fi
}

if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
    check_dev_dependencies
    echo "All required development libraries are present."
fi
