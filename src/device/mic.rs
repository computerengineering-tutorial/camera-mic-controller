use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::Path;
use std::process::{Child, Command, Stdio};

pub const VIRTUAL_SINK_NAME: &str = "Rust_VirtualMicSink";
pub const VIRTUAL_SOURCE_NAME: &str = "Rust_VirtualMic";
pub const SAVED_DEFAULT_SOURCE_PATH: &str = "/tmp/cam_mic_original_source.txt";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MicStatus {
    pub exists: bool,
    pub source_name: String,
    pub sink_name: String,
    pub is_muted: bool,
    pub active: bool,
}

pub struct MicController;

impl MicController {
    /// Check if the virtual mic source currently exists in PulseAudio/PipeWire.
    pub fn exists() -> bool {
        let output = Command::new("pactl")
            .args(["list", "sources", "short"])
            .output();

        match output {
            Ok(out) => {
                let stdout = String::from_utf8_lossy(&out.stdout);
                stdout.contains(VIRTUAL_SOURCE_NAME)
            }
            Err(_) => false,
        }
    }

    /// Read the currently configured default audio source from pactl.
    pub fn get_default_source() -> Option<String> {
        let res = Command::new("pactl")
            .args(["get-default-source"])
            .output()
            .ok()?;
        if res.status.success() {
            let src = String::from_utf8_lossy(&res.stdout).trim().to_string();
            if !src.is_empty() && !src.contains(VIRTUAL_SOURCE_NAME) {
                return Some(src);
            }
        }
        None
    }

    /// Ensure the virtual mic sink and source are created and registered.
    pub fn ensure_virtual_mic() -> Result<()> {
        if Self::exists() {
            return Ok(());
        }

        // Save original default source if not already saved, so teardown can restore it
        if !Path::new(SAVED_DEFAULT_SOURCE_PATH).exists()
            && let Some(orig) = Self::get_default_source()
        {
            let _ = std::fs::write(SAVED_DEFAULT_SOURCE_PATH, orig);
        }

        // 1. Create Null-Sink
        let sink_res = Command::new("pactl")
            .args([
                "load-module",
                "module-null-sink",
                &format!("sink_name={}", VIRTUAL_SINK_NAME),
                "sink_properties=device.description=Rust_Virtual_Mic_Sink",
            ])
            .output()
            .context("Failed to run pactl to create null sink")?;

        if !sink_res.status.success() {
            let err = String::from_utf8_lossy(&sink_res.stderr);
            bail!("Failed to create null sink: {}", err);
        }

        // 2. Create Remap-Source so it appears as a microphone input
        let source_res = Command::new("pactl")
            .args([
                "load-module",
                "module-remap-source",
                &format!("master={}.monitor", VIRTUAL_SINK_NAME),
                &format!("source_name={}", VIRTUAL_SOURCE_NAME),
                "source_properties=device.description=Rust_Virtual_Microphone",
            ])
            .output()
            .context("Failed to run pactl to create remap source")?;

        if !source_res.status.success() {
            let err = String::from_utf8_lossy(&source_res.stderr);
            bail!("Failed to create remap source: {}", err);
        }

        // Set as default system recording source so browsers & websites capture it automatically
        let _ = Command::new("pactl")
            .args(["set-default-source", VIRTUAL_SOURCE_NAME])
            .output();

        // Unmute initially & set volume to 100%
        let _ = Self::set_mute(false);
        let _ = Self::set_volume(100);

        Ok(())
    }

    /// Turn Mic ON (unmute).
    pub fn turn_on() -> Result<()> {
        Self::ensure_virtual_mic()?;
        Self::set_mute(false)
    }

    /// Turn Mic OFF (mute).
    pub fn turn_off() -> Result<()> {
        Self::ensure_virtual_mic()?;
        Self::set_mute(true)
    }

    /// Set mute status (true = mute/off, false = unmute/on).
    pub fn set_mute(muted: bool) -> Result<()> {
        let flag = if muted { "1" } else { "0" };
        let res = Command::new("pactl")
            .args(["set-source-mute", VIRTUAL_SOURCE_NAME, flag])
            .output()
            .context("Failed to toggle source mute")?;

        if !res.status.success() {
            let err = String::from_utf8_lossy(&res.stderr);
            bail!("Failed to set mute: {}", err);
        }
        Ok(())
    }

    /// Check if currently muted.
    pub fn is_muted() -> Result<bool> {
        let res = Command::new("pactl")
            .args(["get-source-mute", VIRTUAL_SOURCE_NAME])
            .output()
            .context("Failed to get source mute state")?;

        if !res.status.success() {
            return Ok(false);
        }

        let out = String::from_utf8_lossy(&res.stdout);
        Ok(out.to_lowercase().contains("yes"))
    }

    /// Set source volume percentage (e.g. 100, 80, 120).
    pub fn set_volume(percent: u32) -> Result<()> {
        let res = Command::new("pactl")
            .args([
                "set-source-volume",
                VIRTUAL_SOURCE_NAME,
                &format!("{}%", percent),
            ])
            .output()
            .context("Failed to set source volume")?;

        if !res.status.success() {
            let err = String::from_utf8_lossy(&res.stderr);
            bail!("Failed to set volume: {}", err);
        }
        Ok(())
    }

