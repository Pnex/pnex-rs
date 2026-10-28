//! Ingestion settings read from `settings.ingestion` of the Loco config.
//! Absent from the config (tests) → defaults below.

use loco_rs::config::Config;
use serde::Deserialize;

#[derive(Clone, Debug, Deserialize)]
// Partial yaml section: absent fields take their default — without this
// container default, `from_value` fails and `from_config` silently falls
// back to the WHOLE default struct (found 2026-09-14: notify token lost
// because delivery_ttl_days/prune_interval_secs were absent from the yaml).
#[serde(default)]
pub struct IngestSettings {
    /// Liveness lease: silence after which a device is considered gone
    /// (reaper → `active=false`, anti-clone). 10 s = 2 missed PINGs at 5 s
    /// (user value 2026-08-16).
    pub silence_ttl_secs: i64,
    /// Reaper cadence.
    pub reaper_interval_secs: u64,
    /// Per-frame token/device revalidation cache (§7.8 ws-channels-crypto:
    /// 0 = always revalidate).
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
    /// Telemetry batch: max number of points before a flush.
    pub batch_max: usize,
    /// Telemetry batch: max delay before flush.
    pub batch_flush_secs: u64,
    /// D154: device links (`/ws/device`, `/ws/camera`,
    /// OTA download) are accepted only through the TLS edge, which sets
    /// `X-Forwarded-Proto: https` (overwriting the client's value). Off in
    /// the test config only (tests talk to the server directly).
    pub require_tls: bool,
    /// D153 (L4): a device link must present a client certificate issued by
    /// its org CA (verified by the backend from the edge's `X-Client-Cert`).
    pub require_client_cert: bool,
    /// Shared secret the edge adds to every request (`X-Pnex-Edge`,
    /// env `PNEX_EDGE_SECRET`): edge-provided headers (`X-Forwarded-Proto`,
    /// `X-Client-Cert`) are trusted only with it, so a client reaching the
    /// backend port directly cannot forge them. Never logged.
    #[serde(skip)]
    pub edge_secret: Option<String>,
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
            require_tls: true,
            require_client_cert: true,
            edge_secret: None,
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

    /// Optional `settings.ingestion` — defaults if absent/incomplete.
    /// The edge secret comes from the environment only.
    pub fn from_config(config: &Config) -> Self {
        let mut settings: Self = config
            .settings
            .as_ref()
            .and_then(|s| s.get("ingestion"))
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();
        settings.edge_secret = std::env::var("PNEX_EDGE_SECRET")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| s.len() >= 16);
        settings
    }
}

/// Notification settings read from `settings.notifications` of the Loco
/// config (D53/D54). `internal_token`: env `PNEX_NOTIFY_INTERNAL_TOKEN`
/// takes precedence over the yaml value — empty/absent ⇒
/// `/internal/notify/deliver` rejects everything (and a warn is emitted at boot).
#[derive(Clone, Debug, Deserialize)]
// Container default required: the yaml only defines a subset of the
// fields (internal_token/deliver_url in dev) — see IngestSettings.
#[serde(default)]
pub struct NotifySettings {
    /// Service token shared between backend ↔ flow runtime (websocket
    /// delivery of `pnex-notify` nodes). Never logged.
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
    /// Optional `settings.notifications` — defaults if absent/incomplete.
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

    /// Regression 2026-09-14: the yaml section only defines a subset of the
    /// fields — without a container default, `from_value` failed and
    /// `from_config` silently fell back to `Default` (internal_token None
    /// ⇒ fail-closed 401 on /internal/notify/deliver + "missing token"
    /// toast on the Test button).
    #[test]
    fn section_partielle_garde_le_token() {
        let s: NotifySettings = serde_json::from_value(serde_json::json!({
            "internal_token": "tok-dev",
            "deliver_url": "http://127.0.0.1:5150/internal/notify/deliver"
        }))
        .expect("the partial section must deserialize");
        assert_eq!(s.internal_token.as_deref(), Some("tok-dev"));
        assert_eq!(
            s.journal_url(),
            "http://127.0.0.1:5150/internal/notify/journal"
        );

        let i: IngestSettings = serde_json::from_value(serde_json::json!({
            "silence_ttl_secs": 10
        }))
        .expect("the partial ingestion section must deserialize");
        assert_eq!(i.batch_max, 500);
    }
}
