#!/usr/bin/env bash
#
# test_mpp_registration.sh -- Smoke test for Rockchip MPP factory registration
#
# Verifies that the LiveKit SDK binary registers the Rockchip video encoder
# and decoder factories at startup.
#
# Usage:
#   ./test_mpp_registration.sh                          # Build and test basic_room
#   ./test_mpp_registration.sh /path/to/basic_room      # Test an existing binary
#
# The script runs the binary with dummy credentials and a short timeout, then
# checks the log output for factory registration messages.

set -euo pipefail

# ---------------------------------------------------------------------------
# Configuration
# ---------------------------------------------------------------------------
TIMEOUT_SECONDS=5
EXPECTED_ENCODER="RockchipVideoEncoderFactory"
EXPECTED_DECODER="RockchipVideoDecoderFactory"

# Dummy credentials -- the connection will fail, but factories are registered
# before the connection attempt.
DUMMY_URL="wss://dummy.livekit.cloud"
DUMMY_TOKEN="eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiJ0ZXN0In0.dummy"

# ---------------------------------------------------------------------------
# Parse arguments
# ---------------------------------------------------------------------------
BINARY=""

usage() {
    cat <<EOF
Usage: $(basename "$0") [PATH_TO_BINARY]

Smoke test for Rockchip MPP video codec factory registration.

Arguments:
  PATH_TO_BINARY   Path to a built LiveKit binary (e.g., basic_room).
                   If omitted, attempts to build basic_room from the
                   current Cargo workspace.

Environment:
  LK_CUSTOM_WEBRTC   Required if building from source.
  RUST_SDK_DIR        Path to LiveKit rust-sdks (for building). Defaults
                      to the current directory.

Examples:
  $(basename "$0")
  $(basename "$0") ./target/debug/basic_room
  $(basename "$0") /home/user/rust-sdks/target/release/basic_room
EOF
    exit "${1:-0}"
}

case "${1:-}" in
    -h|--help)
        usage 0
        ;;
    "")
        ;;
    *)
        BINARY="$1"
        ;;
esac

# ---------------------------------------------------------------------------
# Find or build the binary
# ---------------------------------------------------------------------------
if [[ -n "$BINARY" ]]; then
    if [[ ! -x "$BINARY" ]]; then
        echo "Error: Binary not found or not executable: $BINARY" >&2
        exit 1
    fi
    echo "Using provided binary: $BINARY"
else
    SDK_DIR="${RUST_SDK_DIR:-$(pwd)}"
    echo "No binary specified. Attempting to build basic_room in $SDK_DIR ..."

    if [[ ! -f "$SDK_DIR/Cargo.toml" ]]; then
        echo "Error: No Cargo.toml found in $SDK_DIR" >&2
        echo "       Set RUST_SDK_DIR or pass the binary path as an argument." >&2
        exit 1
    fi

    (cd "$SDK_DIR" && cargo build -p basic_room 2>&1)
    BINARY="$SDK_DIR/target/debug/basic_room"

    if [[ ! -x "$BINARY" ]]; then
        echo "Error: Build succeeded but binary not found at $BINARY" >&2
        exit 1
    fi
    echo "Built: $BINARY"
fi

# ---------------------------------------------------------------------------
# Run the binary and capture output
# ---------------------------------------------------------------------------
echo ""
echo "Running smoke test (timeout: ${TIMEOUT_SECONDS}s)..."
echo "  Binary: $BINARY"
echo "  URL: $DUMMY_URL (dummy -- connection will fail, that is expected)"
echo ""

LOGFILE=$(mktemp /tmp/mpp_registration_test.XXXXXX.log)
trap "rm -f '$LOGFILE'" EXIT

# Run with debug logging, timeout after N seconds. We expect the process to
# fail (bad credentials), so we ignore the exit code.
set +e
timeout "$TIMEOUT_SECONDS" env RUST_LOG=debug \
    "$BINARY" --url "$DUMMY_URL" --token "$DUMMY_TOKEN" \
    >"$LOGFILE" 2>&1
EXIT_CODE=$?
set -e

# timeout returns 124 when the process is killed by timeout -- that is fine
if [[ $EXIT_CODE -ne 0 && $EXIT_CODE -ne 124 ]]; then
    echo "  (Binary exited with code $EXIT_CODE -- this is expected for dummy credentials)"
fi

# ---------------------------------------------------------------------------
# Check for factory registration
# ---------------------------------------------------------------------------
echo ""
echo "========================================================"
echo "  Test Results"
echo "========================================================"

PASS=true

echo -n "  Encoder factory: "
if grep -q "$EXPECTED_ENCODER" "$LOGFILE"; then
    echo "PASS -- '$EXPECTED_ENCODER' found in output"
else
    echo "FAIL -- '$EXPECTED_ENCODER' NOT found in output"
    PASS=false
fi

echo -n "  Decoder factory: "
if grep -q "$EXPECTED_DECODER" "$LOGFILE"; then
    echo "PASS -- '$EXPECTED_DECODER' found in output"
else
    echo "FAIL -- '$EXPECTED_DECODER' NOT found in output"
    PASS=false
fi

echo "========================================================"

if $PASS; then
    echo ""
    echo "  OVERALL: PASS"
    echo ""
    echo "  Rockchip MPP factories are registered correctly."
    exit 0
else
    echo ""
    echo "  OVERALL: FAIL"
    echo ""
    echo "  The Rockchip MPP factories were not detected in the log output."
    echo "  Possible causes:"
    echo "    - Patches were not applied before building"
    echo "    - USE_ROCKCHIP_MPP_VIDEO_CODEC was not defined at compile time"
    echo "    - librockchip_mpp.so is not installed on this system"
    echo ""
    echo "  Full log output saved to: $LOGFILE"
    echo "  (First 30 lines shown below)"
    echo ""
    head -30 "$LOGFILE"
    # Keep the logfile on failure (disable the trap)
    trap - EXIT
    exit 1
fi
