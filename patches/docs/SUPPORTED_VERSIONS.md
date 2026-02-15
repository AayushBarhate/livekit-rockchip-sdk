# Supported Versions

This document tracks which versions of the LiveKit Rust SDK have been tested
with the Rockchip MPP patches.

## Tested Versions

| LiveKit SDK Commit | Date | Status | Notes |
|---|---|---|---|
| `9e635aa6` | Feb 2026 | Tested | Initial integration |

## Compatibility Checks

The [check-upstream.yml](../.github/workflows/check-upstream.yml) workflow runs
every Monday at 9:00 AM UTC and tests whether the patches still apply cleanly
to the latest `main` branch of
[livekit/rust-sdks](https://github.com/livekit/rust-sdks). If they do not, it
automatically creates a GitHub issue.

## How to Add a New Version

When you have verified that the patches work with a new version of the LiveKit
SDK:

1. **Test the patches:**
   ```bash
   git clone https://github.com/livekit/rust-sdks /tmp/rust-sdks
   cd /tmp/rust-sdks
   git checkout <new-commit-hash>
   /path/to/livekit-rockchip-sdk/apply_patches.sh /tmp/rust-sdks
   ```

2. **Build and verify:**
   ```bash
   export LK_CUSTOM_WEBRTC=/path/to/webrtc-rockchip-mpp/artifacts
   cargo build -p webrtc-sys
   ```

3. **Test on hardware (if possible):**
   ```bash
   RUST_LOG=debug cargo run -p basic_room -- --url wss://... --token ...
   # Verify "RockchipVideoEncoderFactory created" appears in logs
   ```

4. **Update this table:** Add a new row with the commit hash, date, and status.

5. **If patches needed updating:** Follow the steps in
   [PATCHING_GUIDE.md](PATCHING_GUIDE.md) and commit the updated patch files
   along with this table update.

## Status Legend

| Status | Meaning |
|---|---|
| **Tested** | Patches apply, build succeeds, verified on RK3588 hardware |
| **Build OK** | Patches apply and build succeeds, but not tested on hardware |
| **Patches OK** | Patches apply cleanly, build not attempted |
| **Conflict** | Patches do not apply cleanly, manual rebase needed |
