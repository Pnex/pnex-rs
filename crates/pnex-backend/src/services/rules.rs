//! Rules (analysis.md D829–D830): stored conditions on objects or series, a pass
//! every five minutes under the rights of each rule's author that notifies when a
//! condition turns true, and a replay on the past that sends nothing.

use std::collections::BTreeSet;

use pnex_core::rules::{Rule, RuleCondition, RuleInput, RuleKind};
use sea_orm::{
    ConnectionTrait, DatabaseBackend, DatabaseConnection, DbErr, QueryResult, Statement,
};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::services::ontology::redact::Redaction;

/// Objects a rule follows at most: beyond, the newest ones are not seen.
pub const OBJECTS_FOLLOWED: usize = 2_000;

#[derive(Debug)]
pub enum RuleError {
    Invalid { field: String, token: String },
    NotFound,
    NameTaken,
    VersionConflict { current: i32 },
    Db(DbErr),
}

impl From<DbErr> for RuleError {
    fn from(e: DbErr) -> Self {
        Self::Db(e)
    }
}

fn stmt(sql: &str, values: Vec<sea_orm::Value>) -> Statement {
    Statement::from_sql_and_values(DatabaseBackend::Postgres, sql, values)
}

const TS: &str = "'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"'";

fn select() -> String {
    format!(
        "SELECT id, name, condition, channel_id, enabled, firing, version, created_by, state, \
           to_char(last_fired_at AT TIME ZONE 'UTC', {TS}) AS last_fired_at, \
           to_char(updated_at AT TIME ZONE 'UTC', {TS}) AS updated_at FROM rules"
    )
}

fn view(r: &QueryResult) -> Result<Rule, DbErr> {
    let condition: Value = r.try_get("", "condition")?;
    Ok(Rule {
        id: r.try_get::<Uuid>("", "id")?.to_string(),
        name: r.try_get("", "name")?,
        condition: serde_json::from_value(condition).unwrap_or_default(),
        channel_id: r
            .try_get::<Option<Uuid>>("", "channel_id")?
            .map(|u| u.to_string()),
        enabled: r.try_get("", "enabled")?,
        firing: r.try_get("", "firing")?,
        last_fired_at: r.try_get("", "last_fired_at")?,
        version: r.try_get("", "version")?,
        updated_at: r.try_get("", "updated_at")?,
    })
}

pub async fn list(
    db: &DatabaseConnection,
    org_id: i64,
    limit: i64,
    offset: i64,
) -> Result<(i64, Vec<Rule>), DbErr> {
    let total: i64 = db
        .query_one_raw(stmt(
            "SELECT count(*) AS n FROM rules WHERE org_id = $1",
            vec![org_id.into()],
        ))
        .await?
        .map_or(Ok(0), |r| r.try_get("", "n"))?;
    let sql = format!(
        "{} WHERE org_id = $1 ORDER BY name LIMIT $2 OFFSET $3",
        select()
    );
    let rows = db
        .query_all_raw(stmt(&sql, vec![org_id.into(), limit.into(), offset.into()]))
        .await?;
    Ok((total, rows.iter().map(view).collect::<Result<_, _>>()?))
}

pub async fn get(db: &DatabaseConnection, org_id: i64, id: Uuid) -> Result<Rule, RuleError> {
    let sql = format!("{} WHERE org_id = $1 AND id = $2", select());
    let row = db
        .query_one_raw(stmt(&sql, vec![org_id.into(), id.into()]))
        .await?
        .ok_or(RuleError::NotFound)?;
    Ok(view(&row)?)
}

/// The channel must be one of the org's.
async fn checked(
    db: &DatabaseConnection,
    org_id: i64,
    input: &RuleInput,
) -> Result<Option<Uuid>, RuleError> {
    if let Some((field, token)) = input.violation() {
        return Err(RuleError::Invalid {
            field: field.into(),
            token,
        });
    }
    let Some(raw) = input.channel_id.as_deref().filter(|c| !c.is_empty()) else {
        return Ok(None);
    };
    let bad = || RuleError::Invalid {
        field: "channel_id".into(),
        token: "invalid".into(),
    };
    let id = Uuid::parse_str(raw).map_err(|_| bad())?;
    db.query_one_raw(stmt(
        "SELECT 1 AS x FROM notify_channels WHERE org_id = $1 AND id = $2",
        vec![org_id.into(), id.into()],
    ))
    .await?
    .ok_or_else(bad)?;
    Ok(Some(id))
}

