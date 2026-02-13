use anyhow::Result;
use clap::Parser;
use futures::StreamExt;
use livekit::options::{TrackPublishOptions, VideoCodec, VideoEncoding};
use livekit::prelude::*;
use livekit::webrtc::video_frame::{I420Buffer, VideoFrame, VideoRotation};
use livekit::webrtc::video_source::native::NativeVideoSource;
use livekit::webrtc::video_source::{RtcVideoSource, VideoResolution};
use livekit::webrtc::video_stream::native::NativeVideoStream;
use livekit_api::access_token;
use log::{debug, info};
use nokhwa::pixel_format::RgbFormat;
use nokhwa::utils::{
    ApiBackend, CameraFormat, CameraIndex, FrameFormat, RequestedFormat, RequestedFormatType,
    Resolution,
};
use nokhwa::Camera;
use std::collections::HashMap;
use std::env;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::{Duration, Instant};

#[derive(Parser, Debug)]
#[command(name = "video_call", about = "RK3588 hardware-accelerated video call via LiveKit")]
struct Args {
    /// Camera index (1 = EMEET SmartCam)
    #[arg(long, default_value_t = 1)]
    camera_index: u32,

    /// Video width
    #[arg(long, default_value_t = 1280)]
    width: u32,

    /// Video height
    #[arg(long, default_value_t = 720)]
    height: u32,

    /// Target framerate
    #[arg(long, default_value_t = 30)]
    fps: u32,

    /// LiveKit room name
    #[arg(long, default_value = "rk3588-call")]
    room: String,

    /// Participant identity
    #[arg(long, default_value = "rk3588-cam")]
    identity: String,

    /// List available cameras and exit
    #[arg(long)]
    list_cameras: bool,

    /// Subscribe-only mode (no camera publishing)
    #[arg(long)]
    no_camera: bool,
}

fn list_cameras() -> Result<()> {
    let cams = nokhwa::query(ApiBackend::Auto)?;
    println!("\nAvailable cameras:");
    for (i, cam) in cams.iter().enumerate() {
        println!("  [{}] {}", i, cam.human_name());
    }
    Ok(())
}

fn generate_token(
    api_key: &str,
    api_secret: &str,
    room: &str,
    identity: &str,
    can_publish: bool,
) -> Result<String> {
    let token = access_token::AccessToken::with_api_key(api_key, api_secret)
        .with_identity(identity)
        .with_name(identity)
        .with_grants(access_token::VideoGrants {
            room_join: true,
            room: room.to_string(),
            can_publish,
            can_subscribe: true,
            ..Default::default()
        })
        .to_jwt()?;
    Ok(token)
}

