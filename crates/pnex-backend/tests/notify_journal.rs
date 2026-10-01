//! Notification journal (D86): live round trip against a local O2 — the
//! documents, the journal search and the "last status" aggregate the API
//! uses. `--ignored` (dev compose, localhost:5080).

use pnex_backend::services::notify_journal::{
    delivery_of, document_of, last_status_sql, search_sql, DeliveryQuery,
};
use pnex_backend::services::openobserve::{Client, OpenobserveSettings};
use pnex_core::{NotifyDeliveryEntry, NOTIFY_DELIVERY_STREAM};
use uuid::Uuid;

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.into())
}

#[tokio::test]
#[ignore = "requires a live OpenObserve (dev compose, localhost:5080)"]
async fn journal_roundtrip_live_openobserve() {
    let email = env_or("OPENOBSERVE_ROOT_EMAIL", "root@pnex.local");
    let password = env_or("OPENOBSERVE_ROOT_PASSWORD", "Pnex-dev-2026!");
    let client = Client::new(&OpenobserveSettings {
        base_url: env_or("OPENOBSERVE_URL", "http://localhost:5080"),
        root_email: email.clone(),
        root_password: password.clone(),
    });
    let root = format!("{email}:{password}");
    // Unique channels: the stream is shared with previous runs.
    let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
    let entry = |channel_id, status: &str, http_status| NotifyDeliveryEntry {
        org_id: 0,
        channel_id,
        channel_kind: "webhook".into(),
        source: "flow".into(),
        status: status.into(),
        http_status,
        subject: Some("door open".into()),
        flow_id: Some(12),
        node_id: Some("n7".into()),
        ..Default::default()
    };
    let now = chrono::Utc::now().timestamp_micros();
    let docs = [
        document_of(&entry(a, "sent", None), now - 3_000_000),
        document_of(&entry(a, "failed", Some(502)), now - 2_000_000),
        document_of(&entry(b, "sent", None), now - 1_000_000),
    ];
    client
        .ingest_json("default", NOTIFY_DELIVERY_STREAM, &docs, &root)
        .await
        .expect("ingest");

    let (start, end) = (now - 60_000_000, now + 60_000_000);
    let q = DeliveryQuery {
        channel_id: Some(a),
        status: Some("failed".into()),
        ..Default::default()
    };
    let mut failed = None;
    for _ in 0..30 {
        let r = client
            .search_logs("default", &search_sql(&q), start, end, 0, 10, &root)
            .await
            .expect("search");
        if let Some(hit) = r.hits.first() {
            failed = Some(delivery_of(hit));
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    let failed = failed.expect("journal entry searchable");
    assert_eq!(failed.channel_id, a);
    assert_eq!(failed.http_status, Some(502));
    assert_eq!(failed.flow_id, Some(12));
    assert_eq!(failed.subject.as_deref(), Some("door open"));

    let r = client
        .search_logs(
            "default",
            &last_status_sql(&[a, b]),
            start,
            end,
            0,
            10,
            &root,
        )
        .await
        .expect("last status");
    let status_of = |id: Uuid| {
        r.hits
            .iter()
            .find(|h| h["channel_id"] == id.to_string())
            .and_then(|h| h["status"].as_str().map(str::to_string))
    };
    assert_eq!(status_of(a).as_deref(), Some("failed"), "{:?}", r.hits);
    assert_eq!(status_of(b).as_deref(), Some("sent"), "{:?}", r.hits);
}
