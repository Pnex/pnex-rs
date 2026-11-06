//! Global settings of the agent fabric (agent-environments.md D755): sizes and retentions
//! chosen by whoever runs the platform, read from `settings.agents`, not values of the code.
//! The limits of artifacts and caches live beside their use (`agent_artifacts`).

use loco_rs::config::Config;
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct BuildSettings {
    pub memory: String,
    pub ephemeral_storage: String,
}

impl Default for BuildSettings {
    fn default() -> Self {
        Self {
            memory: "8Gi".into(),
            ephemeral_storage: "40Gi".into(),
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(default)]
pub struct RetentionSettings {
    pub sbom_days: i64,
    pub scan_events_days: i64,
    pub job_logs_days: i64,
}

impl Default for RetentionSettings {
    fn default() -> Self {
        Self {
            sbom_days: 3650,
            scan_events_days: 365,
            job_logs_days: 30,
        }
    }
}

fn section<T: for<'de> Deserialize<'de> + Default>(config: &Config, key: &str) -> T {
    config
        .settings
        .as_ref()
        .and_then(|s| s.get("agents"))
        .and_then(|a| a.get(key))
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default()
}

pub fn build(config: &Config) -> BuildSettings {
    section(config, "build")
}

pub fn retention(config: &Config) -> RetentionSettings {
    section(config, "retention")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_those_of_d755_and_a_setting_overrides_its_own_field() {
        let r: RetentionSettings =
            serde_json::from_value(serde_json::json!({ "job_logs_days": 7 })).unwrap();
        assert_eq!(
            (r.sbom_days, r.scan_events_days, r.job_logs_days),
            (3650, 365, 7)
        );
        let b = BuildSettings::default();
        assert_eq!(
            (b.memory.as_str(), b.ephemeral_storage.as_str()),
            ("8Gi", "40Gi")
        );
    }
}
