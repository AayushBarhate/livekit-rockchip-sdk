use anyhow::{anyhow, Result};
use clap::Parser;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, SampleFormat, SampleRate, SizedSample, Stream, StreamConfig};
use futures_util::StreamExt;
use livekit::options::{TrackPublishOptions, VideoCodec, VideoEncoding};
use livekit::prelude::*;
use livekit::webrtc::audio_frame::AudioFrame;
use std::borrow::Cow;
use livekit::webrtc::audio_source::native::NativeAudioSource;
use livekit::webrtc::audio_stream::native::NativeAudioStream;
use livekit::webrtc::prelude::{AudioSourceOptions, RtcAudioSource};
use livekit::webrtc::video_frame::{I420Buffer, VideoFrame, VideoRotation};
use livekit::webrtc::video_source::native::NativeVideoSource;
use livekit::webrtc::video_source::{RtcVideoSource, VideoResolution};
use livekit::webrtc::video_stream::native::NativeVideoStream;
use livekit_api::access_token;
use log::{error, info, warn};
use nokhwa::pixel_format::RgbFormat;
use nokhwa::utils::{
    ApiBackend, CameraFormat, CameraIndex, FrameFormat, RequestedFormat, RequestedFormatType,
    Resolution,
};
use nokhwa::Camera;
use serde::Deserialize;
use std::collections::VecDeque;
use std::env;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

// ---------------------------------------------------------------------------
// Config file structures
// ---------------------------------------------------------------------------

#[derive(Deserialize, Default)]
struct Config {
    #[serde(default)]
    livekit: LiveKitConfig,
    #[serde(default)]
    video: VideoConfig,
    #[serde(default)]
    audio: AudioConfig,
    #[serde(default)]
    features: FeatureConfig,
}

#[derive(Deserialize)]
struct LiveKitConfig {
    #[serde(default)]
    url: String,
    #[serde(default)]
    api_key: String,
    #[serde(default)]
    api_secret: String,
    #[serde(default = "default_room")]
    room: String,
    #[serde(default = "default_identity")]
    identity: String,
}

impl Default for LiveKitConfig {
    fn default() -> Self {
        Self {
            url: String::new(),
            api_key: String::new(),
            api_secret: String::new(),
            room: default_room(),
            identity: default_identity(),
        }
    }
}

#[derive(Deserialize)]
struct VideoConfig {
    #[serde(default)]
    camera_index: u32,
    #[serde(default = "default_width")]
    width: u32,
    #[serde(default = "default_height")]
    height: u32,
    #[serde(default = "default_fps")]
    fps: u32,
    #[serde(default = "default_codec")]
    codec: String,
    #[serde(default = "default_video_bitrate")]
    max_bitrate: u64,
    #[serde(default)]
    simulcast: bool,
}

impl Default for VideoConfig {
    fn default() -> Self {
        Self {
            camera_index: 0,
            width: default_width(),
            height: default_height(),
            fps: default_fps(),
            codec: default_codec(),
            max_bitrate: default_video_bitrate(),
            simulcast: false,
        }
    }
}

#[derive(Deserialize)]
struct AudioConfig {
    #[serde(default)]
    input_device: String,
    #[serde(default)]
    output_device: String,
    #[serde(default = "default_sample_rate")]
    sample_rate: u32,
    #[serde(default = "default_channels")]
    channels: u32,
    #[serde(default)]
    channel_index: u32,
    #[serde(default = "default_true")]
    echo_cancellation: bool,
    #[serde(default = "default_true")]
    noise_suppression: bool,
    #[serde(default = "default_true")]
    auto_gain_control: bool,
    #[serde(default = "default_volume")]
    volume: f32,
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            input_device: String::new(),
            output_device: String::new(),
            sample_rate: default_sample_rate(),
            channels: default_channels(),
            channel_index: 0,
            echo_cancellation: true,
            noise_suppression: true,
            auto_gain_control: true,
            volume: default_volume(),
        }
    }
}

#[derive(Deserialize, Default)]
struct FeatureConfig {
    #[serde(default)]
    no_camera: bool,
    #[serde(default)]
    no_microphone: bool,
    #[serde(default)]
    no_playback: bool,
}

