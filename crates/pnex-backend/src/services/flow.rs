//! Réglages du moteur de flow ETL (D18) — `settings.flow` de la config Loco,
//! champ par champ avec défauts (pattern `FirmwareSettings`).
//!
//! Le runtime (`pnex-flow-runtime`) est un **process enfant supervisé** (mode
//! B) : le backend ne lie jamais les crates EdgeLinkd — isolation du stade
//! alpha. `enabled` (défaut `false`) coupe tout : sans lui ni process ni
//! déploiement (les flows restent éditables en base).

use loco_rs::config::Config;
use serde::Deserialize;

/// Réglages résolus du superviseur de flows.
///
/// Debug manuel : l'allowlist d'env ne sont que des *noms* de variables
/// (les valeurs ne transitent jamais ici).
#[derive(Clone)]
pub struct FlowSettings {
    /// Supervision + déploiement actifs (défaut : non).
    pub enabled: bool,
    /// Commande du runtime (résolue comme `resolve_program` du firmware :
    /// chemin relatif → cwd → racine du monorepo).
    pub runtime_cmd: String,
    /// Répertoire d'état (flows.json + runtime.json).
    pub state_dir: String,
    /// Backoff initial de relance après crash (exponentiel, borné).
    pub restart_backoff_secs: u64,
    /// Backoff maximal de relance.
    pub restart_backoff_max_secs: u64,
    /// Délai SIGTERM → SIGKILL à l'arrêt du runtime.
    pub terminate_secs: u64,
    /// Délai d'attente de l'acquittement de rechargement (runtime.json).
    pub reload_ack_secs: u64,
    /// Variables d'environnement autorisées à franchir la frontière vers le
    /// runtime (noms seuls — ex. `DATABASE_URL` pour le nœud pnex-sql).
    pub env_allowlist: Vec<String>,
}

impl std::fmt::Debug for FlowSettings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FlowSettings")
            .field("enabled", &self.enabled)
            .field("runtime_cmd", &self.runtime_cmd)
            .field("state_dir", &self.state_dir)
            .field("restart_backoff_secs", &self.restart_backoff_secs)
            .field("restart_backoff_max_secs", &self.restart_backoff_max_secs)
            .field("terminate_secs", &self.terminate_secs)
            .field("reload_ack_secs", &self.reload_ack_secs)
            .field("env_allowlist", &self.env_allowlist)
            .finish()
    }
}

impl Default for FlowSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            runtime_cmd: "pnex-flow-runtime".into(),
            state_dir: "./flow-state".into(),
            restart_backoff_secs: 5,
            restart_backoff_max_secs: 60,
            terminate_secs: 10,
            reload_ack_secs: 10,
            env_allowlist: vec!["DATABASE_URL".into()],
        }
    }
}

/// Forme sérialisable partielle de `settings.flow` (tout optionnel).
#[derive(Default, Deserialize)]
struct FlowPartial {
    enabled: Option<bool>,
    runtime_cmd: Option<String>,
    state_dir: Option<String>,
    restart_backoff_secs: Option<u64>,
    restart_backoff_max_secs: Option<u64>,
    terminate_secs: Option<u64>,
    reload_ack_secs: Option<u64>,
    env_allowlist: Option<Vec<String>>,
}

impl FlowSettings {
    /// `settings.flow` optionnelle — défauts champ par champ.
    pub fn from_config(config: &Config) -> Self {
        let partial: FlowPartial = config
            .settings
            .as_ref()
            .and_then(|s| s.get("flow"))
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();
        let d = Self::default();
        Self {
            enabled: partial.enabled.unwrap_or(d.enabled),
            runtime_cmd: partial.runtime_cmd.unwrap_or(d.runtime_cmd),
            state_dir: partial.state_dir.unwrap_or(d.state_dir),
            restart_backoff_secs: partial.restart_backoff_secs.unwrap_or(d.restart_backoff_secs),
            restart_backoff_max_secs: partial
                .restart_backoff_max_secs
                .unwrap_or(d.restart_backoff_max_secs),
            terminate_secs: partial.terminate_secs.unwrap_or(d.terminate_secs),
            reload_ack_secs: partial.reload_ack_secs.unwrap_or(d.reload_ack_secs),
            env_allowlist: partial.env_allowlist.unwrap_or(d.env_allowlist),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defauts_et_partiel() {
        let d = FlowSettings::default();
        assert!(!d.enabled, "le moteur est coupé par défaut");
        assert_eq!(d.runtime_cmd, "pnex-flow-runtime");
        assert_eq!(d.env_allowlist, vec!["DATABASE_URL".to_string()]);

        // Config sans section `flow` → défauts.
        let config: Config = serde_json::from_value(serde_json::json!({})).unwrap_or_default();
        let s = FlowSettings::from_config(&config);
        assert_eq!(s.state_dir, d.state_dir);
    }
}
