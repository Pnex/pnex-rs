//! Devices domain tests (Phase 4): org-scoped CRUD, filters, implicit
//! reactivation, tier quotas, metadata-only update, shared global
//! catalog.
//!
//! Requires PostgreSQL (TEST_DATABASE_URL) — database emptied between tests.

mod common;

use loco_rs::testing::request::{RequestConfig, RequestConfigBuilder};
use pnex_backend::app::App;
use serial_test::serial;

struct Env {
    alice: String,
    bob: String,
}

/// Boots the app + seeds the minimal catalogue (Free tier 3/1/0, types sensor/
/// actuator/mixed, capabilities, board, predefined devices).
async fn with_app<F, Fut>(f: F)
where
    F: FnOnce(axum_test::TestServer, Env, loco_rs::app::AppContext) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let base = common::spawn_mock_rauthy().await;
    let _ = tracing_subscriber::fmt().with_test_writer().try_init();
    unsafe { std::env::set_var("RAUTHY_URL", &base) };
    // Tier quotas only apply to a SaaS deployment (O37).
    unsafe { std::env::set_var("PNEX_DEPLOYMENT_MODE", "saas") };
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

/// Personal org of the user (created by JIT provisioning).
async fn personal_org(server: &axum_test::TestServer, token: &str) -> i64 {
    server
        .get("/api/v1/user-info")
        .add_header("Authorization", bearer(token))
        .await
        .json::<serde_json::Value>()["orgs"][0]["id"]
        .as_i64()
        .expect("personal org")
}

async fn create_device(
    server: &axum_test::TestServer,
    token: &str,
    org_id: i64,
    device_id: &str,
    predefined: &str,
) -> axum_test::TestResponse {
    server
        .post("/api/v1/devices")
        .add_header("Authorization", bearer(token))
        .add_header("X-Org-Id", org_id.to_string())
        .add_header("Content-Type", "application/json")
        .json(&serde_json::json!({
            "device_id": device_id,
            "predefined_device_name": predefined,
        }))
        .await
}

async fn list_devices(
    server: &axum_test::TestServer,
    token: &str,
    org_id: i64,
    query: &str,
) -> serde_json::Value {
    server
        .get(&format!("/api/v1/devices{query}"))
        .add_header("Authorization", bearer(token))
        .add_header("X-Org-Id", org_id.to_string())
        .await
        .json()
}

async fn patch_device(
    server: &axum_test::TestServer,
    token: &str,
    org_id: i64,
    id: i64,
    body: serde_json::Value,
) -> axum_test::TestResponse {
    server
        .patch(&format!("/api/v1/devices/{id}"))
        .add_header("Authorization", bearer(token))
        .add_header("X-Org-Id", org_id.to_string())
        .add_header("Content-Type", "application/json")
        .json(&body)
        .await
}

#[tokio::test]
#[serial]
async fn without_token_devices_and_catalog_refused() {
    with_app(|server, _env, _ctx| async move {
        for (method, path) in [
            ("GET", "/api/v1/devices"),
            ("POST", "/api/v1/devices"),
            ("GET", "/api/v1/device-capabilities"),
            ("GET", "/api/v1/predefined-devices"),
        ] {
            let res = match method {
                "GET" => server.get(path).await,
                _ => server.post(path).await,
            };
            assert_eq!(res.status_code(), 401, "{method} {path} without a token");
        }
    })
    .await;
}

