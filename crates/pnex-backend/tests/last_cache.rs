//! Integration tests for the Valkey last-value tap sink (write side).
//!
//! Gated on `PNEX_TEST_VALKEY_URL`: skip cleanly when the variable is
//! absent (CI without the compose stack). The sink stays exercised through
//! its public API — `TelemetrySink::send` over a recorder inner sink
//! (pattern of `tests/ws_device.rs`).

use std::sync::{Arc, Mutex};

use pnex_backend::services::last_cache::ValkeyTapSink;
use pnex_backend::services::telemetry::{TelemetryPoint, TelemetrySink};

/// Recorder inner sink — asserts the forward happens even for points the
/// cache skips (non-numeric).
#[derive(Default)]
struct RecSink(Mutex<Vec<TelemetryPoint>>);

impl TelemetrySink for RecSink {
    fn send(&self, point: TelemetryPoint) {
        self.0.lock().expect("lock").push(point);
    }
}

fn point(device: &str, value: &str) -> TelemetryPoint {
    TelemetryPoint {
        org_id: 7,
        device_registry_id: 1,
        device_id: device.into(),
        pred_dev: "soil".into(),
        metric_name: "soil_moisture".to_string(),
        value: value.into(),
        timestamp: chrono::Utc::now(),
        ts_source: "server",
        source_type: "test",
        record: true,
    }
}

fn test_valkey_url() -> Option<String> {
    std::env::var("PNEX_TEST_VALKEY_URL")
        .ok()
        .filter(|u| !u.trim().is_empty())
}

#[tokio::test]
async fn numeric_point_is_cached_with_ttl_and_forwarded() {
    let Some(url) = test_valkey_url() else {
        eprintln!("skip: PNEX_TEST_VALKEY_URL absent (no compose valkey)");
        return;
    };
    let client = redis::Client::open(url.as_str()).expect("client");
    let conn = redis::aio::ConnectionManager::new(client.clone())
        .await
        .expect("conn");
    let inner = Arc::new(RecSink::default());
    let sink = ValkeyTapSink::new(conn, inner.clone());

    sink.send(point("cap-tap-test", "42.5"));

    // The write is fire-and-forget (spawned): poll briefly for convergence.
    let key = pnex_core::last_cache_key(7, "cap-tap-test", "soil_moisture");
    let mut read_conn = redis::aio::ConnectionManager::new(client)
        .await
        .expect("read conn");
    let mut raw: Option<String> = None;
    for _ in 0..50 {
        raw = redis::AsyncCommands::get::<_, Option<String>>(&mut read_conn, &key)
            .await
            .expect("get");
        if raw.is_some() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    let raw = raw.expect("cached value must appear");
    let sample: pnex_core::CachedSample = serde_json::from_str(&raw).expect("json payload");
    assert_eq!(sample.v, 42.5);
    assert!(sample.ts_ms > 0);

    let ttl: i64 = redis::cmd("TTL")
        .arg(&key)
        .query_async(&mut read_conn)
        .await
        .expect("ttl");
    assert!(ttl > 0, "entry carries the GC TTL");

    let fwd = inner.0.lock().expect("lock");
    assert_eq!(fwd.len(), 1, "point forwarded to the inner sink");
    assert_eq!(fwd[0].value, "42.5");
}

#[tokio::test]
async fn non_numeric_point_skips_cache_but_still_forwards() {
    let Some(url) = test_valkey_url() else {
        eprintln!("skip: PNEX_TEST_VALKEY_URL absent (no compose valkey)");
        return;
    };
    // Distinct device: the numeric test above caches the same metric under
    // "cap-tap-test" (TTL 3900 s) — never share keys across cases.
    let device = "cap-tap-test-na";
    let client = redis::Client::open(url.as_str()).expect("client");
    let conn = redis::aio::ConnectionManager::new(client.clone())
        .await
        .expect("conn");
    let inner = Arc::new(RecSink::default());
    let sink = ValkeyTapSink::new(conn, inner.clone());

    sink.send(point(device, "n/a"));

    {
        // Scoped guard: the recorder lock must never span an await point.
        let fwd = inner.0.lock().expect("lock");
        assert_eq!(fwd.len(), 1, "inner sink still receives the point");
        assert_eq!(fwd[0].value, "n/a");
    }

    // Give a (buggy) spawned write a chance to show up, then assert absence.
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    let mut read_conn = redis::aio::ConnectionManager::new(client)
        .await
        .expect("read conn");
    let key = pnex_core::last_cache_key(7, device, "soil_moisture");
    let raw: Option<String> = redis::AsyncCommands::get(&mut read_conn, &key)
        .await
        .expect("get");
    assert!(raw.is_none(), "non-numeric value must never be cached");
}

/// GPIO last values (pins UI, cross-pod): one hash per device with a TTL,
/// later writes win per gpio, cleared at session close.
#[tokio::test]
async fn gpio_last_values_roundtrip_and_clear() {
    use pnex_backend::services::last_cache::{
        apply_gpio_op, gpio_key, gpio_values_with, GpioOp, GPIO_TTL_SECS,
    };
    let Some(url) = test_valkey_url() else {
        eprintln!("skip: PNEX_TEST_VALKEY_URL absent (no compose valkey)");
        return;
    };
    let client = redis::Client::open(url.as_str()).expect("client");
    let mut conn = redis::aio::ConnectionManager::new(client)
        .await
        .expect("conn");
    let device = 880_000_000 + i64::from(std::process::id() % 1000);
    let put = |gpio: i32, value: serde_json::Value| GpioOp::Put {
        device_registry_id: device,
        gpio,
        value: value.to_string(),
    };
    apply_gpio_op(&mut conn, put(4, serde_json::json!(true)))
        .await
        .expect("put");
    apply_gpio_op(&mut conn, put(5, serde_json::json!(512)))
        .await
        .expect("put");
    apply_gpio_op(&mut conn, put(4, serde_json::json!(false)))
        .await
        .expect("put");

    let values = gpio_values_with(&mut conn, device).await.expect("read");
    assert_eq!(values.len(), 2);
    assert_eq!(values[&4], serde_json::json!(false), "latest write wins");
    assert_eq!(values[&5], serde_json::json!(512));
    let ttl: i64 = redis::cmd("TTL")
        .arg(gpio_key(device))
        .query_async(&mut conn)
        .await
        .expect("ttl");
    assert!(ttl > 0 && ttl <= GPIO_TTL_SECS, "ttl {ttl}");

    apply_gpio_op(
        &mut conn,
        GpioOp::Clear {
            device_registry_id: device,
        },
    )
    .await
    .expect("clear");
    assert!(gpio_values_with(&mut conn, device)
        .await
        .expect("read")
        .is_empty());
}
