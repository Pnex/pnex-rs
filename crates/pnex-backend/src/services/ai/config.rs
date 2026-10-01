//! Effective LLM configuration: kill-switch `settings.ai.enabled` (yaml)
//! and the provider an org runs on — its default provider (secrets.md
//! D116, D119: the platform provides no LLM). No provider configuration
//! comes from the environment: providers are managed in the UI.

use loco_rs::config::Config;
use sea_orm::DatabaseConnection;
use serde::Deserialize;

use super::provider::Provider;
use crate::services::secrets::Keyring;

/// Interrupteur global (kill-switch) — `settings.ai.enabled`, défaut `true`.
#[derive(Clone, Copy, Debug)]
pub struct AiSettings {
    pub enabled: bool,
}

impl Default for AiSettings {
    fn default() -> Self {
        Self { enabled: true }
    }
}

#[derive(Default, Deserialize)]
struct AiPartial {
    enabled: Option<bool>,
}

impl AiSettings {
    /// `settings.ai` optionnelle — défauts champ par champ.
    pub fn from_config(config: &Config) -> Self {
        let partial: AiPartial = config
            .settings
            .as_ref()
            .and_then(|s| s.get("ai"))
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();
        Self {
            enabled: partial.enabled.unwrap_or(Self::default().enabled),
        }
    }
}

/// Configuration effective, prête à servir un appel LLM.
#[derive(Clone)]
pub struct ResolvedAiConfig {
    pub provider: Provider,
    /// Name of the provider row.
    pub name: String,
    pub base_url: String,
    pub api_key: String,
    pub model: String,
}

impl std::fmt::Debug for ResolvedAiConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResolvedAiConfig")
            .field("provider", &self.provider.as_str())
            .field("name", &self.name)
            .field("base_url", &self.base_url)
            .field("model", &self.model)
            .field("api_key", &"<secrète>")
            .finish()
    }
}

/// URL de base par défaut d'Anthropic (le champ peut rester vide côté
/// connecteur d'org).
pub const ANTHROPIC_BASE_URL: &str = "https://api.anthropic.com";

/// Effective configuration of an org: its default provider; `None` = not
/// configured (or kill-switch off). The key
/// is decrypted in memory for the call.
pub async fn resolve(
    db: &DatabaseConnection,
    config: &Config,
    org_id: i64,
) -> Option<ResolvedAiConfig> {
    if !AiSettings::from_config(config).enabled {
        return None;
    }
    let row = match super::providers::effective(db, org_id).await {
        Ok(found) => found?,
        Err(e) => {
            tracing::error!(error = %e, "LLM provider lookup failed");
            return None;
        }
    };
    let ring = match Keyring::from_config(config) {
        Ok(ring) => ring,
        Err(e) => {
            tracing::error!(error = %e, "secrets keyring unavailable");
            return None;
        }
    };
    match super::providers::resolved(db, &ring, &row).await {
        Ok(cfg) => cfg,
        Err(e) => {
            tracing::error!(error = %e, provider = %row.id, "LLM provider key could not be read");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    /// Config Loco minimale (logger+server+database requis par le désérialiseur).
    fn minimal_config(settings: Value) -> Config {
        serde_json::from_value(serde_json::json!({
            "logger": { "enable": false, "level": "info", "format": "compact" },
            "server": { "port": 5150, "host": "http://localhost" },
            "database": { "uri": "postgres://pnex:pnex@localhost:5432/pnex", "enable_logging": false, "auto_migrate": false, "connect_timeout": 500, "idle_timeout": 500, "min_connections": 1, "max_connections": 5 },
            "settings": settings
        }))
        .expect("config désérialisable")
    }

    #[test]
    fn kill_switch_par_defaut_actif_et_coupre_si_false() {
        let c = minimal_config(serde_json::json!({}));
        assert!(AiSettings::from_config(&c).enabled, "défaut : activé");
        let c = minimal_config(serde_json::json!({ "ai": { "enabled": false } }));
        assert!(
            !AiSettings::from_config(&c).enabled,
            "kill-switch explicite"
        );
    }
}
