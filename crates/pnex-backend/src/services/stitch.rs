//! Server stitching settings (Take 360 V2, plan cheerful-booping-teacup
//! B4) — `settings.stitch`, `MediaSettings`/`FirmwareSettings` pattern:
//! optional block, per-field defaults, the env **overrides** the config.
//!
//! Frame storage has NO setting of its own: the blobs live in the
//! MediaStore (`settings.media`, same fs/s3 backend as media — see
//! `MediaSettings::stitch_key`).

use loco_rs::config::Config;
use serde::Deserialize;

/// Default output width of a server stitch — the plan's "spec" (mirror of
/// `pnex_stitcher::SPEC_OUT_WIDTH`, value copied: the default must stay a
/// config literal, not a dependency).
pub const DEFAULT_OUT_WIDTH: u32 = 4096;

/// Default directory of the ONNX pose models (SuperPoint + LightGlue)
/// — relative to the repo root, committed (RPi deployment = pull+build).
pub const DEFAULT_MODELS_DIR: &str = "deploy/models";

/// Default number of concurrent stitches (fixed knob, decision 2026-09-10:
/// RPi 8 GB — 2 jobs × ~300 MB peak + OS fit the margin, < 10 min/job).
pub const DEFAULT_MAX_CONCURRENT: u32 = 2;

/// Upper bound of one stitch job (seconds). Must stay below the queue
/// reaper age (`queue.reaper.age_minutes`, 30 min): past it the reaper
/// re-queues the job and a second worker would stitch it again.
pub const DEFAULT_TIMEOUT_SECS: u64 = 1500;

/// States of a stitch job — same vocabulary as `build_phase` (text
/// columns, no PG enum).
pub const STATE_QUEUED: &str = "queued";
pub const STATE_RUNNING: &str = "running";
pub const STATE_SUCCEEDED: &str = "succeeded";
pub const STATE_FAILED: &str = "failed";

/// Resolved server stitching settings.
#[derive(Debug, Clone)]
pub struct StitchSettings {
    /// Kill-switch: `false` → `POST /api/v1/stitch-jobs` answers 503 (the
    /// GET/POST frames stay served — an already created job can finish).
    pub enabled: bool,
    /// Width of the HD panorama produced by the worker (height = /2) —
    /// `PNEX_STITCH_OUT_WIDTH`; the e2e test sets 1024 to stay < 60 s.
    pub out_width: u32,
    /// Directory of the ONNX pose models — `PNEX_STITCH_MODELS_DIR`.
    /// Absent/incomplete → automatic NCC fallback in the stitcher.
    pub models_dir: String,
    /// Number of concurrent stitches (tokio semaphore) —
    /// `PNEX_STITCH_MAX_CONCURRENT`. Independent of the number of queue
    /// workers (which shares its tokens with firmware builds).
    pub max_concurrent: u32,
    /// Per-job timeout (seconds) — `PNEX_STITCH_TIMEOUT_SECS`, clamped to
    /// 60..=1700 (below the 30 min queue reaper age).
    pub timeout_secs: u64,
}

impl Default for StitchSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            out_width: DEFAULT_OUT_WIDTH,
            models_dir: DEFAULT_MODELS_DIR.to_string(),
            max_concurrent: DEFAULT_MAX_CONCURRENT,
            timeout_secs: DEFAULT_TIMEOUT_SECS,
        }
    }
}

/// Partial serializable form of `settings.stitch` (all optional).
#[derive(Default, Deserialize)]
struct StitchPartial {
    enabled: Option<bool>,
    out_width: Option<u32>,
    models_dir: Option<String>,
    max_concurrent: Option<u32>,
    timeout_secs: Option<u64>,
}

impl StitchSettings {
    /// Optional `settings.stitch` — per-field defaults, env takes
    /// precedence (`PNEX_STITCH_ENABLED`, `PNEX_STITCH_OUT_WIDTH`,
    /// `PNEX_STITCH_MODELS_DIR`, `PNEX_STITCH_MAX_CONCURRENT`).
    pub fn from_config(config: &Config) -> Self {
        let partial: StitchPartial = config
            .settings
            .as_ref()
            .and_then(|s| s.get("stitch"))
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();
        let defaults = Self::default();
        let mut settings = Self {
            enabled: partial.enabled.unwrap_or(defaults.enabled),
            out_width: partial.out_width.unwrap_or(defaults.out_width),
            models_dir: partial.models_dir.unwrap_or(defaults.models_dir),
            max_concurrent: partial.max_concurrent.unwrap_or(defaults.max_concurrent),
            timeout_secs: partial
                .timeout_secs
                .unwrap_or(defaults.timeout_secs)
                .clamp(60, 1700),
        };
        // Decision: the env overrides the config (firmware/media pattern).
        if let Ok(flag) = std::env::var("PNEX_STITCH_ENABLED") {
            match flag.trim().to_ascii_lowercase().as_str() {
                "0" | "false" | "off" | "no" => settings.enabled = false,
                "1" | "true" | "on" | "yes" => settings.enabled = true,
                _ => {}
            }
        }
        if let Ok(w) = std::env::var("PNEX_STITCH_OUT_WIDTH") {
            if let Ok(parsed) = w.trim().parse::<u32>() {
                settings.out_width = parsed;
            }
        }
        if let Ok(d) = std::env::var("PNEX_STITCH_MODELS_DIR") {
            if !d.trim().is_empty() {
                settings.models_dir = d.trim().to_string();
            }
        }
        if let Ok(n) = std::env::var("PNEX_STITCH_MAX_CONCURRENT") {
            if let Ok(parsed) = n.trim().parse::<u32>() {
                settings.max_concurrent = parsed.clamp(1, 8);
            }
        }
        if let Ok(n) = std::env::var("PNEX_STITCH_TIMEOUT_SECS") {
            if let Ok(parsed) = n.trim().parse::<u64>() {
                settings.timeout_secs = parsed.clamp(60, 1700);
            }
        }
        settings
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use loco_rs::environment::Environment;

    /// Defaults: enabled, 4096, models in deploy/models, 2 jobs max —
    /// even without a settings block.
    #[test]
    fn default_stitch_settings() {
        let config = Config::new(&Environment::Test).unwrap();
        let settings = StitchSettings::from_config(&config);
        assert!(settings.enabled);
        assert_eq!(settings.out_width, DEFAULT_OUT_WIDTH);
        assert_eq!(settings.models_dir, DEFAULT_MODELS_DIR);
        assert_eq!(settings.max_concurrent, DEFAULT_MAX_CONCURRENT);
    }

    /// Partial `settings.stitch` block: only `out_width` present → the
    /// other fields keep their default.
    #[test]
    fn partial_out_width_only() {
        let mut config = Config::new(&Environment::Test).unwrap();
        config.settings = Some(serde_json::json!({ "stitch": { "out_width": 2048 } }));
        let settings = StitchSettings::from_config(&config);
        assert!(settings.enabled);
        assert_eq!(settings.out_width, 2048);
        assert_eq!(settings.models_dir, DEFAULT_MODELS_DIR);
    }
}
