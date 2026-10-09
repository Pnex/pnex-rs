//! System endpoints `/api/v1/system/*` (D72):
//!
//! - O2 retention: effective value of the current org (any member), org
//!   override and platform default (platform admin only);
//! - O2 cleanup of the current org: per stream, time range, full purge
//!   (owner/admin);
//! - platform status and platform LLM providers (platform admin only).

use axum::extract::{Path, State};
use axum::routing::{delete, get, post, put};
use chrono::{DateTime, Timelike, Utc};
use loco_rs::prelude::*;
use pnex_core::{
    err_codes, O2DeleteRangeRequest, O2DeleteResult, O2PurgeRequest, O2StreamInfo, O2StreamList,
    OrgTierUpdate, RetentionInfo, RetentionUpdate, TierOption,
};

use crate::auth::{OrgContext, PlatformAdmin};
use crate::models::_entities::{organizations, subscription_tiers};
use crate::services::openobserve::{self, Client, OpenobserveSettings};
use crate::services::retention::{self, DeploymentMode};
use crate::services::system_status;

fn coded(status: axum::http::StatusCode, code: &str, msg: &str) -> Error {
    Error::CustomError(
        status,
        loco_rs::controller::ErrorDetail::new(code, msg.to_string()),
    )
}

fn db_err(err: sea_orm::DbErr) -> Error {
    tracing::error!(%err, "system: database error");
    Error::InternalServerError
}

pub fn routes() -> Routes {
    Routes::new()
        .prefix("/api/v1/system")
        .add("/retention", get(retention_get))
        .add("/retention/orgs/{org_id}", put(retention_org_put))
        .add("/retention/default", put(retention_default_put))
        .add("/o2/streams", get(o2_streams))
        .add("/o2/streams/{name}", delete(o2_stream_delete))
        .add(
            "/o2/streams/{name}/delete-range",
            post(o2_stream_delete_range),
        )
        .add("/o2/purge", post(o2_purge))
        .add("/orgs", get(orgs_overview))
        .add("/orgs/{org_id}/tier", put(org_tier_put))
        .add("/tiers", get(tiers_list))
        .add("/status", get(status))
        .add("/secrets/rekey", post(super::secrets::platform_rekey))
}

// ───────────────────────────── Retention ─────────────────────────────

fn days_u32(days: Option<i32>) -> Option<u32> {
    days.and_then(|d| u32::try_from(d).ok())
}

async fn retention_info(
    ctx: &AppContext,
    org: &organizations::Model,
    platform_admin: bool,
) -> Result<RetentionInfo> {
    let effective = retention::effective_for(&ctx.db, org)
        .await
        .map_err(db_err)?;
    let tier = retention::tier_of(&ctx.db, org).await.map_err(db_err)?;
    let mode = DeploymentMode::from_env();
    let global = match mode {
        DeploymentMode::SelfHosted => retention::global_default(&ctx.db).await.map_err(db_err)?,
        DeploymentMode::Saas => None,
    };
    Ok(RetentionInfo {
        days: effective.days,
        source: effective.source.as_str().to_string(),
        clamped: effective.clamped,
        deployment_mode: mode.as_str().to_string(),
        org_override_days: days_u32(org.data_retention_days),
        global_default_days: global,
        tier_name: tier.as_ref().map(|t| t.name.clone()),
        tier_days: tier
            .and_then(|t| t.data_retention_secs)
            .map(|secs| retention::secs_to_days_ceil(secs) as u32),
        editable: platform_admin,
    })
}

/// `GET /api/v1/system/retention` — any member of the org.
async fn retention_get(org: OrgContext, State(ctx): State<AppContext>) -> Result<Response> {
    let info = retention_info(&ctx, &org.org, org.auth.user.platform_admin).await?;
    format::json(info)
}

fn check_days(days: Option<u32>) -> Result<()> {
    match days {
        Some(d) if !(retention::MIN_DAYS..=retention::MAX_DAYS).contains(&d) => Err(coded(
            axum::http::StatusCode::UNPROCESSABLE_ENTITY,
            err_codes::RETENTION_OUT_OF_RANGE,
            "Retention must be between 1 and 3650 days.",
        )),
        _ => Ok(()),
    }
}