fn default_room() -> String { "rk3588-call".to_string() }
fn default_identity() -> String { "rk3588-cam".to_string() }
fn default_width() -> u32 { 1280 }
fn default_height() -> u32 { 720 }
fn default_fps() -> u32 { 30 }
fn default_codec() -> String { "h264".to_string() }
fn default_video_bitrate() -> u64 { 2_500_000 }
fn default_sample_rate() -> u32 { 48000 }
fn default_channels() -> u32 { 1 }
fn default_volume() -> f32 { 1.0 }
fn default_true() -> bool { true }

// ---------------------------------------------------------------------------
// CLI arguments (override config file)
// ---------------------------------------------------------------------------

#[derive(Parser, Debug)]
#[command(
    name = "video_call",
    about = "RK3588 hardware-accelerated video call via LiveKit"
)]
struct Args {
    /// Path to config file
    #[arg(short, long, default_value = "config.toml")]
    config: PathBuf,

    /// List available audio/video devices and exit
    #[arg(long)]
    list_devices: bool,

    /// LiveKit server URL (overrides config and LIVEKIT_URL)
    #[arg(long)]
    url: Option<String>,

    /// LiveKit API key (overrides config and LIVEKIT_API_KEY)
    #[arg(long)]
    api_key: Option<String>,

    /// LiveKit API secret (overrides config and LIVEKIT_API_SECRET)
    #[arg(long)]
    api_secret: Option<String>,

    /// Room name
    #[arg(long)]
    room: Option<String>,

    /// Participant identity
    #[arg(long)]
    identity: Option<String>,

    /// Camera device index
    #[arg(long)]
    camera_index: Option<u32>,

    /// Video width
    #[arg(long)]
    width: Option<u32>,

    /// Video height
    #[arg(long)]
    height: Option<u32>,

    /// Target framerate
    #[arg(long)]
    fps: Option<u32>,

    /// Video codec (h264, vp8, vp9, av1, h265)
    #[arg(long)]
    codec: Option<String>,

    /// Microphone device name (substring match)
    #[arg(long)]
    mic: Option<String>,

    /// Speaker device name (substring match)
    #[arg(long)]
    speaker: Option<String>,

    /// Disable camera
    #[arg(long)]
    no_camera: bool,

    /// Disable microphone
    #[arg(long)]
    no_mic: bool,

    /// Disable speaker playback
    #[arg(long)]
    no_playback: bool,
}

// ---------------------------------------------------------------------------
// Audio mixer: collects remote audio and feeds to speaker
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct AudioMixer {
    buffer: Arc<Mutex<VecDeque<i16>>>,
    volume: f32,
    max_buffer_size: usize,
}

impl AudioMixer {
    fn new(sample_rate: u32, volume: f32) -> Self {
        let max_buffer_size = sample_rate as usize; // 1 second of mono audio
        Self {
            buffer: Arc::new(Mutex::new(VecDeque::with_capacity(max_buffer_size))),
            volume: volume.clamp(0.0, 1.0),
            max_buffer_size,
        }
    }

    fn add_audio_data(&self, data: &[i16]) {
        let mut buffer = self.buffer.lock().unwrap();
        for &sample in data {
            let scaled = (sample as f32 * self.volume) as i16;
            buffer.push_back(scaled);
            if buffer.len() > self.max_buffer_size {
                buffer.pop_front();
            }
        }
    }

    fn get_samples(&self, count: usize) -> Vec<i16> {
        let mut buffer = self.buffer.lock().unwrap();
        let mut out = Vec::with_capacity(count);
        for _ in 0..count {
            out.push(buffer.pop_front().unwrap_or(0));
        }
        out
    }
}

// ---------------------------------------------------------------------------
// Device listing
// ---------------------------------------------------------------------------