/// Spawn a background thread that receives decoded video frames from a remote track.
/// Logs decode stats every 5 seconds and saves periodic JPEG snapshots.
fn spawn_video_receiver(
    video_track: RemoteVideoTrack,
    participant_name: String,
    shutdown: Arc<AtomicBool>,
) {
    let rt = tokio::runtime::Handle::current();
    std::thread::spawn(move || {
        let mut sink = NativeVideoStream::new(video_track.rtc_track());
        let mut frame_count: u64 = 0;
        let mut last_stats = Instant::now();
        let mut last_snapshot = Instant::now();
        let mut logged_first = false;
        let snap_dir = "/tmp/decoded_frames";
        let _ = std::fs::create_dir_all(snap_dir);

        info!("Video receiver started for {}", participant_name);

        loop {
            if shutdown.load(Ordering::Acquire) {
                break;
            }

            let next = rt.block_on(async {
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_secs(1)) => None,
                    frame = sink.next() => frame,
                }
            });

            let Some(frame) = next else {
                if shutdown.load(Ordering::Acquire) {
                    break;
                }
                continue;
            };

            let w = frame.buffer.width();
            let h = frame.buffer.height();
            frame_count += 1;

            if !logged_first {
                info!(
                    "First decoded frame from {}: {}x{}",
                    participant_name, w, h
                );
                logged_first = true;
            }

            // Stats every 5 seconds
            if last_stats.elapsed() >= Duration::from_secs(5) {
                let fps = frame_count as f64 / last_stats.elapsed().as_secs_f64();

                // Check decoder implementation via RTC stats
                if let Ok(stats) = rt.block_on(video_track.get_stats()) {
                    let mut codec_by_id: HashMap<String, (String, String)> = HashMap::new();
                    let mut inbound: Option<livekit::webrtc::stats::InboundRtpStats> = None;
                    for s in stats.iter() {
                        match s {
                            livekit::webrtc::stats::RtcStats::Codec(c) => {
                                codec_by_id.insert(
                                    c.rtc.id.clone(),
                                    (c.codec.mime_type.clone(), c.codec.sdp_fmtp_line.clone()),
                                );
                            }
                            livekit::webrtc::stats::RtcStats::InboundRtp(i) => {
                                if i.stream.kind == "video" {
                                    inbound = Some(i.clone());
                                }
                            }
                            _ => {}
                        }
                    }
                    if let Some(i) = inbound {
                        let codec_name = codec_by_id
                            .get(&i.stream.codec_id)
                            .map(|(m, _)| m.as_str())
                            .unwrap_or("unknown");
                        println!(
                            "  Decoding: {}x{} @ {:.1} fps | codec: {} | decoder: {} | power_efficient: {}",
                            i.inbound.frame_width,
                            i.inbound.frame_height,
                            fps,
                            codec_name,
                            i.inbound.decoder_implementation,
                            i.inbound.power_efficient_decoder
                        );
                    } else {
                        println!(
                            "  Decoding: {}x{} @ {:.1} fps (stats pending)",
                            w, h, fps
                        );
                    }
                } else {
                    println!("  Decoding: {}x{} @ {:.1} fps", w, h, fps);
                }

                frame_count = 0;
                last_stats = Instant::now();
            }

            // Save JPEG snapshot every 5 seconds
            if last_snapshot.elapsed() >= Duration::from_secs(5) {
                let i420 = frame.buffer.to_i420();
                let (stride_y, stride_u, stride_v) = i420.strides();
                let (data_y, data_u, data_v) = i420.data();

                let mut rgb = vec![0u8; (w * h * 3) as usize];
                unsafe {
                    yuv_sys::rs_I420ToRGB24(
                        data_y.as_ptr(),
                        stride_y as i32,
                        data_u.as_ptr(),
                        stride_u as i32,
                        data_v.as_ptr(),
                        stride_v as i32,
                        rgb.as_mut_ptr(),
                        (w * 3) as i32,
                        w as i32,
                        h as i32,
                    );
                }

                let ts = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_secs();
                let path = format!("{}/frame_{}.jpg", snap_dir, ts);
                match image::save_buffer(
                    &path,
                    &rgb,
                    w,
                    h,
                    image::ColorType::Rgb8,
                ) {
                    Ok(_) => println!("  Saved decoded frame: {}", path),
                    Err(e) => log::warn!("Failed to save frame: {}", e),
                }

                last_snapshot = Instant::now();
            }
        }

        info!("Video receiver ended for {}", participant_name);
    });
}

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let args = Args::parse();

    if args.list_cameras {
        return list_cameras();
    }

    // Setup Ctrl-C handler
    let shutdown = Arc::new(AtomicBool::new(false));
    {
        let shutdown = shutdown.clone();
        tokio::spawn(async move {
            let _ = tokio::signal::ctrl_c().await;
            shutdown.store(true, Ordering::Release);
        });
    }

    // Read LiveKit credentials from environment
    let url = env::var("LIVEKIT_URL").expect("Set LIVEKIT_URL");
    let api_key = env::var("LIVEKIT_API_KEY").expect("Set LIVEKIT_API_KEY");
    let api_secret = env::var("LIVEKIT_API_SECRET").expect("Set LIVEKIT_API_SECRET");

    // Generate tokens — viewer gets can_publish=true so browser can send video back
    let pub_token = generate_token(&api_key, &api_secret, &args.room, &args.identity, true)?;
    let viewer_token = generate_token(&api_key, &api_secret, &args.room, "viewer", true)?;

    // Print join info
    let mode = if args.no_camera { "Decode-Only" } else { "MPP H.264" };
    println!("\n╔══════════════════════════════════════════════════════════╗");
    println!("║           RK3588 Video Call ({})              ║", mode);
    println!("╠══════════════════════════════════════════════════════════╣");
    println!("║  Room:     {}", args.room);
    println!("║  Identity: {}", args.identity);
    if !args.no_camera {
        println!("║  Camera:   index {}", args.camera_index);
        println!("║  Video:    {}x{} @ {}fps H.264", args.width, args.height, args.fps);
    } else {
        println!("║  Mode:     subscribe-only (decode test)");
    }
    println!("╠══════════════════════════════════════════════════════════╣");
    println!("║  Join from browser:                                     ║");
    println!("║  https://meet.livekit.io/custom?liveKitUrl={}", url);
    println!("║                                                         ║");
    println!("║  Viewer token (paste into meet.livekit.io):             ║");
    println!("║  {}", &viewer_token[..60.min(viewer_token.len())]);
    println!("║  ...(truncated, full token in logs)                     ║");
    println!("╚══════════════════════════════════════════════════════════╝\n");
    info!("Full viewer token: {}", viewer_token);

    // Connect to LiveKit
    info!("Connecting to {}...", url);
    let (room, _rx) = Room::connect(&url, &pub_token, RoomOptions::default()).await?;
    let room = Arc::new(room);
    info!("Connected to room: {}", room.name());

    // Spawn room event listener with video track subscription
    {
        let room = room.clone();
        let shutdown = shutdown.clone();
        tokio::spawn(async move {
            let mut events = room.subscribe();
            while let Some(evt) = events.recv().await {
                match evt {
                    RoomEvent::ParticipantConnected(p) => {
                        println!("  >> {} joined the call", p.name());
                    }
                    RoomEvent::ParticipantDisconnected(p) => {
                        println!("  << {} left the call", p.name());
                    }
                    RoomEvent::TrackSubscribed {
                        track,
                        publication,
                        participant,
                    } => {
                        info!(
                            "Subscribed to track '{}' (mime: {}) from {}",
                            publication.name(),
                            publication.mime_type(),
                            participant.name()
                        );
                        match track {
                            RemoteTrack::Video(video_track) => {
                                println!(
                                    "  >> Receiving video from {} ({})",
                                    participant.name(),
                                    publication.mime_type()
                                );
                                // Log initial stats after a short delay
                                let vt = video_track.clone();
                                tokio::spawn(async move {
                                    tokio::time::sleep(Duration::from_secs(2)).await;
                                    match vt.get_stats().await {
                                        Ok(stats) => {
                                            for s in stats.iter() {
                                                if let livekit::webrtc::stats::RtcStats::InboundRtp(i) = s {
                                                    if i.stream.kind == "video" {
                                                        info!(
                                                            "Initial decoder stats: {}x{} @ {:.1}fps, decoder: {}, power_efficient: {}",
                                                            i.inbound.frame_width,
                                                            i.inbound.frame_height,
                                                            i.inbound.frames_per_second,
                                                            i.inbound.decoder_implementation,
                                                            i.inbound.power_efficient_decoder
                                                        );
                                                    }
                                                }
                                            }
                                        }
                                        Err(e) => debug!("Failed to get initial stats: {:?}", e),
                                    }
                                });
                                // Spawn frame receiver thread
                                spawn_video_receiver(
                                    video_track,
                                    participant.name().to_string(),
                                    shutdown.clone(),
                                );
                            }
                            RemoteTrack::Audio(_) => {
                                info!("Audio track subscribed (ignoring)");
                            }
                        }
                    }
                    RoomEvent::TrackUnsubscribed {
                        publication,
                        participant,
                        ..
                    } => {
                        info!(
                            "Track '{}' from {} unsubscribed",
                            publication.name(),
                            participant.name()
                        );
                    }
                    RoomEvent::ConnectionStateChanged(state) => {
                        info!("Connection state: {:?}", state);
                    }
                    RoomEvent::Disconnected { reason } => {
                        println!("  !! Disconnected: {:?}", reason);
                        break;
                    }
                    _ => {}
                }
            }
        });
    }

    if args.no_camera {
        println!("  Subscribe-only mode. Waiting for remote video...\n");
        println!("  Enable your camera in the browser to test H.264 decoding.\n");
        loop {
            if shutdown.load(Ordering::Acquire) {
                println!("\n  Shutting down...");
                break;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    } else {
        // Open camera
        info!("Opening camera index {}...", args.camera_index);
        let index = CameraIndex::Index(args.camera_index);
        let requested =
            RequestedFormat::new::<RgbFormat>(RequestedFormatType::AbsoluteHighestFrameRate);
        let mut camera = Camera::new(index, requested)?;

        // Try MJPEG format at requested resolution
        let fmt = CameraFormat::new(
            Resolution::new(args.width, args.height),
            FrameFormat::MJPEG,
            args.fps,
        );
        let _ = camera.set_camera_requset(RequestedFormat::new::<RgbFormat>(
            RequestedFormatType::Exact(fmt),
        ));
        camera.open_stream()?;

        let cam_fmt = camera.camera_format();
        let width = cam_fmt.width();
        let height = cam_fmt.height();
        info!(
            "Camera opened: {}x{} @ {}fps {:?}",
            width,
            height,
            cam_fmt.frame_rate(),
            cam_fmt.format()
        );

        // Create video source and track
        let source = NativeVideoSource::new(VideoResolution { width, height });
        let track = LocalVideoTrack::create_video_track(
            "camera",
            RtcVideoSource::Native(source.clone()),
        );

        // Publish with H.264
        let opts = TrackPublishOptions {
            source: TrackSource::Camera,
            video_codec: VideoCodec::H264,
            video_encoding: Some(VideoEncoding {
                max_bitrate: 2_500_000,
                max_framerate: args.fps as f64,
            }),
            ..Default::default()
        };
        room.local_participant()
            .publish_track(LocalTrack::Video(track), opts)
            .await?;
        info!("Publishing video track (H.264 via MPP hardware encoder)");
        println!("\n  Camera is live! Waiting for viewers...\n");

        // Frame capture loop
        let mut frame = VideoFrame {
            rotation: VideoRotation::VideoRotation0,
            timestamp_us: 0,
            buffer: I420Buffer::new(width, height),
        };

        let mut ticker =
            tokio::time::interval(Duration::from_secs_f64(1.0 / args.fps as f64));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        ticker.tick().await;

        let start = Instant::now();
        let mut frame_count: u64 = 0;
        let mut last_stats = Instant::now();

        loop {
            if shutdown.load(Ordering::Acquire) {
                println!("\n  Shutting down...");
                break;
            }

            ticker.tick().await;

            // Capture frame from camera
            let frame_buf = match camera.frame() {
                Ok(f) => f,
                Err(e) => {
                    log::warn!("Camera frame error: {}", e);
                    continue;
                }
            };

            // Convert to I420
            let src = frame_buf.buffer();
            let (stride_y, stride_u, stride_v) = frame.buffer.strides();
            let (data_y, data_u, data_v) = frame.buffer.data_mut();

            let src_bytes = src.as_ref();
            if src_bytes.len() == (width as usize * height as usize * 3) {
                // RGB24
                unsafe {
                    yuv_sys::rs_RGB24ToI420(
                        src_bytes.as_ptr(),
                        (width * 3) as i32,
                        data_y.as_mut_ptr(),
                        stride_y as i32,
                        data_u.as_mut_ptr(),
                        stride_u as i32,
                        data_v.as_mut_ptr(),
                        stride_v as i32,
                        width as i32,
                        height as i32,
                    );
                }
            } else {
                // MJPEG - try fast libyuv decode
                let ret = unsafe {
                    yuv_sys::rs_MJPGToI420(
                        src_bytes.as_ptr(),
                        src_bytes.len(),
                        data_y.as_mut_ptr(),
                        stride_y as i32,
                        data_u.as_mut_ptr(),
                        stride_u as i32,
                        data_v.as_mut_ptr(),
                        stride_v as i32,
                        width as i32,
                        height as i32,
                        width as i32,
                        height as i32,
                    )
                };
                if ret != 0 {
                    log::warn!("MJPEG decode failed, skipping frame");
                    continue;
                }
            }

            frame.timestamp_us = start.elapsed().as_micros() as i64;
            source.capture_frame(&frame);
            frame_count += 1;

            // Stats every 5 seconds
            if last_stats.elapsed() >= Duration::from_secs(5) {
                let fps = frame_count as f64 / last_stats.elapsed().as_secs_f64();
                println!(
                    "  Publishing: {}x{} @ {:.1} fps (H.264/MPP)",
                    width, height, fps
                );
                frame_count = 0;
                last_stats = Instant::now();
            }
        }
    }

    Ok(())
}
