//! Per-device liveness lease (D9, revised D108) — Valkey hot path, Postgres
//! cold record.
//!
//! The per-frame traffic of a live session (claim, periodic touch, release)
//! never reaches Postgres anymore:
//!
//! - **lease** `…lease:{id}` = owning session id, expiring after the silence
//!   TTL (`PX`). Claim / touch / release are atomic Lua compare-and-set
//!   scripts: first-live-wins across pods, owner-checked refresh and release.
//! - **last seen** `…seen` = sorted set, member = device registry id, score
//!   = epoch ms of the last sign of life.
//!
//! Postgres only receives transitions: `device_states.last_seen_at` on a
//! clean disconnect and when the reaper retires a silent device from the
//! sorted set, and `device_registries.active` flips (the reaper remains its
//! sole writer). Reads
//! merge both sources (freshest wins), so a Valkey restart loses no history.
//!
//! Valkey is mandatory: [`init`] fails the boot when `settings.valkey.url`
//! is not set. Keys are namespaced by database name, so two instances (or
//! two test databases) sharing one Valkey never see each other's leases.

use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

use chrono::{DateTime, FixedOffset, TimeDelta, Utc};
use loco_rs::config::Config;
use loco_rs::prelude::*;
use redis::aio::ConnectionManager;
use sea_orm::{sea_query::OnConflict, ColumnTrait, EntityTrait, QueryFilter, QuerySelect, Set};

use crate::models::_entities::{device_registries, device_states};
use crate::services::settings::IngestSettings;

/// Grant the lease when free, expired or already ours; refresh last seen.
/// KEYS: lease, seen — ARGV: session, ttl_ms, now_ms, device id.
const CLAIM: &str = r"
local cur = redis.call('GET', KEYS[1])
if cur and cur ~= ARGV[1] then return 0 end
redis.call('SET', KEYS[1], ARGV[1], 'PX', ARGV[2])
redis.call('ZADD', KEYS[2], ARGV[3], ARGV[4])
return 1";

/// Same as [`CLAIM`] for a running session: an expired lease (silence the
/// session survived, Valkey restart) is taken back; a lease held by another
/// session means this one is superseded.
const TOUCH: &str = CLAIM;

/// Owner-checked release: honest last seen, lease dropped.
/// KEYS: lease, seen — ARGV: session, now_ms, device id.
const RELEASE: &str = r"
local cur = redis.call('GET', KEYS[1])
if cur and cur ~= ARGV[1] then return 0 end
redis.call('DEL', KEYS[1])
redis.call('ZADD', KEYS[2], ARGV[2], ARGV[3])
return 1";

struct Store {
    url: String,
    ns: String,
}

static STORE: OnceLock<Store> = OnceLock::new();

/// Connects the liveness store. Mandatory: an error when Valkey is not
/// configured (boot fails). Idempotent (first config wins per process).
pub async fn init(config: &Config) -> Result<()> {
    if STORE.get().is_some() {
        return Ok(());
    }
    let url = crate::services::last_cache::ValkeySettings::from_config(config)
        .and_then(|v| v.url)
        .ok_or_else(|| {
            Error::string(
                "Valkey is required for device liveness: set settings.valkey.url (VALKEY_URL)",
            )
        })?;
    if crate::services::shared_valkey::conn_url(&url)
        .await
        .is_none()
    {
        return Err(Error::string(&format!("invalid Valkey url: {url}")));
    }
    let _ = STORE.set(Store {
        url,
        ns: namespace(&config.database.uri),
    });
    Ok(())
}

/// Key prefix derived from the database name (`pnex:{db}:live:`).
fn namespace(uri: &str) -> String {
    let path = uri.split(['?', '#']).next().unwrap_or_default();
    let last = path.rsplit('/').next().unwrap_or_default();
    let db = if last.is_empty() { "default" } else { last };
    format!("pnex:{db}:live:")
}

