use anyhow::{Context, Result};
use crossbeam_channel::Sender;
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crate::feeder::playlist::StreamControl;

pub const IPC_SOCKET_PATH: &str = "/tmp/cam_mic_controller.sock";

#[derive(Debug, Serialize, Deserialize)]
pub enum IpcRequest {
    GetStatus,
    SetCamera(bool),
    SetMic(bool),
    ToggleCamera,
    ToggleMic,
    Control(String),
}

#[derive(Debug, Serialize, Deserialize)]
pub struct IpcResponse {
    pub success: bool,
    pub message: String,
    pub camera_on: Option<bool>,
    pub mic_on: Option<bool>,
    pub active_stream: bool,
}

pub struct IpcClient;

impl IpcClient {
    /// Send an IPC request to an active streaming session.
    pub fn send(req: &IpcRequest) -> Result<IpcResponse> {
        if !Path::new(IPC_SOCKET_PATH).exists() {
            return Ok(IpcResponse {
                success: false,
                message: "No active streaming session running.".to_string(),
                camera_on: None,
                mic_on: None,
                active_stream: false,
            });
        }

        let mut stream = UnixStream::connect(IPC_SOCKET_PATH)
            .context("Failed to connect to cam-mic IPC socket")?;
        stream.set_read_timeout(Some(Duration::from_millis(500)))?;
        stream.set_write_timeout(Some(Duration::from_millis(500)))?;

        let serialized = serde_json::to_string(req)?;
        stream.write_all(serialized.as_bytes())?;
        stream.write_all(b"\n")?;
        stream.flush()?;

        let mut reader = BufReader::new(stream);
        let mut response_line = String::new();
        reader.read_line(&mut response_line)?;

        let resp: IpcResponse =
            serde_json::from_str(&response_line).context("Invalid response from cam-mic daemon")?;
        Ok(resp)
    }

    /// Check if an active streaming session is running.
    pub fn is_stream_active() -> bool {
        Self::send(&IpcRequest::GetStatus)
            .map(|r| r.active_stream)
            .unwrap_or(false)
    }
}

pub struct IpcServerHandle {
    stop_signal: Arc<AtomicBool>,
}

impl Drop for IpcServerHandle {
    fn drop(&mut self) {
        self.stop_signal.store(true, Ordering::SeqCst);
        let _ = std::fs::remove_file(IPC_SOCKET_PATH);
    }
}

pub struct IpcServer;