#[tokio::test]
#[serial]
async fn create_reactivate_cycle_and_active_device_refusal() {
    with_app(|server, env, ctx| async move {
        let org = personal_org(&server, &env.alice).await;

        // Creation: inactive + token + encryption key (base64 44 chars).
        let res = create_device(&server, &env.alice, org, "esp-001", "temp_sensor").await;
        assert_eq!(res.status_code(), 201, "creation → 201");
        let body: serde_json::Value = res.json();
        let device_pk = body["id"].as_i64().expect("id");
        assert_eq!(body["active"], false, "created inactive");
        assert_eq!(body["org_id"], org);
        assert_eq!(body["device_type"], "sensor");
        assert_eq!(body["predefined_device_name"], "temp_sensor");
        assert_eq!(body["allow_dynamic_measurements"], false);
        assert_eq!(body["discovered_measurements"], serde_json::json!([]));
        let caps = body["capabilities"].as_array().expect("capabilities");
        assert_eq!(caps[0]["name"], "read_temperature");
        assert_eq!(caps[0]["mode"], "input", "lowercase mode on the wire");
        let token = body["device_token"].as_object().expect("token auto");
        assert!(token["token"].as_str().is_some_and(|t| t.len() >= 40));
        assert_eq!(token["encryption_key"].as_str().unwrap().len(), 44);
        assert_eq!(token["is_active"], true);

        // Edge agent: the only family with free-form measurements.
        let res = create_device(&server, &env.alice, org, "agent-1", "edge_agent").await;
        let agent: serde_json::Value = res.json();
        assert_eq!(agent["allow_dynamic_measurements"], true);

        // Known inactive device → reactivation 200 (no new creation).
        let res = create_device(&server, &env.alice, org, "esp-001", "temp_sensor").await;
        assert_eq!(res.status_code(), 200);
        assert_eq!(res.json::<serde_json::Value>()["reactivated"], true);
        let list: serde_json::Value = server
            .get("/api/v1/devices")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await
            .json();
        assert_eq!(
            list["results"].as_array().unwrap().len(),
            2,
            "no duplicate: {list:?}"
        );

        // Inactive device + disabled token → reactivation re-enables the token.
        use pnex_backend::models::_entities::{device_registries, device_tokens};
        use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};
        let dev = device_registries::Entity::find_by_id(device_pk)
            .one(&ctx.db)
            .await
            .expect("find")
            .expect("device");
        let tok_row = device_tokens::Entity::find()
            .filter(device_tokens::Column::DeviceRegistryId.eq(device_pk))
            .one(&ctx.db)
            .await
            .expect("token")
            .expect("token");
        let mut t: device_tokens::ActiveModel = tok_row.into();
        t.is_active = Set(false);
        t.update(&ctx.db).await.expect("disable token");
        let mut d: device_registries::ActiveModel = dev.into();
        d.active = Set(false);
        d.update(&ctx.db).await.expect("disable device");
        let res = create_device(&server, &env.alice, org, "esp-001", "temp_sensor").await;
        assert_eq!(res.status_code(), 200, "inactive device reactivation");
        let detail: serde_json::Value = server
            .get(&format!("/api/v1/devices/{device_pk}"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await
            .json();
        assert_eq!(detail["active"], true);
        assert_eq!(
            detail["device_token"]["is_active"], true,
            "token reactivated"
        );

        // Active device → exact 400.
        let res = create_device(&server, &env.alice, org, "esp-001", "temp_sensor").await;
        assert_eq!(res.status_code(), 400);
        assert_eq!(
            res.json::<serde_json::Value>()["error"],
            "device-already-active"
        );

        // Unknown predefined → 400 field by field.
        let res = create_device(&server, &env.alice, org, "esp-x", "inconnu").await;
        assert_eq!(res.status_code(), 400);
        let body: serde_json::Value = res.json();
        assert_eq!(body["error"], "field-invalid");
        assert_eq!(
            body["errors"]["fields"]["predefined_device_name"],
            "not-found"
        );
    })
    .await;
}

#[tokio::test]
#[serial]
async fn list_filters() {
    with_app(|server, env, _ctx| async move {
        let org = personal_org(&server, &env.alice).await;
        for (id, predefined) in [("esp-s1", "temp_sensor"), ("esp-a1", "relay_1ch")] {
            create_device(&server, &env.alice, org, id, predefined).await;
        }
        let get = |query: &'static str| list_devices(&server, &env.alice, org, query);

        let all = get("").await;
        assert_eq!(all["results"].as_array().unwrap().len(), 2);
        assert_eq!(all["count"], 2);

        let sensors = get("?device_type=sensor").await;
        assert_eq!(sensors["results"].as_array().unwrap().len(), 1);
        assert_eq!(sensors["results"][0]["device_id"], "esp-s1");

        // "all" = no-op.
        assert_eq!(
            get("?device_type=all").await["results"]
                .as_array()
                .unwrap()
                .len(),
            2
        );

        let by_cap = get("?capability=relay").await;
        assert_eq!(by_cap["results"].as_array().unwrap().len(), 1);
        assert_eq!(by_cap["results"][0]["device_id"], "esp-a1");

        let by_id = get("?device_id=esp-s1").await;
        assert_eq!(by_id["results"].as_array().unwrap().len(), 1);

        // Multi-field search: by identifier, then by capability.
        let by_search = get("?search=S1").await;
        assert_eq!(by_search["results"].as_array().unwrap().len(), 1);
        assert_eq!(by_search["results"][0]["device_id"], "esp-s1");
        let by_search_cap = get("?search=relay").await;
        assert_eq!(by_search_cap["results"].as_array().unwrap().len(), 1);
        assert_eq!(by_search_cap["results"][0]["device_id"], "esp-a1");
        // Case ignored, term not found → empty.
        assert_eq!(
            get("?search=ESP-A1").await["results"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            get("?search=zzz").await["results"]
                .as_array()
                .unwrap()
                .len(),
            0
        );

        // No active one: the active=true filter empties the list.
        assert_eq!(
            get("?active=true").await["results"]
                .as_array()
                .unwrap()
                .len(),
            0
        );
        assert_eq!(
            get("?active=false").await["results"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
    })
    .await;
}

/// No device update route: a device's identity and model are fixed at
/// registration (board and screen have their own routes).
#[tokio::test]
#[serial]
async fn device_has_no_generic_update_route() {
    with_app(|server, env, _ctx| async move {
        let org = personal_org(&server, &env.alice).await;
        let created: serde_json::Value =
            create_device(&server, &env.alice, org, "esp-001", "temp_sensor")
                .await
                .json();
        let id = created["id"].as_i64().expect("id");
        assert!(created.get("metadata").is_none(), "{created}");
        let res = patch_device(
            &server,
            &env.alice,
            org,
            id,
            serde_json::json!({ "metadata": { "location": "serre" } }),
        )
        .await;
        assert_eq!(res.status_code(), 405, "{}", res.text());
    })
    .await;
}

#[tokio::test]
#[serial]
async fn tier_quotas_per_type() {
    with_app(|server, env, _ctx| async move {
        let org = personal_org(&server, &env.alice).await;

        // Tier Free : 3 sensors, 1 actuator, 0 mixed.
        for n in 1..=3 {
            let res = create_device(
                &server,
                &env.alice,
                org,
                &format!("esp-s{n}"),
                "temp_sensor",
            )
            .await;
            assert_eq!(res.status_code(), 201, "sensor {n}");
        }
        let res = create_device(&server, &env.alice, org, "esp-s4", "temp_sensor").await;
        assert_eq!(res.status_code(), 400, "4th sensor refused");
        let body = res.json::<serde_json::Value>();
        assert_eq!(body["error"], "device-quota-reached");
        assert_eq!(body["errors"]["args"]["type"], "sensor");

        // Inactive devices count toward the quota:
        // a single actuator created inactive → the 2nd is already over the quota.
        let res = create_device(&server, &env.alice, org, "esp-a1", "relay_1ch").await;
        assert_eq!(res.status_code(), 201);
        let res = create_device(&server, &env.alice, org, "esp-a2", "relay_1ch").await;
        assert_eq!(res.status_code(), 400);
        let body = res.json::<serde_json::Value>();
        assert_eq!(body["error"], "device-quota-reached");
        assert_eq!(body["errors"]["args"]["type"], "actuator");

        // mixed: 1 allowed (Brick 0 — generic device prototypable in Free),
        // the 2nd is refused.
        let res = create_device(&server, &env.alice, org, "esp-m1", "mixed_hub_v1").await;
        assert_eq!(res.status_code(), 201);
        let res = create_device(&server, &env.alice, org, "esp-m2", "mixed_hub_v1").await;
        assert_eq!(res.status_code(), 400);
        let body = res.json::<serde_json::Value>();
        assert_eq!(body["error"], "device-quota-reached");
        assert_eq!(body["errors"]["args"]["type"], "mixed");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn tenant_isolation_and_roles() {
    with_app(|server, env, ctx| async move {
        let alice_org = personal_org(&server, &env.alice).await;
        let bob_org = personal_org(&server, &env.bob).await;

        // Same device_id in two orgs: two distinct devices.
        let created: serde_json::Value =
            create_device(&server, &env.alice, alice_org, "esp-001", "temp_sensor")
                .await
                .json();
        let alice_device = created["id"].as_i64().expect("id");
        let created_bob: serde_json::Value =
            create_device(&server, &env.bob, bob_org, "esp-001", "temp_sensor")
                .await
                .json();
        assert_ne!(
            created_bob["id"].as_i64().unwrap(),
            alice_device,
            "distinct devices per org"
        );

        // Each org only sees its own devices.
        for (token, org_id, expected) in [(&env.alice, alice_org, 1), (&env.bob, bob_org, 1)] {
            let list: serde_json::Value = server
                .get("/api/v1/devices")
                .add_header("Authorization", bearer(token))
                .add_header("X-Org-Id", org_id.to_string())
                .await
                .json();
            assert_eq!(list["results"].as_array().unwrap().len(), expected);
        }

        // Bob does not reach alice's device (neither read nor write).
        let res = server
            .get(&format!("/api/v1/devices/{alice_device}"))
            .add_header("Authorization", bearer(&env.bob))
            .add_header("X-Org-Id", bob_org.to_string())
            .await;
        assert_eq!(res.status_code(), 404);
        let res = server
            .delete(&format!("/api/v1/devices/{alice_device}"))
            .add_header("Authorization", bearer(&env.bob))
            .add_header("X-Org-Id", bob_org.to_string())
            .await;
        assert_eq!(res.status_code(), 404);

        // Without X-Org-Id: rejected (scoping is explicit).
        let res = server
            .get("/api/v1/devices")
            .add_header("Authorization", bearer(&env.alice))
            .await;
        assert_eq!(res.status_code(), 400);

        // Viewer: read OK, write refused.
        server
            .post(&format!("/api/v1/orgs/{alice_org}/members"))
            .add_header("Content-Type", "application/json")
            .add_header("Authorization", bearer(&env.alice))
            .json(&serde_json::json!({ "email": "bob@example.com", "role": "viewer" }))
            .await;
        let res = server
            .get("/api/v1/devices")
            .add_header("Authorization", bearer(&env.bob))
            .add_header("X-Org-Id", alice_org.to_string())
            .await;
        assert_eq!(res.status_code(), 200, "viewer reads the org's devices");
        let res = create_device(&server, &env.bob, alice_org, "esp-v", "temp_sensor").await;
        assert_eq!(res.status_code(), 403, "viewer does not create");

        // SEC-8: device credentials never reach a viewer (list + detail),
        // nor the firmware image that embeds them; the owner keeps them.
        let list: serde_json::Value = server
            .get("/api/v1/devices")
            .add_header("Authorization", bearer(&env.bob))
            .add_header("X-Org-Id", alice_org.to_string())
            .await
            .json();
        assert!(list["results"][0]["device_token"].is_null(), "{list}");
        let detail: serde_json::Value = server
            .get(&format!("/api/v1/devices/{alice_device}"))
            .add_header("Authorization", bearer(&env.bob))
            .add_header("X-Org-Id", alice_org.to_string())
            .await
            .json();
        assert!(detail["device_token"].is_null(), "{detail}");
        let owner: serde_json::Value = server
            .get(&format!("/api/v1/devices/{alice_device}"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", alice_org.to_string())
            .await
            .json();
        assert!(owner["device_token"]["token"].is_string(), "{owner}");
        let res = server
            .get(&format!("/api/v1/download/firmware/{alice_device}"))
            .add_header("Authorization", bearer(&env.bob))
            .add_header("X-Org-Id", alice_org.to_string())
            .await;
        assert_eq!(
            res.status_code(),
            403,
            "viewer does not download the firmware"
        );

        // SEC-7: a viewer neither deploys nor cancels an OTA rollout.
        let res = server
            .post(&format!("/api/v1/devices/{alice_device}/ota"))
            .add_header("Authorization", bearer(&env.bob))
            .add_header("X-Org-Id", alice_org.to_string())
            .add_header("Content-Type", "application/json")
            .json(&serde_json::json!({ "force": true }))
            .await;
        assert_eq!(res.status_code(), 403, "viewer does not deploy OTA");
        let res = server
            .delete(&format!("/api/v1/devices/{alice_device}/ota"))
            .add_header("Authorization", bearer(&env.bob))
            .add_header("X-Org-Id", alice_org.to_string())
            .await;
        assert_eq!(res.status_code(), 403, "viewer does not cancel an OTA");
        let _ = ctx;
    })
    .await;
}

#[tokio::test]
#[serial]
async fn deletion_cleans_token_and_build_records() {
    with_app(|server, env, ctx| async move {
        use pnex_backend::models::_entities::{build_records, device_registries, device_tokens};
        use sea_orm::{
            ActiveModelTrait, ColumnTrait, EntityTrait, PaginatorTrait, QueryFilter, Set,
        };

        let org = personal_org(&server, &env.alice).await;
        let created: serde_json::Value =
            create_device(&server, &env.alice, org, "esp-001", "temp_sensor")
                .await
                .json();
        let id = created["id"].as_i64().expect("id");

        // Two firmware records to clean up.
        for phase in ["compile", "link"] {
            build_records::ActiveModel {
                device_id: Set(Some("esp-001".into())),
                build_phase: Set(phase.into()),
                org_id: Set(org),
                ..Default::default()
            }
            .insert(&ctx.db)
            .await
            .expect("build record");
        }

        let res = server
            .delete(&format!("/api/v1/devices/{id}"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await;
        assert_eq!(res.status_code(), 204);

        let remaining_dev = device_registries::Entity::find()
            .filter(device_registries::Column::Id.eq(id))
            .one(&ctx.db)
            .await
            .unwrap();
        assert!(remaining_dev.is_none(), "device deleted");
        let remaining_tok = device_tokens::Entity::find()
            .filter(device_tokens::Column::DeviceRegistryId.eq(id))
            .one(&ctx.db)
            .await
            .unwrap();
        assert!(remaining_tok.is_none(), "token deleted");
        let remaining_builds = build_records::Entity::find()
            .filter(build_records::Column::OrgId.eq(org))
            .count(&ctx.db)
            .await
            .unwrap();
        assert_eq!(remaining_builds, 0, "build records cleaned up");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn latest_build_hydrates_list_and_detail() {
    with_app(|server, env, ctx| async move {
        use pnex_backend::models::_entities::build_records;
        use sea_orm::{ActiveModelTrait, Set};

        let org = personal_org(&server, &env.alice).await;
        let s1: serde_json::Value =
            create_device(&server, &env.alice, org, "esp-s1", "temp_sensor")
                .await
                .json();
        create_device(&server, &env.alice, org, "esp-a1", "temp_sensor").await;

        // Succeeded build record for esp-s1 only (direct insert —
        // one record per (org, device_id), upsert in the builds controller).
        build_records::ActiveModel {
            device_id: Set(Some("esp-s1".into())),
            build_phase: Set("succeeded".into()),
            org_id: Set(org),
            ..Default::default()
        }
        .insert(&ctx.db)
        .await
        .expect("build record");

        // List: per-device hydration of the page (Firmware column).
        let list = list_devices(&server, &env.alice, org, "").await;
        let results = list["results"].as_array().expect("results");
        let s1_row = results
            .iter()
            .find(|d| d["device_id"] == "esp-s1")
            .expect("esp-s1 in the list");
        let build = s1_row["latest_build"].as_object().expect("latest_build");
        assert!(build.get("success").is_none());
        assert_eq!(build["build_phase"], "succeeded");
        assert!(build["updated_at"].as_str().is_some(), "RFC 3339");
        let a1_row = results
            .iter()
            .find(|d| d["device_id"] == "esp-a1")
            .expect("esp-a1 in the list");
        assert!(a1_row["latest_build"].is_null(), "without a build → null");

        // Detail: same hydration via device_full.
        let detail: serde_json::Value = server
            .get(&format!("/api/v1/devices/{}", s1["id"].as_i64().unwrap()))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await
            .json();
        assert_eq!(detail["latest_build"]["build_phase"], "succeeded");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn shared_global_catalog() {
    with_app(|server, env, _ctx| async move {
        // Capabilities: exact shapes + mode filter (+ unknown value empty).
        let caps: serde_json::Value = server
            .get("/api/v1/device-capabilities")
            .add_header("Authorization", bearer(&env.alice))
            .await
            .json();
        let caps = caps["results"].as_array().expect("envelope");
        assert!(caps
            .iter()
            .any(|c| c["name"] == "relay" && c["mode"] == "output"));
        assert!(caps.iter().any(|c| c["mode"] == "input"));

        let outputs: serde_json::Value = server
            .get("/api/v1/device-capabilities?mode=output")
            .add_header("Authorization", bearer(&env.alice))
            .await
            .json();
        assert_eq!(outputs["results"].as_array().unwrap().len(), 1);

        let none: serde_json::Value = server
            .get("/api/v1/device-capabilities?mode=bidon")
            .add_header("Authorization", bearer(&env.alice))
            .await
            .json();
        assert_eq!(none["results"].as_array().unwrap().len(), 0);

        // Predefined devices: capabilities = names, combinable filters.
        let pds: serde_json::Value = server
            .get("/api/v1/predefined-devices")
            .add_header("Authorization", bearer(&env.alice))
            .await
            .json();
        let pds = pds["results"].as_array().expect("envelope");
        assert_eq!(
            pds.len(),
            7,
            "4 predefined + generic_esp8266 (Brick 0) + edge_agent (D95) + browser_agent (D255)"
        );
        let relay_pd = pds
            .iter()
            .find(|p| p["name"] == "relay_1ch")
            .expect("relay_1ch");
        assert_eq!(relay_pd["device_type"], "actuator");
        assert_eq!(relay_pd["board"], "esp32");
        assert_eq!(relay_pd["capabilities"], serde_json::json!(["relay"]));
        assert!(
            relay_pd.get("id").is_none(),
            "no id in the catalogue contract"
        );

        // Filters: device_type, capabilities (OR), name icontains.
        let sensors: serde_json::Value = server
            .get("/api/v1/predefined-devices?device_type=sensor")
            .add_header("Authorization", bearer(&env.alice))
            .await
            .json();
        // temp_sensor is the only sensor model of the test catalogue.
        assert_eq!(sensors["results"].as_array().unwrap().len(), 1);

        let by_caps: serde_json::Value = server
            .get("/api/v1/predefined-devices?capabilities=relay&capabilities=read_temperature")
            .add_header("Authorization", bearer(&env.alice))
            .await
            .json();
        assert_eq!(
            by_caps["results"].as_array().unwrap().len(),
            3,
            "OR over capabilities: temp_sensor, relay_1ch and mixed_hub_v1"
        );

        let icontains: serde_json::Value = server
            .get("/api/v1/predefined-devices?name=TEMP")
            .add_header("Authorization", bearer(&env.alice))
            .await
            .json();
        assert_eq!(icontains["results"].as_array().unwrap().len(), 1);

        // Multi-field search (D14): board, capability, type — in SQL.
        let by_board: serde_json::Value = server
            .get("/api/v1/predefined-devices?search=ESP32")
            .add_header("Authorization", bearer(&env.alice))
            .await
            .json();
        assert_eq!(
            by_board["count"], 6,
            "esp32 board only (edge_agent and browser_agent included) — generic_esp8266 is on esp8266, filtered out"
        );
        let by_cap_search: serde_json::Value = server
            .get("/api/v1/predefined-devices?search=RELAY")
            .add_header("Authorization", bearer(&env.alice))
            .await
            .json();
        assert_eq!(
            by_cap_search["count"], 2,
            "relay_1ch (name and cap) + mixed_hub_v1 (relay cap)"
        );
        let by_type_search: serde_json::Value = server
            .get("/api/v1/predefined-devices?search=actuator")
            .add_header("Authorization", bearer(&env.alice))
            .await
            .json();
        assert_eq!(by_type_search["count"], 1);

        // Bob sees the same catalog (global, not org-scoped).
        let bob_pds: serde_json::Value = server
            .get("/api/v1/predefined-devices")
            .add_header("Authorization", bearer(&env.bob))
            .await
            .json();
        assert_eq!(bob_pds["results"].as_array().unwrap().len(), 7);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn list_pagination() {
    with_app(|server, env, _ctx| async move {
        let org = personal_org(&server, &env.alice).await;
        // Free tier: 3 sensors max — perfect for 3 pages of 2.
        for n in 1..=3 {
            create_device(
                &server,
                &env.alice,
                org,
                &format!("esp-s{n}"),
                "temp_sensor",
            )
            .await;
        }

        // Registry: page 1 → explicit next, previous absent.
        let page1 = list_devices(&server, &env.alice, org, "?limit=2").await;
        assert_eq!(page1["count"], 3);
        assert_eq!(page1["results"].as_array().unwrap().len(), 2);
        assert_eq!(
            page1["next"].as_str().unwrap(),
            "/api/v1/devices?limit=2&offset=2"
        );
        assert!(page1["previous"].is_null());

        // Incomplete last page: next absent, previous points at 0.
        let page2 = list_devices(&server, &env.alice, org, "?limit=2&offset=2").await;
        assert_eq!(page2["results"].as_array().unwrap().len(), 1);
        assert!(page2["next"].is_null());
        assert_eq!(
            page2["previous"].as_str().unwrap(),
            "/api/v1/devices?limit=2&offset=0"
        );

        // Offset past the end: consistent empty page.
        let beyond = list_devices(&server, &env.alice, org, "?limit=2&offset=9").await;
        assert_eq!(beyond["count"], 3);
        assert_eq!(beyond["results"].as_array().unwrap().len(), 0);
        assert!(beyond["next"].is_null());

        // The links keep the active filters.
        let filtered = list_devices(&server, &env.alice, org, "?device_type=sensor&limit=2").await;
        assert_eq!(
            filtered["next"].as_str().unwrap(),
            "/api/v1/devices?device_type=sensor&limit=2&offset=2"
        );

        // Default: 10 per page (env var PAGINATION_DEFAULT_LIMIT).
        let def = list_devices(&server, &env.alice, org, "").await;
        assert_eq!(def["count"], 3);
        assert_eq!(
            def["results"].as_array().unwrap().len(),
            3,
            "3 < default 10"
        );

        // Catalogue : pagination SQL (count exact, pages).
        let cat1: serde_json::Value = server
            .get("/api/v1/predefined-devices?limit=2")
            .add_header("Authorization", bearer(&env.alice))
            .await
            .json();
        assert_eq!(
            cat1["count"], 7,
            "4 + generic_esp8266 (Brick 0) + edge_agent (D95) + browser_agent (D255)"
        );
        assert_eq!(cat1["results"].as_array().unwrap().len(), 2);
        assert_eq!(
            cat1["next"].as_str().unwrap(),
            "/api/v1/predefined-devices?limit=2&offset=2"
        );
        let cat3: serde_json::Value = server
            .get("/api/v1/predefined-devices?limit=2&offset=2")
            .add_header("Authorization", bearer(&env.alice))
            .await
            .json();
        assert_eq!(cat3["results"].as_array().unwrap().len(), 2);
        // 5th, 6th and 7th predefined (generic_esp8266, edge_agent, browser_agent) → 2 + 1.
        assert_eq!(
            cat3["next"].as_str().unwrap(),
            "/api/v1/predefined-devices?limit=2&offset=4"
        );
        let cat4: serde_json::Value = server
            .get("/api/v1/predefined-devices?limit=2&offset=4")
            .add_header("Authorization", bearer(&env.alice))
            .await
            .json();
        assert_eq!(cat4["results"].as_array().unwrap().len(), 2);
        assert_eq!(
            cat4["next"].as_str().unwrap(),
            "/api/v1/predefined-devices?limit=2&offset=6"
        );
        let cat5: serde_json::Value = server
            .get("/api/v1/predefined-devices?limit=2&offset=6")
            .add_header("Authorization", bearer(&env.alice))
            .await
            .json();
        assert_eq!(cat5["results"].as_array().unwrap().len(), 1);
        assert!(cat5["next"].is_null(), "fin de catalogue");

        // Capabilities: same envelope.
        let caps: serde_json::Value = server
            .get("/api/v1/device-capabilities?limit=1")
            .add_header("Authorization", bearer(&env.alice))
            .await
            .json();
        assert_eq!(caps["count"], 2);
        assert_eq!(caps["results"].as_array().unwrap().len(), 1);
        assert_eq!(
            caps["next"].as_str().unwrap(),
            "/api/v1/device-capabilities?limit=1&offset=1"
        );

        // User's orgs: envelope too (one personal org here).
        let orgs: serde_json::Value = server
            .get("/api/v1/orgs")
            .add_header("Authorization", bearer(&env.alice))
            .await
            .json();
        assert_eq!(orgs["count"], 1);
        assert_eq!(orgs["results"].as_array().unwrap().len(), 1);
    })
    .await;
}

/// `GET /devices/{id}/pinout` exposes `connected` (WS state, not DB): a
/// never-connected device answers `connected: false` with its overlay pins —
/// the flow editor uses it to say "offline" instead of a false
/// "not found" when the server restarted (user feedback).
#[tokio::test]
#[serial]
async fn pinout_exposes_connected_flag() {
    with_app(|server, env, _ctx| async move {
        let org = personal_org(&server, &env.alice).await;
        create_device(&server, &env.alice, org, "esp-off", "temp_sensor").await;
        let list = list_devices(&server, &env.alice, org, "?device_id=esp-off").await;
        let pk = list["results"][0]["id"].as_i64().expect("device created");

        let body: serde_json::Value = server
            .get(&format!("/api/v1/devices/{pk}/pinout"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await
            .json();
        assert_eq!(body["connected"], false, "never connected → offline");
        assert_eq!(body["device_id"], "esp-off");
        assert!(body["pins"].is_array());
    })
    .await;
}