fn is_unique_violation(e: &DbErr) -> bool {
    e.to_string().contains("duplicate key")
}

pub async fn create(
    db: &DatabaseConnection,
    org_id: i64,
    by: i64,
    input: &RuleInput,
) -> Result<Rule, RuleError> {
    let channel = checked(db, org_id, input).await?;
    let id = Uuid::new_v4();
    let condition = serde_json::to_string(&input.condition).unwrap_or_default();
    db.execute_raw(stmt(
        "INSERT INTO rules (id, org_id, name, condition, channel_id, enabled, created_by) \
         VALUES ($1, $2, $3, $4::jsonb, $5, $6, $7)",
        vec![
            id.into(),
            org_id.into(),
            input.name.trim().into(),
            condition.into(),
            channel.into(),
            input.enabled.into(),
            by.into(),
        ],
    ))
    .await
    .map_err(|e| {
        if is_unique_violation(&e) {
            RuleError::NameTaken
        } else {
            RuleError::Db(e)
        }
    })?;
    get(db, org_id, id).await
}

/// On the version read; a changed condition starts again from what is true now.
pub async fn update(
    db: &DatabaseConnection,
    org_id: i64,
    id: Uuid,
    by: i64,
    input: &RuleInput,
) -> Result<Rule, RuleError> {
    let channel = checked(db, org_id, input).await?;
    let Some(expected) = input.expected_version else {
        return Err(RuleError::Invalid {
            field: "expected_version".into(),
            token: "required".into(),
        });
    };
    let current = get(db, org_id, id).await?;
    let restart = current.condition != input.condition;
    let condition = serde_json::to_string(&input.condition).unwrap_or_default();
    let done = db
        .execute_raw(stmt(
            "UPDATE rules SET name = $3, condition = $4::jsonb, channel_id = $5, enabled = $6, \
               created_by = $7, state = CASE WHEN $8 THEN NULL ELSE state END, \
               firing = CASE WHEN $8 THEN false ELSE firing END, \
               version = version + 1, updated_at = now() \
             WHERE org_id = $1 AND id = $2 AND version = $9",
            vec![
                org_id.into(),
                id.into(),
                input.name.trim().into(),
                condition.into(),
                channel.into(),
                input.enabled.into(),
                // The rule now runs with the rights of who changed it last.
                by.into(),
                restart.into(),
                expected.into(),
            ],
        ))
        .await
        .map_err(|e| {
            if is_unique_violation(&e) {
                RuleError::NameTaken
            } else {
                RuleError::Db(e)
            }
        })?;
    if done.rows_affected() != 1 {
        return Err(RuleError::VersionConflict {
            current: current.version,
        });
    }
    get(db, org_id, id).await
}

pub async fn delete(db: &DatabaseConnection, org_id: i64, id: Uuid) -> Result<(), RuleError> {
    let done = db
        .execute_raw(stmt(
            "DELETE FROM rules WHERE org_id = $1 AND id = $2",
            vec![org_id.into(), id.into()],
        ))
        .await?;
    if done.rows_affected() == 0 {
        return Err(RuleError::NotFound);
    }
    Ok(())
}

/// The mask of a rule's author, from the managed role of their membership (as a
/// schedule, D303); `None` when they left the org or may no longer write rules.
pub async fn author_mask(
    db: &DatabaseConnection,
    org_id: i64,
    author: i64,
) -> Result<Option<Redaction>, DbErr> {
    use crate::models::_entities::organization_members as om;
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
    let role = om::Entity::find()
        .filter(om::Column::OrgId.eq(org_id))
        .filter(om::Column::UserId.eq(author))
        .one(db)
        .await?
        .map(|m| m.role);
    let Some(role) = role else {
        return Ok(None);
    };
    let can = |p: &str| crate::services::iam::authorize(&role, p).allowed();
    if !can("rules:write") {
        return Ok(None);
    }
    Ok(Some(
        Redaction::of(can).with_markings(db, org_id, author).await?,
    ))
}