impl IpcServer {
    /// Start the background IPC server on `/tmp/cam_mic_controller.sock`.
    pub fn start(
        is_camera_on: Arc<AtomicBool>,
        is_mic_on: Arc<AtomicBool>,
        ctrl_tx: Sender<StreamControl>,
        running: Arc<AtomicBool>,
    ) -> Result<IpcServerHandle> {
        let _ = std::fs::remove_file(IPC_SOCKET_PATH);

        let listener = UnixListener::bind(IPC_SOCKET_PATH)
            .context("Failed to bind Unix domain socket for IPC")?;
        listener.set_nonblocking(true)?;

        let stop_signal = Arc::new(AtomicBool::new(false));
        let stop_clone = stop_signal.clone();

        std::thread::Builder::new()
            .name("ipc-listener".to_string())
            .spawn(move || {
                while running.load(Ordering::Relaxed) && !stop_clone.load(Ordering::Relaxed) {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            let cam = is_camera_on.clone();
                            let mic = is_mic_on.clone();
                            let tx = ctrl_tx.clone();
                            Self::handle_client(stream, cam, mic, tx);
                        }
                        Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(50));
                        }
                        Err(_) => {
                            break;
                        }
                    }
                }
                let _ = std::fs::remove_file(IPC_SOCKET_PATH);
            })?;

        Ok(IpcServerHandle { stop_signal })
    }

    fn handle_client(
        mut stream: UnixStream,
        is_camera_on: Arc<AtomicBool>,
        is_mic_on: Arc<AtomicBool>,
        ctrl_tx: Sender<StreamControl>,
    ) {
        let _ = stream.set_read_timeout(Some(Duration::from_millis(300)));
        let _ = stream.set_write_timeout(Some(Duration::from_millis(300)));

        let mut reader = BufReader::new(match stream.try_clone() {
            Ok(s) => s,
            Err(_) => return,
        });

        let mut line = String::new();
        if reader.read_line(&mut line).is_err() || line.trim().is_empty() {
            return;
        }

        let req: Result<IpcRequest, _> = serde_json::from_str(&line);
        let resp = match req {
            Ok(IpcRequest::GetStatus) => IpcResponse {
                success: true,
                message: "Stream is active".to_string(),
                camera_on: Some(is_camera_on.load(Ordering::Relaxed)),
                mic_on: Some(is_mic_on.load(Ordering::Relaxed)),
                active_stream: true,
            },
            Ok(IpcRequest::SetCamera(state)) => {
                is_camera_on.store(state, Ordering::SeqCst);
                IpcResponse {
                    success: true,
                    message: format!(
                        "Virtual Camera set to {}",
                        if state { "ON" } else { "OFF (Blanked)" }
                    ),
                    camera_on: Some(state),
                    mic_on: Some(is_mic_on.load(Ordering::Relaxed)),
                    active_stream: true,
                }
            }
            Ok(IpcRequest::SetMic(state)) => {
                is_mic_on.store(state, Ordering::SeqCst);
                let _ = crate::device::mic::MicController::set_mute(!state);
                IpcResponse {
                    success: true,
                    message: format!(
                        "Virtual Mic set to {}",
                        if state { "ON" } else { "OFF (Muted)" }
                    ),
                    camera_on: Some(is_camera_on.load(Ordering::Relaxed)),
                    mic_on: Some(state),
                    active_stream: true,
                }
            }
            Ok(IpcRequest::ToggleCamera) => {
                let cur = is_camera_on.load(Ordering::SeqCst);
                let new_state = !cur;
                is_camera_on.store(new_state, Ordering::SeqCst);
                IpcResponse {
                    success: true,
                    message: format!(
                        "Virtual Camera toggled to {}",
                        if new_state { "ON" } else { "OFF (Blanked)" }
                    ),
                    camera_on: Some(new_state),
                    mic_on: Some(is_mic_on.load(Ordering::Relaxed)),
                    active_stream: true,
                }
            }
            Ok(IpcRequest::ToggleMic) => {
                let cur = is_mic_on.load(Ordering::SeqCst);
                let new_state = !cur;
                is_mic_on.store(new_state, Ordering::SeqCst);
                let _ = crate::device::mic::MicController::set_mute(!new_state);
                IpcResponse {
                    success: true,
                    message: format!(
                        "Virtual Mic toggled to {}",
                        if new_state { "ON" } else { "OFF (Muted)" }
                    ),
                    camera_on: Some(is_camera_on.load(Ordering::Relaxed)),
                    mic_on: Some(new_state),
                    active_stream: true,
                }
            }
            Ok(IpcRequest::Control(cmd)) => {
                match cmd.as_str() {
                    "next" => {
                        let _ = ctrl_tx.send(StreamControl::Next);
                    }
                    "prev" => {
                        let _ = ctrl_tx.send(StreamControl::Prev);
                    }
                    "pause" => {
                        let _ = ctrl_tx.send(StreamControl::TogglePause);
                    }
                    "loop" => {
                        let _ = ctrl_tx.send(StreamControl::ToggleLoop);
                    }
                    "seek_fwd" => {
                        let _ = ctrl_tx.send(StreamControl::SeekDelta(10.0));
                    }
                    "seek_back" => {
                        let _ = ctrl_tx.send(StreamControl::SeekDelta(-10.0));
                    }
                    "stop" => {
                        let _ = ctrl_tx.send(StreamControl::Stop);
                    }
                    _ => {}
                }
                IpcResponse {
                    success: true,
                    message: format!("Action '{}' triggered", cmd),
                    camera_on: Some(is_camera_on.load(Ordering::Relaxed)),
                    mic_on: Some(is_mic_on.load(Ordering::Relaxed)),
                    active_stream: true,
                }
            }
            Err(e) => IpcResponse {
                success: false,
                message: format!("Malformed request: {}", e),
                camera_on: None,
                mic_on: None,
                active_stream: false,
            },
        };

        if let Ok(json_bytes) = serde_json::to_vec(&resp) {
            let _ = stream.write_all(&json_bytes);
            let _ = stream.write_all(b"\n");
            let _ = stream.flush();
        }
    }
}
