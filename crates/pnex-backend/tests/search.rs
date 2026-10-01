//! Tests for `GET /api/v1/search` (D69) — org-scoped global typeahead:
//! 401 auth, absent/empty q → empty 200 (never 400), org isolation,
//! grouping + per-group cap, prefix/substring ranking + LIKE wildcard
//! escaping, tokenized matching.
//!
//! Requires PostgreSQL (TEST_DATABASE_URL) — DB cleared between tests.

mod common;

use loco_rs::testing::request::{RequestConfig, RequestConfigBuilder};
use pnex_backend::app::App;
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter};
use serial_test::serial;

struct Env {
    alice: String,
    bob: String,
}

async fn with_app<F, Fut>(f: F)
where
    F: FnOnce(axum_test::TestServer, Env, loco_rs::app::AppContext) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let base = common::spawn_mock_rauthy().await;
    let _ = tracing_subscriber::fmt().with_test_writer().try_init();
    unsafe { std::env::set_var("RAUTHY_URL", &base) };
    unsafe { std::env::remove_var("PNEX_NOTIFY_INTERNAL_TOKEN") };
    let config: RequestConfig = RequestConfigBuilder::new().build();
    let env = Env {
        alice: common::valid_token(
            &base,
            "00000000-0000-0000-0000-00000000000a",
            "alice",
            "alice@example.com",
        ),
        bob: common::valid_token(
            &base,
            "00000000-0000-0000-0000-00000000000b",
            "bob",
            "bob@example.com",
        ),
    };
    loco_rs::testing::request::request_with_config::<App, _, _>(
        config,
        move |server, ctx| async move {
            common::seed_catalogue(&ctx.db).await;
            f(server, env, ctx).await;
        },
    )
    .await;
}

fn bearer(token: &str) -> String {
    format!("Bearer {token}")
}

async fn personal_org(server: &axum_test::TestServer, token: &str) -> i64 {
    server
        .get("/api/v1/user-info")
        .add_header("Authorization", bearer(token))
        .await
        .json::<serde_json::Value>()["orgs"][0]["id"]
        .as_i64()
        .expect("personal org")
}

/// GET /api/v1/search — 200 + JSON body.
async fn get_search(
    server: &axum_test::TestServer,
    token: &str,
    org: i64,
    query: &str,
) -> serde_json::Value {
    server
        .get(&format!("/api/v1/search{query}"))
        .add_header("Authorization", bearer(token))
        .add_header("X-Org-Id", org.to_string())
        .await
        .json::<serde_json::Value>()
}

