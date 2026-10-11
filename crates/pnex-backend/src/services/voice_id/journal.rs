//! Usage journal of voice identification (K19: every generation, every
//! comparison) → O2 stream [`VOICE_ID_STREAM`] of the org. Best effort and
//! off the request path; ids and scores only, never a vector or audio.

use loco_rs::app::AppContext;

use crate::services::openobserve::{ensure_org_credentials, Client, OpenobserveSettings};

pub const VOICE_ID_STREAM: &str = "voice_id_usage";

pub fn record(ctx: &AppContext, org_id: i64, mut doc: serde_json::Value) {
    tracing::info!(org_id, entry = %doc, "voice id usage");
    let Some(settings) = OpenobserveSettings::from_config(&ctx.config) else {
        return;
    };
    if let Some(o) = doc.as_object_mut() {
        o.insert(
            "_timestamp".into(),
            chrono::Utc::now().timestamp_micros().into(),
        );
    }
    let db = ctx.db.clone();
    tokio::spawn(async move {
        let client = Client::new(&settings);
        let res = match ensure_org_credentials(&db, &client, org_id).await {
            Ok(creds) => {
                client
                    .ingest_json(
                        &creds.o2_org,
                        VOICE_ID_STREAM,
                        &[doc],
                        &creds.email_passcode,
                    )
                    .await
            }
            Err(e) => Err(e),
        };
        if let Err(e) = res {
            tracing::warn!(org_id, error = %e, "voice id journal not written");
        }
    });
}