/// Ids of the objects matching an objects condition, as `hide` sees them.
pub async fn matching(
    db: &DatabaseConnection,
    org_id: i64,
    c: &RuleCondition,
    hide: &Redaction,
) -> Result<BTreeSet<String>, String> {
    use crate::services::ontology::query;
    use pnex_core::ontology::api::{OntologyQuery, QUERY_LIMIT_MAX};
    let mut out = BTreeSet::new();
    let mut offset = 0u64;
    while out.len() < OBJECTS_FOLLOWED {
        let q = OntologyQuery {
            type_key: Some(c.type_key.clone()),
            filters: c.filters.clone(),
            limit: Some(QUERY_LIMIT_MAX),
            offset: Some(offset),
            ..Default::default()
        };
        let page = query::run(db, None, org_id, &q, hide.clone())
            .await
            .map_err(|e| e.to_string())?;
        if page.rows.is_empty() {
            break;
        }
        offset += page.rows.len() as u64;
        out.extend(page.rows.into_iter().map(|r| r.object.id));
        if offset >= page.count as u64 {
            break;
        }
    }
    Ok(out)
}

/// The smallest window preset covering `seconds`.
fn window_for(seconds: i64) -> &'static str {
    pnex_core::viz::VIZ_WINDOW_PRESETS
        .iter()
        .find(|(_, s)| *s >= seconds)
        .map(|(k, _)| *k)
        .unwrap_or("30d")
}

/// The points of a series over a window preset, oldest first, in µs.
pub async fn series_points(
    db: &DatabaseConnection,
    client: Option<&crate::services::openobserve::Client>,
    org_id: i64,
    c: &RuleCondition,
    window: &str,
) -> Option<Vec<(i64, f64)>> {
    let spec = pnex_core::SeriesSpec {
        metric: c.metric.clone(),
        device_id: c.device_id.clone(),
        window: window.to_string(),
        labels: Default::default(),
    };
    let batch = crate::services::visualization::series_batch(db, client, org_id, &[spec]).await;
    let s = batch.results.into_iter().next()?;
    s.available.then(|| {
        s.points
            .into_iter()
            .map(|p| ((p.ts * 1_000_000.0) as i64, p.value))
            .collect()
    })
}

/// Sends the message of a rule: by its channel, or in-app to the org.
async fn notify(ctx: &loco_rs::app::AppContext, org_id: i64, rule: &Rule, body: String) {
    let msg = pnex_notify::Message {
        subject: Some(format!("Rule: {}", rule.name)),
        body,
        meta: json!({ "rule_id": rule.id }),
    };
    let entry = pnex_core::NotifyDeliveryEntry {
        org_id,
        source: "rule".into(),
        ..Default::default()
    };
    let channel = rule
        .channel_id
        .as_deref()
        .and_then(|c| Uuid::parse_str(c).ok());
    let Some(channel_id) = channel else {
        crate::services::notify::deliver_in_app(entry, &msg);
        return;
    };
    use crate::models::_entities::notify_channels as nc;
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
    let row = nc::Entity::find()
        .filter(nc::Column::OrgId.eq(org_id))
        .filter(nc::Column::Id.eq(channel_id))
        .one(&ctx.db)
        .await
        .ok()
        .flatten();
    let Some(m) = row.filter(|m| m.enabled) else {
        crate::services::notify::deliver_in_app(entry, &msg);
        return;
    };
    let sent = async {
        let ring = crate::controllers::secrets::keyring(ctx).map_err(|e| format!("{e:?}"))?;
        let config =
            crate::services::secrets::notify::sendable(&ctx.db, &ring, org_id, &m.kind, &m.config)
                .await
                .map_err(|e| format!("{e:?}"))?;
        let ch = pnex_notify::channel(&m.kind).ok_or("unknown channel kind")?;
        ch.send(&config, &msg).await.map_err(|e| e.to_string())
    }
    .await;
    crate::services::notify_journal::submit(pnex_core::NotifyDeliveryEntry {
        channel_id: m.id,
        channel_kind: m.kind.clone(),
        status: if sent.is_ok() {
            pnex_core::DELIVERY_SENT
        } else {
            pnex_core::DELIVERY_FAILED
        }
        .into(),
        error: sent.err(),
        subject: msg.subject.clone(),
        ..entry
    });
}

