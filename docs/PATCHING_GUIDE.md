# Patching Guide

How to reapply the Rockchip MPP patches when LiveKit releases a new version of
their Rust SDK.

## Overview

The patches in this repository are intentionally small (8-10 lines each) and
follow the exact same pattern that LiveKit already uses for NVIDIA and VAAPI
hardware video codecs. When upstream changes the files we patch, the resolution
is usually straightforward.

## Step 1: Check if patches apply cleanly

```bash
# Clone (or pull) the latest LiveKit SDK
git clone https://github.com/livekit/rust-sdks /tmp/rust-sdks-new
cd /tmp/rust-sdks-new

# Dry-run the patches
/path/to/livekit-rockchip-sdk/apply_patches.sh --dry-run /tmp/rust-sdks-new
```

If the output shows all patches as "can be applied", you are done. Update
`docs/SUPPORTED_VERSIONS.md` with the new commit hash and move on.

## Step 2: If patches conflict

When `--dry-run` reports a conflict, you need to reapply the changes manually.
This is easy because each patch adds only a small, self-contained block.

### 2a. Understand what changed upstream

```bash
cd /tmp/rust-sdks-new
# See what changed in the files we patch
git log --oneline -20 -- webrtc-sys/build.rs
git log --oneline -20 -- webrtc-sys/src/video_encoder_factory.cpp
git log --oneline -20 -- webrtc-sys/src/video_decoder_factory.cpp
```

### 2b. Reapply `001-build-rs-rockchip.patch` (build.rs)

Open `webrtc-sys/build.rs` and find the section that handles Linux linking. Look
for the NVIDIA block -- it will contain something like:

```rust
// NVIDIA
if nvidia {
    println!("cargo:rustc-link-lib=nvidia-encode");
    builder.flag("-DUSE_NVIDIA_VIDEO_CODEC=1");
}
```

Add the Rockchip block **after** the NVIDIA block (or after the VAAPI block if
it exists):

```rust
// RK3588 MPP hardware video codec (ARM64 only)
if arm {
    println!("cargo:rustc-link-lib=rockchip_mpp");
    builder.flag("-DUSE_ROCKCHIP_MPP_VIDEO_CODEC=1");
}
```

Also ensure these flags are present in the builder chain:

```rust
builder
    .flag("-Wno-changes-meaning")
    .flag("-Wno-deprecated-declarations")
    .flag("-fpermissive")       // <-- add this if not present
    .flag("-std=c++20");
```

### 2c. Reapply `002-encoder-factory.patch` (video_encoder_factory.cpp)

Open `webrtc-sys/src/video_encoder_factory.cpp`.

1. Find the NVIDIA include block near the top:
   ```cpp
   #if defined(USE_NVIDIA_VIDEO_CODEC)
   #include "nvidia/nvidia_encoder_factory.h"
   #endif
   ```

2. Add the Rockchip include **after** it:
   ```cpp
   #if defined(USE_ROCKCHIP_MPP_VIDEO_CODEC)
   #include "modules/video_coding/codecs/mpp/rockchip_video_encoder_factory.h"
   #endif
   ```

3. Find the `InternalFactory::InternalFactory()` constructor. Look for where
   NVIDIA pushes its factory:
   ```cpp
   #if defined(USE_NVIDIA_VIDEO_CODEC)
     }
   #endif
   ```

4. Add the Rockchip factory push **after** that closing block:
   ```cpp
   #if defined(USE_ROCKCHIP_MPP_VIDEO_CODEC)
     factories_.push_back(std::make_unique<webrtc::RockchipVideoEncoderFactory>());
   #endif
   ```

### 2d. Reapply `003-decoder-factory.patch` (video_decoder_factory.cpp)

Open `webrtc-sys/src/video_decoder_factory.cpp`. The pattern is identical to
the encoder:

1. Add the include after the NVIDIA include block:
   ```cpp
   #if defined(USE_ROCKCHIP_MPP_VIDEO_CODEC)
   #include "modules/video_coding/codecs/mpp/rockchip_video_decoder_factory.h"
   #endif
   ```

2. Add the factory push after the NVIDIA factory push:
   ```cpp
   #if defined(USE_ROCKCHIP_MPP_VIDEO_CODEC)
     factories_.push_back(std::make_unique<webrtc::RockchipVideoDecoderFactory>());
   #endif
   ```

## Step 3: Regenerate the patch files

After making the manual changes, regenerate the patches from the git diff:

```bash
cd /tmp/rust-sdks-new

# Stage nothing -- we want unstaged diffs
git diff -- webrtc-sys/build.rs > /path/to/livekit-rockchip-sdk/patches/001-build-rs-rockchip.patch
git diff -- webrtc-sys/src/video_encoder_factory.cpp > /path/to/livekit-rockchip-sdk/patches/002-encoder-factory.patch
git diff -- webrtc-sys/src/video_decoder_factory.cpp > /path/to/livekit-rockchip-sdk/patches/003-decoder-factory.patch
```

## Step 4: Test the build

```bash
cd /tmp/rust-sdks-new
export LK_CUSTOM_WEBRTC=/path/to/webrtc-rockchip-mpp/artifacts
cargo build -p webrtc-sys
```

If the build succeeds:

1. Update `docs/SUPPORTED_VERSIONS.md` with the new commit.
2. Commit the updated patches and version table.
3. Push and verify the CI workflow passes.

## Step 5: Verify on hardware

On an RK3588 board with MPP installed:

```bash
RUST_LOG=debug cargo run -p basic_room -- --url wss://... --token ... 2>&1 | grep -i rockchip
```

You should see:
```
RockchipVideoEncoderFactory created
RockchipVideoDecoderFactory created
```

## Tips

- The patches always go **after** the NVIDIA block. If upstream adds new
  hardware backends (e.g., Qualcomm), our block stays in the same relative
  position -- after NVIDIA/VAAPI, before the closing brace.

- If upstream renames `InternalFactory` or restructures the factory vector,
  look at how NVIDIA is registered and follow the same pattern.

- The `USE_ROCKCHIP_MPP_VIDEO_CODEC` define is the single compile-time gate.
  As long as it is defined when compiling the C++ files and the MPP library
  is linked, the factories will be registered.
