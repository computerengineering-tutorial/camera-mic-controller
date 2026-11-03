use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::fs::{File, OpenOptions};
use std::io::{Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

#[allow(dead_code)]
pub const DEFAULT_CAMERA_DEVICE: &str = "/dev/video10";
pub const FALLBACK_SINK_PATH: &str = "/tmp/rust_vcam.raw";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CameraStatus {
    pub device_path: String,
    pub is_v4l2: bool,
    pub exists: bool,
    pub is_on: bool,
    pub default_width: u32,
    pub default_height: u32,
    pub default_fps: u32,
}

#[allow(dead_code)]
pub struct CameraController {
    is_on: Arc<AtomicBool>,
}

#[allow(dead_code)]
impl CameraController {
    pub fn new() -> Self {
        Self {
            is_on: Arc::new(AtomicBool::new(true)),
        }
    }

    /// Detect existing V4L2 loopback devices or fallback device.
    pub fn detect_device(preferred: Option<&str>) -> (PathBuf, bool) {
        if let Some(pref) = preferred {
            let p = PathBuf::from(pref);
            if p.exists() {
                return (p, true);
            }
        }

        // Check common v4l2 devices
        for nr in [10, 0, 1, 2, 3, 4] {
            let dev = PathBuf::from(format!("/dev/video{}", nr));
            if dev.exists() {
                return (dev, true);
            }
        }

        // Fallback to shared frame buffer sink for simulation and testing
        (PathBuf::from(FALLBACK_SINK_PATH), false)
    }

    /// Check camera status.
    pub fn status(preferred: Option<&str>) -> CameraStatus {
        let (dev_path, is_v4l2) = Self::detect_device(preferred);
        let exists = dev_path.exists();

        CameraStatus {
            device_path: dev_path.to_string_lossy().to_string(),
            is_v4l2,
            exists,
            is_on: true,
            default_width: 640,
            default_height: 480,
            default_fps: 30,
        }
    }

    /// Instruction to load v4l2loopback if not present.
    pub fn loopback_load_instructions() -> String {
        "Kernel module v4l2loopback is not currently active.\n\
         To register /dev/video10 as a Virtual Camera for Chrome/Zoom/Discord, run:\n  \
         sudo modprobe v4l2loopback video_nr=10 card_label=\"RustVirtualCam\" exclusive_caps=1"
            .to_string()
    }

    /// Clean up virtual camera files and temporary sinks.
    pub fn teardown() -> Result<()> {
        let _ = std::fs::remove_file(FALLBACK_SINK_PATH);
        let _ = std::fs::remove_file("/tmp/rust_vcam.yuv");
        Ok(())
    }
}

pub struct CameraWriter {
    device_path: PathBuf,
    #[allow(dead_code)]
    is_v4l2: bool,
    v4l2_child: Option<std::process::Child>,
    v4l2_stdin: Option<std::process::ChildStdin>,
    file: Option<File>,
    #[allow(dead_code)]
    width: u32,
    #[allow(dead_code)]
    height: u32,
    blank_frame: Vec<u8>,
    pub is_on: Arc<AtomicBool>,
}

impl CameraWriter {
    pub fn open(
        target: Option<&str>,
        width: u32,
        height: u32,
    ) -> Result<Self> {
        let (device_path, is_v4l2) = CameraController::detect_device(target);

        // Precompute black YUV420p frame
        let y_size = (width * height) as usize;
        let uv_size = ((width / 2) * (height / 2)) as usize;
        let total_size = y_size + uv_size * 2;

        let mut blank_frame = vec![0u8; total_size];
        // Y = 16 (broadcast black)
        for i in 0..y_size {
            blank_frame[i] = 16;
        }
        // U = 128, V = 128 (neutral chroma)
        for i in y_size..total_size {
            blank_frame[i] = 128;
        }

        let mut v4l2_child = None;
        let mut v4l2_stdin = None;
        let mut file = None;

        if is_v4l2 {
            // Spawn FFmpeg V4L2 bridge: negotiates V4L2 caps, format and buffers for Chromium/WebRTC
            let mut cmd = std::process::Command::new("ffmpeg");
            cmd.args([
                "-v",
                "quiet",
                "-f",
                "rawvideo",
                "-pix_fmt",
                "yuv420p",
                "-s",
                &format!("{}x{}", width, height),
                "-r",
                "30",
                "-i",
                "pipe:0",
                "-f",
                "v4l2",
                "-pix_fmt",
                "yuv420p",
                device_path.to_str().unwrap_or(""),
            ])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());

            if let Ok(mut ch) = cmd.spawn() {
                v4l2_stdin = ch.stdin.take();
                v4l2_child = Some(ch);
            }
        } else {
            // Shared frame buffer file: rewound per frame to prevent pipe blocking & unbounded disk growth
            let _ = std::fs::remove_file(&device_path);
            file = OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(true)
                .open(&device_path)
                .ok();
        }

        Ok(Self {
            device_path,
            is_v4l2,
            v4l2_child,
            v4l2_stdin,
            file,
            width,
            height,
            blank_frame,
            is_on: Arc::new(AtomicBool::new(true)),
        })
    }

    pub fn turn_on(&self) {
        self.is_on.store(true, Ordering::SeqCst);
    }

    pub fn turn_off(&self) {
        self.is_on.store(false, Ordering::SeqCst);
    }

    pub fn is_camera_on(&self) -> bool {
        self.is_on.load(Ordering::SeqCst)
    }

    pub fn device_name(&self) -> String {
        self.device_path.to_string_lossy().to_string()
    }

    /// Push a raw YUV420p frame to the virtual camera device.
    /// If camera is toggled OFF, writes the blank black frame instead.
    pub fn write_frame(&mut self, frame: &[u8]) -> Result<()> {
        let frame_to_write = if self.is_camera_on() {
            frame
        } else {
            &self.blank_frame
        };

        if let Some(ref mut stdin) = self.v4l2_stdin {
            let _ = stdin.write_all(frame_to_write);
        } else if let Some(ref mut f) = self.file {
            let _ = f.seek(SeekFrom::Start(0));
            let _ = f.write_all(frame_to_write);
        }
        Ok(())
    }

    pub fn frame_size(&self) -> usize {
        self.blank_frame.len()
    }
}

impl Drop for CameraWriter {
    fn drop(&mut self) {
        if let Some(ref mut ch) = self.v4l2_child {
            let _ = ch.kill();
            let _ = ch.wait();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_yuv_frame_calculation() {
        let cam = CameraWriter::open(None, 640, 480).expect("Should open fallback camera sink");
        // Y size = 640*480 = 307,200
        // U size = 320*240 = 76,800
        // V size = 320*240 = 76,800
        // Total = 460,800 bytes
        assert_eq!(cam.frame_size(), 460800);
        assert_eq!(cam.blank_frame[0], 16); // Broadcast black Y
        assert_eq!(cam.blank_frame[307200], 128); // Chroma U
        assert_eq!(cam.blank_frame[307200 + 76800], 128); // Chroma V
    }

    #[test]
    fn test_camera_toggle_states() {
        let cam = CameraWriter::open(None, 320, 240).expect("Should open fallback camera sink");
        assert!(cam.is_camera_on());
        cam.turn_off();
        assert!(!cam.is_camera_on());
        cam.turn_on();
        assert!(cam.is_camera_on());
    }
}
