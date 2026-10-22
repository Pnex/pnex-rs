//! Provenance journal (D184): every object and link change lands in the
//! org's O2 `object_changes` stream (before / after, `source_ref`). No
//! audit table in the database; retention is O2's (D72).
//!
//! Writes are best effort and queued (`notify_journal` school): a journal
//! failure never fails the write it describes. Reads never provision O2.
//! A transaction wraps its work in [`deferred`] so its changes are queued
//! only once it committed (data-pipelines.md D350).

use std::cell::RefCell;
use std::collections::HashMap;
use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use loco_rs::prelude::*;
use pnex_core::ontology::api::{ChangeView, LinkView, ObjectView};
use serde_json::Value;
use tokio::sync::mpsc;

use crate::services::openobserve::{
    ensure_org_credentials, provisioned_credentials, Client, OpenobserveSettings,
};

pub const OBJECT_CHANGES_STREAM: &str = "object_changes";
const WINDOW_US: i64 = 365 * 24 * 3600 * 1_000_000;

/// One change. `object_id` is the object, or each end for a link change
/// (a link change is journaled once per end so both histories show it).
#[derive(Debug, Clone)]
pub struct Change {
    pub org_id: i64,
    pub action: String,
    pub object_id: String,
    pub type_key: String,
    pub source_ref: String,
    pub before: Value,
    pub after: Value,
}

impl Change {
    pub fn object(
        org_id: i64,
        action: &str,
        source_ref: &str,
        before: Value,
        after: &ObjectView,
    ) -> Self {
        Self {
            org_id,
            action: action.into(),
            object_id: after.id.clone(),
            type_key: after.type_key.clone(),
            source_ref: source_ref.into(),
            before,
            after: serde_json::to_value(after).unwrap_or_default(),
        }
    }

    /// The two entries of a link change, one per end.
    pub fn link(org_id: i64, action: &str, source_ref: &str, link: &LinkView) -> [Self; 2] {
        let doc = serde_json::to_value(link).unwrap_or_default();
        let (before, after) = if action == "link.close" {
            (Value::Null, doc)
        } else {
            (Value::Null, doc)
        };
        [&link.source, &link.target].map(|end| Self {
            org_id,
            action: action.into(),
            object_id: end.id.clone(),
            type_key: end.type_key.clone(),
            source_ref: source_ref.into(),
            before: before.clone(),
            after: after.clone(),
        })
    }
}

pub fn document_of(c: &Change, ts_us: i64) -> Value {
    serde_json::json!({
        "_timestamp": ts_us,
        "action": c.action,
        "object_id": c.object_id,
        "type_key": c.type_key,
        "source_ref": c.source_ref,
        // Stored as text: O2 flattens nested objects into columns.
        "before": c.before.to_string(),
        "after": c.after.to_string(),
    })
}

