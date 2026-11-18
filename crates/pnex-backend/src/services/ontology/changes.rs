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
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use loco_rs::prelude::*;
use pnex_core::ontology::api::{ChangeView, LinkView, ObjectView};
use pnex_core::ontology::trigger::{
    ontology_channel, OntologyEvent, ON_LINK, ON_OBJECT, ON_PROPOSAL,
};
use serde_json::Value;
use tokio::sync::mpsc;

use crate::services::openobserve::{
    ensure_org_credentials, provisioned_credentials, Client, OpenobserveSettings,
};

pub const OBJECT_CHANGES_STREAM: &str = "object_changes";
/// One row per change of a property with a history (sales-dashboards.md
/// D722): `before` and `after` of the journal are text, not to be grouped.
pub const OBJECT_TRANSITIONS_STREAM: &str = "object_transitions";
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
    pub before: Doc,
    pub after: Doc,
}

/// A change document kept as JSON text: an import holds thousands of them
/// until it commits, and a `Value` tree costs several times its text.
#[derive(Debug, Clone)]
pub struct Doc(Arc<str>);

impl Doc {
    pub fn of<T: serde::Serialize>(v: &T) -> Self {
        Self(
            serde_json::to_string(v)
                .unwrap_or_else(|_| "null".into())
                .into(),
        )
    }
}

impl From<Value> for Doc {
    fn from(v: Value) -> Self {
        Self(v.to_string().into())
    }
}

impl std::fmt::Display for Doc {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
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
            before: before.into(),
            after: Doc::of(after),
        }
    }

    /// The two entries of a link change, one per end.
    pub fn link(org_id: i64, action: &str, source_ref: &str, link: &LinkView) -> [Self; 2] {
        // Both ends share one document.
        let (before, after) = (Doc::from(Value::Null), Doc::of(link));
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
/// The transitions a change holds, for the properties with a history. A value
/// the journal does not show as changed is no transition.
pub fn transitions_of(
    c: &Change,
    defs: &[pnex_core::ontology::schema::PropertyDef],
    ts_us: i64,
) -> Vec<Value> {
    if !matches!(c.action.as_str(), "object.create" | "object.update") {
        return Vec::new();
    }
    let tracked: Vec<_> = defs.iter().filter(|d| d.history.is_some()).collect();
    if tracked.is_empty() {
        return Vec::new();
    }
    let read = |d: &Doc| serde_json::from_str::<Value>(&d.to_string()).unwrap_or_default();
    let (before, after) = (read(&c.before), read(&c.after));
    tracked
        .iter()
        .filter_map(|d| {
            let of = |v: &Value| v["properties"][&d.key].as_str().map(str::to_string);
            let (from, to) = (of(&before), of(&after));
            (from != to).then(|| {
                serde_json::json!({
                    "_timestamp": ts_us,
                    "object_id": c.object_id,
                    "type_key": c.type_key,
                    "property": d.key,
                    "from_value": from.unwrap_or_default(),
                    "to_value": to.unwrap_or_default(),
                })
            })
        })
        .collect()
}

pub fn submit(change: Change) {
    let mut change = Some(change);
    if PENDING
        .try_with(|p| {
            let mut p = p.borrow_mut();
            // The writer's queue takes no more than this at once: a large
            // import drops the rest now instead of holding it to its commit.
            match change.take() {
                Some(c) if p.len() < QUEUE_MAX => p.push(c),
                Some(_) => dropped(),
                None => {}
            }
        })
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
        dropped();
    }
}

fn dropped() {
    let n = DROPPED.fetch_add(1, Ordering::Relaxed) + 1;
    if n == 1 || n.is_multiple_of(10_000) {
        tracing::warn!(dropped = n, "object changes queue full, changes dropped");
    }
}

fn client(ctx: &AppContext) -> Option<Client> {
    OpenobserveSettings::from_config(&ctx.config).map(|s| Client::new(&s))
}