fn store() -> Result<&'static Store> {
    STORE
        .get()
        .ok_or_else(|| Error::string("device liveness store not initialized"))
}

impl Store {
    async fn conn(&self) -> Result<ConnectionManager> {
        crate::services::shared_valkey::conn_url(&self.url)
            .await
            .ok_or(Error::InternalServerError)
    }
    fn lease(&self, id: i64) -> String {
        format!("{}lease:{id}", self.ns)
    }
    fn seen(&self) -> String {
        format!("{}seen", self.ns)
    }
    fn epoch(&self) -> String {
        format!("{}epoch", self.ns)
    }
}

fn valkey_err(op: &'static str, device: i64) -> impl FnOnce(redis::RedisError) -> Error {
    move |e| {
        tracing::warn!(device, error = %e, "liveness {op} failed");
        Error::InternalServerError
    }
}

fn ttl_ms(silence_ttl_secs: i64) -> i64 {
    (silence_ttl_secs * 1000).max(1)
}

/// `last_seen` still fresh with respect to the silence TTL?
pub fn is_fresh(last_seen: DateTime<Utc>, silence_ttl_secs: i64) -> bool {
    last_seen + TimeDelta::seconds(silence_ttl_secs) > Utc::now()
}

/// Atomic lease claim of a new session (admission, anti-clone across pods).
/// Granted when no live session holds the lease (released, expired after
/// the silence TTL, never seen) or when it is already this session's.
/// `Ok(false)` = a live session holds the lease (4003).
pub async fn claim(device_registry_id: i64, session: &str, silence_ttl_secs: i64) -> Result<bool> {
    run_claim(
        CLAIM,
        "claim",
        device_registry_id,
        session,
        silence_ttl_secs,
    )
    .await
}

/// Periodic liveness refresh of the session holding the lease. `Ok(false)`
/// = another session claimed the lease meanwhile (this one is superseded
/// and should close).
pub async fn touch_owned(
    device_registry_id: i64,
    session: &str,
    silence_ttl_secs: i64,
) -> Result<bool> {
    run_claim(
        TOUCH,
        "touch",
        device_registry_id,
        session,
        silence_ttl_secs,
    )
    .await
}

async fn run_claim(
    script: &str,
    op: &'static str,
    device_registry_id: i64,
    session: &str,
    silence_ttl_secs: i64,
) -> Result<bool> {
    let s = store()?;
    let mut conn = s.conn().await?;
    let granted: i64 = redis::Script::new(script)
        .key(s.lease(device_registry_id))
        .key(s.seen())
        .arg(session)
        .arg(ttl_ms(silence_ttl_secs))
        .arg(Utc::now().timestamp_millis())
        .arg(device_registry_id)
        .invoke_async(&mut conn)
        .await
        .map_err(valkey_err(op, device_registry_id))?;
    Ok(granted == 1)
}

/// Clean disconnect of `session`: lease released (an immediate reconnect is
/// accepted) and honest last seen, persisted to Postgres. Owner-checked: the
/// late close of a superseded session never touches the newer session's
/// lease. `Ok(true)` = this session still held the lease.
pub async fn release(
    db: &DatabaseConnection,
    device_registry_id: i64,
    session: &str,
) -> Result<bool> {
    let s = store()?;
    let mut conn = s.conn().await?;
    let now = Utc::now();
    let owned: i64 = redis::Script::new(RELEASE)
        .key(s.lease(device_registry_id))
        .key(s.seen())
        .arg(session)
        .arg(now.timestamp_millis())
        .arg(device_registry_id)
        .invoke_async(&mut conn)
        .await
        .map_err(valkey_err("release", device_registry_id))?;
    if owned != 1 {
        return Ok(false);
    }
    persist_last_seen(db, &[(device_registry_id, now)]).await?;
    Ok(true)
}

