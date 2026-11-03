use anyhow::{Context, Result};
use colored::*;
use crossbeam_channel::unbounded;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crate::device::camera::CameraWriter;
use crate::feeder::playlist::{StreamControl, spawn_hotkey_listener};
use crate::feeder::process::ManagedChild;
use crate::ipc::IpcServer;
use crate::sync::clock::MasterClock;

pub const DEFAULT_WIDTH: u32 = 640;
pub const DEFAULT_HEIGHT: u32 = 480;
pub const DEFAULT_FPS: f64 = 30.0;

#[derive(Debug, Clone)]
pub struct VideoFeedConfig {
    pub playlist: Vec<PathBuf>,
    pub camera_target: Option<String>,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub initial_seek: Option<f64>,
    pub initial_loop: bool,
}

pub struct VideoFeeder;

impl VideoFeeder {
    /// Spawn FFmpeg to decode video frames to raw YUV420p with optional seek.
    pub fn spawn_decoder(
        file_path: &Path,
        width: u32,
        height: u32,
        fps: u32,
        seek_seconds: Option<f64>,
    ) -> Result<Child> {
        let mut cmd = Command::new("ffmpeg");
        cmd.arg("-v").arg("quiet");

        if let Some(ss) = seek_seconds.filter(|&s| s > 0.0) {
            cmd.arg("-ss").arg(format!("{:.3}", ss));
        }

        cmd.args([
            "-i",
            file_path.to_str().unwrap_or(""),
            "-an", // No audio
            "-f",
            "rawvideo",
            "-pix_fmt",
            "yuv420p",
            "-s",
            &format!("{}x{}", width, height),
            "-r",
            &fps.to_string(),
            "pipe:1",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::null());

        let child = cmd
            .spawn()
            .context("Failed to spawn ffmpeg video decoder")?;
        Ok(child)
    }

    /// Generate a synthetic moving pattern YUV420p frame.
    pub fn generate_synthetic_frame(
        width: u32,
        height: u32,
        frame_idx: u64,
        is_flash: bool,
    ) -> Vec<u8> {
        let y_size = (width * height) as usize;
        let uv_size = ((width / 2) * (height / 2)) as usize;
        let total_size = y_size + uv_size * 2;

        let mut frame = vec![0u8; total_size];

        // Background Y
        let bg_y = if is_flash { 235u8 } else { 40u8 };
        frame[0..y_size].fill(bg_y);

        // Moving box based on frame_idx
        let box_size = 60u32;
        let box_x = ((frame_idx * 8) % (width - box_size) as u64) as u32;
        let box_y = ((frame_idx * 4) % (height - box_size) as u64) as u32;

        for r in box_y..(box_y + box_size) {
            let row_offset = (r * width) as usize;
            for c in box_x..(box_x + box_size) {
                let idx = row_offset + c as usize;
                if idx < y_size {
                    frame[idx] = 210; // Bright box
                }
            }
        }

        // Neutral chroma U & V
        frame[y_size..total_size].fill(128);

        frame
    }

    /// Feed a playlist of video files into the virtual camera.
    pub fn feed_playlist(
        config: VideoFeedConfig,
        is_camera_on: Arc<AtomicBool>,
        running: Arc<AtomicBool>,
    ) -> Result<()> {
        let (ctrl_tx, ctrl_rx) = unbounded::<StreamControl>();
        spawn_hotkey_listener(ctrl_tx.clone());

        // Start IPC server for remote CLI control
        let is_mic_dummy = Arc::new(AtomicBool::new(false));
        let _ipc_handle =
            IpcServer::start(is_camera_on.clone(), is_mic_dummy, ctrl_tx, running.clone())?;

        let mut writer = CameraWriter::open(
            config.camera_target.as_deref(),
            config.width,
            config.height,
            config.fps,
        )?;
        let frame_size = (config.width * config.height * 3 / 2) as usize;
        let mut buf = vec![0u8; frame_size];

        let mut track_idx = 0usize;
        let mut loop_mode = config.initial_loop;
        let mut seek_pos = config.initial_seek;
        let mut is_paused = false;

        println!(
            "{}",
            "══════════════════════════════════════════════════════════════".bright_cyan()
        );
        println!(
            "{}",
            "           RUST VIRTUAL CAMERA STREAMING CONTROLLER           "
                .bold()
                .white()
        );
        println!(
            "{}",
            "══════════════════════════════════════════════════════════════".bright_cyan()
        );
        println!(
            "  Controls: [n] Next | [p] Prev | [l] Loop | [f] +10s | [b] -10s | [space] Pause | [c] Cam | [q] Stop"
        );
        println!(
            "{}",
            "──────────────────────────────────────────────────────────────".bright_cyan()
        );

        while running.load(Ordering::Relaxed) && track_idx < config.playlist.len() {
            let current_track = &config.playlist[track_idx];
            println!(
                "  {} [{}/{}] {}",
                "Playing:".bright_yellow(),
                track_idx + 1,
                config.playlist.len(),
                current_track.display()
            );

            let mut decoder = ManagedChild::new(Self::spawn_decoder(
                current_track,
                config.width,
                config.height,
                config.fps,
                seek_pos,
            )?);
            let mut stdout = decoder
                .as_mut()
                .unwrap()
                .stdout
                .take()
                .context("Failed to take video stdout")?;
            seek_pos = None;

            let mut clock = MasterClock::new(48000, config.fps as f64);
            let mut current_sec = 0.0f64;

            while running.load(Ordering::Relaxed) {
                while let Ok(ctrl) = ctrl_rx.try_recv() {
                    match ctrl {
                        StreamControl::Next => {
                            if track_idx + 1 < config.playlist.len() {
                                track_idx += 1;
                            } else if loop_mode {
                                track_idx = 0;
                            }
                            match Self::spawn_decoder(
                                &config.playlist[track_idx],
                                config.width,
                                config.height,
                                config.fps,
                                None,
                            ) {
                                Ok(mut ch) => {
                                    stdout = ch.stdout.take().unwrap();
                                    decoder.replace(ch);
                                }
                                Err(_) => break,
                            }
                            clock.reset();
                            current_sec = 0.0;
                            println!(
                                "\n  {} [{}/{}] {}",
                                "Skipped to:".bright_yellow(),
                                track_idx + 1,
                                config.playlist.len(),
                                config.playlist[track_idx].display()
                            );
                        }
                        StreamControl::Prev => {
                            if track_idx > 0 {
                                track_idx -= 1;
                            } else {
                                track_idx = config.playlist.len().saturating_sub(1);
                            }
                            match Self::spawn_decoder(
                                &config.playlist[track_idx],
                                config.width,
                                config.height,
                                config.fps,
                                None,
                            ) {
                                Ok(mut ch) => {
                                    stdout = ch.stdout.take().unwrap();
                                    decoder.replace(ch);
                                }
                                Err(_) => break,
                            }
                            clock.reset();
                            current_sec = 0.0;
                            println!(
                                "\n  {} [{}/{}] {}",
                                "Previous track:".bright_yellow(),
                                track_idx + 1,
                                config.playlist.len(),
                                config.playlist[track_idx].display()
                            );
                        }
                        StreamControl::SeekDelta(delta) => {
                            let new_pos = (current_sec + delta).max(0.0);
                            match Self::spawn_decoder(
                                &config.playlist[track_idx],
                                config.width,
                                config.height,
                                config.fps,
                                Some(new_pos),
                            ) {
                                Ok(mut ch) => {
                                    stdout = ch.stdout.take().unwrap();
                                    decoder.replace(ch);
                                }
                                Err(_) => break,
                            }
                            current_sec = new_pos;
                            println!("\n  {} {:.1}s", "Seeked to:".bright_yellow(), new_pos);
                        }
                        StreamControl::TogglePause => {
                            is_paused = !is_paused;
                        }
                        StreamControl::ToggleLoop => {
                            loop_mode = !loop_mode;
                            println!(
                                "\n  {} {}",
                                "Loop Mode:".bright_cyan(),
                                if loop_mode {
                                    "ON".green().bold()
                                } else {
                                    "OFF".red().bold()
                                }
                            );
                        }
                        StreamControl::ToggleCamera => {
                            let cur = is_camera_on.load(Ordering::SeqCst);
                            is_camera_on.store(!cur, Ordering::SeqCst);
                        }
                        StreamControl::Stop => {
                            running.store(false, Ordering::SeqCst);
                            break;
                        }
                        _ => {}
                    }
                }

                if is_paused {
                    // Inject blank or freeze frame while paused
                    let _ = writer.write_frame(&buf);
                    std::thread::sleep(Duration::from_millis(33));
                    continue;
                }

                match stdout.read_exact(&mut buf) {
                    Ok(_) => {
                        let v_pts = clock.advance_video();
                        current_sec = clock.video_pts_seconds();

                        writer
                            .is_on
                            .store(is_camera_on.load(Ordering::Relaxed), Ordering::Relaxed);
                        writer.write_frame(&buf)?;

                        clock.wait_until_pts(v_pts);
                    }
                    Err(ref e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                        break;
                    }
                    Err(e) => return Err(e.into()),
                }
            }

            decoder.kill_and_wait();

            if !running.load(Ordering::Relaxed) {
                break;
            }

            if track_idx + 1 < config.playlist.len() {
                track_idx += 1;
            } else if loop_mode {
                track_idx = 0;
            } else {
                break;
            }
        }

        crate::feeder::playlist::restore_terminal();
        println!("\n{}", "Video stream completed.".bright_green());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_synthetic_frame_size() {
        let frame = VideoFeeder::generate_synthetic_frame(320, 240, 0, false);
        // 320 * 240 * 1.5 = 115,200 bytes
        assert_eq!(frame.len(), 115200);
        // Neutral chroma at U & V offsets
        let y_size = 320 * 240;
        assert_eq!(frame[y_size], 128);
        assert_eq!(frame[y_size + 1], 128);
    }
}
