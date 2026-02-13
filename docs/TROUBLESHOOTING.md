# Troubleshooting

Common issues encountered when building or running LiveKit with Rockchip MPP
support, and how to fix them.

---

## Build Errors

### "changes meaning of Network"

**Error message:**
```
error: declaration of 'Network' changes meaning of 'Network' [-Werror=changes-meaning]
```

**Cause:** A strict C++ diagnostic that GCC enables by default in C++20 mode.
Some WebRTC headers trigger this when compiled with the MPP codec sources.

**Fix:** The `001-build-rs-rockchip.patch` already adds `-fpermissive` to the
compiler flags. If you see this error, verify that the patch was applied
correctly to `webrtc-sys/build.rs`. Look for:

```rust
.flag("-fpermissive")
```

---

### ".eh_frame" / CREL relocation errors

**Error message:**
```
ld: error: .eh_frame: unexpected relocation type
```
or
```
ld: error: CREL relocation not supported
```

**Cause:** The `libwebrtc.a` was built with a newer version of Clang that
emits CREL relocations, which the system linker does not understand.

**Fix:** Rebuild `libwebrtc.a` using the
[webrtc-rockchip-mpp](https://github.com/user/webrtc-rockchip-mpp) project,
which disables CREL by default. If building manually, ensure your GN args
include:

```
use_crel=false
```

---

### "lock file version 4" / cargo lockfile error

**Error message:**
```
error: lock file version 4 requires `-Znext-lockfile-bump`
```

**Cause:** The LiveKit SDK uses a Cargo lockfile version that requires Rust
1.78 or newer.

**Fix:** Update Rust to the latest stable version:

```bash
rustup update stable
rustc --version   # Should be >= 1.78.0
```

---

### "__arm_tpidr2_save" undefined symbol

**Error message:**
```
undefined reference to `__arm_tpidr2_save'
```

**Cause:** The `libwebrtc.a` was built with SME (Scalable Matrix Extension)
support enabled in libyuv, but the target system's libc does not provide the
SME runtime symbols.

**Fix:** Rebuild `libwebrtc.a` with the SME flag disabled. In your GN args:

```
libyuv_use_sme=false
```

The [webrtc-rockchip-mpp](https://github.com/user/webrtc-rockchip-mpp) project
sets this by default.

---

### pkg-config cross-compilation errors

**Error message:**
```
pkg-config has not been configured to support cross-compilation
```
or
```
Could not run `"pkg-config"`
```

**Cause:** When cross-compiling for `aarch64-unknown-linux-gnu` on an x86_64
host, pkg-config needs to be configured to use the aarch64 sysroot.

**Fix:** Create a wrapper script and point Cargo to it:

```bash
# Create wrapper at /usr/local/bin/aarch64-pkg-config
cat > /tmp/aarch64-pkg-config <<'EOF'
#!/bin/sh
export PKG_CONFIG_DIR=
export PKG_CONFIG_LIBDIR=/usr/aarch64-linux-gnu/lib/pkgconfig:/usr/lib/aarch64-linux-gnu/pkgconfig
export PKG_CONFIG_SYSROOT_DIR=/usr/aarch64-linux-gnu
exec pkg-config "$@"
EOF
chmod +x /tmp/aarch64-pkg-config

# Tell Cargo to use the wrapper for aarch64 builds
export PKG_CONFIG_aarch64_unknown_linux_gnu=/tmp/aarch64-pkg-config
export PKG_CONFIG_ALLOW_CROSS=1
```

Or add to `.cargo/config.toml`:

```toml
[env]
PKG_CONFIG_ALLOW_CROSS = "1"
```

---

## Runtime Issues

### Camera not using hardware encoder

**Symptoms:** Video works but CPU usage is high. Hardware encoder not being used.

**Diagnosis:**

```bash
RUST_LOG=debug cargo run -p basic_room -- --url wss://... --token ... 2>&1 | grep -i rockchip
```

**Expected output:**
```
RockchipVideoEncoderFactory created
RockchipVideoDecoderFactory created
```

**If you do NOT see these messages:**

1. Verify that `USE_ROCKCHIP_MPP_VIDEO_CODEC` is defined at compile time.
   Check the build output for:
   ```
   -DUSE_ROCKCHIP_MPP_VIDEO_CODEC=1
   ```

2. Verify that `librockchip_mpp.so` is available on the system:
   ```bash
   ldconfig -p | grep rockchip_mpp
   ```

3. Verify the patches were applied. Check the source files:
   ```bash
   grep -n "RockchipVideoEncoderFactory" webrtc-sys/src/video_encoder_factory.cpp
   grep -n "RockchipVideoDecoderFactory" webrtc-sys/src/video_decoder_factory.cpp
   ```

**If the factory IS created but hardware encoding is not used:**

The remote peer might be negotiating a codec that the Rockchip encoder does
not support (e.g., VP9 or AV1). The Rockchip MPP encoder currently supports
H.264 only. Ensure the LiveKit room is configured to prefer H.264:

```
# LiveKit server config (livekit.yaml)
room:
  enabled_codecs:
    - mime: video/h264
```

---

### "Frame buffer is not kNative type"

**Error message (in logs):**
```
Frame buffer is not kNative type, falling back to CPU copy
```

**Cause:** This is a **warning**, not an error. It means the encoder received
a frame in I420 format (from a CPU-based capture path) instead of a native
DMA-BUF frame. The encoder will copy the frame to MPP's buffer pool, which
adds some CPU overhead but still uses hardware encoding.

**Fix:** This is expected behavior when capturing from a V4L2 camera that does
not support DMA-BUF export. The hardware encoder is still being used -- the
only overhead is the CPU-to-VPU buffer copy. For zero-copy, the camera capture
pipeline needs to provide frames as DMA-BUF file descriptors.

---

### MPP initialization failure

**Error message (in logs):**
```
mpp_create failed
```
or
```
Failed to initialize MPP context
```

**Cause:** The MPP library could not open the VPU device nodes.

**Fix:**

1. Check that the VPU device nodes exist:
   ```bash
   ls -la /dev/mpp_service
   ls -la /dev/dri/renderD128
   ```

2. Check permissions. The running user needs access to these devices:
   ```bash
   # Add your user to the video group
   sudo usermod -aG video $USER
   # Log out and back in for group change to take effect
   ```

3. Check that the MPP kernel module is loaded:
   ```bash
   lsmod | grep rkvdec
   lsmod | grep rkvenc
   ```

---

### High latency or frame drops

**Symptoms:** Video arrives but with noticeable delay or frequent frame drops.

**Diagnosis:**

1. Check MPP encoder load:
   ```bash
   cat /sys/class/devfreq/fdab0000.npu/load  # NPU (should NOT be high)
   # VPU status (varies by kernel):
   cat /sys/kernel/debug/mpp_service/session_summary 2>/dev/null
   ```

2. Check CPU usage (should be low if hardware encode is working):
   ```bash
   top -p $(pgrep basic_room)
   ```

**Common causes:**

- Network bandwidth too low for the bitrate.
- Encoding resolution too high for the VPU's real-time capability at the
  requested framerate. Try lowering resolution or framerate.
- Another process is using the VPU simultaneously.

---

## Getting Help

If your issue is not listed here:

1. Check the [webrtc-rockchip-mpp](https://github.com/user/webrtc-rockchip-mpp)
   repository for libwebrtc build issues.
2. Check the [LiveKit SDK issues](https://github.com/livekit/rust-sdks/issues)
   for general SDK problems.
3. Open an issue in this repository with:
   - Your board model (e.g., Orange Pi 5, Rock 5B)
   - Output of `uname -a`
   - Output of `mpp_info_test` (from the MPP tools package)
   - Full build log or runtime error log