/// The bus event of a change (operational-platform.md D998): `None` for what
/// a flow does not follow (an action invocation).
pub fn event_of(c: &Change, at_us: i64) -> Option<OntologyEvent> {
    let parse = |d: &Doc| serde_json::from_str::<Value>(&d.0).unwrap_or(Value::Null);
    let (kind, rest) = c.action.split_once('.')?;
    let mut ev = OntologyEvent {
        event: String::new(),
        action: c.action.clone(),
        object_id: c.object_id.clone(),
        object_type: c.type_key.clone(),
        changed: Vec::new(),
        link_type: String::new(),
        other_id: String::new(),
        origin: String::new(),
        proposal_id: String::new(),
        source_ref: c.source_ref.clone(),
        ts_ms: at_us / 1000,
    };
    let after = parse(&c.after);
    match kind {
        "object" => {
            ev.event = ON_OBJECT.into();
            let before = parse(&c.before);
            let props = |v: &Value| v.get("properties").and_then(Value::as_object).cloned();
            let (b, a) = (
                props(&before).unwrap_or_default(),
                props(&after).unwrap_or_default(),
            );
            let mut changed: Vec<String> = a
                .iter()
                .filter(|(k, v)| b.get(*k) != Some(*v))
                .map(|(k, _)| k.clone())
                .chain(b.keys().filter(|k| !a.contains_key(*k)).cloned())
                .collect();
            changed.sort();
            ev.changed = changed;
        }
        "link" => {
            ev.event = ON_LINK.into();
            ev.link_type = after["link_type"].as_str().unwrap_or_default().into();
            let (src, dst) = (
                after["source"]["id"].as_str(),
                after["target"]["id"].as_str(),
            );
            let other = if src == Some(c.object_id.as_str()) {
                dst
            } else {
                src
            };
            ev.other_id = other.unwrap_or_default().into();
        }
        "proposal" => {
            ev.event = ON_PROPOSAL.into();
            ev.action = rest.into();
            ev.link_type = after["type_key"].as_str().unwrap_or_default().into();
            ev.other_id = after["other_id"].as_str().unwrap_or_default().into();
            ev.origin = after["origin"].as_str().unwrap_or_default().into();
            ev.proposal_id = after["proposal_id"].as_str().unwrap_or_default().into();
        }
        _ => return None,
    }
    Some(ev)
}

/// Publishes the batch's events on each org's ontology channel, best
/// effort: a flow is told, never relied on (no replay).
async fn publish_events(ctx: &AppContext, batch: &[Queued]) {
    let mut by_org: HashMap<i64, Vec<String>> = HashMap::new();
    for (c, at) in batch {
        if let Some(frame) = event_of(c, *at).and_then(|e| serde_json::to_string(&e).ok()) {
            by_org.entry(c.org_id).or_default().push(frame);
        }
    }
    if by_org.is_empty() {
        return;
    }
    let Some(mut conn) = crate::services::shared_valkey::conn(&ctx.config).await else {
        return;
    };
    for (org_id, frames) in by_org {
        let channel = ontology_channel(org_id);
        let mut pipe = redis::pipe();
        for f in &frames {
            pipe.publish(&channel, f).ignore();
        }
        let op = pipe.query_async::<()>(&mut conn);
        match tokio::time::timeout(Duration::from_secs(5), op).await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => tracing::warn!(org_id, error = %e, "ontology events publish failed"),
            Err(_) => tracing::warn!(org_id, "ontology events publish timed out"),
        }
    }
}

