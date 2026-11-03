use anyhow::{bail, Result};
use crossbeam_channel::Sender;
use std::io::BufRead;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub enum StreamControl {
    Next,
    Prev,
    SeekDelta(f64),
    VolumeDelta(i32),
    TogglePause,
    ToggleLoop,
    ToggleCamera,
    ToggleMic,
    Stop,
}

/// Resolve command-line paths and optional playlist text files into a clean list of media files.
pub fn resolve_playlist(files: &[PathBuf], playlist_file: Option<&PathBuf>) -> Result<Vec<PathBuf>> {
    let mut resolved = Vec::new();

    if let Some(pl) = playlist_file {
        if pl.exists() {
            let content = std::fs::read_to_string(pl)?;
            for line in content.lines() {
                let trimmed = line.trim();
                if !trimmed.is_empty() && !trimmed.starts_with('#') {
                    let p = PathBuf::from(trimmed);
                    if p.exists() {
                        resolved.push(p);
                    }
                }
            }
        } else {
            bail!("Playlist file not found: {}", pl.display());
        }
    }

    for f in files {
        if f.exists() {
            resolved.push(f.clone());
        } else {
            eprintln!("Warning: file does not exist: {}", f.display());
        }
    }

    if resolved.is_empty() {
        bail!("No valid media files found to stream. Please specify existing files or a valid playlist.");
    }

    Ok(resolved)
}

/// Spawn a background thread listening for interactive hotkeys from stdin.
pub fn spawn_hotkey_listener(tx: Sender<StreamControl>) {
    std::thread::Builder::new()
        .name("hotkey-listener".to_string())
        .spawn(move || {
            let stdin = std::io::stdin();
            for line in stdin.lock().lines().flatten() {
                let cmd = line.trim().to_lowercase();
                match cmd.as_str() {
                    "n" => { let _ = tx.send(StreamControl::Next); }
                    "p" => { let _ = tx.send(StreamControl::Prev); }
                    "l" => { let _ = tx.send(StreamControl::ToggleLoop); }
                    "+" | "u" | "vol+" => { let _ = tx.send(StreamControl::VolumeDelta(5)); }
                    "-" | "d" | "vol-" => { let _ = tx.send(StreamControl::VolumeDelta(-5)); }
                    "f" | ">" | "+10" => { let _ = tx.send(StreamControl::SeekDelta(10.0)); }
                    "b" | "<" | "-10" => { let _ = tx.send(StreamControl::SeekDelta(-10.0)); }
                    " " | "k" | "pause" => { let _ = tx.send(StreamControl::TogglePause); }
                    "c" => { let _ = tx.send(StreamControl::ToggleCamera); }
                    "m" => { let _ = tx.send(StreamControl::ToggleMic); }
                    "q" | "exit" | "quit" => {
                        let _ = tx.send(StreamControl::Stop);
                        break;
                    }
                    _ => {}
                }
            }
        })
        .expect("Failed to spawn hotkey listener thread");
}