/// `PUT /api/v1/system/retention/orgs/{org_id}` — platform admin; `null`
/// clears the override (back to global default / subscription tier).
async fn retention_org_put(
    admin: PlatformAdmin,
    State(ctx): State<AppContext>,
    Path(org_id): Path<i64>,
    Json(body): Json<RetentionUpdate>,
) -> Result<Response> {
    check_days(body.days)?;
    let org = organizations::Entity::find_by_id(org_id)
        .one(&ctx.db)
        .await
        .map_err(db_err)?
        .ok_or(Error::NotFound)?;
    let mut active: organizations::ActiveModel = org.into();
    active.data_retention_days = sea_orm::Set(body.days.map(|d| d as i32));
    let org = active.update(&ctx.db).await.map_err(db_err)?;
    tracing::info!(org_id, days = ?body.days, by = admin.0.user.id, "org retention override updated");
    let bg = ctx.clone();
    tokio::spawn(async move { retention::reconcile_org_id(&bg, org_id).await });
    format::json(retention_info(&ctx, &org, true).await?)
}

/// `PUT /api/v1/system/retention/default` — platform admin. Irrelevant in
/// SaaS mode (the tier decides): refused there to avoid a silent no-op.
async fn retention_default_put(
    admin: PlatformAdmin,
    State(ctx): State<AppContext>,
    Json(body): Json<RetentionUpdate>,
) -> Result<Response> {
    if DeploymentMode::from_env() == DeploymentMode::Saas {
        return Err(coded(
            axum::http::StatusCode::CONFLICT,
            err_codes::RETENTION_LOCKED_BY_PLAN,
            "SaaS mode: retention follows the subscription tier.",
        ));
    }
    check_days(body.days)?;
    retention::set_global_default(&ctx.db, body.days, admin.0.user.id)
        .await
        .map_err(db_err)?;
    tracing::info!(days = ?body.days, by = admin.0.user.id, "global retention default updated");
    let bg = ctx.clone();
    tokio::spawn(async move { retention::reconcile_all(&bg).await });
    format::json(serde_json::json!({ "days": body.days }))
}

// ───────────────────────────── O2 cleanup ─────────────────────────────

/// O2 client + identifier of the org, or `None` when not provisioned yet.
async fn o2_target(ctx: &AppContext, org_id: i64) -> Result<Option<(Client, String)>> {
    let Some(settings) = OpenobserveSettings::from_config(&ctx.config) else {
        return Err(coded(
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            err_codes::O2_NOT_CONFIGURED,
            "OpenObserve is not configured on this server.",
        ));
    };
    let creds = openobserve::provisioned_credentials(&ctx.db, org_id)
        .await
        .map_err(|err| {
            tracing::error!(%err, "system: O2 credentials lookup failed");
            Error::InternalServerError
        })?;
    Ok(creds.map(|c| (Client::new(&settings), c.o2_org)))
}

fn require_write(org: &OrgContext) -> Result<()> {
    if org.can_administer() {
        Ok(())
    } else {
        Err(coded(
            axum::http::StatusCode::FORBIDDEN,
            err_codes::SYSTEM_DATA_FORBIDDEN,
            "Owner or admin role required to delete telemetry data.",
        ))
    }
}

fn delete_failed(err: String) -> Error {
    tracing::warn!(%err, "system: O2 deletion failed");
    coded(
        axum::http::StatusCode::BAD_GATEWAY,
        err_codes::O2_DELETE_FAILED,
        "OpenObserve rejected the data deletion.",
    )
}

fn us_to_rfc3339(us: Option<i64>) -> Option<String> {
    us.filter(|v| *v > 0)
        .and_then(DateTime::<Utc>::from_timestamp_micros)
        .map(|d| d.to_rfc3339())
}

fn mb_to_bytes(mb: Option<f64>) -> Option<u64> {
    mb.map(|v| (v * 1024.0 * 1024.0).round().max(0.0) as u64)
}

