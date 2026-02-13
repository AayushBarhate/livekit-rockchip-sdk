# LiveKit Rust SDK - Rockchip RK3588 MPP Integration

<!-- Badges -->
[![Build LiveKit SDK with Rockchip MPP](https://github.com/user/livekit-rockchip-sdk/actions/workflows/build.yml/badge.svg)](https://github.com/user/livekit-rockchip-sdk/actions/workflows/build.yml)
[![Check LiveKit SDK Compatibility](https://github.com/user/livekit-rockchip-sdk/actions/workflows/check-upstream.yml/badge.svg)](https://github.com/user/livekit-rockchip-sdk/actions/workflows/check-upstream.yml)
[![License](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)

Patches for [livekit/rust-sdks](https://github.com/livekit/rust-sdks) that enable
**RK3588 hardware H.264 encoding and decoding** via Rockchip's Media Process Platform
(MPP) library. These patches wire Rockchip's video codec factories into LiveKit's
`InternalFactory`, following the exact same pattern used for NVIDIA and VAAPI hardware
acceleration.

## What This Does

LiveKit's `webrtc-sys` crate already supports pluggable hardware video codec factories.
On Linux it checks for NVIDIA and VAAPI at compile time. This patch set adds a third
hardware backend -- **Rockchip MPP** -- so that RK3588 boards can offload H.264
encode/decode to the on-chip VPU.

Specifically, the patches:

1. **Add `RockchipVideoEncoderFactory`** to the encoder factory chain in
   `video_encoder_factory.cpp`, guarded by `USE_ROCKCHIP_MPP_VIDEO_CODEC`.
2. **Add `RockchipVideoDecoderFactory`** to the decoder factory chain in
   `video_decoder_factory.cpp`, using the same guard.
3. **Update `build.rs`** to link `librockchip_mpp.so`, define the compile-time flag,
   and add `-fpermissive` to suppress a template name-lookup diagnostic that surfaces
   with the MPP headers.

## Prerequisites

| Requirement | Details |
|---|---|
| **Hardware** | RK3588 / RK3588S board (Orange Pi 5, Rock 5B, etc.) |
| **MPP runtime** | `librockchip_mpp.so` installed (usually via `librockchip-mpp-dev`) |
| **Pre-built libwebrtc.a** | Must include the MPP codec sources. Build with [webrtc-rockchip-mpp](https://github.com/user/webrtc-rockchip-mpp). |
| **LiveKit rust-sdks** | Source checkout, tested against commit `9e635aa6` |
| **Rust** | 1.78+ (lockfile v4 support) |
| **C++ toolchain** | GCC/G++ with C++20 support for aarch64 |

## Quick Start

```bash
# 1. Clone LiveKit's Rust SDK
git clone https://github.com/livekit/rust-sdks
cd rust-sdks
git checkout 9e635aa6

# 2. Apply the Rockchip patches
/path/to/livekit-rockchip-sdk/apply_patches.sh $(pwd)

# 3. Build (on the RK3588 board, or cross-compile)
export LK_CUSTOM_WEBRTC=/path/to/webrtc-rockchip-mpp/artifacts
cargo build -p webrtc-sys

# 4. Verify -- run the basic_room example and check logs
RUST_LOG=debug cargo run -p basic_room -- \
    --url wss://your-livekit-server --token YOUR_TOKEN 2>&1 \
    | grep -i rockchip
# Expected: "RockchipVideoEncoderFactory created"
# Expected: "RockchipVideoDecoderFactory created"
```

## Patches Explained

The `patches/` directory contains three minimal, focused patches:

### `001-build-rs-rockchip.patch`

Modifies `webrtc-sys/build.rs`:

- Links `librockchip_mpp.so` when building for ARM64.
- Defines the `USE_ROCKCHIP_MPP_VIDEO_CODEC` preprocessor flag.
- Adds `-fpermissive` to the C++ compiler flags to work around a template
  name-lookup diagnostic triggered by MPP-related WebRTC code.

### `002-encoder-factory.patch`

Modifies `webrtc-sys/src/video_encoder_factory.cpp`:

- Conditionally includes `rockchip_video_encoder_factory.h`.
- Pushes a `RockchipVideoEncoderFactory` instance into the `factories_` vector
  inside `InternalFactory::InternalFactory()`.

### `003-decoder-factory.patch`

Modifies `webrtc-sys/src/video_decoder_factory.cpp`:

- Conditionally includes `rockchip_video_decoder_factory.h`.
- Pushes a `RockchipVideoDecoderFactory` instance into the `factories_` vector
  inside `VideoDecoderFactory::VideoDecoderFactory()`.

## How It Works

LiveKit's `webrtc-sys` uses a **factory-of-factories** pattern. Both
`VideoEncoderFactory` and `VideoDecoderFactory` hold a `std::vector` of
`std::unique_ptr` to concrete factory implementations. At runtime, each
factory in the list is queried for supported formats and asked to create
encoder/decoder instances.

```
InternalFactory
  |-- SoftwareEncoderFactory  (VP8, VP9, AV1 - always present)
  |-- NvidiaVideoEncoderFactory (NVIDIA, if USE_NVIDIA_VIDEO_CODEC)
  |-- VaapiEncoderFactory       (VAAPI, if USE_VAAPI_VIDEO_CODEC)
  +-- RockchipVideoEncoderFactory (Rockchip MPP, if USE_ROCKCHIP_MPP_VIDEO_CODEC)
```

The Rockchip factories are compiled from the MPP codec sources that live inside
the custom `libwebrtc.a` (built by the
[webrtc-rockchip-mpp](https://github.com/user/webrtc-rockchip-mpp) project).
The patches here simply tell LiveKit's build system to link the MPP library and
register the factories.

## Environment Variables

| Variable | Purpose |
|---|---|
| `LK_CUSTOM_WEBRTC` | Path to the directory containing the pre-built `libwebrtc.a` and associated headers. Required when using a custom WebRTC build. |
| `RUST_LOG` | Set to `debug` or `trace` to see factory registration messages and codec negotiation details. |

## Project Structure

```
livekit-rockchip-sdk/
  patches/
    001-build-rs-rockchip.patch      # build.rs: link MPP, define flag
    002-encoder-factory.patch        # Register encoder factory
    003-decoder-factory.patch        # Register decoder factory
  apply_patches.sh                   # Apply/revert patch script
  docs/
    PATCHING_GUIDE.md                # How to reapply when upstream updates
    SUPPORTED_VERSIONS.md            # Tested version matrix
    TROUBLESHOOTING.md               # Common issues and solutions
  examples/
    test_mpp_registration.sh         # Smoke test for factory registration
  .github/workflows/
    build.yml                        # CI build workflow
    check-upstream.yml               # Weekly upstream compatibility check
```

## Related Projects

- [webrtc-rockchip-mpp](https://github.com/user/webrtc-rockchip-mpp) -- Builds
  `libwebrtc.a` with Rockchip MPP codec support.
- [livekit/rust-sdks](https://github.com/livekit/rust-sdks) -- The upstream
  LiveKit Rust SDK that these patches target.
- [rockchip-linux/mpp](https://github.com/rockchip-linux/mpp) -- Rockchip's
  Media Process Platform library.

## License

This project is licensed under the [Apache License 2.0](LICENSE).

The patches modify code from [livekit/rust-sdks](https://github.com/livekit/rust-sdks),
which is also Apache-2.0 licensed.
