# Standalone Patches

This directory preserves the original patch files and scripts from before this
repo became a full fork. They are kept here for reference and in case you need
to re-apply them to a fresh upstream checkout.

## Contents

- `001-build-rs-rockchip.patch` — `build.rs`: link rockchip_mpp, define `USE_ROCKCHIP_MPP_VIDEO_CODEC`, add `-fpermissive`
- `002-encoder-factory.patch` — `video_encoder_factory.cpp`: register `RockchipVideoEncoderFactory`
- `003-decoder-factory.patch` — `video_decoder_factory.cpp`: register `RockchipVideoDecoderFactory`
- `004-workspace-video-call.patch` — `Cargo.toml`: add `examples/video_call` to workspace
- `apply_patches.sh` — Script to apply/revert patches against a vanilla `livekit/rust-sdks` checkout
- `start_meet.sh` / `test_mpp_registration.sh` — Helper scripts
- `docs/` — Patching guide, supported versions, troubleshooting
- `ORIGINAL_README.md` — The README from the original patch-based repo

## Using the patches standalone

If you prefer to patch upstream yourself instead of using this fork:

```bash
git clone https://github.com/livekit/rust-sdks
cd rust-sdks
git checkout 9e635aa6
/path/to/patches/apply_patches.sh $(pwd)
```