/// Records a sign of life at `at` without any session (tests, tooling).
pub async fn mark_seen(device_registry_id: i64, at: DateTime<Utc>) -> Result<()> {
    let s = store()?;
    let mut conn = s.conn().await?;
    redis::cmd("ZADD")
        .arg(s.seen())
        .arg(at.timestamp_millis())
        .arg(device_registry_id)
        .query_async::<i64>(&mut conn)
        .await
        .map_err(valkey_err("mark_seen", device_registry_id))?;
    Ok(())
}

/// Test hook: simulates a Valkey restart as seen by the reaper (the epoch
/// marker is lost, the restart grace starts at the next pass).
pub async fn forget_epoch() -> Result<()> {
    let s = store()?;
    let mut conn = s.conn().await?;
    redis::cmd("DEL")
        .arg(s.epoch())
        .query_async::<i64>(&mut conn)
        .await
        .map_err(|_| Error::InternalServerError)?;
    Ok(())
}

/// Liveness of one device as seen by the API.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Seen {
    /// Last sign of life (Valkey while recent, Postgres otherwise).
    pub last_seen: Option<DateTime<Utc>>,
    /// A session currently holds the lease.
    pub connected: bool,
}

/// Liveness of a batch of devices: one Valkey round trip (lease `EXISTS`
/// per id + `ZMSCORE`) and one Postgres query; the freshest last seen wins.
/// Devices absent from both sources map to `Seen { None, false }`.
pub async fn seen_many(db: &DatabaseConnection, ids: &[i64]) -> Result<HashMap<i64, Seen>> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let s = store()?;
    let mut conn = s.conn().await?;
    let mut pipe = redis::pipe();
    for id in ids {
        pipe.cmd("EXISTS").arg(s.lease(*id));
    }
    pipe.cmd("ZMSCORE").arg(s.seen()).arg(ids);
    let mut reply: Vec<redis::Value> = pipe.query_async(&mut conn).await.map_err(|e| {
        tracing::warn!(error = %e, "liveness read failed");
        Error::InternalServerError
    })?;
    let scores: Vec<Option<f64>> =
        redis::from_redis_value(reply.pop().unwrap_or(redis::Value::Nil))
            .map_err(|_| Error::InternalServerError)?;
    let connected: Vec<bool> = reply
        .into_iter()
        .map(|v| redis::from_redis_value::<i64>(v).unwrap_or(0) == 1)
        .collect();

    let cold: HashMap<i64, DateTime<Utc>> = device_states::Entity::find()
        .filter(device_states::Column::DeviceRegistryId.is_in(ids.to_vec()))
        .all(db)
        .await
        .map_err(|_| Error::InternalServerError)?
        .into_iter()
        .map(|st| (st.device_registry_id, st.last_seen_at.with_timezone(&Utc)))
        .collect();

    Ok(ids
        .iter()
        .enumerate()
        .map(|(i, id)| {
            let hot = scores
                .get(i)
                .copied()
                .flatten()
                .and_then(|ms| DateTime::from_timestamp_millis(ms as i64));
            let last_seen = match (hot, cold.get(id).copied()) {
                (Some(h), Some(c)) => Some(h.max(c)),
                (h, c) => h.or(c),
            };
            (
                *id,
                Seen {
                    last_seen,
                    connected: connected.get(i).copied().unwrap_or(false),
                },
            )
        })
        .collect())
}

/// Liveness of a single device (see [`seen_many`]).
pub async fn seen_of(db: &DatabaseConnection, device_registry_id: i64) -> Result<Seen> {
    Ok(seen_many(db, &[device_registry_id])
        .await?
        .remove(&device_registry_id)
        .unwrap_or(Seen {
            last_seen: None,
            connected: false,
        }))
}

