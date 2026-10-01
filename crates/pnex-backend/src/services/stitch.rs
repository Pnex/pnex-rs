//! Réglages du stitching serveur (Take 360 V2, plan cheerful-booping-teacup
//! B4) — `settings.stitch`, école `MediaSettings`/`FirmwareSettings` : bloc
//! optionnel, défauts champ par champ, l'env **surcharge** la config.
//!
//! Le stockage des frames n'a PAS de réglage propre : les blobs vivent dans
//! le MediaStore (`settings.media`, même backend fs/s3 que les médias — cf.
//! `MediaSettings::stitch_key`).

use loco_rs::config::Config;
use serde::Deserialize;

/// Largeur de sortie par défaut d'un assemblage serveur — la « spec » du
/// plan (miroir de `pnex_stitcher::SPEC_OUT_WIDTH`, valeur recopiée : le
/// défaut doit rester un littéral de config, pas une dépendance).
pub const DEFAULT_OUT_WIDTH: u32 = 4096;

/// Répertoire des modèles ONNX de pose (SuperPoint + LightGlue) par défaut
/// — relatif à la racine du dépôt, committés (déploiement RPi = pull+build).
pub const DEFAULT_MODELS_DIR: &str = "deploy/models";

/// Nombre de stitches simultanés par défaut (knob fixe, décision 2026-09-10 :
/// RPi 8 Go — 2 jobs × ~300 Mo de pic + OS tiennent la marge, < 10 min/job).
pub const DEFAULT_MAX_CONCURRENT: u32 = 2;

/// Upper bound of one stitch job (seconds). Must stay below the queue
/// reaper age (`queue.reaper.age_minutes`, 30 min): past it the reaper
/// re-queues the job and a second worker would stitch it again.
pub const DEFAULT_TIMEOUT_SECS: u64 = 1500;

/// États d'un job de stitch — même vocabulaire que `build_phase` (colonnes
/// texte, pas de PG enum).
pub const STATE_QUEUED: &str = "queued";
pub const STATE_RUNNING: &str = "running";
pub const STATE_SUCCEEDED: &str = "succeeded";
pub const STATE_FAILED: &str = "failed";

/// Réglages résolus du stitching serveur.
#[derive(Debug, Clone)]
pub struct StitchSettings {
    /// Kill-switch : `false` → `POST /api/v1/stitch-jobs` répond 503 (les
    /// GET/POST frames restent servis — un job déjà créé peut finir).
    pub enabled: bool,
    /// Largeur du panorama HD produit par le worker (hauteur = /2) —
    /// `PNEX_STITCH_OUT_WIDTH` ; le test e2e passe à 1024 pour rester < 60 s.
    pub out_width: u32,
    /// Répertoire des modèles ONNX de pose — `PNEX_STITCH_MODELS_DIR`.
    /// Absent/incomplet → repli NCC automatique côté stitcher.
    pub models_dir: String,
    /// Nombre de stitches simultanés (sémaphore tokio) —
    /// `PNEX_STITCH_MAX_CONCURRENT`. Indépendant du nombre de workers de
    /// queue (qui partage ses jetons avec les builds firmware).
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

/// Forme sérialisable partielle de `settings.stitch` (tout optionnel).
#[derive(Default, Deserialize)]
struct StitchPartial {
    enabled: Option<bool>,
    out_width: Option<u32>,
    models_dir: Option<String>,
    max_concurrent: Option<u32>,
    timeout_secs: Option<u64>,
}

impl StitchSettings {
    /// `settings.stitch` optionnelle — défauts champ par champ, env
    /// prioritaire (`PNEX_STITCH_ENABLED`, `PNEX_STITCH_OUT_WIDTH`,
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
        // Décision : l'env surcharge la config (école firmware/media).
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

    /// Défauts : activé, 4096, modèles dans deploy/models, 2 jobs max —
    /// même sans bloc settings.
    #[test]
    fn defauts_stitch_settings() {
        let config = Config::new(&Environment::Test).unwrap();
        let settings = StitchSettings::from_config(&config);
        assert!(settings.enabled);
        assert_eq!(settings.out_width, DEFAULT_OUT_WIDTH);
        assert_eq!(settings.models_dir, DEFAULT_MODELS_DIR);
        assert_eq!(settings.max_concurrent, DEFAULT_MAX_CONCURRENT);
    }

    /// Bloc `settings.stitch` partiel : seul `out_width` présent → les
    /// autres champs gardent leur défaut.
    #[test]
    fn partiel_out_width_seul() {
        let mut config = Config::new(&Environment::Test).unwrap();
        config.settings = Some(serde_json::json!({ "stitch": { "out_width": 2048 } }));
        let settings = StitchSettings::from_config(&config);
        assert!(settings.enabled);
        assert_eq!(settings.out_width, 2048);
        assert_eq!(settings.models_dir, DEFAULT_MODELS_DIR);
    }
}
