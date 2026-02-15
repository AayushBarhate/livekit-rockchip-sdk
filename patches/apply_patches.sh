#!/usr/bin/env bash
#
# apply_patches.sh -- Apply (or revert) Rockchip MPP patches to LiveKit rust-sdks
#
# Usage:
#   ./apply_patches.sh /path/to/rust-sdks            Apply all patches
#   ./apply_patches.sh --dry-run /path/to/rust-sdks   Check without applying
#   ./apply_patches.sh --reverse /path/to/rust-sdks   Revert all patches
#
# The script looks for patch files in the patches/ directory relative to its
# own location, applies them in sorted order, and prints a summary.

set -euo pipefail

# ---------------------------------------------------------------------------
# Resolve the directory where this script lives (and where patches/ is).
# ---------------------------------------------------------------------------
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PATCHES_DIR="${SCRIPT_DIR}/patches"

# ---------------------------------------------------------------------------
# Defaults
# ---------------------------------------------------------------------------
DRY_RUN=false
REVERSE=false
SDK_DIR=""

# ---------------------------------------------------------------------------
# Usage
# ---------------------------------------------------------------------------
usage() {
    cat <<EOF
Usage: $(basename "$0") [OPTIONS] <path-to-rust-sdks>

Apply Rockchip MPP patches to a LiveKit rust-sdks checkout.

Options:
  --dry-run     Only check whether patches can be applied; do not modify files.
  --reverse     Revert (unapply) the patches instead of applying them.
  -h, --help    Show this help message and exit.

Arguments:
  <path-to-rust-sdks>   Path to the root of the livekit/rust-sdks repository.

Examples:
  $(basename "$0") /home/user/rust-sdks
  $(basename "$0") --dry-run /home/user/rust-sdks
  $(basename "$0") --reverse /home/user/rust-sdks
EOF
    exit "${1:-0}"
}

# ---------------------------------------------------------------------------
# Parse arguments
# ---------------------------------------------------------------------------
while [[ $# -gt 0 ]]; do
    case "$1" in
        --dry-run)
            DRY_RUN=true
            shift
            ;;
        --reverse)
            REVERSE=true
            shift
            ;;
        -h|--help)
            usage 0
            ;;
        -*)
            echo "Error: Unknown option '$1'" >&2
            usage 1
            ;;
        *)
            if [[ -z "$SDK_DIR" ]]; then
                SDK_DIR="$1"
            else
                echo "Error: Unexpected argument '$1'" >&2
                usage 1
            fi
            shift
            ;;
    esac
done

if [[ -z "$SDK_DIR" ]]; then
    echo "Error: Path to rust-sdks is required." >&2
    usage 1
fi

# ---------------------------------------------------------------------------
# Validate the target directory
# ---------------------------------------------------------------------------
if [[ ! -d "$SDK_DIR" ]]; then
    echo "Error: Directory does not exist: $SDK_DIR" >&2
    exit 1
fi

if [[ ! -d "$SDK_DIR/webrtc-sys" ]]; then
    echo "Error: '$SDK_DIR' does not look like a LiveKit rust-sdks checkout." >&2
    echo "       Expected to find webrtc-sys/ subdirectory." >&2
    exit 1
fi

if [[ ! -d "$PATCHES_DIR" ]]; then
    echo "Error: Patches directory not found: $PATCHES_DIR" >&2
    exit 1
fi

# ---------------------------------------------------------------------------
# Collect patch files (sorted)
# ---------------------------------------------------------------------------
mapfile -t PATCH_FILES < <(find "$PATCHES_DIR" -maxdepth 1 -name '*.patch' -type f | sort)

if [[ ${#PATCH_FILES[@]} -eq 0 ]]; then
    echo "Error: No .patch files found in $PATCHES_DIR" >&2
    exit 1
fi

echo "========================================================"
if $REVERSE; then
    echo "  Reverting Rockchip MPP patches"
elif $DRY_RUN; then
    echo "  Dry-run: checking Rockchip MPP patches"
else
    echo "  Applying Rockchip MPP patches"
fi
echo "  Target: $SDK_DIR"
echo "  Patches: ${#PATCH_FILES[@]} file(s) from $PATCHES_DIR"
echo "========================================================"
echo ""

# ---------------------------------------------------------------------------
# Apply / check / revert each patch
# ---------------------------------------------------------------------------
APPLIED=0
SKIPPED=0
FAILED=0

for PATCH in "${PATCH_FILES[@]}"; do
    PATCH_NAME="$(basename "$PATCH")"

    if $REVERSE; then
        # --- Reverse mode ---
        echo -n "  [$PATCH_NAME] Checking reverse... "
        if git -C "$SDK_DIR" apply --check --reverse "$PATCH" 2>/dev/null; then
            if $DRY_RUN; then
                echo "can be reversed (dry-run, not reverting)"
                APPLIED=$((APPLIED + 1))
            else
                echo -n "reversing... "
                if git -C "$SDK_DIR" apply --reverse "$PATCH"; then
                    echo "OK"
                    APPLIED=$((APPLIED + 1))
                else
                    echo "FAILED"
                    FAILED=$((FAILED + 1))
                fi
            fi
        else
            echo "not applied or conflict (skipping)"
            SKIPPED=$((SKIPPED + 1))
        fi
    else
        # --- Forward (apply) mode ---
        echo -n "  [$PATCH_NAME] Checking... "
        if git -C "$SDK_DIR" apply --check "$PATCH" 2>/dev/null; then
            if $DRY_RUN; then
                echo "can be applied (dry-run, not applying)"
                APPLIED=$((APPLIED + 1))
            else
                echo -n "applying... "
                if git -C "$SDK_DIR" apply "$PATCH"; then
                    echo "OK"
                    APPLIED=$((APPLIED + 1))
                else
                    echo "FAILED"
                    FAILED=$((FAILED + 1))
                fi
            fi
        else
            # Check if already applied by testing reverse
            if git -C "$SDK_DIR" apply --check --reverse "$PATCH" 2>/dev/null; then
                echo "already applied (skipping)"
                SKIPPED=$((SKIPPED + 1))
            else
                echo "CONFLICT -- patch cannot be applied cleanly"
                FAILED=$((FAILED + 1))
            fi
        fi
    fi
done

# ---------------------------------------------------------------------------
# Summary
# ---------------------------------------------------------------------------
echo ""
echo "========================================================"
echo "  Summary"
echo "========================================================"
if $REVERSE; then
    ACTION="Reversed"
elif $DRY_RUN; then
    ACTION="Would apply"
else
    ACTION="Applied"
fi
echo "  $ACTION: $APPLIED"
echo "  Skipped: $SKIPPED"
echo "  Failed:  $FAILED"
echo "========================================================"

if [[ $FAILED -gt 0 ]]; then
    echo ""
    echo "Some patches failed. See docs/PATCHING_GUIDE.md for manual resolution."
    exit 1
fi

exit 0