/// Upserts `device_states.last_seen_at` of devices that still exist (a
/// device deleted meanwhile is skipped, never an FK error for the batch).
async fn persist_last_seen(db: &DatabaseConnection, rows: &[(i64, DateTime<Utc>)]) -> Result<()> {
    if rows.is_empty() {
        return Ok(());
    }
    let ids: Vec<i64> = rows.iter().map(|(id, _)| *id).collect();
    let existing: HashSet<i64> = device_registries::Entity::find()
        .select_only()
        .column(device_registries::Column::Id)
        .filter(device_registries::Column::Id.is_in(ids))
        .into_tuple::<i64>()
        .all(db)
        .await
        .map_err(|_| Error::InternalServerError)?
        .into_iter()
        .collect();
    let models: Vec<device_states::ActiveModel> = rows
        .iter()
        .filter(|(id, _)| existing.contains(id))
        .map(|(id, at)| device_states::ActiveModel {
            device_registry_id: Set(*id),
            last_seen_at: Set(DateTime::<FixedOffset>::from(*at)),
            ..Default::default()
        })
        .collect();
    if models.is_empty() {
        return Ok(());
    }
    device_states::Entity::insert_many(models)
        .on_conflict(
            OnConflict::column(device_states::Column::DeviceRegistryId)
                .update_column(device_states::Column::LastSeenAt)
                .to_owned(),
        )
        .exec(db)
        .await
        .map_err(|e| {
            tracing::warn!(error = %e, "liveness persist failed");
            Error::InternalServerError
        })?;
    Ok(())
}

/// Reaper, sole writer of `device_registries.active`: fresh in the sorted
/// set → `active=true`; active without a fresh sign of life → `false`.
/// Silent devices are moved out of the sorted set into Postgres
/// (`last_seen_at`), which keeps the set to the recently-live fleet.
///
/// After a Valkey restart (the `epoch` marker is gone) deactivations are
/// held back for one silence TTL: live sessions re-register at their next
/// touch instead of the whole fleet flapping offline. Valkey unreachable →
/// error, nothing is flipped. Returns (activated, deactivated).
pub async fn deactivate_stale(
    db: &DatabaseConnection,
    silence_ttl_secs: i64,
) -> Result<(u64, u64)> {
    let s = store()?;
    let mut conn = s.conn().await?;
    let now_ms = Utc::now().timestamp_millis();
    let cutoff = now_ms - ttl_ms(silence_ttl_secs);
    let rerr = |e: redis::RedisError| {
        tracing::warn!(error = %e, "liveness reaper read failed");
        Error::InternalServerError
    };

    let _: Option<String> = redis::cmd("SET")
        .arg(s.epoch())
        .arg(now_ms)
        .arg("NX")
        .query_async(&mut conn)
        .await
        .map_err(rerr)?;
    let epoch: i64 = redis::cmd("GET")
        .arg(s.epoch())
        .query_async::<Option<i64>>(&mut conn)
        .await
        .map_err(rerr)?
        .unwrap_or(now_ms);
    let in_grace = epoch > cutoff;

    let fresh: Vec<i64> = redis::cmd("ZRANGE")
        .arg(s.seen())
        .arg(format!("({cutoff}"))
        .arg("+inf")
        .arg("BYSCORE")
        .query_async(&mut conn)
        .await
        .map_err(rerr)?;

    let on = if fresh.is_empty() {
        0
    } else {
        device_registries::Entity::update_many()
            .col_expr(
                device_registries::Column::Active,
                sea_orm::sea_query::Expr::value(true),
            )
            .filter(device_registries::Column::Active.eq(false))
            .filter(device_registries::Column::Id.is_in(fresh.clone()))
            .exec(db)
            .await
            .map_err(|_| Error::InternalServerError)?
            .rows_affected
    };
    if in_grace {
        return Ok((on, 0));
    }

    // Retire the silent devices: persist, then drop them from the set. The
    // removal is bounded by the same cutoff, so a device touched meanwhile
    // (score raised above it) stays.
    let stale: Vec<(i64, f64)> = redis::cmd("ZRANGE")
        .arg(s.seen())
        .arg("-inf")
        .arg(cutoff)
        .arg("BYSCORE")
        .arg("WITHSCORES")
        .query_async(&mut conn)
        .await
        .map_err(rerr)?;
    if !stale.is_empty() {
        let rows: Vec<(i64, DateTime<Utc>)> = stale
            .iter()
            .filter_map(|(id, ms)| Some((*id, DateTime::from_timestamp_millis(*ms as i64)?)))
            .collect();
        persist_last_seen(db, &rows).await?;
        let _: i64 = redis::cmd("ZREMRANGEBYSCORE")
            .arg(s.seen())
            .arg("-inf")
            .arg(cutoff)
            .query_async(&mut conn)
            .await
            .map_err(rerr)?;
    }

    let off = device_registries::Entity::update_many()
        .col_expr(
            device_registries::Column::Active,
            sea_orm::sea_query::Expr::value(false),
        )
        .filter(device_registries::Column::Active.eq(true))
        .filter(device_registries::Column::Id.is_not_in(fresh))
        .exec(db)
        .await
        .map_err(|_| Error::InternalServerError)?
        .rows_affected;
    Ok((on, off))
}

