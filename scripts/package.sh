#!/usr/bin/env bash
set -euo pipefail

# Package Windows App for Linux and macOS distributions
# Usage: ./scripts/package.sh [deb|rpm|opensuse|arch|macos|all]

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
TARGET="${1:-auto}"

detect_target() {
    if [[ "$OSTYPE" == "darwin"* ]]; then
        echo "macos"
        return
    fi

    if [ -f /etc/os-release ]; then
        # shellcheck disable=SC1091
        source /etc/os-release
        case "${ID:-}" in
            ubuntu|debian|linuxmint|pop)
                echo "deb"
                return
                ;;
            fedora|rhel|centos|rocky|alma)
                echo "rpm"
                return
                ;;
            opensuse*|suse)
                echo "opensuse"
                return
                ;;
            arch|manjaro|endeavouros)
                echo "arch"
                return
                ;;
        esac

        # Check ID_LIKE fallback
        for like in ${ID_LIKE:-}; do
            case "${like}" in
                debian|ubuntu)
                    echo "deb"
                    return
                    ;;
                fedora|rhel)
                    echo "rpm"
                    return
                    ;;
                suse)
                    echo "opensuse"
                    return
                    ;;
                arch)
                    echo "arch"
                    return
                    ;;
            esac
        done
    fi

    echo "deb"
}

if [ "${TARGET}" = "auto" ]; then
    TARGET="$(detect_target)"
    echo "==> Auto-detected platform target: ${TARGET}"
fi

case "${TARGET}" in
    deb)
        "${SCRIPT_DIR}/package-deb.sh"
        ;;
    rpm)
        "${SCRIPT_DIR}/package-rpm.sh"
        ;;
    opensuse)
        "${SCRIPT_DIR}/package-opensuse.sh"
        ;;
    arch)
        "${SCRIPT_DIR}/package-arch.sh"
        ;;
    macos)
        "${SCRIPT_DIR}/package-macos.sh"
        ;;
    all)
        echo "==> Packaging all supported targets..."
        "${SCRIPT_DIR}/package-deb.sh"
        "${SCRIPT_DIR}/package-rpm.sh"
        "${SCRIPT_DIR}/package-opensuse.sh"
        "${SCRIPT_DIR}/package-arch.sh"
        "${SCRIPT_DIR}/package-macos.sh"
        ;;
    *)
        echo "Usage: $0 [deb|rpm|opensuse|arch|macos|all]"
        exit 1
        ;;
esac