fn list_devices() -> Result<()> {
    println!("\n=== Video Devices ===");
    match nokhwa::query(ApiBackend::Auto) {
        Ok(cams) => {
            for (i, cam) in cams.iter().enumerate() {
                println!("  [{}] {}", i, cam.human_name());
            }
            if cams.is_empty() {
                println!("  (none found)");
            }
        }
        Err(e) => println!("  Error querying cameras: {}", e),
    }

    let host = cpal::default_host();

    println!("\n=== Audio Input Devices (Microphones) ===");
    if let Ok(devices) = host.input_devices() {
        for (i, device) in devices.enumerate() {
            let name = device.name().unwrap_or_else(|_| "Unknown".to_string());
            let info = device
                .default_input_config()
                .map(|c| {
                    format!(
                        "{}Hz, {} ch, {:?}",
                        c.sample_rate().0,
                        c.channels(),
                        c.sample_format()
                    )
                })
                .unwrap_or_else(|_| "no config".to_string());
            println!("  [{}] {} ({})", i, name, info);
        }
    }
    if let Some(device) = host.default_input_device() {
        println!(
            "  Default: {}",
            device.name().unwrap_or_else(|_| "Unknown".to_string())
        );
    }

    println!("\n=== Audio Output Devices (Speakers) ===");
    if let Ok(devices) = host.output_devices() {
        for (i, device) in devices.enumerate() {
            let name = device.name().unwrap_or_else(|_| "Unknown".to_string());
            let info = device
                .default_output_config()
                .map(|c| {
                    format!(
                        "{}Hz, {} ch, {:?}",
                        c.sample_rate().0,
                        c.channels(),
                        c.sample_format()
                    )
                })
                .unwrap_or_else(|_| "no config".to_string());
            println!("  [{}] {} ({})", i, name, info);
        }
    }
    if let Some(device) = host.default_output_device() {
        println!(
            "  Default: {}",
            device.name().unwrap_or_else(|_| "Unknown".to_string())
        );
    }

    println!();
    Ok(())
}

// ---------------------------------------------------------------------------
// Audio helpers
// ---------------------------------------------------------------------------

fn find_input_device(name: &str) -> Result<Device> {
    let host = cpal::default_host();
    for device in host.input_devices()? {
        if let Ok(n) = device.name() {
            if n.contains(name) {
                return Ok(device);
            }
        }
    }
    Err(anyhow!("Input device '{}' not found", name))
}

fn find_output_device(name: &str) -> Result<Device> {
    let host = cpal::default_host();
    for device in host.output_devices()? {
        if let Ok(n) = device.name() {
            if n.contains(name) {
                return Ok(device);
            }
        }
    }
    Err(anyhow!("Output device '{}' not found", name))
}

fn start_audio_capture(
    device: Device,
    config: StreamConfig,
    sample_format: SampleFormat,
    tx: mpsc::UnboundedSender<Vec<i16>>,
    channel_index: u32,
    num_channels: u32,
) -> Result<Stream> {
    let stream = match sample_format {
        SampleFormat::F32 => build_input_stream::<f32>(device, config, tx, channel_index, num_channels)?,
        SampleFormat::I16 => build_input_stream::<i16>(device, config, tx, channel_index, num_channels)?,
        SampleFormat::U16 => build_input_stream::<u16>(device, config, tx, channel_index, num_channels)?,
        f => return Err(anyhow!("Unsupported sample format: {:?}", f)),
    };
    stream.play()?;
    Ok(stream)
}

fn build_input_stream<T: SizedSample + Send + 'static>(
    device: Device,
    config: StreamConfig,
    tx: mpsc::UnboundedSender<Vec<i16>>,
    channel_index: u32,
    num_channels: u32,
) -> Result<Stream> {
    let stream = device.build_input_stream(
        &config,
        move |data: &[T], _: &cpal::InputCallbackInfo| {
            let converted: Vec<i16> = data
                .iter()
                .skip(channel_index as usize)
                .step_by(num_channels as usize)
                .map(|sample| convert_to_i16(sample))
                .collect();
            let _ = tx.send(converted);
        },
        |err| error!("Audio input error: {}", err),
        None,
    )?;
    Ok(stream)
}

fn convert_to_i16<T: SizedSample>(sample: &T) -> i16 {
    let size = std::mem::size_of::<T>();
    if size == std::mem::size_of::<f32>() {
        let f = unsafe { std::mem::transmute_copy::<T, f32>(sample) };
        (f.clamp(-1.0, 1.0) * i16::MAX as f32) as i16
    } else if size == std::mem::size_of::<i16>() {
        unsafe { std::mem::transmute_copy::<T, i16>(sample) }
    } else if size == std::mem::size_of::<u16>() {
        let u = unsafe { std::mem::transmute_copy::<T, u16>(sample) };
        ((u as i32) - (u16::MAX as i32 / 2)) as i16
    } else {
        0
    }
}