/// Deletes every liveness key of this instance (test truncation: ids are
/// reset, a lease of a previous test must not lock a new device out), then
/// pins the epoch in the past so the reaper starts outside the restart
/// grace. `KEYS` is acceptable here: test databases only.
pub async fn clear_all() -> Result<()> {
    let Some(s) = STORE.get() else {
        return Ok(());
    };
    let mut conn = s.conn().await?;
    let keys: Vec<String> = redis::cmd("KEYS")
        .arg(format!("{}*", s.ns))
        .query_async(&mut conn)
        .await
        .map_err(|_| Error::InternalServerError)?;
    if !keys.is_empty() {
        let _: i64 = redis::cmd("DEL")
            .arg(keys)
            .query_async(&mut conn)
            .await
            .map_err(|_| Error::InternalServerError)?;
    }
    let _: () = redis::cmd("SET")
        .arg(s.epoch())
        .arg(0)
        .query_async(&mut conn)
        .await
        .map_err(|_| Error::InternalServerError)?;
    Ok(())
}

/// Background reaper task, started at boot (`after_routes`) in every server
/// mode — `loco start` without flag is `ServerOnly` (connect_workers is
/// NOT called). Skipped in tests (`ForegroundBlocking`), where the logic is
/// called directly. Detached: lives as long as the runtime.
pub fn spawn_reaper(ctx: &AppContext) {
    use loco_rs::config::WorkerMode;
    if matches!(ctx.config.workers.mode, WorkerMode::ForegroundBlocking) {
        return;
    }
    let settings = IngestSettings::from_config(&ctx.config);
    let db = ctx.db.clone();
    tokio::spawn(async move {
        let every = std::time::Duration::from_secs(settings.reaper_interval_secs);
        let mut tick = tokio::time::interval(every);
        loop {
            tick.tick().await;
            // One pod reaps (D106).
            if !crate::services::singleton::my_turn(&db, "liveness-reaper", every).await {
                continue;
            }
            match deactivate_stale(&db, settings.silence_ttl_secs).await {
                Ok((on, off)) if on + off > 0 => {
                    tracing::info!(activated = on, deactivated = off, "liveness reaper");
                }
                Ok(_) => {}
                Err(_) => tracing::warn!("liveness reaper failed, retrying at next tick"),
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Exact TTL boundary: fresh strictly before, stale after.
    #[test]
    fn fraicheur_bord_du_ttl() {
        let ttl = 10;
        let now = Utc::now();
        assert!(is_fresh(now - TimeDelta::seconds(ttl - 1), ttl));
        assert!(!is_fresh(now - TimeDelta::seconds(ttl), ttl));
    }

    /// The key namespace follows the database name.
    #[test]
    fn namespace_follows_database_name() {
        assert_eq!(
            namespace("postgres://u:p@h:5432/pnex_test"),
            "pnex:pnex_test:live:"
        );
        assert_eq!(
            namespace("postgres://u:p@h/pnex?sslmode=disable"),
            "pnex:pnex:live:"
        );
    }
}
