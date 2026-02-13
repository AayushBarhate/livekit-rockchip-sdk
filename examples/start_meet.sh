#!/bin/bash
# Convenience launcher for the video_call binary on RK3588.
# Place this in your home directory and set the environment variables below.
#
# Usage: ./start_meet.sh [--camera-index 1] [--width 1280] [--height 720]

export LIVEKIT_URL="${LIVEKIT_URL:?Set LIVEKIT_URL}"
export LIVEKIT_API_KEY="${LIVEKIT_API_KEY:?Set LIVEKIT_API_KEY}"
export LIVEKIT_API_SECRET="${LIVEKIT_API_SECRET:?Set LIVEKIT_API_SECRET}"

exec ~/rust-sdks/target/release/video_call "$@"
