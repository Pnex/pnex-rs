//! Réglages d'ingestion lus depuis `settings.ingestion` de la config Loco.
//! Absents de la config (tests) → défauts ci-dessous.

use loco_rs::config::Config;
use serde::Deserialize;

#[derive(Clone, Debug, Deserialize)]
// Section yaml partielle : les champs absents prennent leur défaut — sans
// ce default conteneur, `from_value` échoue et `from_config` retombe en
// silence sur TOUT le struct par défaut (constat 2026-09-14 : token notify
// perdu car delivery_ttl_days/prune_interval_secs absents du yaml).
#[serde(default)]
pub struct IngestSettings {
    /// Bail de vie : silence au-delà duquel un device est considéré parti
    /// (reaper → `active=false`, anti-clone). 10 s = 2 PING manqués à 5 s
    /// (legacy: 12 s — user value 2026-08-16).
    pub silence_ttl_secs: i64,
    /// Cadence du reaper.
    pub reaper_interval_secs: u64,
    /// Cache de revalidation token/device par frame (§7.8 ws-channels-crypto :
    /// the legacy design queried the DB on every frame; 0 = always revalidate).
    /// Each revalidation is a cheap token + fingerprint check; the full
    /// session snapshot is rebuilt only when the fingerprint changed (or
    /// every `snapshot_rebuild_secs`). Revocation semantics: a deactivated
    /// token closes the session (4005) within this delay (default 30 s).
    pub token_cache_secs: u64,
    /// Safety-net full snapshot rebuild period of a device session, even
    /// when the cheap fingerprint did not change (board profile edits).
    pub snapshot_rebuild_secs: u64,
    /// Min interval between two Postgres `last_seen_at` writes of one
    /// session. 0 = auto: `silence_ttl_secs / 4` (at least 1 s), which keeps
    /// a live session well inside the silence TTL of the reaper/anti-clone.
    pub liveness_touch_secs: u64,
    /// Per-pod bound of concurrent device admissions (WS handshake auth +
    /// lease claim, and `Announce` provisioning): a fleet reconnect storm
    /// queues here instead of draining the Postgres pool.
    pub admission_concurrency: usize,
    /// Max wait for an admission slot at handshake before the socket is
    /// closed with 1013 (try again later — the firmware reconnects).
    pub admission_wait_ms: u64,
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
            token_cache_secs: 30,
            snapshot_rebuild_secs: 300,
            liveness_touch_secs: 0,
            admission_concurrency: 32,
            admission_wait_ms: 5000,
            batch_max: 500,
            batch_flush_secs: 10,
        }
    }
}

impl IngestSettings {
    /// Effective min interval between two liveness writes of a session.
    pub fn liveness_touch_interval(&self) -> std::time::Duration {
        let secs = if self.liveness_touch_secs > 0 {
            self.liveness_touch_secs
        } else {
            (self.silence_ttl_secs.max(0) as u64 / 4).max(1)
        };
        std::time::Duration::from_secs(secs)
    }

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

/// Réglages notifications lus depuis `settings.notifications` de la config
/// Loco (D53/D54). `internal_token` : env `PNEX_NOTIFY_INTERNAL_TOKEN`
/// prioritaire sur la valeur yaml — vide/absent ⇒ `/internal/notify/deliver`
/// rejette tout (et un warn est émis au boot).
#[derive(Clone, Debug, Deserialize)]
// Default conteneur obligatoire : le yaml ne définit qu'un sous-ensemble
// des champs (internal_token/deliver_url en dev) — voir IngestSettings.
#[serde(default)]
pub struct NotifySettings {
    /// Jeton de service partagé backend ↔ runtime de flows (livraison
    /// websocket des nœuds `pnex-notify`). Jamais loggé.
    #[serde(default)]
    pub internal_token: Option<String>,
    /// Internal delivery endpoint URL (Test loopback + runtime env).
    /// Default: this pod's own loopback. Any pod is a valid target in a
    /// cluster: frames are fanned out to the `/ws/notify` sessions of every
    /// pod through Valkey (`services::notify`), so the loopback reaches
    /// browsers connected elsewhere too.
    pub deliver_url: String,
}

impl Default for NotifySettings {
    fn default() -> Self {
        Self {
            internal_token: None,
            deliver_url: "http://127.0.0.1:5150/internal/notify/deliver".into(),
        }
    }
}

impl NotifySettings {
    /// `settings.notifications` optionnelle — défauts si absente/incomplète.
    pub fn from_config(config: &Config) -> Self {
        let mut settings: Self = config
            .settings
            .as_ref()
            .and_then(|s| s.get("notifications"))
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();
        // Default loopback follows the real server port (an explicit yaml
        // `deliver_url` is kept as is).
        if settings.deliver_url == Self::default().deliver_url {
            settings.deliver_url = format!(
                "http://127.0.0.1:{}/internal/notify/deliver",
                config.server.port
            );
        }
        if let Ok(env_token) = std::env::var("PNEX_NOTIFY_INTERNAL_TOKEN") {
            let env_token = env_token.trim().to_string();
            if !env_token.is_empty() {
                settings.internal_token = Some(env_token);
            }
        }
        // Request-time override, mirrors the token one: lets a test server
        // (or a second instance) retarget the deliver loopback without a
        // config rewrite.
        if let Ok(env_url) = std::env::var("PNEX_NOTIFY_DELIVER_URL") {
            let env_url = env_url.trim().to_string();
            if !env_url.is_empty() {
                settings.deliver_url = env_url;
            }
        }
        settings
    }

    /// Journal endpoint of the flow runtime (D86) — same host and token as
    /// the deliver endpoint.
    pub fn journal_url(&self) -> String {
        journal_url_of(&self.deliver_url)
    }
}

/// `…/internal/notify/deliver` → `…/internal/notify/journal`.
pub fn journal_url_of(deliver_url: &str) -> String {
    deliver_url.replace("/internal/notify/deliver", "/internal/notify/journal")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Régression 2026-09-14 : la section yaml ne définit qu'un sous-ensemble
    /// des champs — sans default conteneur, `from_value` échouait et
    /// `from_config` retombait en silence sur `Default` (internal_token None
    /// ⇒ fail-closed 401 sur /internal/notify/deliver + toast « token
    /// manquant » au bouton Test).
    #[test]
    fn section_partielle_garde_le_token() {
        let s: NotifySettings = serde_json::from_value(serde_json::json!({
            "internal_token": "tok-dev",
            "deliver_url": "http://127.0.0.1:5150/internal/notify/deliver"
        }))
        .expect("la section partielle doit désérialiser");
        assert_eq!(s.internal_token.as_deref(), Some("tok-dev"));
        assert_eq!(
            s.journal_url(),
            "http://127.0.0.1:5150/internal/notify/journal"
        );

        let i: IngestSettings = serde_json::from_value(serde_json::json!({
            "silence_ttl_secs": 10
        }))
        .expect("la section ingestion partielle doit désérialiser");
        assert_eq!(i.batch_max, 500);
    }
}
