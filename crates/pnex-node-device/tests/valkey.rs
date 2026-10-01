//! Integration tests for the Valkey last-value cache reader.
//!
//! Tests touching `VALKEY_URL` are process-global: they are serialized with
//! `#[serial]`. The roundtrip cases need a real Valkey — they are gated on
//! `PNEX_TEST_VALKEY_URL` and skip cleanly when the variable is absent
//! (CI without the compose stack).

use serial_test::serial;

#[test]
#[serial]
fn valkey_url_absent_is_feature_off() {
    std::env::remove_var("VALKEY_URL");
    let client = pnex_node_device::valkey::ValkeyLastClient::from_env_opt();
    assert!(
        client.unwrap().is_none(),
        "VALKEY_URL absent = legacy O2 path (feature off)"
    );
}

#[test]
#[serial]
fn valkey_url_invalid_fails_loud_at_build() {
    std::env::set_var("VALKEY_URL", "not-a-url");
    let client = pnex_node_device::valkey::ValkeyLastClient::from_env_opt();
    assert!(
        client.is_err(),
        "set-but-invalid URL = node rejected at build"
    );
    std::env::remove_var("VALKEY_URL");
}

/// Connects to the test Valkey and returns (client, org, device). Skips the
/// caller when `PNEX_TEST_VALKEY_URL` is unset.
fn test_valkey_url() -> Option<String> {
    std::env::var("PNEX_TEST_VALKEY_URL")
        .ok()
        .filter(|u| !u.trim().is_empty())
}

#[tokio::test]
#[serial]
async fn mget_returns_samples_in_pin_order_with_misses() {
    let Some(url) = test_valkey_url() else {
        eprintln!("skip: PNEX_TEST_VALKEY_URL absent (no compose valkey)");
        return;
    };
    std::env::set_var("VALKEY_URL", &url);

    let writer = redis::Client::open(url.as_str()).expect("writer client");
    let mut conn = redis::aio::ConnectionManager::new(writer)
        .await
        .expect("writer conn");

    let org: i64 = 42;
    let device = "cap-read-test";
    let pin_fresh = "A0";
    let pin_stale = "A1";
    let pin_miss = "A2";

    let now_ms = chrono::Utc::now().timestamp_millis();
    let fresh = pnex_core::CachedSample {
        v: 1.5,
        ts_ms: now_ms,
    };
    let stale = pnex_core::CachedSample {
        v: 9.9,
        ts_ms: now_ms - 60_000,
    };
    for (pin, sample) in [(pin_fresh, fresh), (pin_stale, stale)] {
        let key =
            pnex_core::last_cache_key(org, device, &pnex_core::normalize_measurement_name(pin));
        redis::AsyncCommands::set_ex::<_, _, ()>(
            &mut conn,
            key,
            serde_json::to_string(&sample).unwrap(),
            300_u64,
        )
        .await
        .expect("seed");
    }
    // Hygiene: drop the miss pin in case an old run left something there.
    let key_miss = pnex_core::last_cache_key(
        org,
        device,
        &pnex_core::normalize_measurement_name(pin_miss),
    );
    let _: () = redis::AsyncCommands::del(&mut conn, key_miss)
        .await
        .expect("del");

    let reader = pnex_node_device::valkey::ValkeyLastClient::from_env_opt()
        .expect("client")
        .expect("some client");
    let samples = reader
        .mget_last(
            org,
            device,
            &[pin_fresh.into(), pin_stale.into(), pin_miss.into()],
        )
        .await
        .expect("mget");

    assert_eq!(samples.len(), 3);
    // Fresh sample comes back; resolve() keeps it inside a 2 s window.
    assert_eq!(
        pnex_core::last_cache::resolve(samples[0], now_ms, 2.0),
        Some(1.5),
        "fresh pin resolves within the window"
    );
    // Stale sample is returned by the cache but rejected by the resolver.
    assert_eq!(samples[1], Some(stale));
    assert_eq!(
        pnex_core::last_cache::resolve(samples[1], now_ms, 2.0),
        None,
        "stale pin fails the freshness window"
    );
    // Missing key = None (a miss, never a fabricated value).
    assert_eq!(samples[2], None, "absent pin is a clean miss");

    std::env::remove_var("VALKEY_URL");
}
