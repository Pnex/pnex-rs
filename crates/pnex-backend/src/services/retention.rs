//! O2 data retention (D72): effective value per org and reconciliation onto
//! OpenObserve streams.
//!
//! Precedence:
//! - `self_hosted`: org override → platform default (`system_settings`) →
//!   `PNEX_DEFAULT_RETENTION_DAYS` (30);
//! - `saas`: org override (platform admin only, custom contracts) →
//!   subscription tier `data_retention_secs` (rounded up to days) → env.
//!
//! O2 retention is per stream, in whole days, minimum 1: the effective value
//! is clamped to 1 (the seeded "Admin" tier holds 5 minutes). Streams are
//! created lazily by ingestion, so a periodic reconcile re-applies the
//! value to new streams.

use std::time::Duration;

use loco_rs::app::AppContext;
use sea_orm::{ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, Set};

use crate::models::_entities::{
    openobserve_orgs, organizations, sea_orm_active_enums::OpenobserveOrgStatus,
    subscription_tiers, system_settings,
};
use crate::services::openobserve::{self, Client, OpenobserveSettings};

/// Accepted range for an explicit retention value (days).
pub const MIN_DAYS: u32 = 1;
pub const MAX_DAYS: u32 = 3650;

/// `system_settings` key of the platform default retention.
const DEFAULT_RETENTION_KEY: &str = "default_retention_days";

/// Fallback when nothing else is configured.
const ENV_FALLBACK_DAYS: u32 = 30;

/// Interval of the background reconcile loop.
const RECONCILE_EVERY: Duration = Duration::from_secs(3600);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeploymentMode {
    SelfHosted,
    Saas,
}

impl DeploymentMode {
    /// `PNEX_DEPLOYMENT_MODE` (`saas` | anything else = `self_hosted`).
    pub fn from_env() -> Self {
        Self::parse(&std::env::var("PNEX_DEPLOYMENT_MODE").unwrap_or_default())
    }

