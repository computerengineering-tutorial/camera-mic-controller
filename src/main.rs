mod cli;
mod device;
mod feeder;
pub mod ipc;
mod sync;
mod tester;

use anyhow::Result;
use clap::Parser;
use colored::*;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use cli::{CameraAction, Cli, Commands, MicAction};
use device::camera::CameraController;
use device::mic::MicController;
use feeder::audio::AudioFeeder;
use feeder::playlist::resolve_playlist;
use feeder::sync_feeder::{SyncConfig, SyncFeeder};
use feeder::video::{VideoFeedConfig, VideoFeeder};
use ipc::{IpcClient, IpcRequest};
use tester::TestRunner;

fn main() -> Result<()> {
    // 1. Install global panic hook to guarantee terminal is always restored
    let default_panic = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        feeder::playlist::restore_terminal();
        default_panic(info);
    }));

    // 2. RAII Guard to restore terminal on any exit path from main()
    struct TerminalGuard;
    impl Drop for TerminalGuard {
        fn drop(&mut self) {
            feeder::playlist::restore_terminal();
        }
    }
    let _term_guard = TerminalGuard;

    let args = Cli::parse();
    let json_mode = args.json;

    // 3. Graceful Ctrl+C and termination signal handler: IMMEDIATELY restores terminal
    let running = Arc::new(AtomicBool::new(true));
    let r_sig = running.clone();
    ctrlc::set_handler(move || {
        feeder::playlist::restore_terminal();
        r_sig.store(false, Ordering::SeqCst);
    })?;

    match args.command {
        Commands::Setup => {
            println!(
                "{}",
                "══════════════════════════════════════════════════════════════".bright_cyan()
            );
            println!(
                "{}",
                "   PROVISIONING INBUILT VIRTUAL CAMERA & MICROPHONE          "
                    .bold()
                    .white()
            );
            println!(
                "{}",
                "══════════════════════════════════════════════════════════════".bright_cyan()
            );

            // 1. Mic setup
            match MicController::ensure_virtual_mic() {
                Ok(_) => {
                    println!(
                        "  {} Virtual Microphone registered ({})",
                        "[OK]".green().bold(),
                        device::mic::VIRTUAL_SOURCE_NAME
                    );
                    println!(
                        "  {} Set as System Default Audio Input Device (all websites record this by default)",
                        "[OK]".green().bold()
                    );
                }
                Err(e) => {
                    eprintln!(
                        "  {} Virtual Microphone error: {}",
                        "[FAIL]".red().bold(),
                        e
                    );
                }
            }

            // 2. Camera setup check
            let cam_status = CameraController::status(None);
            if cam_status.is_v4l2 && cam_status.exists {
                println!(
                    "  {} V4L2 Hardware/Kernel Virtual Camera active at {}",
                    "[OK]".green().bold(),
                    cam_status.device_path
                );
                println!(
                    "  {} Websites & Chrome will detect this as 'VirtualCamera'",
                    "[OK]".green().bold()
                );
            } else {
                println!(
                    "  {} V4L2 kernel device not yet loaded (using fallback: {})",
                    "[PENDING]".yellow().bold(),
                    cam_status.device_path
                );
                println!(
                    "\n  {} To enable Chrome & websites to see the Virtual Camera as a real webcam, run:",
                    "[ACTION REQUIRED]".bright_yellow().bold()
                );
                println!("  {}", "sudo modprobe v4l2loopback exclusive_caps=1 card_label=\"VirtualCamera\" video_nr=10".bold().green());
            }
            println!(
                "{}",
                "──────────────────────────────────────────────────────────────".bright_cyan()
            );
            println!("  Test in Browser: Open https://webcamtests.com or https://mictests.com");
            println!(
                "{}",
                "══════════════════════════════════════════════════════════════".bright_cyan()
            );
        }

        Commands::Destroy => {
            println!(
                "{}",
                "Destroying & Cleaning Up Virtual Devices...".bold().white()
            );

            // 1. Unload virtual mic modules & restore user default recording device
            match MicController::teardown() {
                Ok(_) => {
                    println!(
                        "  {} Virtual Microphone sink & remap sources unloaded (original default restored)",
                        "[OK]".green().bold()
                    );
                }
                Err(e) => {
                    eprintln!(
                        "  {} Virtual Microphone cleanup error: {}",
                        "[FAIL]".red().bold(),
                        e
                    );
                }
            }

            // 2. Clean up camera sinks
            let _ = CameraController::teardown();
            let _ = std::fs::remove_file(ipc::IPC_SOCKET_PATH);
            println!(
                "  {} Virtual Camera files, IPC socket, and frame buffers removed",
                "[OK]".green().bold()
            );

            // 3. Inform about v4l2loopback module if loaded
            let v4l2_loaded = std::process::Command::new("lsmod")
                .output()
                .map(|out| String::from_utf8_lossy(&out.stdout).contains("v4l2loopback"))
                .unwrap_or(false);

            if v4l2_loaded {
                println!(
                    "  {} Kernel module v4l2loopback is still active. To unload it, run: sudo modprobe -r v4l2loopback",
                    "[INFO]".yellow().bold()
                );
            }

            println!(
                "\n{} System restored to original clean state.",
                "[DONE]".green().bold()
            );
        }

        Commands::Mic(mic_args) => match mic_args.action {
            MicAction::On => {
                MicController::turn_on()?;
                let _ = IpcClient::send(&IpcRequest::SetMic(true));
                if json_mode {
                    println!("{}", serde_json::to_string(&MicController::status()?)?);
                } else {
                    println!(
                        "{} Virtual Microphone is now {} (unmuted)",
                        "[STATUS]".bright_cyan().bold(),
                        "ON".green().bold()
                    );
                }
            }
            MicAction::Off => {
                MicController::turn_off()?;
                let _ = IpcClient::send(&IpcRequest::SetMic(false));
                if json_mode {
                    println!("{}", serde_json::to_string(&MicController::status()?)?);
                } else {
                    println!(
                        "{} Virtual Microphone is now {} (muted)",
                        "[STATUS]".bright_cyan().bold(),
                        "OFF".red().bold()
                    );
                }
            }
            MicAction::Status => {
                let status = MicController::status()?;
                let ipc_status = IpcClient::send(&IpcRequest::GetStatus).ok();
                let stream_active = ipc_status
                    .as_ref()
                    .map(|r| r.active_stream)
                    .unwrap_or(false);

                if json_mode {
                    println!("{}", serde_json::to_string_pretty(&status)?);
                } else {
                    println!(
                        "{}",
                        "──────── Virtual Microphone Status ────────".bright_cyan()
                    );
                    println!(
                        "  Device Exists: {}",
                        if status.exists {
                            "Yes".green()
                        } else {
                            "No".red()
                        }
                    );
                    println!("  Source Name:   {}", status.source_name);
                    println!("  Sink Name:     {}", status.sink_name);
                    println!(
                        "  Muted:         {}",
                        if status.is_muted {
                            "Yes (OFF)".red().bold()
                        } else {
                            "No (ON)".green().bold()
                        }
                    );
                    println!(
                        "  Active Feed:   {}",
                        if stream_active {
                            "STREAMING ACTIVE".green().bold()
                        } else {
                            "IDLE (No active feed)".yellow()
                        }
                    );
                    println!(
                        "{}",
                        "────────────────────────────────────────────".bright_cyan()
                    );
                }
            }
            MicAction::Feed {
                files,
                playlist,
                r#loop,
                seek,
                volume,
            } => {
                let resolved = resolve_playlist(&files, playlist.as_ref())?;
                println!(
                    "{} Feeding audio playlist ({} tracks) into virtual microphone",
                    "[STREAM]".bright_cyan().bold(),
                    resolved.len()
                );
                let is_on = Arc::new(AtomicBool::new(true));
                AudioFeeder::feed_playlist(resolved, seek, volume, r#loop, is_on, running)?;
            }
            MicAction::Teardown => {
                MicController::teardown()?;
                println!(
                    "{} Virtual microphone sinks and sources removed.",
                    "[CLEANUP]".green().bold()
                );
            }
        },

        Commands::Camera(cam_args) => match cam_args.action {
            CameraAction::On => {
                let ipc_resp = IpcClient::send(&IpcRequest::SetCamera(true));
                let active = ipc_resp.as_ref().map(|r| r.active_stream).unwrap_or(false);
                if json_mode {
                    println!(r#"{{"camera": "on", "active_stream": {}}}"#, active);
                } else if active {
                    println!(
                        "{} Virtual Camera stream is now {} (live feed enabled)",
                        "[STATUS]".bright_cyan().bold(),
                        "ON".green().bold()
                    );
                } else {
                    println!(
                        "{} Virtual Camera is configured {} (Device ready; no active feed currently running)",
                        "[STATUS]".bright_cyan().bold(),
                        "ON".green().bold()
                    );
                }
            }
            CameraAction::Off => {
                let ipc_resp = IpcClient::send(&IpcRequest::SetCamera(false));
                let active = ipc_resp.as_ref().map(|r| r.active_stream).unwrap_or(false);
                if json_mode {
                    println!(r#"{{"camera": "off", "active_stream": {}}}"#, active);
                } else if active {
                    println!(
                        "{} Virtual Camera stream is now {} (blank black frame injection active)",
                        "[STATUS]".bright_cyan().bold(),
                        "OFF".red().bold()
                    );
                } else {
                    println!(
                        "{} Virtual Camera is configured {} (Blank frame injection set)",
                        "[STATUS]".bright_cyan().bold(),
                        "OFF".red().bold()
                    );
                }
            }
            CameraAction::Status => {
                let status = CameraController::status(None);
                if json_mode {
                    println!("{}", serde_json::to_string_pretty(&status)?);
                } else {
                    println!(
                        "{}",
                        "───────── Virtual Camera Status ─────────".bright_cyan()
                    );
                    println!("  Device Path:   {}", status.device_path);
                    println!(
                        "  V4L2 Kernel:   {}",
                        if status.is_v4l2 {
                            "Yes".green()
                        } else {
                            "Emulated / Raw Sink".yellow()
                        }
                    );
                    println!(
                        "  Device Exists: {}",
                        if status.exists {
                            "Yes".green()
                        } else {
                            "No (Will create)".yellow()
                        }
                    );
                    println!(
                        "  Stream State:  {}",
                        if status.active_streaming {
                            if status.is_on {
                                "STREAMING (LIVE)".green().bold()
                            } else {
                                "STREAMING (BLANKED)".red().bold()
                            }
                        } else {
                            "IDLE (No active feed)".yellow()
                        }
                    );
                    println!(
                        "  Default Res:   {}x{} @ {} fps",
                        status.default_width, status.default_height, status.default_fps
                    );
                    println!(
                        "{}",
                        "─────────────────────────────────────────".bright_cyan()
                    );
                }
            }
            CameraAction::Feed {
                files,
                playlist,
                r#loop,
                seek,
                device,
                width,
                height,
                fps,
            } => {
                let resolved = resolve_playlist(&files, playlist.as_ref())?;
                println!(
                    "{} Feeding video playlist ({} files) into virtual camera",
                    "[STREAM]".bright_cyan().bold(),
                    resolved.len()
                );
                let is_on = Arc::new(AtomicBool::new(true));
                let config = VideoFeedConfig {
                    playlist: resolved,
                    camera_target: device,
                    width,
                    height,
                    fps,
                    initial_seek: seek,
                    initial_loop: r#loop,
                };
                VideoFeeder::feed_playlist(config, is_on, running)?;
            }
            CameraAction::Teardown => {
                CameraController::teardown()?;
                println!(
                    "{} Virtual camera frame buffer files removed.",
                    "[CLEANUP]".green().bold()
                );
                let v4l2_loaded = std::process::Command::new("lsmod")
                    .output()
                    .map(|out| String::from_utf8_lossy(&out.stdout).contains("v4l2loopback"))
                    .unwrap_or(false);

                if v4l2_loaded {
                    println!(
                        "  {} Kernel module v4l2loopback is loaded. To unload: sudo modprobe -r v4l2loopback",
                        "[INFO]".yellow().bold()
                    );
                }
            }
        },

        Commands::SyncFeed(sync_args) => {
            let resolved = resolve_playlist(&sync_args.files, sync_args.playlist.as_ref())?;
            let config = SyncConfig {
                media_files: resolved,
                camera_target: sync_args.device,
                width: sync_args.width,
                height: sync_args.height,
                fps: sync_args.fps,
                sample_rate: sync_args.sample_rate,
                loop_mode: sync_args.r#loop,
                initial_seek: sync_args.seek,
                initial_volume: sync_args.volume,
            };

            let is_mic_on = Arc::new(AtomicBool::new(true));
            let is_camera_on = Arc::new(AtomicBool::new(true));

            SyncFeeder::run(config, is_mic_on, is_camera_on, running)?;
        }

        Commands::Test => {
            let report = TestRunner::run_full_suite()?;
            if json_mode {
                println!("{}", serde_json::to_string_pretty(&report)?);
            }
        }
    }

    Ok(())
}
