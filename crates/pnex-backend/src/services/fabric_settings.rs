//! Global settings of the agent fabric (agent-environments.md D755): sizes and retentions
//! chosen by whoever runs the platform, read from `settings.agents`, not values of the code.
//! The limits of artifacts and caches live beside their use (`agent_artifacts`).
//! A platform admin may store other values (R9, `system_settings`): read at use time,
//! so a change applies to the next build, upload or sweep without a restart.

use loco_rs::{app::AppContext, config::Config};
use pnex_core::environments::AgentLimits;
use sea_orm::{ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, Set};
use serde::Deserialize;

use crate::models::_entities::system_settings;
use crate::services::agent_artifacts::{ArtifactSettings, CacheSettings};

/// `system_settings` key of the limits stored by a platform admin (JSON).
const LIMITS_KEY: &str = "agent_limits";

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

/// The limits of the configuration (`settings.agents`), or the code defaults.
pub fn defaults(config: &Config) -> AgentLimits {
    let build: BuildSettings = section(config, "build");
    let retention: RetentionSettings = section(config, "retention");
    let artifacts = ArtifactSettings::from_config(config);
    let caches = CacheSettings::from_config(config);
    let days = |d: i64| u32::try_from(d).unwrap_or(u32::MAX);
    AgentLimits {
        build_memory: build.memory,
        build_ephemeral_storage: build.ephemeral_storage,
        sbom_days: days(retention.sbom_days),
        scan_events_days: days(retention.scan_events_days),
        job_logs_days: days(retention.job_logs_days),
        artifact_max_bytes: artifacts.max_bytes,
        artifact_run_max_bytes: artifacts.run_max_bytes,
        artifact_expires_days: artifacts.expires_days,
        depcache_max_bytes: caches.max_bytes,
        depcache_expires_days: caches.expires_days,
    }
}

/// The limits stored by a platform admin, if any.
pub async fn stored(db: &DatabaseConnection) -> Result<Option<AgentLimits>, sea_orm::DbErr> {
    let row = system_settings::Entity::find()
        .filter(system_settings::Column::Key.eq(LIMITS_KEY))
        .one(db)
        .await?;
    // A value that no longer parses (a field added since) falls back to the defaults.
    Ok(row.and_then(|r| serde_json::from_str(&r.value).ok()))
}

/// Stores the limits, already checked.
pub async fn store(
    db: &DatabaseConnection,
    limits: &AgentLimits,
    user_id: i64,
) -> Result<(), sea_orm::DbErr> {
    let value = serde_json::to_string(limits).unwrap_or_default();
    let existing = system_settings::Entity::find()
        .filter(system_settings::Column::Key.eq(LIMITS_KEY))
        .one(db)
        .await?;
    match existing {
        Some(row) => {
            let mut active: system_settings::ActiveModel = row.into();
            active.value = Set(value);
            active.updated_by = Set(Some(user_id));
            active.update(db).await?;
        }
        None => {
            system_settings::ActiveModel {
                key: Set(LIMITS_KEY.to_string()),
                value: Set(value),
                updated_by: Set(Some(user_id)),
                ..Default::default()
            }
            .insert(db)
            .await?;
        }
    }
    Ok(())
}

/// The limits in force: the stored ones, else the configuration.
pub async fn limits(ctx: &AppContext) -> AgentLimits {
    match stored(&ctx.db).await {
        Ok(Some(l)) => l,
        Ok(None) => defaults(&ctx.config),
        Err(e) => {
            tracing::warn!("agent limits: database error, configuration used: {e}");
            defaults(&ctx.config)
        }
    }
}

pub async fn build(ctx: &AppContext) -> BuildSettings {
    let l = limits(ctx).await;
    BuildSettings {
        memory: l.build_memory,
        ephemeral_storage: l.build_ephemeral_storage,
    }
}

pub async fn retention(ctx: &AppContext) -> RetentionSettings {
    let l = limits(ctx).await;
    RetentionSettings {
        sbom_days: l.sbom_days.into(),
        scan_events_days: l.scan_events_days.into(),
        job_logs_days: l.job_logs_days.into(),
    }
}

pub async fn artifacts(ctx: &AppContext) -> ArtifactSettings {
    let l = limits(ctx).await;
    ArtifactSettings {
        max_bytes: l.artifact_max_bytes,
        run_max_bytes: l.artifact_run_max_bytes,
        expires_days: l.artifact_expires_days,
    }
}

pub async fn caches(ctx: &AppContext) -> CacheSettings {
    let l = limits(ctx).await;
    CacheSettings {
        max_bytes: l.depcache_max_bytes,
        expires_days: l.depcache_expires_days,
    }
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