/// D829: one pass over the enabled rules. An objects rule notifies the objects
/// that started matching since the last pass (its first pass only records them);
/// a series rule notifies when its condition starts holding long enough.
pub async fn pass(ctx: &loco_rs::app::AppContext) -> Result<u64, DbErr> {
    let db = &ctx.db;
    let rows = db
        .query_all_raw(stmt(
            &format!(
                "{} WHERE enabled",
                select().replace("FROM rules", ", org_id FROM rules")
            ),
            vec![],
        ))
        .await?;
    let client = crate::controllers::ontology::o2(ctx);
    let mut fired = 0;
    for row in &rows {
        let rule = view(row)?;
        let org_id: i64 = row.try_get("", "org_id")?;
        let state: Option<Value> = row.try_get("", "state")?;
        let Some(author) = row.try_get::<Option<i64>>("", "created_by")? else {
            continue;
        };
        // Its author left or lost the right: the rule rests until someone saves it.
        let Some(hide) = author_mask(db, org_id, author).await? else {
            continue;
        };
        let c = &rule.condition;
        let (now_state, firing, message) = match c.kind {
            RuleKind::Objects => {
                let Ok(now) = matching(db, org_id, c, &hide).await else {
                    continue;
                };
                let before: Option<BTreeSet<String>> = state
                    .as_ref()
                    .and_then(|s| serde_json::from_value(s["ids"].clone()).ok());
                let new = before.as_ref().map_or(0, |b| now.difference(b).count());
                let message =
                    (new > 0).then(|| format!("{new} new object(s) of type {} match.", c.type_key));
                (json!({ "ids": now }), !now.is_empty(), message)
            }
            RuleKind::Series => {
                let hold = i64::from(c.hold_minutes) * 60;
                let window = window_for(hold + 600);
                let Some(points) = series_points(db, client.as_ref(), org_id, c, window).await
                else {
                    continue;
                };
                let events =
                    pnex_core::rules::series_events(&points, c.cmp, c.threshold, hold * 1_000_000);
                // Holding now: the last stretch reached its duration and still holds.
                let holding = points
                    .last()
                    .is_some_and(|(_, v)| c.cmp.holds(*v, c.threshold))
                    && events.last().is_some_and(|(at, _)| {
                        points
                            .iter()
                            .filter(|(t, _)| t >= at)
                            .all(|(_, v)| c.cmp.holds(*v, c.threshold))
                    });
                let message = (holding && !rule.firing).then(|| {
                    let v = points.last().map(|p| p.1).unwrap_or_default();
                    format!("{} of {} is {v}.", c.metric, c.device_id)
                });
                (json!({ "holding": holding }), holding, message)
            }
        };
        if let Some(body) = &message {
            notify(ctx, org_id, &rule, body.clone()).await;
            fired += 1;
        }
        db.execute_raw(stmt(
            "UPDATE rules SET state = $2::jsonb, firing = $3, \
               last_fired_at = CASE WHEN $4 THEN now() ELSE last_fired_at END WHERE id = $1",
            vec![
                Uuid::parse_str(&rule.id).unwrap_or_default().into(),
                now_state.to_string().into(),
                firing.into(),
                message.is_some().into(),
            ],
        ))
        .await?;
    }
    Ok(fired)
}

