use anyhow::Result;
use clap::Parser;
use livekit::options::{TrackPublishOptions, VideoCodec, VideoEncoding};
use livekit::prelude::*;
use livekit::webrtc::video_frame::{I420Buffer, VideoFrame, VideoRotation};
use livekit::webrtc::video_source::native::NativeVideoSource;
use livekit::webrtc::video_source::{RtcVideoSource, VideoResolution};
use livekit_api::access_token;
use log::info;
use nokhwa::pixel_format::RgbFormat;
use nokhwa::utils::{
    ApiBackend, CameraFormat, CameraIndex, FrameFormat, RequestedFormat, RequestedFormatType,
    Resolution,
};
use nokhwa::Camera;
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

    // Generate tokens
    let pub_token = generate_token(&api_key, &api_secret, &args.room, &args.identity, true)?;
    let viewer_token = generate_token(&api_key, &api_secret, &args.room, "viewer", false)?;

    // Print join info
    println!("\n╔══════════════════════════════════════════════════════════╗");
    println!("║           RK3588 Video Call (MPP H.264)                 ║");
    println!("╠══════════════════════════════════════════════════════════╣");
    println!("║  Room:     {}",  args.room);
    println!("║  Identity: {}", args.identity);
    println!("║  Camera:   index {}", args.camera_index);
    println!("║  Video:    {}x{} @ {}fps H.264", args.width, args.height, args.fps);
    println!("╠══════════════════════════════════════════════════════════╣");
    println!("║  Join from browser:                                     ║");
    println!("║  https://meet.livekit.io/custom?liveKitUrl={}", url);
    println!("║                                                         ║");
    println!("║  Viewer token (paste into meet.livekit.io):             ║");
    println!("║  {}", &viewer_token[..60]);
    println!("║  ...(truncated, full token in logs)                     ║");
    println!("╚══════════════════════════════════════════════════════════╝\n");
    info!("Full viewer token: {}", viewer_token);

    // Connect to LiveKit
    info!("Connecting to {}...", url);
    let (room, _rx) = Room::connect(&url, &pub_token, RoomOptions::default()).await?;
    let room = Arc::new(room);
    info!("Connected to room: {}", room.name());

    // Spawn room event listener
    {
        let room = room.clone();
        tokio::spawn(async move {
            let mut events = room.subscribe();
            while let Some(evt) = events.recv().await {
                match &evt {
                    RoomEvent::ParticipantConnected(p) => {
                        println!("  >> {} joined the call", p.name());
                    }
                    RoomEvent::ParticipantDisconnected(p) => {
                        println!("  << {} left the call", p.name());
                    }
                    RoomEvent::TrackSubscribed { participant, publication, .. } => {
                        info!(
                            "Subscribed to {} track from {}",
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

    // Open camera
    info!("Opening camera index {}...", args.camera_index);
    let index = CameraIndex::Index(args.camera_index);
    let requested = RequestedFormat::new::<RgbFormat>(RequestedFormatType::AbsoluteHighestFrameRate);
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
    info!("Camera opened: {}x{} @ {}fps {:?}", width, height, cam_fmt.frame_rate(), cam_fmt.format());

    // Create video source and track
    let source = NativeVideoSource::new(VideoResolution { width, height });
    let track = LocalVideoTrack::create_video_track("camera", RtcVideoSource::Native(source.clone()));

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

    let mut ticker = tokio::time::interval(Duration::from_secs_f64(1.0 / args.fps as f64));
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
                    data_y.as_mut_ptr(), stride_y as i32,
                    data_u.as_mut_ptr(), stride_u as i32,
                    data_v.as_mut_ptr(), stride_v as i32,
                    width as i32, height as i32,
                );
            }
        } else {
            // MJPEG - try fast libyuv decode
            let ret = unsafe {
                yuv_sys::rs_MJPGToI420(
                    src_bytes.as_ptr(), src_bytes.len(),
                    data_y.as_mut_ptr(), stride_y as i32,
                    data_u.as_mut_ptr(), stride_u as i32,
                    data_v.as_mut_ptr(), stride_v as i32,
                    width as i32, height as i32,
                    width as i32, height as i32,
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
            println!("  Publishing: {}x{} @ {:.1} fps (H.264/MPP)", width, height, fps);
            frame_count = 0;
            last_stats = Instant::now();
        }
    }

    Ok(())
}