fn start_audio_playback(
    device: Device,
    config: StreamConfig,
    sample_format: SampleFormat,
    mixer: AudioMixer,
) -> Result<Stream> {
    let stream = match sample_format {
        SampleFormat::F32 => build_output_stream::<f32>(device, config, mixer)?,
        SampleFormat::I16 => build_output_stream::<i16>(device, config, mixer)?,
        SampleFormat::U16 => build_output_stream::<u16>(device, config, mixer)?,
        f => return Err(anyhow!("Unsupported output format: {:?}", f)),
    };
    stream.play()?;
    Ok(stream)
}

fn build_output_stream<T>(
    device: Device,
    config: StreamConfig,
    mixer: AudioMixer,
) -> Result<Stream>
where
    T: SizedSample + cpal::Sample + cpal::FromSample<f32> + Send + 'static,
{
    let stream = device.build_output_stream(
        &config,
        move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
            let samples = mixer.get_samples(data.len());
            for (i, out) in data.iter_mut().enumerate() {
                let f = samples[i] as f32 / i16::MAX as f32;
                *out = T::from_sample(f);
            }
        },
        |err| error!("Audio output error: {}", err),
        None,
    )?;
    Ok(stream)
}

// ---------------------------------------------------------------------------
// Video codec helper
// ---------------------------------------------------------------------------