/// D830: when the rule would have fired over a window preset, under `hide` (the
/// caller's mask), sending nothing.
pub async fn replay(
    db: &DatabaseConnection,
    client: Option<&crate::services::openobserve::Client>,
    org_id: i64,
    rule: &Rule,
    hide: &Redaction,
    window: &str,
) -> Result<pnex_core::rules::RuleReplay, RuleError> {
    use pnex_core::rules::{object_events, series_events, ObjectChange, RuleEvent, RuleReplay};
    let seconds = pnex_core::viz::VIZ_WINDOW_PRESETS
        .iter()
        .find(|(k, _)| *k == window)
        .map(|(_, s)| *s)
        .ok_or(RuleError::Invalid {
            field: "window".into(),
            token: "invalid".into(),
        })?;
    let at = |us: i64| {
        chrono::DateTime::from_timestamp_micros(us)
            .map(|t| t.to_rfc3339())
            .unwrap_or_default()
    };
    let c = &rule.condition;
    match c.kind {
        RuleKind::Series => {
            let points = series_points(db, client, org_id, c, window).await;
            let complete = points.is_some();
            let hold = i64::from(c.hold_minutes) * 60 * 1_000_000;
            let events = series_events(&points.unwrap_or_default(), c.cmp, c.threshold, hold)
                .into_iter()
                .map(|(t, v)| RuleEvent {
                    at: at(t),
                    object_id: None,
                    value: Some(v),
                })
                .collect();
            Ok(RuleReplay { events, complete })
        }
        RuleKind::Objects => {
            // Every object of the type the caller sees, then their history.
            let all = RuleCondition {
                filters: Vec::new(),
                ..c.clone()
            };
            let ids: Vec<String> = matching(db, org_id, &all, hide)
                .await
                .map_err(|_| RuleError::Invalid {
                    field: "condition".into(),
                    token: "invalid".into(),
                })?
                .into_iter()
                .collect();
            let end = chrono::Utc::now().timestamp_micros();
            let start = end - seconds * 1_000_000;
            let history = crate::services::ontology::changes::history_of(
                db, client, org_id, &ids, start, end, 10_000,
            )
            .await
            .ok()
            .flatten();
            let complete = history.is_some();
            let s = crate::services::ontology::schema(db, org_id)
                .await
                .map_err(|_| RuleError::NotFound)?;
            let props = |v: &Value| v.get("properties").and_then(Value::as_object).cloned();
            let mut changes: Vec<ObjectChange> = history
                .unwrap_or_default()
                .into_iter()
                .map(|(id, mut cv)| {
                    hide.change(&s, &mut cv);
                    let at_us = chrono::DateTime::parse_from_rfc3339(&cv.at)
                        .map(|t| t.timestamp_micros())
                        .unwrap_or_default();
                    let gone = cv.action == "object.archive";
                    ObjectChange {
                        at_us,
                        object_id: id,
                        before: props(&cv.before),
                        after: if gone { None } else { props(&cv.after) },
                    }
                })
                .collect();
            changes.sort_by_key(|c| c.at_us);
            let events = object_events(&changes, &c.filters)
                .into_iter()
                .map(|(t, id)| RuleEvent {
                    at: at(t),
                    object_id: Some(id),
                    value: None,
                })
                .collect();
            Ok(RuleReplay { events, complete })
        }
    }
}

/// Seconds between two passes (D829).
const PASS_SECS: u64 = 300;

/// The pass every five minutes, one pod at a time; skipped in tests.
pub fn spawn(ctx: &loco_rs::app::AppContext) {
    use loco_rs::config::WorkerMode;
    if matches!(ctx.config.workers.mode, WorkerMode::ForegroundBlocking) {
        return;
    }
    let ctx = ctx.clone();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(PASS_SECS));
        loop {
            tick.tick().await;
            let turn = crate::services::singleton::my_turn(
                &ctx.db,
                "rules-pass",
                std::time::Duration::from_secs(PASS_SECS),
            )
            .await;
            if !turn {
                continue;
            }
            match pass(&ctx).await {
                Ok(n) if n > 0 => tracing::info!(fired = n, "rules pass"),
                Ok(_) => {}
                Err(e) => tracing::warn!(error = %e, "rules pass failed"),
            }
        }
    });
}