/// Sends a batch, one request per org.
async fn record(ctx: &AppContext, batch: Vec<Queued>) {
    publish_events(ctx, &batch).await;
    let Some(client) = client(ctx) else {
        tracing::debug!(
            "openobserve is not configured, {} change(s) dropped",
            batch.len()
        );
        return;
    };
    let mut by_org: HashMap<i64, Vec<Value>> = HashMap::new();
    let mut transitions: HashMap<i64, Vec<Value>> = HashMap::new();
    let mut schemas: HashMap<i64, Option<super::OrgSchema>> = HashMap::new();
    for (c, at) in &batch {
        by_org
            .entry(c.org_id)
            .or_default()
            .push(document_of(c, *at));
        if let std::collections::hash_map::Entry::Vacant(e) = schemas.entry(c.org_id) {
            e.insert(super::schema(&ctx.db, c.org_id).await.ok());
        }
        let def = schemas
            .get(&c.org_id)
            .and_then(Option::as_ref)
            .and_then(|s| s.object_type(&c.type_key));
        if let Some(def) = def {
            transitions
                .entry(c.org_id)
                .or_default()
                .extend(transitions_of(c, &def.properties, *at));
        }
    }
    for (org_id, docs) in by_org {
        let sent = match ensure_org_credentials(&ctx.db, &client, org_id).await {
            Ok(creds) => {
                let moved = transitions.remove(&org_id).unwrap_or_default();
                if !moved.is_empty() {
                    if let Err(e) = client
                        .ingest_json(
                            &creds.o2_org,
                            OBJECT_TRANSITIONS_STREAM,
                            &moved,
                            &creds.email_passcode,
                        )
                        .await
                    {
                        tracing::warn!(
                            org_id,
                            count = moved.len(),
                            "object transitions write failed: {e}"
                        );
                    }
                }
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
    #[test]
    fn a_transition_is_a_change_of_a_tracked_value_only() {
        use pnex_core::ontology::schema::{PropertyDef, PropertyHistory};
        let mut stage: PropertyDef = serde_json::from_value(serde_json::json!(
            {"key": "stage", "name": "Stage", "kind": "enum", "values": ["a", "b"]}
        ))
        .unwrap();
        stage.history = Some(PropertyHistory::default());
        let of = |action: &str, before: Value, after: Value| {
            let c = Change {
                org_id: 1,
                action: action.into(),
                object_id: "o".into(),
                type_key: "t".into(),
                source_ref: "s".into(),
                before: before.into(),
                after: after.into(),
            };
            transitions_of(&c, std::slice::from_ref(&stage), 5)
        };
        let doc = |v: &str| serde_json::json!({"properties": {"stage": v, "name": "secret"}});
        let moved = of("object.update", doc("a"), doc("b"));
        assert_eq!(moved.len(), 1);
        assert_eq!(moved[0]["from_value"], "a");
        assert_eq!(moved[0]["to_value"], "b");
        assert!(!moved[0].to_string().contains("secret"), "only the value");
        assert!(of("object.update", doc("a"), doc("a")).is_empty());
        assert!(of("object.erase", doc("a"), doc("b")).is_empty());
        // A creation enters its first value.
        let new = of("object.create", Value::Null, doc("a"));
        assert_eq!(
            (new.len(), &new[0]["from_value"]),
            (1, &serde_json::json!(""))
        );
    }

    use super::*;

    fn change(id: &str) -> Change {
        Change {
            org_id: 1,
            action: "object.create".into(),
            object_id: id.into(),
            type_key: "pump".into(),
            source_ref: "manual:7".into(),
            before: Value::Null.into(),
            after: Value::Null.into(),
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
            before: serde_json::json!({"title": "P1"}).into(),
            after: serde_json::json!({"title": "P12"}).into(),
        };
        let v = change_of(&document_of(&c, 1_700_000_000_000_000));
        assert_eq!(v.action, "object.update");
        assert_eq!(v.source_ref, "manual:7");
        assert_eq!(v.after["title"], "P12");
        assert!(v.at.starts_with("2023-11-14"));
    }

    #[test]
    fn events_name_changed_properties_link_ends_and_decisions() {
        let change = |action: &str, before: Value, after: Value| Change {
            org_id: 1,
            action: action.into(),
            object_id: "a".into(),
            type_key: "person".into(),
            source_ref: "manual:1".into(),
            before: before.into(),
            after: after.into(),
        };
        let upd = change(
            "object.update",
            serde_json::json!({"properties": {"city": "Lyon", "name": "Jo", "x": 1}}),
            serde_json::json!({"properties": {"city": "Paris", "name": "Jo", "y": 2}}),
        );
        let ev = event_of(&upd, 5_000).unwrap();
        assert_eq!((ev.event.as_str(), ev.ts_ms), ("object", 5));
        assert_eq!(ev.changed, vec!["city", "x", "y"]);
        let link = change(
            "link.open",
            Value::Null,
            serde_json::json!({"link_type": "employs", "source": {"id": "o"}, "target": {"id": "a"}}),
        );
        let ev = event_of(&link, 0).unwrap();
        assert_eq!(
            (ev.link_type.as_str(), ev.other_id.as_str()),
            ("employs", "o")
        );
        let decided = change(
            "proposal.rejected",
            Value::Null,
            serde_json::json!({"proposal_id": "p", "origin": "pk.m", "type_key": "uses_identifier", "other_id": "i"}),
        );
        let ev = event_of(&decided, 0).unwrap();
        assert_eq!(
            (ev.event.as_str(), ev.action.as_str()),
            ("proposal", "rejected")
        );
        assert_eq!((ev.origin.as_str(), ev.other_id.as_str()), ("pk.m", "i"));
        assert!(event_of(&change("action.invoke", Value::Null, Value::Null), 0).is_none());
    }
}
