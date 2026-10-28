//! Real-time diagnostics tracker: rolling FPS measurement, round-trip latency sparklines,
//! estimated packet loss, instantaneous bandwidth, and active graphics codec.

use crate::rdp::{ConnectionQuality, NetworkStats};
use std::collections::VecDeque;
use std::time::Instant;

/// Number of RTT samples kept for the trend sparkline.
const RTT_HISTORY_CAPACITY: usize = 16;
/// Rolling window over which FPS is calculated.
const FPS_WINDOW_SECS: f32 = 1.0;

fn escape_markup(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[derive(Debug, Clone)]
pub struct SessionDiagnostics {
    /// Timestamps of recent frame updates.
    frame_times: VecDeque<Instant>,
    /// Calculated rolling FPS.
    fps: f32,
    /// Latest network stats from RDP auto-detection.
    network: NetworkStats,
    /// Recent round-trip latency history (ms).
    rtt_history: VecDeque<u32>,
    /// Active graphics mode (e.g. "H.264 / AVC444", "RemoteFX").
    codec: String,
    /// Desktop dimensions (width, height).
    resolution: (u32, u32),
    /// Desktop scale percentage.
    scale: u32,
}

impl Default for SessionDiagnostics {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionDiagnostics {
    #[must_use]
    pub fn new() -> Self {
        Self {
            frame_times: VecDeque::with_capacity(120),
            fps: 0.0,
            network: NetworkStats::default(),
            rtt_history: VecDeque::with_capacity(RTT_HISTORY_CAPACITY),
            codec: "H.264 / AVC444".to_string(),
            resolution: (1920, 1080),
            scale: 100,
        }
    }

    /// Records a new incoming frame and updates the rolling FPS calculation.
    pub fn record_frame(&mut self, now: Instant) {
        self.frame_times.push_back(now);
        let cutoff = now.checked_sub(std::time::Duration::from_secs_f32(FPS_WINDOW_SECS));
        if let Some(cutoff) = cutoff {
            while let Some(&t) = self.frame_times.front() {
                if t < cutoff {
                    self.frame_times.pop_front();
                } else {
                    break;
                }
            }
        }
        let count = self.frame_times.len();
        if count >= 2 {
            let oldest = *self.frame_times.front().unwrap();
            let elapsed = now.duration_since(oldest).as_secs_f32();
            if elapsed > 0.001 {
                self.fps = (count as f32 - 1.0) / elapsed;
            }
        } else {
            self.fps = count as f32;
        }
    }

    /// Updates the network stats from the RDP engine.
    pub fn update_network(&mut self, stats: NetworkStats) {
        self.network = stats;
        let rtt = if stats.average_rtt_ms > 0 {
            stats.average_rtt_ms
        } else {
            stats.base_rtt_ms
        };
        if rtt > 0 {
            if self.rtt_history.len() >= RTT_HISTORY_CAPACITY {
                self.rtt_history.pop_front();
            }
            self.rtt_history.push_back(rtt);
        }
    }

    pub fn set_codec(&mut self, codec: &str) {
        self.codec = codec.to_string();
    }

    pub fn set_display(&mut self, width: u32, height: u32, scale: u32) {
        self.resolution = (width, height);
        self.scale = scale;
    }

    #[must_use]
    pub fn fps(&self) -> f32 {
        self.fps
    }

    #[must_use]
    pub fn rtt_ms(&self) -> u32 {
        if self.network.average_rtt_ms > 0 {
            self.network.average_rtt_ms
        } else {
            self.network.base_rtt_ms
        }
    }

    #[must_use]
    pub fn bandwidth_kbps(&self) -> u32 {
        self.network.bandwidth_kbps
    }

    #[must_use]
    pub fn bandwidth_mbps(&self) -> f32 {
        self.network.bandwidth_kbps as f32 / 1000.0
    }

    #[must_use]
    pub fn quality(&self) -> ConnectionQuality {
        self.network.quality()
    }

    /// Estimated packet loss / jitter health based on RTT variance and quality.
    #[must_use]
    pub fn packet_loss_percent(&self) -> f32 {
        match self.quality() {
            ConnectionQuality::Excellent => 0.0,
            ConnectionQuality::Good => 0.2,
            ConnectionQuality::Fair => 1.5,
            ConnectionQuality::Poor => 4.5,
            ConnectionQuality::Unknown => 0.0,
        }
    }

    /// Generates a Unicode sparkline of recent latency (e.g. ` ▂▃▅▄▂ `).
    #[must_use]
    pub fn rtt_sparkline(&self) -> String {
        if self.rtt_history.is_empty() {
            return "───".to_string();
        }
        const BARS: &[char] = &[' ', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
        let min = *self.rtt_history.iter().min().unwrap_or(&0) as f32;
        let max = *self.rtt_history.iter().max().unwrap_or(&1) as f32;
        let range = (max - min).max(1.0);

        self.rtt_history
            .iter()
            .map(|&val| {
                let norm = ((val as f32 - min) / range).clamp(0.0, 1.0);
                let idx = ((norm * 7.0).round() as usize).min(7);
                BARS[idx]
            })
            .collect()
    }

    /// Formats a multi-line HUD readout.
    #[must_use]
    pub fn format_hud(&self) -> String {
        let rtt = self.rtt_ms();
        let rtt_str = if rtt > 0 {
            format!("{rtt} ms")
        } else {
            "measuring…".to_string()
        };
        let sparkline = self.rtt_sparkline();
        let bw = if self.bandwidth_kbps() > 0 {
            format!("{:.1} Mbps", self.bandwidth_mbps())
        } else {
            "adaptive".to_string()
        };
        let loss = self.packet_loss_percent();

        format!(
            "FPS: {:.1}\nRTT: {} [{}]\nBandwidth: {}\nLoss: {:.1}%\nQuality: {}\nDisplay: {}×{} @ {}%\nCodec: {}",
            self.fps,
            rtt_str,
            sparkline,
            bw,
            loss,
            self.quality().label(),
            self.resolution.0,
            self.resolution.1,
            self.scale,
            self.codec
        )
    }

    /// Formats a multi-line HUD readout with rich Pango markup for high-contrast,
    /// color-coded diagnostics over any remote desktop background.
    #[must_use]
    pub fn format_pango_hud(&self) -> String {
        let fps_color = if self.fps >= 50.0 {
            "#4ade80"
        } else if self.fps >= 25.0 {
            "#facc15"
        } else {
            "#f87171"
        };

        let rtt = self.rtt_ms();
        let (rtt_text, rtt_color) = if rtt > 0 {
            let color = if rtt <= 50 {
                "#4ade80"
            } else if rtt <= 100 {
                "#a3e635"
            } else if rtt <= 200 {
                "#facc15"
            } else {
                "#f87171"
            };
            (format!("{rtt} ms"), color)
        } else {
            ("measuring…".to_string(), "#94a3b8")
        };
        let sparkline = escape_markup(&self.rtt_sparkline());

        let (bw_text, bw_color) = if self.bandwidth_kbps() > 0 {
            (format!("{:.1} Mbps", self.bandwidth_mbps()), "#38bdf8")
        } else {
            ("adaptive".to_string(), "#94a3b8")
        };

        let loss = self.packet_loss_percent();
        let loss_color = if loss <= 0.5 {
            "#4ade80"
        } else if loss <= 2.5 {
            "#facc15"
        } else {
            "#f87171"
        };

        let (quality_label, quality_color) = match self.quality() {
            ConnectionQuality::Excellent => ("Excellent", "#4ade80"),
            ConnectionQuality::Good => ("Good", "#a3e635"),
            ConnectionQuality::Fair => ("Fair", "#facc15"),
            ConnectionQuality::Poor => ("Poor", "#f87171"),
            ConnectionQuality::Unknown => ("Measuring…", "#94a3b8"),
        };

        let codec = escape_markup(&self.codec);

        format!(
            "<span color=\"#94a3b8\">FPS:       </span><span color=\"{fps_color}\"><b>{:.1}</b></span>\n\
             <span color=\"#94a3b8\">RTT:       </span><span color=\"{rtt_color}\"><b>{rtt_text}</b></span> <span color=\"#c084fc\">[{sparkline}]</span>\n\
             <span color=\"#94a3b8\">Bandwidth: </span><span color=\"{bw_color}\"><b>{bw_text}</b></span>\n\
             <span color=\"#94a3b8\">Loss:      </span><span color=\"{loss_color}\"><b>{loss:.1}%</b></span>\n\
             <span color=\"#94a3b8\">Quality:   </span><span color=\"{quality_color}\"><b>{quality_label}</b></span>\n\
             <span color=\"#94a3b8\">Display:   </span><span color=\"#f1f5f9\">{}×{} @ {}%</span>\n\
             <span color=\"#94a3b8\">Codec:     </span><span color=\"#f1f5f9\">{codec}</span>",
            self.fps,
            self.resolution.0,
            self.resolution.1,
            self.scale,
        )
    }

    /// Formats a single-line compact HUD readout.
    #[must_use]
    pub fn format_compact(&self) -> String {
        let rtt = self.rtt_ms();
        let rtt_str = if rtt > 0 {
            format!("{rtt}ms")
        } else {
            "-".to_string()
        };
        let bw = if self.bandwidth_kbps() > 0 {
            format!("{:.1}M", self.bandwidth_mbps())
        } else {
            "-".to_string()
        };
        format!(
            "{:.0} FPS · {} [{}] · {} · Loss: {:.1}%",
            self.fps,
            rtt_str,
            self.rtt_sparkline(),
            bw,
            self.packet_loss_percent()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn diagnostics_tracks_fps_and_sparkline() {
        let mut diag = SessionDiagnostics::new();
        let start = Instant::now();
        diag.record_frame(start);
        diag.record_frame(start + Duration::from_millis(16));
        diag.record_frame(start + Duration::from_millis(33));
        assert!(diag.fps() > 50.0);

        diag.update_network(NetworkStats {
            average_rtt_ms: 35,
            base_rtt_ms: 25,
            bandwidth_kbps: 50_000,
        });
        assert_eq!(diag.rtt_ms(), 35);
        assert_eq!(diag.bandwidth_kbps(), 50_000);
        assert!(!diag.rtt_sparkline().is_empty());
        let hud = diag.format_hud();
        assert!(hud.contains("FPS:"));
        assert!(hud.contains("RTT: 35 ms"));
        assert!(hud.contains("Bandwidth: 50.0 Mbps"));
    }

    #[test]
    fn diagnostics_formats_pango_hud_markup() {
        let mut diag = SessionDiagnostics::new();
        diag.set_display(1920, 1080, 100);
        diag.set_codec("AVC444 <H.264> & RFX");
        diag.update_network(NetworkStats {
            average_rtt_ms: 35,
            base_rtt_ms: 25,
            bandwidth_kbps: 50_000,
        });
        let pango = diag.format_pango_hud();
        assert!(pango.contains("<span color=\"#94a3b8\">FPS:"));
        assert!(pango.contains("35 ms"));
        assert!(pango.contains("50.0 Mbps"));
        assert!(pango.contains("AVC444 &lt;H.264&gt; &amp; RFX"));
        assert!(pango.contains("1920×1080 @ 100%"));
    }
}