    /// Adjust volume by delta percentage (e.g. +5 or -5).
    #[allow(dead_code)]
    pub fn adjust_volume(delta: i32) -> Result<()> {
        let sign = if delta >= 0 {
            format!("+{}%", delta)
        } else {
            format!("{}%", delta)
        };
        let res = Command::new("pactl")
            .args(["set-source-volume", VIRTUAL_SOURCE_NAME, &sign])
            .output()
            .context("Failed to adjust source volume")?;

        if !res.status.success() {
            let err = String::from_utf8_lossy(&res.stderr);
            bail!("Failed to adjust volume: {}", err);
        }
        Ok(())
    }

    /// Read current volume percentage.
    #[allow(dead_code)]
    pub fn get_volume() -> Result<u32> {
        let res = Command::new("pactl")
            .args(["get-source-volume", VIRTUAL_SOURCE_NAME])
            .output()
            .context("Failed to get source volume")?;

        if !res.status.success() {
            return Ok(100);
        }

        let out = String::from_utf8_lossy(&res.stdout);
        for part in out.split_whitespace() {
            if let Some(val) = part.strip_suffix('%').and_then(|s| s.parse::<u32>().ok()) {
                return Ok(val);
            }
        }
        Ok(100)
    }

    /// Get current status.
    pub fn status() -> Result<MicStatus> {
        let exists = Self::exists();
        let is_muted = if exists {
            Self::is_muted().unwrap_or(false)
        } else {
            false
        };

        Ok(MicStatus {
            exists,
            source_name: VIRTUAL_SOURCE_NAME.to_string(),
            sink_name: VIRTUAL_SINK_NAME.to_string(),
            is_muted,
            active: exists && !is_muted,
        })
    }

    /// Remove virtual mic modules from PulseAudio/PipeWire and restore default hardware mic.
    pub fn teardown() -> Result<()> {
        // Restore saved original default recording source first
        let mut restored = false;
        if let Ok(orig) = std::fs::read_to_string(SAVED_DEFAULT_SOURCE_PATH) {
            let orig_trimmed = orig.trim();
            if !orig_trimmed.is_empty() {
                let res = Command::new("pactl")
                    .args(["set-default-source", orig_trimmed])
                    .output();
                if let Ok(out) = res
                    && out.status.success()
                {
                    restored = true;
                }
            }
            let _ = std::fs::remove_file(SAVED_DEFAULT_SOURCE_PATH);
        }

        // Fallback: search for first non-virtual, non-monitor source if original was not restored
        if !restored
            && let Ok(output) = Command::new("pactl")
                .args(["list", "sources", "short"])
                .output()
        {
            let stdout = String::from_utf8_lossy(&output.stdout);
            for line in stdout.lines() {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() >= 2
                    && !parts[1].contains("Rust_Virtual")
                    && !parts[1].contains("monitor")
                {
                    let _ = Command::new("pactl")
                        .args(["set-default-source", parts[1]])
                        .output();
                    break;
                }
            }
        }

        let output = Command::new("pactl")
            .args(["list", "modules", "short"])
            .output()?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        for line in stdout
            .lines()
            .filter(|l| l.contains(VIRTUAL_SINK_NAME) || l.contains(VIRTUAL_SOURCE_NAME))
        {
            if let Some(id) = line.split_whitespace().next() {
                let _ = Command::new("pactl").args(["unload-module", id]).output();
            }
        }
        Ok(())
    }

    /// Spawn an ultra-low-latency pacat process feeding into Rust_VirtualMicSink.
    pub fn open_audio_writer(sample_rate: u32, channels: u16) -> Result<AudioPipeWriter> {
        Self::ensure_virtual_mic()?;

        let mut child = Command::new("pacat")
            .args([
                "--playback",
                "-d",
                VIRTUAL_SINK_NAME,
                "--format=s16le",
                &format!("--rate={}", sample_rate),
                &format!("--channels={}", channels),
                "--latency-msec=10",
                "--raw",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null()) // Null stderr to prevent pipe buffer lockup deadlock
            .spawn()
            .context("Failed to spawn pacat audio playback process")?;

        let stdin = child
            .stdin
            .take()
            .context("Failed to open stdin for pacat")?;

        Ok(AudioPipeWriter { child, stdin })
    }
}

pub struct AudioPipeWriter {
    child: Child,
    stdin: std::process::ChildStdin,
}

impl AudioPipeWriter {
    pub fn open(sample_rate: u32, channels: u16) -> Result<Self> {
        MicController::open_audio_writer(sample_rate, channels)
    }

    pub fn write_pcm(&mut self, chunk: &[u8]) -> Result<()> {
        self.stdin
            .write_all(chunk)
            .context("Failed to write PCM chunk to audio pipe")?;
        Ok(())
    }

    pub fn flush(&mut self) -> Result<()> {
        self.stdin.flush().context("Failed to flush audio pipe")?;
        Ok(())
    }
}

impl Drop for AudioPipeWriter {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_volume_string_parsing() {
        let pactl_sample =
            "Volume: front-left: 65536 / 100% / 0.00 dB,   front-right: 65536 / 100% / 0.00 dB";
        let mut parsed = None;
        for part in pactl_sample.split_whitespace() {
            if let Some(val) = part.strip_suffix('%').and_then(|s| s.parse::<u32>().ok()) {
                parsed = Some(val);
                break;
            }
        }
        assert_eq!(parsed, Some(100));
    }

    #[test]
    fn test_status_fields() {
        let status = MicController::status().unwrap();
        assert_eq!(status.source_name, VIRTUAL_SOURCE_NAME);
        assert_eq!(status.sink_name, VIRTUAL_SINK_NAME);
    }
}