/// Inserts one object of every searchable type for the given org, all
/// named around the "alpha" term (the non-geo pin proves the mode=geo
/// filter).
async fn seed_objects(db: &sea_orm::DatabaseConnection, org: i64) {
    use pnex_backend::models::_entities::*;
    use sea_orm::Set;

    tours::ActiveModel {
        org_id: Set(org),
        name: Set("alpha tour".into()),
        mode: Set("walk".into()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();

    flows::ActiveModel {
        org_id: Set(org),
        name: Set("alpha flow".into()),
        status: Set("draft".into()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();

    map_pins::ActiveModel {
        org_id: Set(org),
        mode: Set("geo".into()),
        label: Set("alpha pin".into()),
        latitude: Set(Some(sea_orm::prelude::Decimal::from(48))),
        longitude: Set(Some(sea_orm::prelude::Decimal::from(2))),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();

    map_pins::ActiveModel {
        org_id: Set(org),
        mode: Set("plan".into()),
        label: Set("alpha shadow pin".into()),
        x: Set(Some(sea_orm::prelude::Decimal::from(10))),
        y: Set(Some(sea_orm::prelude::Decimal::from(20))),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();

    media_assets::ActiveModel {
        org_id: Set(org),
        kind: Set("photo".into()),
        name: Set("alpha photo".into()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();

    annotation_layers::ActiveModel {
        org_id: Set(org),
        name: Set("alpha layer".into()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();

    functions::ActiveModel {
        org_id: Set(org),
        name: Set("alpha func".into()),
        language: Set("javascript".into()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();

    dashboards::ActiveModel {
        org_id: Set(org),
        name: Set("alpha dash".into()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();

    wifi_credentials::ActiveModel {
        org_id: Set(org),
        ssid: Set("alpha ssid".into()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();

    pnex_hosts::ActiveModel {
        org_id: Set(org),
        host: Set("alpha.host".into()),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();
}

// ───────────────────────── Tests ─────────────────────────

#[tokio::test]
#[serial]
async fn search_without_token_rejected() {
    with_app(|server, _env, _ctx| async move {
        let res = server.get("/api/v1/search?q=alpha").await;
        assert_eq!(res.status_code(), 401);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn empty_or_missing_q_returns_empty() {
    with_app(|server, env, _ctx| async move {
        let org = personal_org(&server, &env.alice).await;
        let body = get_search(&server, &env.alice, org, "").await;
        assert_eq!(body["count"], 0);
        assert_eq!(body["q"], "");
        assert_eq!(body["groups"].as_array().unwrap().len(), 0);

        // Blank q only — same silent empty answer (never a 400).
        let body = get_search(&server, &env.alice, org, "?q=%20%20").await;
        assert_eq!(body["count"], 0);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn org_isolation() {
    with_app(|server, env, ctx| async move {
        let alice_org = personal_org(&server, &env.alice).await;
        let bob_org = personal_org(&server, &env.bob).await;
        assert_ne!(alice_org, bob_org);
        seed_objects(&ctx.db, alice_org).await;

        // Alice sees her own objects.
        let body = get_search(&server, &env.alice, alice_org, "?q=alpha").await;
        assert!(body["count"].as_i64().unwrap() >= 9, "body: {body}");

        // Bob searching the same term in his org sees nothing (masked,
        // never a 403).
        let body = get_search(&server, &env.bob, bob_org, "?q=alpha").await;
        assert_eq!(body["count"], 0, "body: {body}");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn canonical_group_order_and_per_group_cap() {
    with_app(|server, env, ctx| async move {
        use pnex_backend::models::_entities::*;
        use sea_orm::Set;

        let org = personal_org(&server, &env.alice).await;
        seed_objects(&ctx.db, org).await;
        // Device (needs a predefined device from the seeded catalogue).
        let soil = predefined_devices::Entity::find()
            .filter(predefined_devices::Column::Name.eq("soil_sensor"))
            .one(&ctx.db)
            .await
            .unwrap()
            .unwrap();
        device_registries::ActiveModel {
            org_id: Set(org),
            device_id: Set("alpha-dev".into()),
            predefined_device_id: Set(soil.id),
            ..Default::default()
        }
        .insert(&ctx.db)
        .await
        .unwrap();
        // Extra flows so the per-group cap actually bites.
        for i in 0..3 {
            flows::ActiveModel {
                org_id: Set(org),
                name: Set(format!("alpha extra {i}")),
                status: Set("draft".into()),
                ..Default::default()
            }
            .insert(&ctx.db)
            .await
            .unwrap();
        }

        let body = get_search(&server, &env.alice, org, "?q=alpha&limit=2").await;
        let groups = body["groups"].as_array().unwrap();
        let types: Vec<&str> = groups
            .iter()
            .map(|g| g["entity_type"].as_str().unwrap())
            .collect();
        assert_eq!(
            types,
            vec![
                "device",
                "poi",
                "tour",
                "media",
                "layer",
                "function",
                "flow",
                "dashboard",
                "edge_ref"
            ],
            "canonical group order, body: {body}"
        );
        for g in groups {
            assert!(
                g["results"].as_array().unwrap().len() <= 2,
                "per-group cap, body: {body}"
            );
        }
        // count = total hits actually returned (≤ 9 groups × 2).
        assert_eq!(
            body["count"],
            groups
                .iter()
                .map(|g| g["results"].as_array().unwrap().len() as i64)
                .sum::<i64>()
        );
    })
    .await;
}

#[tokio::test]
#[serial]
async fn prefix_ranking_and_escaped_wildcards() {
    with_app(|server, env, ctx| async move {
        use pnex_backend::models::_entities::flows;
        use sea_orm::Set;

        let org = personal_org(&server, &env.alice).await;
        // Insertion order matters: SQL fetches recency-first, so the LAST
        // inserted prefix match sorts first within its class.
        for name in ["alpha one", "alpha two", "beta alpha", "100%_done"] {
            flows::ActiveModel {
                org_id: Set(org),
                name: Set(name.into()),
                status: Set("draft".into()),
                ..Default::default()
            }
            .insert(&ctx.db)
            .await
            .unwrap();
        }

        let body = get_search(&server, &env.alice, org, "?q=alpha").await;
        let flow_group = body["groups"]
            .as_array()
            .unwrap()
            .iter()
            .find(|g| g["entity_type"] == "flow")
            .expect("flow group");
        let titles: Vec<&str> = flow_group["results"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["title"].as_str().unwrap())
            .collect();
        // Prefix matches first (recency-first within the class), substring
        // match last.
        assert_eq!(
            titles,
            vec!["alpha two", "alpha one", "beta alpha"],
            "prefix ranks first, body: {body}"
        );

        // A bare `%` is matched LITERALLY (escaped): it finds only the one
        // title holding a real percent sign — unescaped, it would match
        // every row as a wildcard.
        let body = get_search(&server, &env.alice, org, "?q=%25").await;
        assert_eq!(body["count"], 1, "body: {body}");
        assert_eq!(body["groups"][0]["results"][0]["title"], "100%_done");

        // q="100%" matches only the literal name, not everything.
        let body = get_search(&server, &env.alice, org, "?q=100%25").await;
        assert_eq!(body["count"], 1, "body: {body}");
        assert_eq!(
            body["groups"][0]["results"][0]["title"], "100%_done",
            "body: {body}"
        );
    })
    .await;
}

#[tokio::test]
#[serial]
async fn edge_punctuation_tokens_and_multi_word() {
    with_app(|server, env, ctx| async move {
        use pnex_backend::models::_entities::flows;
        use sea_orm::Set;

        let org = personal_org(&server, &env.alice).await;
        for name in ["esp32-relay-1", "esp32-temp", "relay general"] {
            flows::ActiveModel {
                org_id: Set(org),
                name: Set(name.into()),
                status: Set("draft".into()),
                ..Default::default()
            }
            .insert(&ctx.db)
            .await
            .unwrap();
        }

        // Trailing dashes are search noise: "esp---" must find the esp
        // devices (edge punctuation stripped from the token).
        let body = get_search(&server, &env.alice, org, "?q=esp---").await;
        assert_eq!(body["count"], 2, "body: {body}");
        let flow_group = body["groups"]
            .as_array()
            .unwrap()
            .iter()
            .find(|g| g["entity_type"] == "flow")
            .expect("flow group");
        let titles: Vec<&str> = flow_group["results"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["title"].as_str().unwrap())
            .collect();
        assert!(titles.contains(&"esp32-relay-1"), "body: {body}");
        assert!(titles.contains(&"esp32-temp"), "body: {body}");

        // Multi-word: whitespace-separated tokens are ANDed, so
        // "esp32 relay" matches "esp32-relay-1" but not "relay general".
        let body = get_search(&server, &env.alice, org, "?q=esp32%20relay").await;
        assert_eq!(body["count"], 1, "body: {body}");
        assert_eq!(body["groups"][0]["results"][0]["title"], "esp32-relay-1");

        // Internal punctuation stays significant: "relay-1" only matches the
        // id holding that literal sequence.
        let body = get_search(&server, &env.alice, org, "?q=relay-1").await;
        assert_eq!(body["count"], 1, "body: {body}");
    })
    .await;
}
