//! Acceptance of `pnex-dataset-trigger` in the real runtime against a live Valkey:
//! the events the server publishes reach the flow, filtered by table and event.
//!
//! Gated on `PNEX_TEST_VALKEY_URL` (e.g. `redis://127.0.0.1:6379`, compose valkey,
//! set in CI): skips cleanly when absent.

mod common;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use pnex_core::pipelines::{dataset_channel, DatasetEvent, DATASET_COMMITTED, DATASET_SYNCED};

fn node(id: &str, on: &str, to: &str, org: i64) -> serde_json::Value {
    serde_json::json!({
        "id": id, "type": "pnex-dataset-trigger", "z": "pnexflow1",
        "dataset": "clean", "on": on, "pnex_org_id": org, "wires": [[to]]
    })
}

fn debug(id: &str) -> serde_json::Value {
    serde_json::json!({
        "id": id, "type": "debug", "z": "pnexflow1",
        "active": true, "tosidebar": true, "console": false,
        "complete": "true", "wires": []
    })
}

fn event(kind: &str, dataset: &str, reference: &str) -> String {
    serde_json::to_string(&DatasetEvent {
        event: kind.into(),
        dataset_id: "d1".into(),
        dataset: dataset.into(),
        reference: reference.into(),
        counts: serde_json::json!({ "rows": 3 }),
        changed: vec![],
        ts_ms: 5,
    })
    .unwrap()
}

#[test]
fn events_reach_the_flow_filtered_by_table_and_event() {
    let Ok(valkey) = std::env::var("PNEX_TEST_VALKEY_URL") else {
        eprintln!("skip: PNEX_TEST_VALKEY_URL absent (no compose valkey)");
        return;
    };
    // Other runs and other tests must not hear this org.
    let org = 900_000 + i64::from(std::process::id());
    let flows = serde_json::json!([
        { "id": "pnexflow1", "type": "tab", "label": "tables", "pnex_flow_id": 1 },
        node("committed", "committed", "d_committed", org),
        node("synced", "synced", "d_synced", org),
        debug("d_committed"),
        debug("d_synced"),
    ]);
    let home = common::tmp_home("dataset-trigger");
    let path = home.join("flows.json");
    std::fs::write(&path, flows.to_string()).unwrap();
    let rt = common::RuntimeProc::spawn_with_env(&path, &home, [("VALKEY_URL", valkey.as_str())]);
    rt.wait_for(
        |v| v.get("event").and_then(|e| e.as_str()) == Some("started"),
        Duration::from_secs(30),
    );

    // Nothing says when the subscription is live, and a missed frame is gone by design:
    // publish until it answers.
    let done = Arc::new(AtomicBool::new(false));
    let publisher = {
        let done = done.clone();
        let mut conn = redis::Client::open(valkey.as_str())
            .unwrap()
            .get_connection()
            .unwrap();
        std::thread::spawn(move || {
            while !done.load(Ordering::Relaxed) {
                for frame in [
                    event(DATASET_COMMITTED, "other", "wrong-table"),
                    event(DATASET_COMMITTED, "clean", "t-committed"),
                    event(DATASET_SYNCED, "clean", "t-synced"),
                ] {
                    let _: () = redis::cmd("PUBLISH")
                        .arg(dataset_channel(org))
                        .arg(frame)
                        .query(&mut conn)
                        .unwrap();
                }
                std::thread::sleep(Duration::from_millis(200));
            }
        })
    };
    let from = |node: &'static str| {
        move |v: &serde_json::Value| {
            v.get("event").and_then(|e| e.as_str()) == Some("debug") && v["node_red"] == node
        }
    };
    let committed = rt.wait_for(from("d_committed"), Duration::from_secs(30));
    let synced = rt.wait_for(from("d_synced"), Duration::from_secs(30));
    done.store(true, Ordering::Relaxed);
    publisher.join().unwrap();

    let committed = committed["msg"].as_str().unwrap();
    assert!(committed.contains("t-committed"), "{committed}");
    assert!(committed.contains(DATASET_COMMITTED), "topic: {committed}");
    let synced = synced["msg"].as_str().unwrap();
    assert!(synced.contains("t-synced"), "{synced}");
    assert!(!format!("{committed}{synced}").contains("wrong-table"));
}
