//! Placement controller (D106): every flow-enabled process is a candidate,
//! the holder of the `flow-placement` lease acts. Light by construction —
//! it reads the worker registry and the placements (one row per org), and
//! the org weights (one indexed aggregate) every `weights_refresh_ms`.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use sea_orm::DatabaseConnection;

use super::placement::{plan, Change, PlanLimits, WorkerView};
use super::{store, ClusterSettings, PLACEMENT_LEASE};

/// Holders spawned by this process (lease given back at shutdown).
static HOLDERS: LazyLock<Mutex<Vec<(DatabaseConnection, String)>>> =
    LazyLock::new(|| Mutex::new(Vec::new()));

/// Controller state kept across rounds.
#[derive(Default)]
pub struct ControllerState {
    weights: Option<(Instant, HashMap<i64, u64>)>,
    pub leader: bool,
}

pub fn spawn(db: DatabaseConnection, cfg: ClusterSettings, holder: String) {
    HOLDERS
        .lock()
        .expect("holders")
        .push((db.clone(), holder.clone()));
    tokio::spawn(async move {
        let mut state = ControllerState::default();
        loop {
            if let Err(e) = tick(&db, &cfg, &holder, &mut state).await {
                tracing::warn!(error = %e, "flow placement controller round failed");
            }
            tokio::time::sleep(Duration::from_millis(cfg.controller_tick_ms)).await;
        }
    });
}

/// Gives the lease back (graceful shutdown).
pub async fn release_all() {
    let holders = std::mem::take(&mut *HOLDERS.lock().expect("holders"));
    for (db, holder) in holders {
        let _ = store::release_lease(&db, PLACEMENT_LEASE, &holder).await;
    }
}

/// One controller round (public for deterministic tests). Returns the
/// changes applied (`None` = not the leader).
pub async fn tick(
    db: &DatabaseConnection,
    cfg: &ClusterSettings,
    holder: &str,
    state: &mut ControllerState,
) -> Result<Option<Vec<Change>>, sea_orm::DbErr> {
    let leader = store::try_lease(db, PLACEMENT_LEASE, holder, cfg.lease_ttl_ms).await?;
    if leader != state.leader {
        tracing::info!(
            holder,
            leader,
            "flow placement controller leadership changed"
        );
        state.leader = leader;
        // A new leader starts from fresh weights.
        state.weights = None;
    }
    if !leader {
        return Ok(None);
    }
    let workers = store::workers(db).await?;
    let placement_rows = store::placements(db).await?;
    let placements: HashMap<i64, String> = placement_rows
        .iter()
        .map(|p| (p.org_id, p.worker_id.clone()))
        .collect();
    let refresh = state
        .weights
        .as_ref()
        .is_none_or(|(t, _)| t.elapsed() >= Duration::from_millis(cfg.weights_refresh_ms));
    let fresh = if refresh {
        let w = store::org_weights(db).await?;
        state.weights = Some((Instant::now(), w));
        true
    } else {
        false
    };
    let views: Vec<WorkerView> = workers
        .iter()
        .map(|w| WorkerView {
            id: w.id.clone(),
            capacity: w.capacity.max(0) as u32,
            eligible: !w.draining && store::is_alive(w, cfg.worker_ttl_ms),
        })
        .collect();
    let weights = state.weights.as_ref().map(|(_, w)| w);
    let changes = plan(
        &views,
        &placements,
        if fresh { weights } else { None },
        PlanLimits {
            max_moves: cfg.max_moves,
            ..Default::default()
        },
    );

    let updated: HashMap<i64, chrono::DateTime<chrono::FixedOffset>> = placement_rows
        .iter()
        .map(|p| (p.org_id, p.updated_at))
        .collect();
    let mut applied = Vec::new();
    for c in changes {
        let ok = match &c {
            Change::Assign { org_id, worker_id } => {
                store::insert_placement(db, *org_id, worker_id).await?;
                true
            }
            Change::Move { org_id, from, to } => {
                store::move_placement(db, *org_id, from, to).await?
            }
            Change::Release { org_id, worker_id } => {
                // Freeing an idle org is never urgent: only placements
                // untouched for `release_grace_ms` whose org still has no
                // deployed flow (fresh count) are released — a deploy in
                // flight (placement created, database not marked yet) keeps
                // its placement.
                let grace = chrono::Duration::milliseconds(cfg.release_grace_ms as i64);
                let old = updated
                    .get(org_id)
                    .is_some_and(|u| *u < store::now() - grace);
                old && store::org_weight(db, *org_id).await? == 0
                    && store::release_placement(db, *org_id, worker_id).await?
            }
        };
        if ok {
            applied.push(c);
        }
    }
    if !applied.is_empty() {
        tracing::info!(
            changes = applied.len(),
            "flow placements updated: {applied:?}"
        );
    }
    if fresh {
        store::collect_dead_workers(db, cfg.gc_ms, &placements).await?;
    }
    Ok(Some(applied))
}
