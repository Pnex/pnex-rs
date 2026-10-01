//! Database side of the flow cluster (D106): worker registry, placements,
//! row-based leases. Portable Postgres/sqlite (sea-orm only, no advisory
//! lock, no `NOTIFY`). Every write that can race is **conditional** (a
//! `WHERE` on the expected state) so two controllers or two workers can
//! never both win.

use std::collections::HashMap;

use chrono::{DateTime, FixedOffset, Utc};
use sea_orm::sea_query::{Expr, ExprTrait, OnConflict};
use sea_orm::{
    ColumnTrait, DatabaseConnection, DbErr, EntityTrait, FromQueryResult, QueryFilter, QuerySelect,
    Set,
};

use crate::models::_entities::{flow_leases, flow_placements, flow_workers, flows};

pub(crate) fn now() -> DateTime<FixedOffset> {
    Utc::now().fixed_offset()
}

fn ago(ms: u64) -> DateTime<FixedOffset> {
    (Utc::now() - chrono::Duration::milliseconds(ms as i64)).fixed_offset()
}

// ─────────────────────────────── Workers ───────────────────────────────

/// Registers (or re-registers) a worker under a new boot. A previous
/// incarnation with the same id is superseded: its next heartbeat fails
/// and it fences itself.
pub async fn register_worker(
    db: &DatabaseConnection,
    id: &str,
    boot: i64,
    advertise_url: &str,
    capacity: u32,
) -> Result<(), DbErr> {
    let t = now();
    flow_workers::Entity::insert(flow_workers::ActiveModel {
        id: Set(id.to_string()),
        boot: Set(boot),
        advertise_url: Set(advertise_url.to_string()),
        capacity: Set(capacity as i32),
        draining: Set(false),
        started_at: Set(t),
        heartbeat_at: Set(t),
    })
    .on_conflict(
        OnConflict::column(flow_workers::Column::Id)
            .update_columns([
                flow_workers::Column::Boot,
                flow_workers::Column::AdvertiseUrl,
                flow_workers::Column::Capacity,
                flow_workers::Column::Draining,
                flow_workers::Column::StartedAt,
                flow_workers::Column::HeartbeatAt,
            ])
            .to_owned(),
    )
    .exec(db)
    .await?;
    Ok(())
}

/// Heartbeat of `(id, boot)`. `Ok(false)` = superseded (another process
/// registered the same id) or collected: the caller must fence itself.
pub async fn heartbeat(db: &DatabaseConnection, id: &str, boot: i64) -> Result<bool, DbErr> {
    let res = flow_workers::Entity::update_many()
        .col_expr(flow_workers::Column::HeartbeatAt, Expr::value(now()))
        .filter(flow_workers::Column::Id.eq(id))
        .filter(flow_workers::Column::Boot.eq(boot))
        .exec(db)
        .await?;
    Ok(res.rows_affected == 1)
}

pub async fn set_draining(db: &DatabaseConnection, id: &str, boot: i64) -> Result<(), DbErr> {
    flow_workers::Entity::update_many()
        .col_expr(flow_workers::Column::Draining, Expr::value(true))
        .filter(flow_workers::Column::Id.eq(id))
        .filter(flow_workers::Column::Boot.eq(boot))
        .exec(db)
        .await?;
    Ok(())
}

/// Removes the row of a stopping worker (only its own boot).
pub async fn deregister_worker(db: &DatabaseConnection, id: &str, boot: i64) -> Result<(), DbErr> {
    flow_workers::Entity::delete_many()
        .filter(flow_workers::Column::Id.eq(id))
        .filter(flow_workers::Column::Boot.eq(boot))
        .exec(db)
        .await?;
    Ok(())
}

pub async fn workers(db: &DatabaseConnection) -> Result<Vec<flow_workers::Model>, DbErr> {
    flow_workers::Entity::find().all(db).await
}

pub async fn worker(
    db: &DatabaseConnection,
    id: &str,
) -> Result<Option<flow_workers::Model>, DbErr> {
    flow_workers::Entity::find_by_id(id.to_string())
        .one(db)
        .await
}

/// A worker is alive while its heartbeat is younger than `ttl_ms`.
pub fn is_alive(w: &flow_workers::Model, ttl_ms: u64) -> bool {
    w.heartbeat_at >= ago(ttl_ms)
}