fn parse_codec(s: &str) -> VideoCodec {
    match s.to_lowercase().as_str() {
        "vp8" => VideoCodec::VP8,
        "vp9" => VideoCodec::VP9,
        "av1" => VideoCodec::AV1,
        "h265" | "hevc" => VideoCodec::H265,
        _ => VideoCodec::H264,
    }
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let args = Args::parse();

    if args.list_devices {
        return list_devices();
    }

    // Load config file (optional — missing file is fine, we'll use defaults)
    let cfg: Config = if args.config.exists() {
        let text = std::fs::read_to_string(&args.config)?;
        info!("Loaded config from {}", args.config.display());
        toml::from_str(&text)?
    } else {
        info!(
            "No config file at {} — using defaults / env / CLI",
            args.config.display()
        );
        Config::default()
    };

    // Resolve values: CLI > env > config > defaults
    let lk_url = args
        .url
        .or_else(|| env::var("LIVEKIT_URL").ok())
        .unwrap_or(cfg.livekit.url);
    let lk_key = args
        .api_key
        .or_else(|| env::var("LIVEKIT_API_KEY").ok())
        .unwrap_or(cfg.livekit.api_key);
    let lk_secret = args
        .api_secret
        .or_else(|| env::var("LIVEKIT_API_SECRET").ok())
        .unwrap_or(cfg.livekit.api_secret);
    let room_name = args.room.unwrap_or(cfg.livekit.room);
    let identity = args.identity.unwrap_or(cfg.livekit.identity);

    if lk_url.is_empty() || lk_key.is_empty() || lk_secret.is_empty() {
        return Err(anyhow!(
            "LiveKit URL, API key, and API secret are required.\n\
             Set them via config.toml, environment variables, or CLI flags.\n\
             Run with --help for details."
        ));
    }

    let cam_index = args.camera_index.unwrap_or(cfg.video.camera_index);
    let vid_width = args.width.unwrap_or(cfg.video.width);
    let vid_height = args.height.unwrap_or(cfg.video.height);
    let vid_fps = args.fps.unwrap_or(cfg.video.fps);
    let vid_codec = parse_codec(&args.codec.unwrap_or(cfg.video.codec));
    let vid_bitrate = cfg.video.max_bitrate;
    let vid_simulcast = cfg.video.simulcast;

    let mic_name = args.mic.unwrap_or(cfg.audio.input_device);
    let spk_name = args.speaker.unwrap_or(cfg.audio.output_device);
    let sample_rate = cfg.audio.sample_rate;
    let channel_index = cfg.audio.channel_index;
    let volume = cfg.audio.volume.clamp(0.0, 1.0);

    let no_camera = args.no_camera || cfg.features.no_camera;
    let no_mic = args.no_mic || cfg.features.no_microphone;
    let no_playback = args.no_playback || cfg.features.no_playback;

    // Ctrl-C handler
    let shutdown = Arc::new(AtomicBool::new(false));
    {
        let s = shutdown.clone();
        tokio::spawn(async move {
            let _ = tokio::signal::ctrl_c().await;
            s.store(true, Ordering::Release);
        });
    }

    // Generate token
    let token = access_token::AccessToken::with_api_key(&lk_key, &lk_secret)
        .with_identity(&identity)
        .with_name(&identity)
        .with_grants(access_token::VideoGrants {
            room_join: true,
            room: room_name.clone(),
            can_publish: true,
            can_subscribe: true,
            ..Default::default()
        })
        .to_jwt()?;

    // Print banner
    println!();
    println!("  RK3588 Video Call");
    println!("  -----------------");
    println!("  Room:     {}", room_name);
    println!("  Identity: {}", identity);
    if !no_camera {
        println!(
            "  Camera:   index {} ({}x{} @{}fps {})",
            cam_index,
            vid_width,
            vid_height,
            vid_fps,
            vid_codec.as_str()
        );
    }
    if !no_mic {
        println!(
            "  Mic:      {}",
            if mic_name.is_empty() {
                "default"
            } else {
                &mic_name
            }
        );
    }
    if !no_playback {
        println!(
            "  Speaker:  {}",
            if spk_name.is_empty() {
                "default"
            } else {
                &spk_name
            }
        );
    }
    println!();

    // Connect to LiveKit
    info!("Connecting to {}...", lk_url);
    let mut room_opts = RoomOptions::default();
    room_opts.auto_subscribe = true;
    let (room, _rx) = Room::connect(&lk_url, &token, room_opts).await?;
    let room = Arc::new(room);
    info!("Connected to room: {}", room.name());

    // --------------- Audio playback (speaker) setup ---------------
    let mixer = AudioMixer::new(sample_rate, volume);
    let _playback_stream: Option<Stream> = if !no_playback {
        let host = cpal::default_host();
        let out_device = if spk_name.is_empty() {
            host.default_output_device()
                .ok_or_else(|| anyhow!("No default output device"))?
        } else {
            find_output_device(&spk_name)?
        };
        let out_supported = out_device.default_output_config()?;
        let out_config = StreamConfig {
            channels: 1,
            sample_rate: SampleRate(sample_rate),
            buffer_size: cpal::BufferSize::Default,
        };
        info!(
            "Speaker: {} ({}Hz)",
            out_device.name().unwrap_or_default(),
            sample_rate
        );
        Some(start_audio_playback(
            out_device,
            out_config,
            out_supported.sample_format(),
            mixer.clone(),
        )?)
    } else {
        None
    };

    // --------------- Room event handler (subscribe to remote tracks) ---------------
    {
        let room = room.clone();
        let shutdown = shutdown.clone();
        let mixer = mixer.clone();
        tokio::spawn(async move {
            let mut events = room.subscribe();
            while let Some(evt) = events.recv().await {
                if shutdown.load(Ordering::Relaxed) {
                    break;
                }
                match evt {
                    RoomEvent::ParticipantConnected(p) => {
                        println!("  >> {} joined", p.name());
                    }
                    RoomEvent::ParticipantDisconnected(p) => {
                        println!("  << {} left", p.name());
                    }
                    RoomEvent::TrackSubscribed {
                        track,
                        publication,
                        participant,
                    } => {
                        info!(
                            "Subscribed: {} from {} ({})",
                            publication.name(),
                            participant.name(),
                            publication.mime_type()
                        );
                        match track {
                            RemoteTrack::Video(vt) => {
                                println!(
                                    "  >> Receiving video from {} ({})",
                                    participant.name(),
                                    publication.mime_type()
                                );
                                let name = participant.name().to_string();
                                let shut = shutdown.clone();
                                let rt = tokio::runtime::Handle::current();
                                std::thread::spawn(move || {
                                    let mut stream = NativeVideoStream::new(vt.rtc_track());
                                    let mut count: u64 = 0;
                                    let mut last_log = Instant::now();
                                    loop {
                                        if shut.load(Ordering::Relaxed) {
                                            break;
                                        }
                                        let frame = rt.block_on(async {
                                            tokio::select! {
                                                _ = tokio::time::sleep(Duration::from_secs(1)) => None,
                                                f = stream.next() => f,
                                            }
                                        });
                                        if let Some(f) = frame {
                                            count += 1;
                                            if last_log.elapsed() >= Duration::from_secs(5) {
                                                let fps =
                                                    count as f64 / last_log.elapsed().as_secs_f64();
                                                println!(
                                                    "  [{}] {}x{} @ {:.1} fps",
                                                    name,
                                                    f.buffer.width(),
                                                    f.buffer.height(),
                                                    fps
                                                );
                                                count = 0;
                                                last_log = Instant::now();
                                            }
                                        }
                                    }
                                });
                            }
                            RemoteTrack::Audio(at) => {
                                info!("Audio track from {}", participant.name());
                                let mixer = mixer.clone();
                                let shut = shutdown.clone();
                                tokio::spawn(async move {
                                    let mut stream = NativeAudioStream::new(
                                        at.rtc_track(),
                                        sample_rate as i32,
                                        1,
                                    );
                                    while let Some(frame) = stream.next().await {
                                        if shut.load(Ordering::Relaxed) {
                                            break;
                                        }
                                        mixer.add_audio_data(&frame.data);
                                    }
                                });
                            }
                        }
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

    // --------------- Microphone publish ---------------
    let _mic_stream: Option<Stream> = if !no_mic {
        let host = cpal::default_host();
        let in_device = if mic_name.is_empty() {
            host.default_input_device()
                .ok_or_else(|| anyhow!("No default input device"))?
        } else {
            find_input_device(&mic_name)?
        };
        let in_supported = in_device.default_input_config()?;
        let num_channels = in_supported.channels() as u32;
        let in_config = StreamConfig {
            channels: in_supported.channels(),
            sample_rate: SampleRate(sample_rate),
            buffer_size: cpal::BufferSize::Default,
        };
        info!(
            "Mic: {} ({}Hz, {} ch, capturing ch {})",
            in_device.name().unwrap_or_default(),
            sample_rate,
            num_channels,
            channel_index
        );

        let audio_opts = AudioSourceOptions {
            echo_cancellation: cfg.audio.echo_cancellation,
            noise_suppression: cfg.audio.noise_suppression,
            auto_gain_control: cfg.audio.auto_gain_control,
        };
        let lk_audio_source = NativeAudioSource::new(audio_opts, sample_rate, 1, 1000);
        let audio_track = LocalAudioTrack::create_audio_track(
            "microphone",
            RtcAudioSource::Native(lk_audio_source.clone()),
        );
        room.local_participant()
            .publish_track(
                LocalTrack::Audio(audio_track),
                TrackPublishOptions {
                    source: TrackSource::Microphone,
                    ..Default::default()
                },
            )
            .await?;
        info!("Microphone track published");

        // Mic capture -> LiveKit
        let (mic_tx, mut mic_rx) = mpsc::unbounded_channel::<Vec<i16>>();
        let stream = start_audio_capture(
            in_device,
            in_config,
            in_supported.sample_format(),
            mic_tx,
            channel_index,
            num_channels,
        )?;

        // Pump mic data into LiveKit in 10ms chunks
        let samples_per_10ms = (sample_rate / 100) as usize;
        tokio::spawn(async move {
            let mut buf: Vec<i16> = Vec::new();
            while let Some(data) = mic_rx.recv().await {
                buf.extend_from_slice(&data);
                while buf.len() >= samples_per_10ms {
                    let chunk: Vec<i16> = buf.drain(..samples_per_10ms).collect();
                    let frame = AudioFrame {
                        data: Cow::Owned(chunk),
                        sample_rate,
                        num_channels: 1,
                        samples_per_channel: samples_per_10ms as u32,
                    };
                    if let Err(e) = lk_audio_source.capture_frame(&frame).await {
                        error!("Mic capture_frame error: {}", e);
                    }
                }
            }
        });

        Some(stream)
    } else {
        None
    };

    // --------------- Camera publish ---------------
    if !no_camera {
        info!("Opening camera index {}...", cam_index);
        let index = CameraIndex::Index(cam_index);
        let fmt = CameraFormat::new(
            Resolution::new(vid_width, vid_height),
            FrameFormat::MJPEG,
            vid_fps,
        );
        let requested = RequestedFormat::new::<RgbFormat>(RequestedFormatType::Closest(fmt));
        let mut camera = match Camera::new(index.clone(), requested) {
            Ok(c) => c,
            Err(e) => {
                warn!("MJPEG request failed ({}), trying highest framerate...", e);
                let fallback =
                    RequestedFormat::new::<RgbFormat>(RequestedFormatType::AbsoluteHighestFrameRate);
                Camera::new(index, fallback)?
            }
        };
        camera.open_stream()?;

        let cam_fmt = camera.camera_format();
        let w = cam_fmt.width();
        let h = cam_fmt.height();
        info!(
            "Camera: {}x{} @{}fps {:?}",
            w,
            h,
            cam_fmt.frame_rate(),
            cam_fmt.format()
        );

        let source = NativeVideoSource::new(VideoResolution {
            width: w,
            height: h,
        });
        let track = LocalVideoTrack::create_video_track(
            "camera",
            RtcVideoSource::Native(source.clone()),
        );

        let pub_opts = TrackPublishOptions {
            source: TrackSource::Camera,
            video_codec: vid_codec,
            simulcast: vid_simulcast,
            video_encoding: Some(VideoEncoding {
                max_bitrate: vid_bitrate,
                max_framerate: vid_fps as f64,
            }),
            ..Default::default()
        };
        room.local_participant()
            .publish_track(LocalTrack::Video(track), pub_opts)
            .await?;
        info!("Camera track published ({} via MPP)", vid_codec.as_str());

        // Camera capture loop (runs on current task)
        let mut frame = VideoFrame {
            rotation: VideoRotation::VideoRotation0,
            timestamp_us: 0,
            buffer: I420Buffer::new(w, h),
        };
        let mut ticker = tokio::time::interval(Duration::from_secs_f64(1.0 / vid_fps as f64));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        ticker.tick().await;

        let start = Instant::now();
        let mut frame_count: u64 = 0;
        let mut last_stats = Instant::now();

        println!("  Camera is live. Press Ctrl-C to stop.\n");

        loop {
            if shutdown.load(Ordering::Acquire) {
                break;
            }
            ticker.tick().await;

            let frame_buf = match camera.frame() {
                Ok(f) => f,
                Err(e) => {
                    warn!("Camera frame error: {}", e);
                    continue;
                }
            };

            let src = frame_buf.buffer();
            let src_bytes: &[u8] = src.as_ref();
            let (stride_y, stride_u, stride_v) = frame.buffer.strides();
            let (data_y, data_u, data_v) = frame.buffer.data_mut();

            if frame_count < 3 {
                let nonzero = src_bytes.iter().filter(|&&b| b != 0).count();
                info!(
                    "Frame {}: {} bytes (expected RGB={}), non-zero={}, first 16={:?}",
                    frame_count,
                    src_bytes.len(),
                    w as usize * h as usize * 3,
                    nonzero,
                    &src_bytes[..std::cmp::min(16, src_bytes.len())]
                );
            }

            if src_bytes.len() == (w as usize * h as usize * 3) {
                unsafe {
                    yuv_sys::rs_RGB24ToI420(
                        src_bytes.as_ptr(),
                        (w * 3) as i32,
                        data_y.as_mut_ptr(),
                        stride_y as i32,
                        data_u.as_mut_ptr(),
                        stride_u as i32,
                        data_v.as_mut_ptr(),
                        stride_v as i32,
                        w as i32,
                        h as i32,
                    );
                }
            } else {
                // MJPEG
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
                        w as i32,
                        h as i32,
                        w as i32,
                        h as i32,
                    )
                };
                if ret != 0 {
                    warn!("MJPEG decode failed, skipping frame");
                    continue;
                }
            }

            frame.timestamp_us = start.elapsed().as_micros() as i64;
            source.capture_frame(&frame);
            frame_count += 1;

            if last_stats.elapsed() >= Duration::from_secs(5) {
                let fps = frame_count as f64 / last_stats.elapsed().as_secs_f64();
                println!(
                    "  Publishing: {}x{} @ {:.1} fps ({})",
                    w,
                    h,
                    fps,
                    vid_codec.as_str()
                );
                frame_count = 0;
                last_stats = Instant::now();
            }
        }
    } else {
        println!("  No camera. Waiting for remote tracks... (Ctrl-C to stop)\n");
        loop {
            if shutdown.load(Ordering::Acquire) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }

    println!("\n  Shutting down...");
    room.close().await?;
    Ok(())
}
