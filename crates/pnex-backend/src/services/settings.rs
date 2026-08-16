//! Réglages d'ingestion lus depuis `settings.ingestion` de la config Loco.
//! Absents de la config (tests) → défauts ci-dessous.

use loco_rs::config::Config;
use serde::Deserialize;

#[derive(Clone, Debug, Deserialize)]
pub struct IngestSettings {
    /// Bail de vie : silence au-delà duquel un device est considéré parti
    /// (reaper → `active=false`, anti-clone). 10 s = 2 PING manqués à 5 s
    /// (Django : 12 s — valeur user 2026-08-16).
    pub silence_ttl_secs: i64,
    /// Cadence du reaper.
    pub reaper_interval_secs: u64,
    /// Cache de revalidation token/device par frame (§7.8 ws-channels-crypto :
    /// Django requêtait la DB à chaque frame ; 0 = revalidation systématique).
    pub token_cache_secs: u64,
    /// Batch télémétrie : nb max de points avant flush (parité ES 500/10 s).
    pub batch_max: usize,
    /// Batch télémétrie : délai max avant flush.
    pub batch_flush_secs: u64,
}

impl Default for IngestSettings {
    fn default() -> Self {
        Self {
            silence_ttl_secs: 10,
            reaper_interval_secs: 5,
            token_cache_secs: 10,
            batch_max: 500,
            batch_flush_secs: 10,
        }
    }
}

impl IngestSettings {
    /// `settings.ingestion` optionnelle — défauts si absente/incomplète.
    pub fn from_config(config: &Config) -> Self {
        config
            .settings
            .as_ref()
            .and_then(|s| s.get("ingestion"))
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default()
    }
}
