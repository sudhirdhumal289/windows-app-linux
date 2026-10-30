#!/usr/bin/env bash
set -euo pipefail

# Bump application version across all project descriptors, commit, tag, and push.
#
# Usage:
#   ./scripts/bump-version.sh [patch|minor|major|<specific-version>] [--no-push] [--dry-run]
#
# Examples:
#   ./scripts/bump-version.sh               # Bumps patch version (e.g. 0.2.5 -> 0.2.6), tags, and pushes
#   ./scripts/bump-version.sh minor         # Bumps minor version (e.g. 0.2.5 -> 0.3.0), tags, and pushes
#   ./scripts/bump-version.sh 0.3.0         # Sets version to 0.3.0, tags, and pushes
#   ./scripts/bump-version.sh patch --no-push # Bumps patch version and tags without pushing

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
cd "${ROOT_DIR}"

MODE="patch"
DO_PUSH=true
DRY_RUN=false

for arg in "$@"; do
    case "$arg" in
        --no-push)
            DO_PUSH=false
            ;;
        --push)
            DO_PUSH=true
            ;;
        --dry-run)
            DRY_RUN=true
            ;;
        -h|--help)
            echo "Usage: $0 [patch|minor|major|<version>] [--no-push] [--dry-run]"
            exit 0
            ;;
        patch|minor|major)
            MODE="$arg"
            ;;
        v*|[0-9]*)
            MODE="${arg#v}"
            ;;
        *)
            echo "Error: Unknown argument '$arg'" >&2
            echo "Usage: $0 [patch|minor|major|<version>] [--no-push] [--dry-run]" >&2
            exit 1
            ;;
    esac
done

# Ensure git working tree is clean
if ! git diff-index --quiet HEAD --; then
    echo "Error: Git working directory has uncommitted changes. Please commit or stash them first." >&2
    exit 1
fi

CURRENT_VERSION="$(grep '^version' "${ROOT_DIR}/Cargo.toml" | head -n 1 | cut -d '"' -f 2)"
if [ -z "${CURRENT_VERSION}" ]; then
    echo "Error: Could not extract current version from Cargo.toml" >&2
    exit 1
fi

IFS='.' read -r MAJOR MINOR PATCH <<< "${CURRENT_VERSION}"

case "$MODE" in
    patch)
        NEW_PATCH=$((PATCH + 1))
        NEW_VERSION="${MAJOR}.${MINOR}.${NEW_PATCH}"
        ;;
    minor)
        NEW_MINOR=$((MINOR + 1))
        NEW_VERSION="${MAJOR}.${NEW_MINOR}.0"
        ;;
    major)
        NEW_MAJOR=$((MAJOR + 1))
        NEW_VERSION="${NEW_MAJOR}.0.0"
        ;;
    *)
        NEW_VERSION="$MODE"
        ;;
esac

TAG="v${NEW_VERSION}"

echo "=================================================="
echo "  Current version: ${CURRENT_VERSION}"
echo "  New version:     ${NEW_VERSION}"
echo "  Tag:             ${TAG}"
echo "  Push to origin:  ${DO_PUSH}"
echo "  Dry run:         ${DRY_RUN}"
echo "=================================================="

if [ "$DRY_RUN" = true ]; then
    echo "[dry-run] Completed preview. No changes were made."
    exit 0
fi

# Update version across all manifest files
python3 -c "
import sys

current_ver = '${CURRENT_VERSION}'
new_ver = '${NEW_VERSION}'

def replace_in_file(path, old, new, count=None):
    with open(path, 'r', encoding='utf-8') as f:
        content = f.read()
    if old not in content:
        print(f'Warning: Pattern {old!r} not found in {path}', file=sys.stderr)
        return
    if count is None:
        updated = content.replace(old, new)
    else:
        updated = content.replace(old, new, count)
    with open(path, 'w', encoding='utf-8') as f:
        f.write(updated)
    print(f'  Updated {path}')

# 1. Cargo.toml
replace_in_file('Cargo.toml', f'version = \"{current_ver}\"', f'version = \"{new_ver}\"', 1)

# 2. packaging/arch/PKGBUILD
replace_in_file('packaging/arch/PKGBUILD', f'pkgver={current_ver}', f'pkgver={new_ver}', 1)

# 3. packaging/macos/Info.plist
replace_in_file('packaging/macos/Info.plist', f'<string>{current_ver}</string>', f'<string>{new_ver}</string>')

# 4. packaging/rpm/windows-app-opensuse.spec
replace_in_file('packaging/rpm/windows-app-opensuse.spec', f'Version:        {current_ver}', f'Version:        {new_ver}', 1)

# 5. packaging/rpm/windows-app.spec
replace_in_file('packaging/rpm/windows-app.spec', f'Version:        {current_ver}', f'Version:        {new_ver}', 1)

# 6. packaging/windows/installer.iss
replace_in_file('packaging/windows/installer.iss', f'#define AppVersion \"{current_ver}\"', f'#define AppVersion \"{new_ver}\"', 1)
"

# Update Cargo.lock using cargo check
echo "==> Updating Cargo.lock..."
cargo check --quiet

# Stage updated files
echo "==> Staging changed files..."
git add \
    Cargo.toml \
    Cargo.lock \
    packaging/arch/PKGBUILD \
    packaging/macos/Info.plist \
    packaging/rpm/windows-app-opensuse.spec \
    packaging/rpm/windows-app.spec \
    packaging/windows/installer.iss

# Commit
COMMIT_MSG="chore: bump version to ${NEW_VERSION}"
echo "==> Creating commit: ${COMMIT_MSG}"
git commit -m "${COMMIT_MSG}"

# Create git tag
echo "==> Creating tag: ${TAG}"
git tag "${TAG}"

# Push if requested
if [ "$DO_PUSH" = true ]; then
    CURRENT_BRANCH="$(git branch --show-current)"
    echo "==> Pushing ${CURRENT_BRANCH} and ${TAG} to origin..."
    git push origin "${CURRENT_BRANCH}" "${TAG}"
fi

echo "==> Successfully bumped version to ${NEW_VERSION} and created tag ${TAG}!"
