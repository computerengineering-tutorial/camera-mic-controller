use anyhow::{Context, Result};
use colored::*;
use crossbeam_channel::unbounded;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crate::device::mic::{AudioPipeWriter, MicController};
use crate::feeder::playlist::{spawn_hotkey_listener, StreamControl};
use crate::sync::clock::MasterClock;

pub const DEFAULT_SAMPLE_RATE: u32 = 48000;
pub const DEFAULT_CHANNELS: u16 = 2;
pub const BYTES_PER_SAMPLE: usize = 2; // 16-bit PCM = 2 bytes

pub struct AudioFeeder;

impl AudioFeeder {
    /// Spawn FFmpeg to decode audio from a media file into raw PCM s16le, with optional seek.
    pub fn spawn_decoder(file_path: &Path, seek_seconds: Option<f64>) -> Result<Child> {
        let mut cmd = Command::new("ffmpeg");
        cmd.arg("-v").arg("quiet");

        if let Some(ss) = seek_seconds {
            if ss > 0.0 {
                cmd.arg("-ss").arg(format!("{:.3}", ss));
            }
        }

        cmd.args([
            "-i",
            file_path.to_str().unwrap_or(""),
            "-vn", // No video
            "-f",
            "s16le",
            "-acodec",
            "pcm_s16le",
            "-ar",
            &DEFAULT_SAMPLE_RATE.to_string(),
            "-ac",
            &DEFAULT_CHANNELS.to_string(),
            "pipe:1",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::null());

        let child = cmd.spawn().context("Failed to spawn ffmpeg audio decoder")?;
        Ok(child)
    }

    /// Apply software volume scaling to raw 16-bit PCM bytes.
    pub fn apply_software_volume(chunk: &mut [u8], volume_factor: f32) {
        if (volume_factor - 1.0).abs() < 0.01 {
            return;
        }
        for chunk_slice in chunk.chunks_exact_mut(2) {
            let sample = i16::from_le_bytes([chunk_slice[0], chunk_slice[1]]);
            let scaled = (sample as f32 * volume_factor).clamp(i16::MIN as f32, i16::MAX as f32) as i16;
            let bytes = scaled.to_le_bytes();
            chunk_slice[0] = bytes[0];
            chunk_slice[1] = bytes[1];
        }
    }

    /// Feed a playlist of audio files into the virtual microphone with loop, seek, and interactive controls.
    pub fn feed_playlist(
        playlist: Vec<PathBuf>,
        initial_seek: Option<f64>,
        initial_volume: u32,
        initial_loop: bool,
        is_mic_on: Arc<AtomicBool>,
        running: Arc<AtomicBool>,
    ) -> Result<()> {
        let (ctrl_tx, ctrl_rx) = unbounded::<StreamControl>();
        spawn_hotkey_listener(ctrl_tx);

        let mut writer = AudioPipeWriter::open(DEFAULT_SAMPLE_RATE, DEFAULT_CHANNELS)?;
        let chunk_samples = 960usize; // 20ms at 48kHz
        let chunk_bytes = chunk_samples * DEFAULT_CHANNELS as usize * BYTES_PER_SAMPLE;
        let mut buf = vec![0u8; chunk_bytes];
        let silence = vec![0u8; chunk_bytes];

        let mut track_idx = 0usize;
        let mut loop_mode = initial_loop;
        let mut volume_factor = (initial_volume as f32 / 100.0).clamp(0.0, 3.0);
        let mut seek_pos = initial_seek;
        let mut is_paused = false;

        println!("{}", "══════════════════════════════════════════════════════════════".bright_cyan());
        println!("{}", "         RUST VIRTUAL MICROPHONE STREAMING CONTROLLER         ".bold().white());
        println!("{}", "══════════════════════════════════════════════════════════════".bright_cyan());
        println!("  Controls: [n] Next | [p] Prev | [l] Loop | [+] Vol+ | [-] Vol- | [f] +10s | [b] -10s | [space] Pause | [m] Mic | [q] Stop");
        println!("{}", "──────────────────────────────────────────────────────────────".bright_cyan());

        while running.load(Ordering::Relaxed) && track_idx < playlist.len() {
            let current_track = &playlist[track_idx];
            println!(
                "  {} [{}/{}] {}",
                "Playing:".bright_yellow(),
                track_idx + 1,
                playlist.len(),
                current_track.display()
            );

            let mut decoder = Self::spawn_decoder(current_track, seek_pos)?;
            let mut stdout = decoder.stdout.take().context("Failed to take audio stdout")?;
            seek_pos = None; // Reset seek after consuming

            let mut clock = MasterClock::new(DEFAULT_SAMPLE_RATE, 30.0);
            let mut current_sec = 0.0f64;

            while running.load(Ordering::Relaxed) {
                // Check interactive controls
                while let Ok(ctrl) = ctrl_rx.try_recv() {
                    match ctrl {
                        StreamControl::Next => {
                            if track_idx + 1 < playlist.len() {
                                track_idx += 1;
                            } else if loop_mode {
                                track_idx = 0;
                            }
                            let _ = decoder.kill();
                            let _ = decoder.wait();
                            stdout = match Self::spawn_decoder(&playlist[track_idx], None) {
                                Ok(mut ch) => ch.stdout.take().unwrap(),
                                Err(_) => break,
                            };
                            clock.reset();
                            current_sec = 0.0;
                            println!("\n  {} [{}/{}] {}", "Skipped to:".bright_yellow(), track_idx + 1, playlist.len(), playlist[track_idx].display());
                        }
                        StreamControl::Prev => {
                            if track_idx > 0 {
                                track_idx -= 1;
                            } else {
                                track_idx = playlist.len().saturating_sub(1);
                            }
                            let _ = decoder.kill();
                            let _ = decoder.wait();
                            stdout = match Self::spawn_decoder(&playlist[track_idx], None) {
                                Ok(mut ch) => ch.stdout.take().unwrap(),
                                Err(_) => break,
                            };
                            clock.reset();
                            current_sec = 0.0;
                            println!("\n  {} [{}/{}] {}", "Previous track:".bright_yellow(), track_idx + 1, playlist.len(), playlist[track_idx].display());
                        }
                        StreamControl::SeekDelta(delta) => {
                            let new_pos = (current_sec + delta).max(0.0);
                            let _ = decoder.kill();
                            let _ = decoder.wait();
                            stdout = match Self::spawn_decoder(&playlist[track_idx], Some(new_pos)) {
                                Ok(mut ch) => ch.stdout.take().unwrap(),
                                Err(_) => break,
                            };
                            current_sec = new_pos;
                            println!("\n  {} {:.1}s", "Seeked to:".bright_yellow(), new_pos);
                        }
                        StreamControl::TogglePause => {
                            is_paused = !is_paused;
                        }
                        StreamControl::ToggleLoop => {
                            loop_mode = !loop_mode;
                            println!("\n  {} {}", "Loop Mode:".bright_cyan(), if loop_mode { "ON".green().bold() } else { "OFF".red().bold() });
                        }
                        StreamControl::VolumeDelta(d) => {
                            volume_factor = (volume_factor + (d as f32 / 100.0)).clamp(0.0, 3.0);
                            let _ = MicController::adjust_volume(d);
                            println!("\n  {} {:.0}%", "Volume:".bright_cyan(), volume_factor * 100.0);
                        }
                        StreamControl::ToggleMic => {
                            let cur = is_mic_on.load(Ordering::SeqCst);
                            is_mic_on.store(!cur, Ordering::SeqCst);
                            let _ = MicController::set_mute(cur);
                        }
                        StreamControl::Stop => {
                            running.store(false, Ordering::SeqCst);
                            break;
                        }
                        _ => {}
                    }
                }

                if is_paused {
                    writer.write_pcm(&silence)?;
                    std::thread::sleep(Duration::from_millis(20));
                    continue;
                }

                match stdout.read_exact(&mut buf) {
                    Ok(_) => {
                        let audio_pts = clock.advance_audio(chunk_samples as u64);
                        current_sec = audio_pts.as_secs_f64();

                        if is_mic_on.load(Ordering::Relaxed) {
                            Self::apply_software_volume(&mut buf, volume_factor);
                            writer.write_pcm(&buf)?;
                        } else {
                            writer.write_pcm(&silence)?;
                        }

                        clock.wait_until_pts(audio_pts);
                    }
                    Err(ref e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                        break;
                    }
                    Err(e) => return Err(e.into()),
                }
            }

            let _ = decoder.kill();
            let _ = decoder.wait();

            if !running.load(Ordering::Relaxed) {
                break;
            }

            // Playlist advancement
            if track_idx + 1 < playlist.len() {
                track_idx += 1;
            } else if loop_mode {
                track_idx = 0;
            } else {
                break;
            }
        }

        println!("\n{}", "Audio stream completed.".bright_green());
        Ok(())
    }
}