    pub fn parse(raw: &str) -> Self {
        if raw.trim().eq_ignore_ascii_case("saas") {
            Self::Saas
        } else {
            Self::SelfHosted
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::SelfHosted => "self_hosted",
            Self::Saas => "saas",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Override,
    Global,
    Tier,
    Env,
}

impl Source {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Override => "override",
            Self::Global => "global",
            Self::Tier => "tier",
            Self::Env => "env",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Effective {
    pub days: u32,
    pub source: Source,
    pub clamped: bool,
}

/// Inputs of the effective retention, gathered from DB and env.
#[derive(Clone, Copy, Debug, Default)]
pub struct Inputs {
    pub org_override_days: Option<i32>,
    pub global_default_days: Option<u32>,
    pub tier_retention_secs: Option<i64>,
    pub env_default_days: Option<u32>,
}

/// Tier retention in whole days, rounded up (5 min → 1 day, 0 → 0).
pub fn secs_to_days_ceil(secs: i64) -> i64 {
    if secs <= 0 {
        0
    } else {
        (secs + 86_399) / 86_400
    }
}

/// Pure precedence + clamp (unit-tested).
pub fn compute(mode: DeploymentMode, inputs: Inputs) -> Effective {
    let env_days = inputs.env_default_days.unwrap_or(ENV_FALLBACK_DAYS) as i64;
    let (raw, source) = match (inputs.org_override_days, mode) {
        (Some(days), _) => (days as i64, Source::Override),
        (None, DeploymentMode::SelfHosted) => match inputs.global_default_days {
            Some(days) => (days as i64, Source::Global),
            None => (env_days, Source::Env),
        },
        (None, DeploymentMode::Saas) => match inputs.tier_retention_secs {
            Some(secs) => (secs_to_days_ceil(secs), Source::Tier),
            None => (env_days, Source::Env),
        },
    };
    let clamped_days = raw.clamp(MIN_DAYS as i64, MAX_DAYS as i64);
    Effective {
        days: clamped_days as u32,
        source,
        clamped: clamped_days != raw,
    }
}

fn env_default_days() -> Option<u32> {
    std::env::var("PNEX_DEFAULT_RETENTION_DAYS")
        .ok()
        .and_then(|v| v.trim().parse().ok())
}

/// Platform default retention (days) stored in `system_settings`.
pub async fn global_default(db: &DatabaseConnection) -> Result<Option<u32>, sea_orm::DbErr> {
    let row = system_settings::Entity::find()
        .filter(system_settings::Column::Key.eq(DEFAULT_RETENTION_KEY))
        .one(db)
        .await?;
    Ok(row.and_then(|r| serde_json::from_str::<u32>(&r.value).ok()))
}

/// Sets (or clears with `None`) the platform default retention.
pub async fn set_global_default(
    db: &DatabaseConnection,
    days: Option<u32>,
    user_id: i64,
) -> Result<(), sea_orm::DbErr> {
    let existing = system_settings::Entity::find()
        .filter(system_settings::Column::Key.eq(DEFAULT_RETENTION_KEY))
        .one(db)
        .await?;
    match (existing, days) {
        (Some(row), None) => {
            system_settings::Entity::delete_by_id(row.id)
                .exec(db)
                .await?;
        }
        (Some(row), Some(days)) => {
            let mut active: system_settings::ActiveModel = row.into();
            active.value = Set(days.to_string());
            active.updated_by = Set(Some(user_id));
            active.update(db).await?;
        }
        (None, Some(days)) => {
            system_settings::ActiveModel {
                key: Set(DEFAULT_RETENTION_KEY.to_string()),
                value: Set(days.to_string()),
                updated_by: Set(Some(user_id)),
                ..Default::default()
            }
            .insert(db)
            .await?;
        }
        (None, None) => {}
    }
    Ok(())
}

/// Tier of the org, if any.
pub async fn tier_of(
    db: &DatabaseConnection,
    org: &organizations::Model,
) -> Result<Option<subscription_tiers::Model>, sea_orm::DbErr> {
    match org.subscription_tier_id {
        Some(id) => subscription_tiers::Entity::find_by_id(id).one(db).await,
        None => Ok(None),
    }
}

/// Telemetry quota of the org in bytes — SaaS only (self-hosted deployments
/// have no subscription limits); `None` = unlimited.
pub async fn quota_bytes(
    db: &DatabaseConnection,
    org: &organizations::Model,
) -> Result<Option<u64>, sea_orm::DbErr> {
    if DeploymentMode::from_env() != DeploymentMode::Saas {
        return Ok(None);
    }
    Ok(tier_of(db, org)
        .await?
        .and_then(|t| t.max_telemetry_mb)
        .and_then(|mb| u64::try_from(mb).ok())
        .map(|mb| mb * 1024 * 1024))
}

/// Effective retention of an org (reads DB + env).
pub async fn effective_for(
    db: &DatabaseConnection,
    org: &organizations::Model,
) -> Result<Effective, sea_orm::DbErr> {
    let tier = tier_of(db, org).await?;
    Ok(compute(
        DeploymentMode::from_env(),
        Inputs {
            org_override_days: org.data_retention_days,
            global_default_days: global_default(db).await?,
            tier_retention_secs: tier.and_then(|t| t.data_retention_secs),
            env_default_days: env_default_days(),
        },
    ))
}

/// Applies the effective retention to every metrics stream of one org whose
/// value differs. Returns the number of streams updated.
pub async fn reconcile_org(
    db: &DatabaseConnection,
    client: &Client,
    org: &organizations::Model,
    o2_org: &str,
) -> Result<usize, String> {
    let effective = effective_for(db, org)
        .await
        .map_err(|e| format!("db: {e}"))?;
    let streams = client.metric_streams_detailed(o2_org).await?;
    let mut updated = 0;
    for stream in streams {
        if stream.data_retention_days == effective.days as i64 {
            continue;
        }
        client
            .set_stream_retention(o2_org, &stream.name, effective.days as i64)
            .await?;
        updated += 1;
    }
    Ok(updated)
}

/// Reconciles one org by id (no-op when O2 is off or the org is not
/// provisioned yet). Errors are logged, never propagated: retention is
/// best-effort and re-applied by the hourly loop.
pub async fn reconcile_org_id(ctx: &AppContext, org_id: i64) {
    let Some(settings) = OpenobserveSettings::from_config(&ctx.config) else {
        return;
    };
    let client = Client::new(&settings);
    let org = match organizations::Entity::find_by_id(org_id).one(&ctx.db).await {
        Ok(Some(org)) => org,
        _ => return,
    };
    let creds = match openobserve::provisioning::provisioned_credentials(&ctx.db, org_id).await {
        Ok(Some(creds)) => creds,
        _ => return,
    };
    match reconcile_org(&ctx.db, &client, &org, &creds.o2_org).await {
        Ok(n) if n > 0 => tracing::info!(org_id, updated = n, "O2 retention reconciled"),
        Ok(_) => {}
        Err(err) => tracing::warn!(org_id, %err, "O2 retention reconcile failed"),
    }
}

/// Reconciles every provisioned org.
pub async fn reconcile_all(ctx: &AppContext) {
    let rows = match openobserve_orgs::Entity::find()
        .filter(openobserve_orgs::Column::Status.eq(OpenobserveOrgStatus::Provisioned))
        .all(&ctx.db)
        .await
    {
        Ok(rows) => rows,
        Err(err) => {
            tracing::warn!(%err, "O2 retention reconcile: listing orgs failed");
            return;
        }
    };
    for row in rows {
        reconcile_org_id(ctx, row.org_id).await;
    }
}

/// Spawns the hourly reconcile loop (no-op without O2 settings). The first
/// pass runs shortly after boot so that a changed env default applies.
pub fn spawn_reconciler(ctx: &AppContext) {
    if OpenobserveSettings::from_config(&ctx.config).is_none() {
        return;
    }
    let ctx = ctx.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(30)).await;
        loop {
            // One pod reconciles (D106).
            if crate::services::singleton::my_turn(&ctx.db, "retention-reconcile", RECONCILE_EVERY)
                .await
            {
                reconcile_all(&ctx).await;
            }
            tokio::time::sleep(RECONCILE_EVERY).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inputs(
        org: Option<i32>,
        global: Option<u32>,
        tier_secs: Option<i64>,
        env: Option<u32>,
    ) -> Inputs {
        Inputs {
            org_override_days: org,
            global_default_days: global,
            tier_retention_secs: tier_secs,
            env_default_days: env,
        }
    }

    #[test]
    fn self_hosted_precedence_override_global_env() {
        let m = DeploymentMode::SelfHosted;
        let e = compute(m, inputs(Some(90), Some(30), Some(86_400), None));
        assert_eq!((e.days, e.source), (90, Source::Override));
        let e = compute(m, inputs(None, Some(14), Some(86_400), None));
        assert_eq!((e.days, e.source), (14, Source::Global));
        // The tier is ignored in self-hosted mode.
        let e = compute(m, inputs(None, None, Some(86_400), Some(7)));
        assert_eq!((e.days, e.source), (7, Source::Env));
        let e = compute(m, inputs(None, None, None, None));
        assert_eq!((e.days, e.source), (ENV_FALLBACK_DAYS, Source::Env));
    }

    #[test]
    fn saas_follows_the_tier_unless_overridden() {
        let m = DeploymentMode::Saas;
        let e = compute(m, inputs(None, Some(365), Some(7 * 86_400), None));
        assert_eq!((e.days, e.source), (7, Source::Tier));
        let e = compute(m, inputs(Some(400), None, Some(7 * 86_400), None));
        assert_eq!((e.days, e.source), (400, Source::Override));
        let e = compute(m, inputs(None, None, None, Some(10)));
        assert_eq!((e.days, e.source), (10, Source::Env));
    }

    #[test]
    fn sub_day_tier_is_rounded_up_and_zero_is_clamped() {
        // Seeded "Admin" tier = 300 s → 1 day, not clamped (rounded up).
        let e = compute(DeploymentMode::Saas, inputs(None, None, Some(300), None));
        assert_eq!((e.days, e.clamped), (1, false));
        let e = compute(DeploymentMode::Saas, inputs(None, None, Some(0), None));
        assert_eq!((e.days, e.clamped), (1, true));
        let e = compute(
            DeploymentMode::SelfHosted,
            inputs(Some(99_999), None, None, None),
        );
        assert_eq!((e.days, e.clamped), (MAX_DAYS, true));
    }

    #[test]
    fn deployment_mode_parse() {
        assert_eq!(DeploymentMode::parse("SaaS"), DeploymentMode::Saas);
        assert_eq!(DeploymentMode::parse(""), DeploymentMode::SelfHosted);
        assert_eq!(DeploymentMode::parse("other"), DeploymentMode::SelfHosted);
    }
}