/// Collects worker rows dead for longer than `gc_ms` that own nothing.
pub async fn collect_dead_workers(
    db: &DatabaseConnection,
    gc_ms: u64,
    placements: &HashMap<i64, String>,
) -> Result<u64, DbErr> {
    let mut n = 0;
    for w in workers(db).await? {
        if w.heartbeat_at < ago(gc_ms) && !placements.values().any(|p| *p == w.id) {
            n += flow_workers::Entity::delete_many()
                .filter(flow_workers::Column::Id.eq(w.id.clone()))
                .filter(flow_workers::Column::Boot.eq(w.boot))
                .exec(db)
                .await?
                .rows_affected;
        }
    }
    Ok(n)
}

// ────────────────────────────── Placements ──────────────────────────────

pub async fn placement(
    db: &DatabaseConnection,
    org_id: i64,
) -> Result<Option<flow_placements::Model>, DbErr> {
    flow_placements::Entity::find_by_id(org_id).one(db).await
}

pub async fn placements(db: &DatabaseConnection) -> Result<Vec<flow_placements::Model>, DbErr> {
    flow_placements::Entity::find().all(db).await
}

pub async fn placements_of(
    db: &DatabaseConnection,
    worker_id: &str,
) -> Result<Vec<flow_placements::Model>, DbErr> {
    flow_placements::Entity::find()
        .filter(flow_placements::Column::WorkerId.eq(worker_id))
        .all(db)
        .await
}

/// Creates the placement of `org_id` on `worker_id` unless one exists
/// (a concurrent creator wins: the caller re-reads).
pub async fn insert_placement(
    db: &DatabaseConnection,
    org_id: i64,
    worker_id: &str,
) -> Result<(), DbErr> {
    let res = flow_placements::Entity::insert(flow_placements::ActiveModel {
        org_id: Set(org_id),
        worker_id: Set(worker_id.to_string()),
        epoch: Set(1),
        revision: Set(0),
        updated_at: Set(now()),
    })
    .on_conflict(
        OnConflict::column(flow_placements::Column::OrgId)
            .do_nothing()
            .to_owned(),
    )
    .exec_without_returning(db)
    .await;
    match res {
        Ok(_) | Err(DbErr::RecordNotInserted) => Ok(()),
        Err(e) => Err(e),
    }
}

/// Moves `org_id` from `from` to `to` (conditional on the current owner):
/// bumps the epoch (fencing) and the revision (the new owner projects).
pub async fn move_placement(
    db: &DatabaseConnection,
    org_id: i64,
    from: &str,
    to: &str,
) -> Result<bool, DbErr> {
    let res = flow_placements::Entity::update_many()
        .col_expr(flow_placements::Column::WorkerId, Expr::value(to))
        .col_expr(
            flow_placements::Column::Epoch,
            Expr::col(flow_placements::Column::Epoch).add(1),
        )
        .col_expr(
            flow_placements::Column::Revision,
            Expr::col(flow_placements::Column::Revision).add(1),
        )
        .col_expr(flow_placements::Column::UpdatedAt, Expr::value(now()))
        .filter(flow_placements::Column::OrgId.eq(org_id))
        .filter(flow_placements::Column::WorkerId.eq(from))
        .exec(db)
        .await?;
    Ok(res.rows_affected == 1)
}

pub async fn release_placement(
    db: &DatabaseConnection,
    org_id: i64,
    worker_id: &str,
) -> Result<bool, DbErr> {
    let res = flow_placements::Entity::delete_many()
        .filter(flow_placements::Column::OrgId.eq(org_id))
        .filter(flow_placements::Column::WorkerId.eq(worker_id))
        .exec(db)
        .await?;
    Ok(res.rows_affected == 1)
}

/// The projected flows of `org_id` changed: its owner must reproject it.
pub async fn bump_revision(db: &DatabaseConnection, org_id: i64) -> Result<(), DbErr> {
    flow_placements::Entity::update_many()
        .col_expr(
            flow_placements::Column::Revision,
            Expr::col(flow_placements::Column::Revision).add(1),
        )
        .col_expr(flow_placements::Column::UpdatedAt, Expr::value(now()))
        .filter(flow_placements::Column::OrgId.eq(org_id))
        .exec(db)
        .await?;
    Ok(())
}

#[derive(Debug, FromQueryResult)]
struct OrgWeight {
    org_id: i64,
    n: i64,
}

/// Deployed flows per org (the placement weight). One indexed aggregate
/// (`idx_flows_status_org_id`).
pub async fn org_weights(db: &DatabaseConnection) -> Result<HashMap<i64, u64>, DbErr> {
    let rows = flows::Entity::find()
        .select_only()
        .column(flows::Column::OrgId)
        .column_as(flows::Column::Id.count(), "n")
        .filter(flows::Column::Status.eq(pnex_core::FLOW_STATUS_DEPLOYED))
        .group_by(flows::Column::OrgId)
        .into_model::<OrgWeight>()
        .all(db)
        .await?;
    Ok(rows
        .into_iter()
        .map(|r| (r.org_id, Ord::max(r.n, 0) as u64))
        .collect())
}