/// `GET /api/v1/system/o2/streams` — any member (read-only listing).
async fn o2_streams(org: OrgContext, State(ctx): State<AppContext>) -> Result<Response> {
    if OpenobserveSettings::from_config(&ctx.config).is_none() {
        return format::json(O2StreamList::default());
    }
    let Some((client, o2_org)) = o2_target(&ctx, org.org.id).await? else {
        return format::json(O2StreamList {
            configured: true,
            provisioned: false,
            quota_bytes: retention::quota_bytes(&ctx.db, &org.org)
                .await
                .map_err(db_err)?,
            ..Default::default()
        });
    };
    let mut streams: Vec<O2StreamInfo> = client
        .metric_streams_detailed(&o2_org)
        .await
        .map_err(|err| {
            tracing::warn!(%err, "system: O2 streams listing failed");
            coded(
                axum::http::StatusCode::BAD_GATEWAY,
                err_codes::O2_NOT_CONFIGURED,
                "OpenObserve is unreachable.",
            )
        })?
        .into_iter()
        .map(|s| O2StreamInfo {
            name: s.name,
            doc_num: s.stats.doc_num,
            storage_bytes: mb_to_bytes(s.stats.storage_size),
            compressed_bytes: mb_to_bytes(s.stats.compressed_size),
            time_min: us_to_rfc3339(s.stats.doc_time_min),
            time_max: us_to_rfc3339(s.stats.doc_time_max),
            retention_days: s.data_retention_days,
        })
        .collect();
    streams.sort_by(|a, b| a.name.cmp(&b.name));
    let used_bytes: u64 = streams.iter().filter_map(|s| s.compressed_bytes).sum();
    let quota_bytes = retention::quota_bytes(&ctx.db, &org.org)
        .await
        .map_err(db_err)?;
    format::json(O2StreamList {
        configured: true,
        provisioned: true,
        streams,
        used_bytes,
        quota_bytes,
        over_quota: quota_bytes.is_some_and(|q| used_bytes > q),
    })
}

/// `DELETE /api/v1/system/o2/streams/{name}` — owner/admin.
async fn o2_stream_delete(
    org: OrgContext,
    State(ctx): State<AppContext>,
    Path(name): Path<String>,
) -> Result<Response> {
    require_write(&org)?;
    let Some((client, o2_org)) = o2_target(&ctx, org.org.id).await? else {
        return Err(Error::NotFound);
    };
    client
        .delete_stream(&o2_org, &name)
        .await
        .map_err(delete_failed)?;
    tracing::info!(org_id = org.org.id, stream = %name, by = org.auth.user.id, "O2 stream deleted");
    format::json(O2DeleteResult {
        streams: vec![name],
        ..Default::default()
    })
}

/// `POST /api/v1/system/o2/purge` — owner/admin, typed org-name confirmation.
async fn o2_purge(
    org: OrgContext,
    State(ctx): State<AppContext>,
    Json(body): Json<O2PurgeRequest>,
) -> Result<Response> {
    require_write(&org)?;
    if body.confirm.trim() != org.org.name {
        return Err(coded(
            axum::http::StatusCode::UNPROCESSABLE_ENTITY,
            err_codes::CONFIRMATION_MISMATCH,
            "The confirmation does not match the organization name.",
        ));
    }
    let Some((client, o2_org)) = o2_target(&ctx, org.org.id).await? else {
        return format::json(O2DeleteResult::default());
    };
    let streams = client
        .purge_metric_streams(&o2_org)
        .await
        .map_err(delete_failed)?;
    tracing::info!(
        org_id = org.org.id,
        purged = streams.len(),
        by = org.auth.user.id,
        "O2 org purged"
    );
    format::json(O2DeleteResult {
        streams,
        ..Default::default()
    })
}

/// Aligns a range on full hours as O2 requires (start floored, end ceiled):
/// the deleted window always covers the requested one.
pub fn align_hours(start: DateTime<Utc>, end: DateTime<Utc>) -> (DateTime<Utc>, DateTime<Utc>) {
    let floor = |d: DateTime<Utc>| {
        d.with_minute(0)
            .and_then(|d| d.with_second(0))
            .and_then(|d| d.with_nanosecond(0))
            .unwrap_or(d)
    };
    let start = floor(start);
    let end_floor = floor(end);
    let end = if end_floor == end {
        end
    } else {
        end_floor + chrono::Duration::hours(1)
    };
    (start, end)
}

