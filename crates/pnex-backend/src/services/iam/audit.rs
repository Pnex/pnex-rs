//! Access audit (iam.md D294): every refusal, every access management
//! write and every call made with an access key land in the org's
//! [`IAM_DECISIONS_STREAM`] OpenObserve stream. Plain reads by members are
//! not journaled (volume). Best effort: a journal failure never fails the
//! request; retention is O2's (D72).

use std::sync::{LazyLock, Mutex};

use loco_rs::prelude::*;
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::services::openobserve::{ensure_org_credentials, Client, OpenobserveSettings};

pub const IAM_DECISIONS_STREAM: &str = "iam_decisions";

/// One audited decision.
#[derive(Debug, Clone)]
pub struct Entry {
    pub org_id: i64,
    /// Identity of the caller (a member, or a service account's identity).
    pub user_id: i64,
    pub service_account_id: Option<Uuid>,
    /// operationId, or `tool:<name>` for an assistant tool.
    pub operation: String,
    pub permission: String,
    pub allowed: bool,
    /// HTTP status answered (0 for an assistant tool).
    pub status: u16,
    /// `ui` (a person's token), `access-key` or `assistant`.
    pub origin: &'static str,
}

/// Flat O2 document of an entry.
pub fn document_of(e: &Entry, ts_us: i64) -> serde_json::Value {
    let mut doc = serde_json::json!({
        "_timestamp": ts_us,
        "message": format!(
            "{} {} {}",
            e.operation,
            e.permission,
            if e.allowed { "allowed" } else { "denied" }
        ),
        "user_id": e.user_id,
        "operation": e.operation,
        "permission": e.permission,
        "allowed": e.allowed,
        "status": e.status,
        "origin": e.origin,
    });
    if let Some(sa) = e.service_account_id {
        doc["service_account_id"] = sa.to_string().into();
    }
    doc
}

static WRITER: LazyLock<Mutex<Option<mpsc::UnboundedSender<Entry>>>> =
    LazyLock::new(|| Mutex::new(None));

/// Queues an entry for the background writer.
pub fn submit(entry: Entry) {
    let tx = WRITER.lock().ok().and_then(|w| w.clone());
    match tx {
        Some(tx) if tx.send(entry).is_ok() => {}
        _ => tracing::debug!("iam audit writer not running, entry dropped"),
    }
}

async fn record(ctx: &AppContext, e: &Entry) -> std::result::Result<(), String> {
    let Some(settings) = OpenobserveSettings::from_config(&ctx.config) else {
        return Err("openobserve is not configured".into());
    };
    let client = Client::new(&settings);
    let creds = ensure_org_credentials(&ctx.db, &client, e.org_id).await?;
    let ts = chrono::Utc::now().timestamp_micros();
    client
        .ingest_json(
            &creds.o2_org,
            IAM_DECISIONS_STREAM,
            &[document_of(e, ts)],
            &creds.email_passcode,
        )
        .await
}

/// Background writer, spawned at boot (`after_routes`).
pub fn spawn_writer(ctx: &AppContext) {
    let (tx, mut rx) = mpsc::unbounded_channel::<Entry>();
    if let Ok(mut w) = WRITER.lock() {
        *w = Some(tx);
    }
    let ctx = ctx.clone();
    tokio::spawn(async move {
        let mut drain = crate::services::drain::register();
        loop {
            let e = tokio::select! {
                _ = drain.stopped() => break,
                e = rx.recv() => match e {
                    Some(e) => e,
                    None => break,
                },
            };
            if let Err(err) = record(&ctx, &e).await {
                tracing::warn!(operation = %e.operation, "iam audit write failed: {err}");
            }
        }
        while let Ok(e) = rx.try_recv() {
            let _ = record(&ctx, &e).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn documents_name_the_decision_and_never_a_secret() {
        let e = Entry {
            org_id: 3,
            user_id: 7,
            service_account_id: Some(Uuid::nil()),
            operation: "dashboards_list".into(),
            permission: "dashboards:read".into(),
            allowed: true,
            status: 200,
            origin: "access-key",
        };
        let doc = document_of(&e, 1);
        assert_eq!(doc["message"], "dashboards_list dashboards:read allowed");
        assert_eq!(doc["origin"], "access-key");
        assert!(doc.get("service_account_id").is_some());
        assert!(!doc.to_string().contains("pnex_sk_"));
    }
}
