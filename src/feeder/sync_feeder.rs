use anyhow::{Context, Result};
use colored::*;
use crossbeam_channel::unbounded;
use std::io::Read;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use crate::device::camera::CameraWriter;
use crate::device::mic::{AudioPipeWriter, MicController};
use crate::feeder::audio::{AudioFeeder, BYTES_PER_SAMPLE, DEFAULT_CHANNELS, DEFAULT_SAMPLE_RATE};
use crate::feeder::playlist::{StreamControl, spawn_hotkey_listener};
use crate::feeder::video::{DEFAULT_FPS, DEFAULT_HEIGHT, DEFAULT_WIDTH, VideoFeeder};
use crate::ipc::IpcServer;
use crate::sync::clock::MasterClock;

#[derive(Debug, Clone)]
pub struct SyncConfig {
    pub media_files: Vec<PathBuf>,
    pub camera_target: Option<String>,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub sample_rate: u32,
    pub loop_mode: bool,
    pub initial_seek: Option<f64>,
    pub initial_volume: u32,
}

impl Default for SyncConfig {
    fn default() -> Self {
        Self {
            media_files: Vec::new(),
            camera_target: None,
            width: DEFAULT_WIDTH,
            height: DEFAULT_HEIGHT,
            fps: DEFAULT_FPS as u32,
            sample_rate: DEFAULT_SAMPLE_RATE,
            loop_mode: false,
            initial_seek: None,
            initial_volume: 100,
        }
    }
}

pub struct SyncFeeder;

