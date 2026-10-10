//! Provenance journal (D184): every object and link change lands in the
//! org's O2 `object_changes` stream (before / after, `source_ref`). No
//! audit table in the database; retention is O2's (D72).
//!
//! Writes are best effort and queued (`notify_journal` school): a journal
//! failure never fails the write it describes. Reads never provision O2.

use std::sync::{LazyLock, Mutex};

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

static WRITER: LazyLock<Mutex<Option<mpsc::UnboundedSender<Change>>>> =
    LazyLock::new(|| Mutex::new(None));

/// Queues a change for the background writer.
pub fn submit(change: Change) {
    let tx = WRITER.lock().expect("object changes writer").clone();
    match tx {
        Some(tx) if tx.send(change).is_ok() => {}
        _ => tracing::debug!("object changes writer not running, change dropped"),
    }
}

fn client(ctx: &AppContext) -> Option<Client> {
    OpenobserveSettings::from_config(&ctx.config).map(|s| Client::new(&s))
}

async fn record(ctx: &AppContext, c: &Change) -> std::result::Result<(), String> {
    let Some(client) = client(ctx) else {
        return Err("openobserve is not configured".into());
    };
    let creds = ensure_org_credentials(&ctx.db, &client, c.org_id).await?;
    let ts = chrono::Utc::now().timestamp_micros();
    client
        .ingest_json(
            &creds.o2_org,
            OBJECT_CHANGES_STREAM,
            &[document_of(c, ts)],
            &creds.email_passcode,
        )
        .await
}

/// Background writer, spawned at boot (`after_routes`).
pub fn spawn_writer(ctx: &AppContext) {
    let (tx, mut rx) = mpsc::unbounded_channel::<Change>();
    *WRITER.lock().expect("object changes writer") = Some(tx);
    let ctx = ctx.clone();
    tokio::spawn(async move {
        let mut drain = crate::services::drain::register();
        loop {
            let c = tokio::select! {
                _ = drain.stopped() => break,
                c = rx.recv() => match c {
                    Some(c) => c,
                    None => break,
                },
            };
            if let Err(e) = record(&ctx, &c).await {
                tracing::warn!(action = %c.action, "object change journal write failed: {e}");
            }
        }
        while let Ok(c) = rx.try_recv() {
            let _ = record(&ctx, &c).await;
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
    let Some(client) = client(ctx) else {
        return Ok(None);
    };
    let Some(creds) = provisioned_credentials(&ctx.db, org_id).await? else {
        return Ok(None);
    };
    // The id is a UUID parsed by the controller: safe in the literal.
    let sql = format!(
        "SELECT * FROM \"{OBJECT_CHANGES_STREAM}\" WHERE object_id = '{}' ORDER BY _timestamp DESC",
        object_id.replace('\'', "''")
    );
    let end = chrono::Utc::now().timestamp_micros();
    match client
        .search_logs(
            &creds.o2_org,
            &sql,
            end - WINDOW_US,
            end,
            0,
            limit,
            &creds.email_passcode,
        )
        .await
    {
        Ok(page) => Ok(Some(page.hits.iter().map(change_of).collect())),
        Err(e) if e.contains("not found") => Ok(Some(Vec::new())),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
