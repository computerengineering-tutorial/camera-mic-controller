use std::time::{Duration, Instant};

/// Precision Master Clock and Audio-Video synchronizer.
#[allow(dead_code)]
#[derive(Debug)]
pub struct MasterClock {
    start_time: Instant,
    sample_rate: u32,
    total_audio_samples: u64,
    total_video_frames: u64,
    fps: f64,
    max_drift_ms: f64,
}

#[allow(dead_code)]
impl MasterClock {
    pub fn new(sample_rate: u32, fps: f64) -> Self {
        Self {
            start_time: Instant::now(),
            sample_rate,
            total_audio_samples: 0,
            total_video_frames: 0,
            fps,
            max_drift_ms: 0.0,
        }
    }

    /// Reset clock to now.
    pub fn reset(&mut self) {
        self.start_time = Instant::now();
        self.total_audio_samples = 0;
        self.total_video_frames = 0;
        self.max_drift_ms = 0.0;
    }

    /// Monotonic elapsed time since stream began.
    pub fn elapsed(&self) -> Duration {
        self.start_time.elapsed()
    }

    /// Audio Presentation Timestamp (PTS) in seconds.
    pub fn audio_pts_seconds(&self) -> f64 {
        self.total_audio_samples as f64 / self.sample_rate as f64
    }

    /// Video Presentation Timestamp (PTS) in seconds.
    pub fn video_pts_seconds(&self) -> f64 {
        self.total_video_frames as f64 / self.fps
    }

    /// Record audio samples written and return current audio PTS.
    pub fn advance_audio(&mut self, sample_count: u64) -> Duration {
        self.total_audio_samples += sample_count;
        Duration::from_secs_f64(self.audio_pts_seconds())
    }

    /// Record a video frame written and return current video PTS.
    pub fn advance_video(&mut self) -> Duration {
        self.total_video_frames += 1;
        Duration::from_secs_f64(self.video_pts_seconds())
    }

    /// Calculate the current Audio-to-Video drift in milliseconds.
    /// Positive value means Video is ahead of Audio.
    /// Negative value means Video is lagging behind Audio.
    pub fn compute_drift_ms(&mut self) -> f64 {
        let v_pts = self.video_pts_seconds();
        let a_pts = self.audio_pts_seconds();
        let drift = (v_pts - a_pts) * 1000.0;

        if drift.abs() > self.max_drift_ms {
            self.max_drift_ms = drift.abs();
        }
        drift
    }

    /// Wait until target presentation timestamp with hybrid coarse sleep + fine spin-wait.
    pub fn wait_until_pts(&self, target_pts: Duration) {
        let target_instant = self.start_time + target_pts;
        let now = Instant::now();

        if now >= target_instant {
            return;
        }

        let remaining = target_instant - now;

        // Coarse sleep if more than 2.5ms remains to save CPU
        if remaining > Duration::from_millis(3) {
            std::thread::sleep(remaining - Duration::from_millis(2));
        }

        // Sub-millisecond spin-wait for rock-solid precision
        while Instant::now() < target_instant {
            std::hint::spin_loop();
        }
    }

    pub fn stats(&mut self) -> SyncStats {
        let drift = self.compute_drift_ms();
        SyncStats {
            elapsed_secs: self.elapsed().as_secs_f64(),
            video_frames: self.total_video_frames,
            audio_samples: self.total_audio_samples,
            drift_ms: drift,
            max_drift_ms: self.max_drift_ms,
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SyncStats {
    pub elapsed_secs: f64,
    pub video_frames: u64,
    pub audio_samples: u64,
    pub drift_ms: f64,
    pub max_drift_ms: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_clock_pts_calculation() {
        let mut clock = MasterClock::new(48000, 30.0);
        let audio_pts = clock.advance_audio(48000);
        assert_eq!(audio_pts, Duration::from_secs(1));

        let _video_pts = clock.advance_video();
        assert!((clock.video_pts_seconds() - (1.0 / 30.0)).abs() < 1e-6);

        // Advance 29 more frames -> exactly 30 frames = 1.0 second
        for _ in 0..29 {
            clock.advance_video();
        }
        assert!((clock.video_pts_seconds() - 1.0).abs() < 1e-6);

        let drift = clock.compute_drift_ms();
        assert!(drift.abs() < 0.1, "Drift should be near 0 at 1.0s sync");
    }

    #[test]
    fn test_drift_computation() {
        let mut clock = MasterClock::new(48000, 30.0);
        clock.advance_audio(48000); // 1.0s audio
        for _ in 0..15 {
            clock.advance_video(); // 0.5s video
        }
        let drift = clock.compute_drift_ms();
        // Video is 0.5s behind audio -> -500ms
        assert!((drift - (-500.0)).abs() < 1.0);
    }
}