impl SyncFeeder {
    /// Start unified synchronized streaming from a video playlist into both Camera and Mic.
    pub fn run(
        config: SyncConfig,
        is_mic_on: Arc<AtomicBool>,
        is_camera_on: Arc<AtomicBool>,
        running: Arc<AtomicBool>,
    ) -> Result<()> {
        if config.media_files.is_empty() {
            anyhow::bail!("Playlist contains no media files to stream.");
        }

        println!(
            "{}",
            "══════════════════════════════════════════════════════════════".bright_cyan()
        );
        println!(
            "{}",
            "   RUST A/V SYNCHRONIZED STREAMING CONTROLLER (LOW LATENCY)   "
                .bold()
                .white()
        );
        println!(
            "{}",
            "══════════════════════════════════════════════════════════════".bright_cyan()
        );
        println!(
            "  {} {} files queued",
            "Playlist:".bright_yellow(),
            config.media_files.len()
        );
        println!(
            "  {} {}x{} @ {} fps",
            "Video Stream:".bright_yellow(),
            config.width,
            config.height,
            config.fps
        );
        println!(
            "  {} {} Hz, 2ch s16le",
            "Audio Stream:".bright_yellow(),
            config.sample_rate
        );
        println!(
            "  {} {}",
            "Loop Mode:".bright_yellow(),
            if config.loop_mode {
                "ON".green().bold()
            } else {
                "OFF".red().bold()
            }
        );
        println!(
            "{}",
            "──────────────────────────────────────────────────────────────".bright_cyan()
        );
        println!("  Controls: [n] Next | [p] Prev | [l] Loop | [+] Vol+ | [-] Vol-");
        println!("            [f] +10s | [b] -10s | [space] Pause | [c] Cam | [m] Mic | [q] Quit");
        println!(
            "{}",
            "──────────────────────────────────────────────────────────────".bright_cyan()
        );

        // Initialize Virtual Camera and Mic writers once
        let mut cam_writer = CameraWriter::open(
            config.camera_target.as_deref(),
            config.width,
            config.height,
            config.fps,
        )?;
        println!(
            "  {} {}",
            "Virtual Camera:".green(),
            cam_writer.device_name()
        );

        let mut audio_writer = AudioPipeWriter::open(config.sample_rate, DEFAULT_CHANNELS)?;
        println!("  {} Rust_VirtualMic (Active)", "Virtual Mic:".green());
        println!(
            "{}",
            "──────────────────────────────────────────────────────────────".bright_cyan()
        );

        let (ctrl_tx, ctrl_rx) = unbounded::<StreamControl>();
        spawn_hotkey_listener(ctrl_tx.clone());

        // Start IPC Server for external CLI control (cam-mic camera off/on, mic off/on)
        let _ipc_handle = IpcServer::start(
            is_camera_on.clone(),
            is_mic_on.clone(),
            ctrl_tx,
            running.clone(),
        )?;

        let chunk_samples = 960usize; // 20ms audio slice
        let audio_chunk_bytes = chunk_samples * DEFAULT_CHANNELS as usize * BYTES_PER_SAMPLE;
        let video_frame_bytes = (config.width * config.height * 3 / 2) as usize;

        let mut track_idx = 0usize;
        let mut loop_mode = config.loop_mode;
        let mut volume_factor = (config.initial_volume as f32 / 100.0).clamp(0.0, 3.0);
        let mut seek_pos = config.initial_seek;
        let mut is_paused = false;

        // Apply initial volume if set
        if config.initial_volume != 100 {
            let _ = MicController::set_volume(config.initial_volume);
        }

        while running.load(Ordering::Relaxed) && track_idx < config.media_files.len() {
            let current_media = &config.media_files[track_idx];
            println!(
                "\n  {} [{}/{}] {}",
                "Now Playing:".bright_yellow().bold(),
                track_idx + 1,
                config.media_files.len(),
                current_media.display()
            );

            // Spawn decoders with seek
            let mut v_child = VideoFeeder::spawn_decoder(
                current_media,
                config.width,
                config.height,
                config.fps,
                seek_pos,
            )?;
            let mut v_stdout = v_child
                .stdout
                .take()
                .context("Failed to take video decoder stdout")?;

            let mut a_child_raw = AudioFeeder::spawn_decoder(current_media, seek_pos)?;
            let mut a_stdout = a_child_raw
                .stdout
                .take()
                .context("Failed to take audio decoder stdout")?;

            let a_child_shared = Arc::new(std::sync::Mutex::new(Some(a_child_raw)));
            let a_child_for_thread = a_child_shared.clone();

            let current_start_sec = seek_pos.unwrap_or(0.0);
            seek_pos = None; // Reset seek after consuming

            // Synchronizer clocks & thread communication (both start from 0 for the decoder streams)
            let audio_pts_ns = Arc::new(AtomicU64::new(0));
            let video_pts_ns = Arc::new(AtomicU64::new(0));

            let track_running = Arc::new(AtomicBool::new(true));
            let a_running = track_running.clone();
            let a_mic_on = is_mic_on.clone();
            let a_pts_clone = audio_pts_ns.clone();

            let a_writer_mutex = Arc::new(std::sync::Mutex::new(audio_writer));
            let a_writer_clone = a_writer_mutex.clone();

            let a_volume_factor = Arc::new(std::sync::atomic::AtomicU32::new(
                (volume_factor * 1000.0) as u32,
            ));
            let a_vol_clone = a_volume_factor.clone();

            let a_paused_flag = Arc::new(AtomicBool::new(false));
            let a_paused_clone = a_paused_flag.clone();

            // AUDIO THREAD
            let audio_handle = std::thread::Builder::new()
                .name("audio-feeder".to_string())
                .spawn(move || -> Result<()> {
                    let mut a_buf = vec![0u8; audio_chunk_bytes];
                    let a_silence = vec![0u8; audio_chunk_bytes];
                    let mut clock = MasterClock::new(config.sample_rate, config.fps as f64);

                    while a_running.load(Ordering::Relaxed) {
                        if a_paused_clone.load(Ordering::Relaxed) {
                            if let Ok(mut w) = a_writer_clone.lock() {
                                let _ = w.write_pcm(&a_silence);
                            }
                            std::thread::sleep(Duration::from_millis(20));
                            continue;
                        }

                        match a_stdout.read_exact(&mut a_buf) {
                            Ok(_) => {
                                let pts = clock.advance_audio(chunk_samples as u64);
                                a_pts_clone.store(pts.as_nanos() as u64, Ordering::Relaxed);

                                if let Ok(mut w) = a_writer_clone.lock() {
                                    if a_mic_on.load(Ordering::Relaxed) {
                                        let v_factor =
                                            a_vol_clone.load(Ordering::Relaxed) as f32 / 1000.0;
                                        AudioFeeder::apply_software_volume(&mut a_buf, v_factor);
                                        w.write_pcm(&a_buf)?;
                                    } else {
                                        w.write_pcm(&a_silence)?;
                                    }
                                }

                                clock.wait_until_pts(pts);
                            }
                            Err(ref e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                                break;
                            }
                            Err(e) => return Err(e.into()),
                        }
                    }

                    if let Ok(mut lock) = a_child_for_thread.lock()
                        && let Some(mut ch) = lock.take()
                    {
                        let _ = ch.kill();
                        let _ = ch.wait();
                    }
                    Ok(())
                })?;

            // VIDEO & CONTROLLER LOOP
            let mut v_buf = vec![0u8; video_frame_bytes];
            let mut clock = MasterClock::new(config.sample_rate, config.fps as f64);
            let mut total_frames = 0u64;
            let mut dropped_frames = 0u64;
            let start_time = Instant::now();
            let mut last_status_update = Instant::now();
            let mut current_video_sec = current_start_sec;

            while running.load(Ordering::Relaxed) && track_running.load(Ordering::Relaxed) {
                // Check interactive controls
                while let Ok(ctrl) = ctrl_rx.try_recv() {
                    match ctrl {
                        StreamControl::Next => {
                            if track_idx + 1 < config.media_files.len() {
                                track_idx += 1;
                            } else if loop_mode {
                                track_idx = 0;
                            }
                            track_running.store(false, Ordering::SeqCst);
                            break;
                        }
                        StreamControl::Prev => {
                            if track_idx > 0 {
                                track_idx -= 1;
                            } else {
                                track_idx = config.media_files.len().saturating_sub(1);
                            }
                            track_running.store(false, Ordering::SeqCst);
                            break;
                        }
                        StreamControl::SeekDelta(delta) => {
                            let new_pos = (current_video_sec + delta).max(0.0);
                            seek_pos = Some(new_pos);
                            track_running.store(false, Ordering::SeqCst);
                            break;
                        }
                        StreamControl::TogglePause => {
                            is_paused = !is_paused;
                            a_paused_flag.store(is_paused, Ordering::SeqCst);
                            println!(
                                "\n  {} {}",
                                "Playback:".bright_cyan(),
                                if is_paused {
                                    "PAUSED".yellow().bold()
                                } else {
                                    "RESUMED".green().bold()
                                }
                            );
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
                        StreamControl::VolumeDelta(d) => {
                            volume_factor = (volume_factor + (d as f32 / 100.0)).clamp(0.0, 3.0);
                            a_volume_factor
                                .store((volume_factor * 1000.0) as u32, Ordering::SeqCst);
                            println!(
                                "\n  {} {:.0}%",
                                "Volume:".bright_cyan(),
                                volume_factor * 100.0
                            );
                        }
                        StreamControl::ToggleCamera => {
                            let cur = is_camera_on.load(Ordering::SeqCst);
                            is_camera_on.store(!cur, Ordering::SeqCst);
                        }
                        StreamControl::ToggleMic => {
                            let cur = is_mic_on.load(Ordering::SeqCst);
                            is_mic_on.store(!cur, Ordering::SeqCst);
                            let _ = MicController::set_mute(cur);
                        }
                        StreamControl::Stop => {
                            running.store(false, Ordering::SeqCst);
                            track_running.store(false, Ordering::SeqCst);
                            break;
                        }
                    }
                }

                if !track_running.load(Ordering::Relaxed) {
                    break;
                }

                if is_paused {
                    // Write blank/freeze frame
                    let _ = cam_writer.write_frame(&v_buf);
                    std::thread::sleep(Duration::from_millis(33));
                    continue;
                }

                match v_stdout.read_exact(&mut v_buf) {
                    Ok(_) => {
                        total_frames += 1;
                        let target_v_pts = clock.advance_video();
                        video_pts_ns.store(target_v_pts.as_nanos() as u64, Ordering::Relaxed);
                        current_video_sec = current_start_sec + target_v_pts.as_secs_f64();

                        // Measure drift against concurrent audio thread
                        let a_pts_duration =
                            Duration::from_nanos(audio_pts_ns.load(Ordering::Relaxed));
                        let a_sec = a_pts_duration.as_secs_f64();
                        let v_sec = target_v_pts.as_secs_f64();
                        let drift_ms = (v_sec - a_sec) * 1000.0;

                        if drift_ms < -40.0 {
                            dropped_frames += 1;
                            continue;
                        }

                        clock.wait_until_pts(target_v_pts);

                        cam_writer
                            .is_on
                            .store(is_camera_on.load(Ordering::Relaxed), Ordering::Relaxed);
                        cam_writer.write_frame(&v_buf)?;

                        // Status update
                        if last_status_update.elapsed() >= Duration::from_millis(250) {
                            last_status_update = Instant::now();
                            let cam_badge = if is_camera_on.load(Ordering::Relaxed) {
                                "[CAM: ON]".green().bold()
                            } else {
                                "[CAM: OFF]".red().bold()
                            };
                            let mic_badge = if is_mic_on.load(Ordering::Relaxed) {
                                "[MIC: ON]".green().bold()
                            } else {
                                "[MIC: OFF]".red().bold()
                            };

                            let loop_badge = if loop_mode {
                                "[LOOP]".cyan()
                            } else {
                                "".normal()
                            };
                            let drift_str = if drift_ms.abs() < 10.0 {
                                format!("{:+.1} ms", drift_ms).green()
                            } else {
                                format!("{:+.1} ms", drift_ms).yellow()
                            };

                            print!(
                                "\r  {} {} {} | Pos: {:.1}s ({:.1}s) | Frames: {} | Vol: {:.0}% | Drift: {} | Drop: {}   ",
                                cam_badge,
                                mic_badge,
                                loop_badge,
                                current_video_sec,
                                start_time.elapsed().as_secs_f64(),
                                total_frames,
                                volume_factor * 100.0,
                                drift_str,
                                dropped_frames
                            );
                            use std::io::Write;
                            let _ = std::io::stdout().flush();
                        }
                    }
                    Err(ref e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                        println!("\n  {} Track finished.", "End of track:".cyan());
                        break;
                    }
                    Err(e) => {
                        eprintln!("\nError reading video frame: {}", e);
                        break;
                    }
                }
            }

            // Cleanup track processes cleanly: kill video child and audio child directly
            track_running.store(false, Ordering::SeqCst);
            let _ = v_child.kill();
            let _ = v_child.wait();

            if let Ok(mut lock) = a_child_shared.lock()
                && let Some(mut ch) = lock.take()
            {
                let _ = ch.kill();
                let _ = ch.wait();
            }
            let _ = audio_handle.join();

            // Reclaim audio_writer from Arc Mutex
            audio_writer = match Arc::try_unwrap(a_writer_mutex) {
                Ok(mutex) => mutex.into_inner().unwrap(),
                Err(_) => AudioPipeWriter::open(config.sample_rate, DEFAULT_CHANNELS)?,
            };

            if !running.load(Ordering::Relaxed) {
                break;
            }

            // If seek was requested on the same track, do not advance track_idx
            if seek_pos.is_some() {
                continue;
            }

            // Advance playlist
            if track_idx + 1 < config.media_files.len() {
                track_idx += 1;
            } else if loop_mode {
                track_idx = 0;
            } else {
                break;
            }
        }

        running.store(false, Ordering::SeqCst);
        crate::feeder::playlist::restore_terminal();
        println!(
            "\n{}",
            "A/V streaming session ended cleanly.".bright_green()
        );
        Ok(())
    }
}