fn parse_rfc3339(raw: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(raw.trim())
        .ok()
        .map(|d| d.with_timezone(&Utc))
}

/// `POST /api/v1/system/o2/streams/{name}/delete-range` — owner/admin.
/// Asynchronous on O2 (compactor job): the response says `scheduled`.
async fn o2_stream_delete_range(
    org: OrgContext,
    State(ctx): State<AppContext>,
    Path(name): Path<String>,
    Json(body): Json<O2DeleteRangeRequest>,
) -> Result<Response> {
    require_write(&org)?;
    let (Some(start), Some(end)) = (parse_rfc3339(&body.start), parse_rfc3339(&body.end)) else {
        return Err(coded(
            axum::http::StatusCode::UNPROCESSABLE_ENTITY,
            err_codes::O2_TIME_RANGE_INVALID,
            "Invalid range: RFC 3339 start and end are required.",
        ));
    };
    if start >= end {
        return Err(coded(
            axum::http::StatusCode::UNPROCESSABLE_ENTITY,
            err_codes::O2_TIME_RANGE_INVALID,
            "Invalid range: start must be before end.",
        ));
    }
    let (start, end) = align_hours(start, end);
    let Some((client, o2_org)) = o2_target(&ctx, org.org.id).await? else {
        return Err(Error::NotFound);
    };
    client
        .delete_stream_range(
            &o2_org,
            &name,
            start.timestamp_micros(),
            end.timestamp_micros(),
        )
        .await
        .map_err(|err| {
            if err.contains("not supported") || err.contains("405") || err.contains("501") {
                tracing::warn!(%err, "system: O2 range deletion unsupported");
                coded(
                    axum::http::StatusCode::BAD_GATEWAY,
                    err_codes::O2_TIME_RANGE_UNSUPPORTED,
                    "OpenObserve does not support time-range deletion on this stream.",
                )
            } else {
                delete_failed(err)
            }
        })?;
    tracing::info!(org_id = org.org.id, stream = %name, %start, %end, by = org.auth.user.id, "O2 range deletion scheduled");
    format::json(O2DeleteResult {
        streams: vec![name],
        scheduled: true,
        start: Some(start.to_rfc3339()),
        end: Some(end.to_rfc3339()),
    })
}

// ───────────────────────────── Platform status ─────────────────────────────

/// `GET /api/v1/system/orgs` — platform admin: every organization with its
/// effective retention, O2 usage and quota. O2 listings run concurrently,
/// each bounded (an unreachable org yields `used_bytes: None`).
async fn orgs_overview(_admin: PlatformAdmin, State(ctx): State<AppContext>) -> Result<Response> {
    use sea_orm::QueryOrder;
    let orgs = organizations::Entity::find()
        .order_by_asc(organizations::Column::Name)
        .all(&ctx.db)
        .await
        .map_err(db_err)?;
    let client = OpenobserveSettings::from_config(&ctx.config).map(|s| Client::new(&s));
    let mut rows = Vec::with_capacity(orgs.len());
    let mut usage_jobs = Vec::new();
    for org in orgs {
        let effective = retention::effective_for(&ctx.db, &org)
            .await
            .map_err(db_err)?;
        let tier = retention::tier_of(&ctx.db, &org).await.map_err(db_err)?;
        let quota = retention::quota_bytes(&ctx.db, &org)
            .await
            .map_err(db_err)?;
        let o2_org = openobserve::provisioned_credentials(&ctx.db, org.id)
            .await
            .ok()
            .flatten()
            .map(|c| c.o2_org);
        usage_jobs.push((rows.len(), o2_org.clone()));
        rows.push(pnex_core::OrgSystemRow {
            org_id: org.id,
            name: org.name.clone(),
            tier_id: org.subscription_tier_id,
            tier_name: tier.map(|t| t.name),
            retention_days: effective.days,
            retention_source: effective.source.as_str().to_string(),
            org_override_days: days_u32(org.data_retention_days),
            provisioned: o2_org.is_some(),
            used_bytes: None,
            stream_count: None,
            quota_bytes: quota,
        });
    }
    if let Some(client) = client {
        let futures = usage_jobs.into_iter().filter_map(|(idx, o2_org)| {
            let client = client.clone();
            o2_org.map(|o2_org| async move {
                let listing = tokio::time::timeout(
                    system_status::PROBE_TIMEOUT,
                    client.metric_streams_detailed(&o2_org),
                )
                .await;
                (idx, listing.ok().and_then(Result::ok))
            })
        });
        for (idx, streams) in futures_util::future::join_all(futures).await {
            if let Some(streams) = streams {
                let used: u64 = streams
                    .iter()
                    .filter_map(|s| mb_to_bytes(s.stats.compressed_size))
                    .sum();
                rows[idx].used_bytes = Some(used);
                rows[idx].stream_count = Some(streams.len());
            }
        }
    }
    format::json(rows)
}

