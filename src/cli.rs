use clap::{Args, Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(
    name = "cam-mic",
    author = "Jenil Sheth <jenilsheth@example.com>",
    version = "0.1.0",
    about = "High-performance, low-latency Rust Virtual Camera & Microphone Controller",
    long_about = "A high-performance Linux CLI tool written in Rust to control virtual camera and microphone feeds, \
                  support live media injection, toggle devices, and ensure microsecond-level Audio/Video synchronization."
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,

    /// Output responses in machine-readable JSON format
    #[arg(long, global = true)]
    pub json: bool,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Manage Virtual Microphone state, feeding, and lifecycle
    Mic(MicArgs),

    /// Manage Virtual Camera state, feeding, and lifecycle
    Camera(CameraArgs),

    /// Stream video into camera and video's audio into mic with real-time A/V synchronization
    #[command(name = "sync-feed")]
    SyncFeed(SyncFeedArgs),

    /// Check system environment and provision virtual devices
    Setup,

    /// Destroy and remove all virtual camera and mic devices, restoring original system state
    #[command(name = "destroy", alias = "clean")]
    Destroy,

    /// Run real-world synchronization, latency, and throughput verification tests
    Test,
}

#[derive(Args, Debug)]
pub struct MicArgs {
    #[command(subcommand)]
    pub action: MicAction,
}

#[derive(Subcommand, Debug)]
pub enum MicAction {
    /// Turn Virtual Microphone ON (unmute)
    On,
    /// Turn Virtual Microphone OFF (mute)
    Off,
    /// Display current virtual microphone status
    Status,
    /// Feed audio track(s) directly into the virtual microphone
    Feed {
        /// Path(s) to audio or media files (MP3, WAV, AAC, MP4, etc.)
        #[arg(required_unless_present = "playlist")]
        files: Vec<PathBuf>,

        /// Path to a text file containing list of audio files (one per line)
        #[arg(long)]
        playlist: Option<PathBuf>,

        /// Loop playback continuously
        #[arg(short, long)]
        r#loop: bool,

        /// Start position in seconds (e.g. 15.0)
        #[arg(short, long)]
        seek: Option<f64>,

        /// Initial volume percentage (0 - 150)
        #[arg(short, long, default_value = "100")]
        volume: u32,
    },
    /// Teardown and clean up virtual microphone devices
    Teardown,
}

#[derive(Args, Debug)]
pub struct CameraArgs {
    #[command(subcommand)]
    pub action: CameraAction,
}

#[derive(Subcommand, Debug)]
pub enum CameraAction {
    /// Turn Virtual Camera ON (feed live frames)
    On,
    /// Turn Virtual Camera OFF (inject blank frame)
    Off,
    /// Display current virtual camera status
    Status,
    /// Feed video track(s) directly into the virtual camera
    Feed {
        /// Path(s) to video files (MP4, MKV, AVI, etc.)
        #[arg(required_unless_present = "playlist")]
        files: Vec<PathBuf>,

        /// Path to a text file containing list of video files (one per line)
        #[arg(long)]
        playlist: Option<PathBuf>,

        /// Loop playback continuously
        #[arg(short, long)]
        r#loop: bool,

        /// Start position in seconds (e.g. 15.0)
        #[arg(short, long)]
        seek: Option<f64>,

        /// Target V4L2 device (e.g. /dev/video10) or FIFO sink
        #[arg(long)]
        device: Option<String>,

        /// Frame width (default: 640)
        #[arg(long, default_value = "640")]
        width: u32,

        /// Frame height (default: 480)
        #[arg(long, default_value = "480")]
        height: u32,

        /// Target framerate (default: 30)
        #[arg(long, default_value = "30")]
        fps: u32,
    },
    /// Destroy and clean up virtual camera devices and temporary frame buffers
    Teardown,
}

#[derive(Args, Debug)]
pub struct SyncFeedArgs {
    /// Path(s) to input video file(s) containing both video and audio streams
    #[arg(required_unless_present = "playlist")]
    pub files: Vec<PathBuf>,

    /// Path to a text file containing list of video files (one per line)
    #[arg(long)]
    pub playlist: Option<PathBuf>,

    /// Loop playback continuously
    #[arg(short, long)]
    pub r#loop: bool,

    /// Initial start position in seconds (e.g. 10.5)
    #[arg(short, long)]
    pub seek: Option<f64>,

    /// Initial volume percentage (0 - 150)
    #[arg(short, long, default_value = "100")]
    pub volume: u32,

    /// Target V4L2 device (e.g. /dev/video10) or FIFO sink
    #[arg(long)]
    pub device: Option<String>,

    /// Video frame width (default: 640)
    #[arg(long, default_value = "640")]
    pub width: u32,

    /// Video frame height (default: 480)
    #[arg(long, default_value = "480")]
    pub height: u32,

    /// Target framerate (default: 30)
    #[arg(long, default_value = "30")]
    pub fps: u32,

    /// Target audio sample rate (default: 48000)
    #[arg(long, default_value = "48000")]
    pub sample_rate: u32,
}

