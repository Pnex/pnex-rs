//! Org shared memory read side (dashboards): key listing + numeric value
//! resolution over entries written in the `memory-write` node format.
//!
//! Gated on `PNEX_TEST_VALKEY_URL`: skip cleanly when absent.

use pnex_backend::services::memory::{list_keys_with, values_with};
use pnex_core::memory::{memory_cache_key, MemoryEntry, MemoryRef};

fn test_valkey_url() -> Option<String> {
    std::env::var("PNEX_TEST_VALKEY_URL")
        .ok()
        .filter(|u| !u.trim().is_empty())
}

#[tokio::test]
async fn lists_keys_and_resolves_numeric_fields() {
    let Some(url) = test_valkey_url() else {
        eprintln!("skip: PNEX_TEST_VALKEY_URL absent (no compose valkey)");
        return;
    };
    let client = redis::Client::open(url.as_str()).expect("client");
    let mut conn = redis::aio::ConnectionManager::new(client)
        .await
        .expect("conn");
    // Org id unique per run: the listing must only see this test's keys.
    let org = 900_000_000 + i64::from(std::process::id());
    let entry = MemoryEntry {
        v: serde_json::json!({"Hmass": 412.5, "phase": "gas"}),
        ts_ms: 1_789_000_000_000,
    };
    let raw = serde_json::to_string(&entry).unwrap();
    let _: () = redis::AsyncCommands::set_ex(&mut conn, memory_cache_key(org, "cycle.p1"), raw, 60)
        .await
        .unwrap();
    let raw = serde_json::to_string(&MemoryEntry {
        v: serde_json::json!(3.5),
        ts_ms: 1,
    })
    .unwrap();
    let _: () = redis::AsyncCommands::set_ex(&mut conn, memory_cache_key(org, "a.scalar"), raw, 60)
        .await
        .unwrap();

    let keys = list_keys_with(conn.clone(), org).await;
    let names: Vec<&str> = keys.iter().map(|k| k.key.as_str()).collect();
    assert_eq!(names, ["a.scalar", "cycle.p1"]);
    assert_eq!(keys[0].fields, [""]);
    assert_eq!(keys[1].fields, ["Hmass"]);

    let r = |key: &str, field: &str| MemoryRef {
        key: key.into(),
        field: field.into(),
    };
    let res = values_with(
        Some(conn),
        org,
        &[
            r("cycle.p1", "Hmass"),
            r("cycle.p1", "phase"),
            r("a.scalar", ""),
            r("missing", ""),
        ],
    )
    .await;
    assert!(res.available);
    let got: Vec<Option<f64>> = res.results.iter().map(|v| v.value).collect();
    assert_eq!(got, [Some(412.5), None, Some(3.5), None]);
    assert_eq!(res.results[0].ts_ms, Some(1_789_000_000_000));
    assert!(!res.results[1].available);

    let off = values_with(None, org, &[r("cycle.p1", "")]).await;
    assert!(!off.available && !off.results[0].available);
}