pub fn change_of(hit: &Value) -> ChangeView {
    let s = |k: &str| {
        hit.get(k)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    let doc = |k: &str| {
        hit.get(k)
            .and_then(Value::as_str)
            .and_then(|t| serde_json::from_str(t).ok())
            .unwrap_or(Value::Null)
    };
    let at = hit
        .get("_timestamp")
        .and_then(Value::as_i64)
        .and_then(chrono::DateTime::from_timestamp_micros)
        .map(|t| t.to_rfc3339())
        .unwrap_or_default();
    ChangeView {
        at,
        action: s("action"),
        source_ref: s("source_ref"),
        before: doc("before"),
        after: doc("after"),
    }
}

/// Queue bound: past it changes are dropped (counted, logged), never
/// blocking the write they describe.
const QUEUE_MAX: usize = 100_000;
/// One O2 request carries up to this many documents...
const BATCH_MAX: usize = 1_000;
/// ...or what arrived within this window.
const BATCH_WINDOW: Duration = Duration::from_secs(1);

/// A change with the instant it was queued (µs), i.e. after its commit.
type Queued = (Change, i64);

static WRITER: LazyLock<Mutex<Option<mpsc::Sender<Queued>>>> = LazyLock::new(|| Mutex::new(None));
static DROPPED: AtomicU64 = AtomicU64::new(0);

tokio::task_local! {
    static PENDING: RefCell<Vec<Change>>;
}

/// Runs `work` (which opens and commits a transaction) holding back the
/// changes it submits: queued when it returns `Ok`, discarded on `Err`
/// (rolled back, nothing to journal). Nests: an inner scope hands its
/// changes to the outer one.
pub async fn deferred<T, E, F>(work: F) -> std::result::Result<T, E>
where
    F: Future<Output = std::result::Result<T, E>>,
{
    let (out, held) = PENDING
        .scope(RefCell::new(Vec::new()), async {
            let out = work.await;
            (out, PENDING.with(|p| p.take()))
        })
        .await;
    if out.is_ok() {
        held.into_iter().for_each(submit);
    }
    out
}

/// Queues a change for the background writer, or holds it until the
/// enclosing [`deferred`] scope commits.
pub fn submit(change: Change) {
    let mut change = Some(change);
    if PENDING
        .try_with(|p| p.borrow_mut().extend(change.take()))
        .is_ok()
    {
        return;
    }
    if let Some(change) = change {
        queue(change);
    }
}

/// Queues what the enclosing [`deferred`] scope holds, right after its
/// transaction committed: later errors of the same call (a read after the
/// commit) no longer discard it.
pub fn flush() {
    let held = PENDING.try_with(|p| p.take()).unwrap_or_default();
    held.into_iter().for_each(queue);
}

fn queue(change: Change) {
    let tx = WRITER.lock().expect("object changes writer").clone();
    let Some(tx) = tx else {
        tracing::debug!("object changes writer not running, change dropped");
        return;
    };
    let at = chrono::Utc::now().timestamp_micros();
    if tx.try_send((change, at)).is_err() {
        let n = DROPPED.fetch_add(1, Ordering::Relaxed) + 1;
        if n == 1 || n % 10_000 == 0 {
            tracing::warn!(dropped = n, "object changes queue full, changes dropped");
        }
    }
}

fn client(ctx: &AppContext) -> Option<Client> {
    OpenobserveSettings::from_config(&ctx.config).map(|s| Client::new(&s))
}

/// Sends a batch, one request per org.
async fn record(ctx: &AppContext, batch: Vec<Queued>) {
    let Some(client) = client(ctx) else {
        tracing::debug!(
            "openobserve is not configured, {} change(s) dropped",
            batch.len()
        );
        return;
    };
    let mut by_org: HashMap<i64, Vec<Value>> = HashMap::new();
    for (c, at) in &batch {
        by_org
            .entry(c.org_id)
            .or_default()
            .push(document_of(c, *at));
    }
    for (org_id, docs) in by_org {
        let sent = match ensure_org_credentials(&ctx.db, &client, org_id).await {
            Ok(creds) => {
                client
                    .ingest_json(
                        &creds.o2_org,
                        OBJECT_CHANGES_STREAM,
                        &docs,
                        &creds.email_passcode,
                    )
                    .await
            }
            Err(e) => Err(e),
        };
        if let Err(e) = sent {
            tracing::warn!(
                org_id,
                count = docs.len(),
                "object change journal write failed: {e}"
            );
        }
    }
}

/// Takes what is queued after `first`, up to `BATCH_MAX` or `BATCH_WINDOW`.
async fn batch_after(first: Queued, rx: &mut mpsc::Receiver<Queued>) -> Vec<Queued> {
    let mut batch = vec![first];
    let until = tokio::time::Instant::now() + BATCH_WINDOW;
    while batch.len() < BATCH_MAX {
        match tokio::time::timeout_at(until, rx.recv()).await {
            Ok(Some(q)) => batch.push(q),
            _ => break,
        }
    }
    batch
}

/// Background writer, spawned at boot (`after_routes`).
pub fn spawn_writer(ctx: &AppContext) {
    let (tx, mut rx) = mpsc::channel::<Queued>(QUEUE_MAX);
    *WRITER.lock().expect("object changes writer") = Some(tx);
    let ctx = ctx.clone();
    tokio::spawn(async move {
        let mut drain = crate::services::drain::register();
        loop {
            let first = tokio::select! {
                _ = drain.stopped() => break,
                q = rx.recv() => match q {
                    Some(q) => q,
                    None => break,
                },
            };
            let batch = batch_after(first, &mut rx).await;
            record(&ctx, batch).await;
        }
        let mut rest = Vec::new();
        while let Ok(q) = rx.try_recv() {
            rest.push(q);
        }
        for chunk in rest.chunks(BATCH_MAX) {
            record(&ctx, chunk.to_vec()).await;
        }
    });
}

/// History of an object, newest first. `None` = O2 absent or org never
/// provisioned (the UI says "no history").
pub async fn history(
    ctx: &AppContext,
    org_id: i64,
    object_id: &str,
    limit: i64,
) -> std::result::Result<Option<Vec<ChangeView>>, String> {
    let end = chrono::Utc::now().timestamp_micros();
    Ok(history_of(
        &ctx.db,
        client(ctx).as_ref(),
        org_id,
        &[object_id.to_string()],
        end - WINDOW_US,
        end,
        limit,
    )
    .await?
    .map(|rows| rows.into_iter().map(|(_, c)| c).collect()))
}

/// Changes of several objects between two instants (µs), newest first,
/// each with its object id. `None` = journal unavailable.
pub async fn history_of(
    db: &sea_orm::DatabaseConnection,
    client: Option<&Client>,
    org_id: i64,
    object_ids: &[String],
    start_us: i64,
    end_us: i64,
    limit: i64,
) -> std::result::Result<Option<Vec<(String, ChangeView)>>, String> {
    let Some(client) = client else {
        return Ok(None);
    };
    let Some(creds) = provisioned_credentials(db, org_id).await? else {
        return Ok(None);
    };
    if object_ids.is_empty() {
        return Ok(Some(Vec::new()));
    }
    // Ids are UUIDs parsed by the callers; quotes escaped all the same.
    let ids: Vec<String> = object_ids
        .iter()
        .map(|id| format!("'{}'", id.replace('\'', "''")))
        .collect();
    let sql = format!(
        "SELECT * FROM \"{OBJECT_CHANGES_STREAM}\" WHERE object_id IN ({}) ORDER BY _timestamp DESC",
        ids.join(",")
    );
    match client
        .search_logs(
            &creds.o2_org,
            &sql,
            start_us,
            end_us,
            0,
            limit,
            &creds.email_passcode,
        )
        .await
    {
        Ok(page) => Ok(Some(
            page.hits
                .iter()
                .map(|h| {
                    let id = h
                        .get("object_id")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    (id, change_of(h))
                })
                .collect(),
        )),
        Err(e) if e.contains("not found") => Ok(Some(Vec::new())),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn change(id: &str) -> Change {
        Change {
            org_id: 1,
            action: "object.create".into(),
            object_id: id.into(),
            type_key: "pump".into(),
            source_ref: "manual:7".into(),
            before: Value::Null,
            after: Value::Null,
        }
    }

    fn held() -> Vec<String> {
        PENDING
            .try_with(|p| p.borrow().iter().map(|c| c.object_id.clone()).collect())
            .unwrap_or_default()
    }

    #[tokio::test]
    async fn deferred_keeps_only_committed_changes() {
        // Outer scope observes what inner scopes hand over.
        PENDING
            .scope(RefCell::new(Vec::new()), async {
                let _: std::result::Result<(), ()> = deferred(async {
                    submit(change("rolled-back"));
                    Err(())
                })
                .await;
                assert!(held().is_empty());
                let _: std::result::Result<(), ()> = deferred(async {
                    submit(change("committed"));
                    assert_eq!(held(), vec!["committed".to_string()]);
                    Ok(())
                })
                .await;
                assert_eq!(held(), vec!["committed".to_string()]);
            })
            .await;
    }

    #[tokio::test]
    async fn batch_stops_at_window() {
        let (tx, mut rx) = mpsc::channel::<Queued>(8);
        tx.send((change("b"), 2)).await.unwrap();
        let batch = batch_after((change("a"), 1), &mut rx).await;
        let ids: Vec<_> = batch.iter().map(|(c, _)| c.object_id.as_str()).collect();
        assert_eq!(ids, ["a", "b"]);
    }

    #[test]
    fn document_round_trip() {
        let c = Change {
            org_id: 1,
            action: "object.update".into(),
            object_id: "o1".into(),
            type_key: "pump".into(),
            source_ref: "manual:7".into(),
            before: serde_json::json!({"title": "P1"}),
            after: serde_json::json!({"title": "P12"}),
        };
        let v = change_of(&document_of(&c, 1_700_000_000_000_000));
        assert_eq!(v.action, "object.update");
        assert_eq!(v.source_ref, "manual:7");
        assert_eq!(v.after["title"], "P12");
        assert!(v.at.starts_with("2023-11-14"));
    }
}