/// Deployed flows of one org.
pub async fn org_weight(db: &DatabaseConnection, org_id: i64) -> Result<u64, DbErr> {
    use sea_orm::PaginatorTrait;
    Ok(flows::Entity::find()
        .filter(flows::Column::OrgId.eq(org_id))
        .filter(flows::Column::Status.eq(pnex_core::FLOW_STATUS_DEPLOYED))
        .count(db)
        .await?)
}

// ──────────────────────────────── Leases ────────────────────────────────

/// Acquires or renews the lease `name` for `holder` for `ttl_ms`.
/// Conditional update (held by me, or expired) then insert-if-absent: at
/// most one holder at a time, portable, no session state.
///
/// On Postgres the expiry is computed AND compared with the database clock
/// (`now()`), never with the pod clocks: a pod whose clock runs ahead would
/// otherwise see live leases as expired and steal them.
pub async fn try_lease(
    db: &DatabaseConnection,
    name: &str,
    holder: &str,
    ttl_ms: u64,
) -> Result<bool, DbErr> {
    use sea_orm::ConnectionTrait;
    let pg = db.get_database_backend() == sea_orm::DatabaseBackend::Postgres;
    let ttl_secs = ttl_ms as f64 / 1000.0;
    let expires = (Utc::now() + chrono::Duration::milliseconds(ttl_ms as i64)).fixed_offset();
    let (expires_expr, now_expr) = if pg {
        (
            Expr::cust_with_values("now() + make_interval(secs => $1)", [ttl_secs]),
            Expr::cust("now()"),
        )
    } else {
        (Expr::value(expires), Expr::value(now()))
    };
    let res = flow_leases::Entity::update_many()
        .col_expr(flow_leases::Column::Holder, Expr::value(holder))
        .col_expr(flow_leases::Column::ExpiresAt, expires_expr)
        .filter(flow_leases::Column::Name.eq(name))
        .filter(
            sea_orm::Condition::any()
                .add(flow_leases::Column::Holder.eq(holder))
                .add(Expr::col(flow_leases::Column::ExpiresAt).lt(now_expr)),
        )
        .exec(db)
        .await?;
    if res.rows_affected == 1 {
        return Ok(true);
    }
    if pg {
        let ins = db
            .execute_raw(sea_orm::Statement::from_sql_and_values(
                sea_orm::DatabaseBackend::Postgres,
                "INSERT INTO flow_leases (name, holder, expires_at) \
                 VALUES ($1, $2, now() + make_interval(secs => $3)) \
                 ON CONFLICT (name) DO NOTHING",
                [name.into(), holder.into(), ttl_secs.into()],
            ))
            .await?;
        return Ok(ins.rows_affected() == 1);
    }
    let ins = flow_leases::Entity::insert(flow_leases::ActiveModel {
        name: Set(name.to_string()),
        holder: Set(holder.to_string()),
        expires_at: Set(expires),
    })
    .on_conflict(
        OnConflict::column(flow_leases::Column::Name)
            .do_nothing()
            .to_owned(),
    )
    .exec_without_returning(db)
    .await;
    match ins {
        Ok(n) => Ok(n == 1),
        Err(DbErr::RecordNotInserted) => Ok(false),
        Err(e) => Err(e),
    }
}

/// Releases every `task:*` singleton lease held by `holder` (graceful
/// shutdown: another pod takes the sweeps over at its next tick instead of
/// waiting for the expiry).
pub async fn release_task_leases(db: &DatabaseConnection, holder: &str) -> Result<u64, DbErr> {
    let res = flow_leases::Entity::delete_many()
        .filter(flow_leases::Column::Name.starts_with("task:"))
        .filter(flow_leases::Column::Holder.eq(holder))
        .exec(db)
        .await?;
    Ok(res.rows_affected)
}

/// Gives the lease back (graceful shutdown: the next candidate takes over
/// at once instead of waiting for the expiry).
pub async fn release_lease(db: &DatabaseConnection, name: &str, holder: &str) -> Result<(), DbErr> {
    flow_leases::Entity::delete_many()
        .filter(flow_leases::Column::Name.eq(name))
        .filter(flow_leases::Column::Holder.eq(holder))
        .exec(db)
        .await?;
    Ok(())
}