/// `GET /api/v1/system/tiers` — platform admin.
async fn tiers_list(_admin: PlatformAdmin, State(ctx): State<AppContext>) -> Result<Response> {
    use sea_orm::QueryOrder;
    let tiers = subscription_tiers::Entity::find()
        .order_by_asc(subscription_tiers::Column::Id)
        .all(&ctx.db)
        .await
        .map_err(db_err)?;
    format::json(
        tiers
            .into_iter()
            .map(|t| TierOption {
                id: t.id,
                name: t.name,
            })
            .collect::<Vec<_>>(),
    )
}

/// `PUT /api/v1/system/orgs/{org_id}/tier` — platform admin (O37). The
/// tier drives quotas and retention in SaaS mode only; it is still stored
/// in self-hosted mode so that a later switch to SaaS keeps it.
async fn org_tier_put(
    admin: PlatformAdmin,
    State(ctx): State<AppContext>,
    Path(org_id): Path<i64>,
    Json(body): Json<OrgTierUpdate>,
) -> Result<Response> {
    if let Some(tier_id) = body.tier_id {
        let exists = subscription_tiers::Entity::find_by_id(tier_id)
            .one(&ctx.db)
            .await
            .map_err(db_err)?
            .is_some();
        if !exists {
            return Err(coded(
                axum::http::StatusCode::UNPROCESSABLE_ENTITY,
                err_codes::TIER_UNKNOWN,
                "Unknown subscription tier.",
            ));
        }
    }
    let org = organizations::Entity::find_by_id(org_id)
        .one(&ctx.db)
        .await
        .map_err(db_err)?
        .ok_or(Error::NotFound)?;
    let mut active: organizations::ActiveModel = org.into();
    active.subscription_tier_id = sea_orm::Set(body.tier_id);
    active.update(&ctx.db).await.map_err(db_err)?;
    tracing::info!(org_id, tier_id = ?body.tier_id, by = admin.0.user.id, "org subscription tier updated");
    // The tier sets the retention in SaaS mode.
    let bg = ctx.clone();
    tokio::spawn(async move { retention::reconcile_org_id(&bg, org_id).await });
    format::json(OrgTierUpdate {
        tier_id: body.tier_id,
    })
}

/// `GET /api/v1/system/status` — platform admin.
async fn status(_admin: PlatformAdmin, State(ctx): State<AppContext>) -> Result<Response> {
    format::json(system_status::collect(&ctx).await)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(s: &str) -> DateTime<Utc> {
        parse_rfc3339(s).unwrap()
    }

    #[test]
    fn range_is_widened_to_full_hours() {
        let (s, e) = align_hours(at("2026-09-28T10:17:05Z"), at("2026-09-28T12:40:00Z"));
        assert_eq!(s, at("2026-09-28T10:00:00Z"));
        assert_eq!(e, at("2026-09-28T13:00:00Z"));
        // Already aligned bounds are kept as-is.
        let (s, e) = align_hours(at("2026-09-28T10:00:00Z"), at("2026-09-28T11:00:00Z"));
        assert_eq!(
            (s, e),
            (at("2026-09-28T10:00:00Z"), at("2026-09-28T11:00:00Z"))
        );
    }
}
