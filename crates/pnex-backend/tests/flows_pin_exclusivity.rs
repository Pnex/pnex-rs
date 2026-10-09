//! Cross-flow output-pin exclusivity ("one write source per output"):
//! deploy gate (`pin-already-assigned`), manual-write 409
//! (`pin-reserved-by-flow`), pinout `reserved_by` markers, own-flow
//! exemption and release-on-stop.
//!
//! Harness = `ws_device.rs` (firmware-role mirror client) × `regulator.rs`
//! school. Engine OFF (`PNEX_FLOW_ENABLED=false`): a deploy answers 503 only
//! AFTER the pin-exclusivity gate (post-ack DB marking never runs), so "503"
//! is the observable for "the gate let the deploy through", and deployed
//! state is seeded directly in DB through the entities API.
//! Requires PostgreSQL (TEST_DATABASE_URL).

mod common;

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use loco_rs::testing::request::{RequestConfig, RequestConfigBuilder};
use pnex_backend::app::App;
use serial_test::serial;

// ─────────────────── Encrypted WS mirror client (firmware role) ───────────────────

fn key_bytes(b64: &str) -> [u8; 32] {
    STANDARD
        .decode(b64)
        .expect("key b64")
        .try_into()
        .expect("32-byte key")
}

// ─────────────────── Harness ───────────────────

async fn with_app<F, Fut>(f: F)
where
    F: FnOnce(axum_test::TestServer, String, loco_rs::app::AppContext) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let base = common::spawn_mock_rauthy().await;
    let _ = tracing_subscriber::fmt().with_test_writer().try_init();
    unsafe { std::env::set_var("RAUTHY_URL", &base) };
    // Flow engine OFF: deploy answers 503 only AFTER the pin-exclusivity
    // gate, so 503 = "the gate let the deploy through".
    unsafe { std::env::set_var("PNEX_FLOW_ENABLED", "false") };
    unsafe {
        std::env::set_var(
            "PNEX_FLOW_STATE_DIR",
            format!("/tmp/pnex-pin-excl-tests-{}", std::process::id()),
        )
    };
    unsafe { std::env::set_var("PNEX_FLOW_RELOAD_ACK_SECS", "5") };
    unsafe { std::env::set_var("PNEX_FLOW_DEBUG_TOOLS", "false") };
    let config: RequestConfig = RequestConfigBuilder::new().build();
    let alice = common::valid_token(
        &base,
        "00000000-0000-0000-0000-00000000000a",
        "alice",
        "alice@example.com",
    );
    loco_rs::testing::request::request_with_config::<App, _, _>(
        config,
        move |server, ctx| async move {
            common::seed_catalogue(&ctx.db).await;
            f(server, alice, ctx).await;
        },
    )
    .await;
}

struct Dev {
    id: i64,
    device_id: String,
    token: String,
    key: [u8; 32],
}

async fn personal_org(server: &axum_test::TestServer, token: &str) -> i64 {
    server
        .get("/api/v1/user-info")
        .add_header("Authorization", format!("Bearer {token}"))
        .await
        .json::<serde_json::Value>()["orgs"][0]["id"]
        .as_i64()
        .expect("personal org")
}

async fn create_generic(server: &axum_test::TestServer, auth: &str, device_id: &str) -> Dev {
    let org = personal_org(server, auth).await;
    let res = server
        .post("/api/v1/devices")
        .add_header("Authorization", format!("Bearer {auth}"))
        .add_header("X-Org-Id", org.to_string())
        .add_header("Content-Type", "application/json")
        .json(&serde_json::json!({
            "device_id": device_id,
            "predefined_device_name": "generic_esp8266",
        }))
        .await;
    res.assert_status(axum_test::http::StatusCode::CREATED);
    let dto: serde_json::Value = res.json();
    Dev {
        id: dto["id"].as_i64().expect("id"),
        device_id: device_id.into(),
        token: dto["device_token"]["token"].as_str().expect("token").into(),
        key: key_bytes(dto["device_token"]["encryption_key"].as_str().expect("key")),
    }
}

async fn connect(server: &axum_test::TestServer, d: &Dev) -> common::DevWs {
    common::DevWs::connect(
        server,
        &format!("/ws/device?token={}", &d.token,),
        &d.key,
        &d.device_id,
    )
    .await
}

/// Announce → ProvisionAck (pins provisioned on first connection).
async fn announce_and_expect_provision(ws: &mut common::DevWs) -> Vec<pnex_core::PinSpec> {
    let announce = serde_json::json!({
        "t": "announce", "chip": "esp8266", "board": "nodemcu", "fw": "0.1.0"
    })
    .to_string();
    ws.send_plain(&announce).await;
    let raw = ws.recv_plain().await;
    let msg: pnex_core::ServerMsg = serde_json::from_str(&raw.clone()).expect("ServerMsg");
    match msg {
        pnex_core::ServerMsg::ProvisionAck { caps, .. } => caps,
        other => panic!("ProvisionAck expected, got: {other:?}"),
    }
}

/// set_mode D1 (gpio 5) → digital_out, safe low. Requires a live session.
async fn set_d1_output(server: &axum_test::TestServer, auth: &str, org: i64, d: &Dev) {
    let res = server
        .post(&format!("/api/v1/devices/{}/commands", d.id))
        .add_header("Authorization", format!("Bearer {auth}"))
        .add_header("X-Org-Id", org.to_string())
        .json(&serde_json::json!({
            "op": "set_mode", "gpio": 5, "mode": "digital_out",
            "opts": {"safe_state": "low"}
        }))
        .await;
    res.assert_status_ok();
}

async fn create_write_flow(
    server: &axum_test::TestServer,
    auth: &str,
    org: i64,
    name: &str,
    device_slug: &str,
    pin: &str,
) -> i64 {
    let res = server
        .post("/api/v1/flows")
        .add_header("Authorization", format!("Bearer {auth}"))
        .add_header("X-Org-Id", org.to_string())
        .add_header("Content-Type", "application/json")
        .json(&serde_json::json!({
            "name": name,
            "graph": {"nodes": [{
                "id": "w1", "kind": "device_write",
                "config": {"device_id": device_slug, "pins": [pin]},
            }]},
        }))
        .await;
    res.assert_status(axum_test::http::StatusCode::CREATED);
    res.json::<serde_json::Value>()["id"]
        .as_i64()
        .expect("flow id")
}

async fn create_read_flow(
    server: &axum_test::TestServer,
    auth: &str,
    org: i64,
    name: &str,
    device_slug: &str,
    pin: &str,
) -> i64 {
    let res = server
        .post("/api/v1/flows")
        .add_header("Authorization", format!("Bearer {auth}"))
        .add_header("X-Org-Id", org.to_string())
        .add_header("Content-Type", "application/json")
        .json(&serde_json::json!({
            "name": name,
            "graph": {"nodes": [{
                "id": "d1", "kind": "device_read",
                "config": {"device_id": device_slug, "pins": [pin], "window_secs": 30.0},
            }]},
        }))
        .await;
    res.assert_status(axum_test::http::StatusCode::CREATED);
    res.json::<serde_json::Value>()["id"]
        .as_i64()
        .expect("flow id")
}

/// Seeds the deployed state the runtime would have acked (engine-off tests):
/// status=deployed + deployed_version_id=<latest>, mirroring deploy_version's
/// post-ack marking.
async fn seed_deployed(ctx: &loco_rs::app::AppContext, flow_id: i64) {
    use pnex_backend::models::_entities::{flow_versions, flows};
    use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, QueryOrder, Set};
    let latest = flow_versions::Entity::find()
        .filter(flow_versions::Column::FlowId.eq(flow_id))
        .order_by_desc(flow_versions::Column::VersionNumber)
        .one(&ctx.db)
        .await
        .expect("versions")
        .expect("latest version");
    let flow = flows::Entity::find_by_id(flow_id)
        .one(&ctx.db)
        .await
        .expect("find flow")
        .expect("flow");
    let mut f: flows::ActiveModel = flow.into();
    f.status = Set(pnex_core::FLOW_STATUS_DEPLOYED.to_string());
    f.deployed_version_id = Set(Some(latest.id));
    f.update(&ctx.db).await.expect("seed deployed");
}

/// Releases the pin claim (stop/undeploy equivalent).
async fn unseed_deployed(ctx: &loco_rs::app::AppContext, flow_id: i64) {
    use pnex_backend::models::_entities::flows;
    use sea_orm::{ActiveModelTrait, EntityTrait, Set};
    let flow = flows::Entity::find_by_id(flow_id)
        .one(&ctx.db)
        .await
        .expect("find flow")
        .expect("flow");
    let mut f: flows::ActiveModel = flow.into();
    f.status = Set(pnex_core::FLOW_STATUS_DRAFT.to_string());
    f.deployed_version_id = Set(None);
    f.update(&ctx.db).await.expect("unseed deployed");
}

/// POST /flows/{id}/deploy — raw response (assertions stay at call sites).
async fn deploy(
    server: &axum_test::TestServer,
    auth: &str,
    org: i64,
    flow_id: i64,
) -> axum_test::TestResponse {
    server
        .post(&format!("/api/v1/flows/{flow_id}/deploy"))
        .add_header("Authorization", format!("Bearer {auth}"))
        .add_header("X-Org-Id", org.to_string())
        .json(&serde_json::json!({}))
        .await
}

/// POST /devices/{id}/commands `write` — raw response.
async fn write_pin(
    server: &axum_test::TestServer,
    auth: &str,
    org: i64,
    device_pk: i64,
    gpio: u16,
    value: serde_json::Value,
) -> axum_test::TestResponse {
    server
        .post(&format!("/api/v1/devices/{device_pk}/commands"))
        .add_header("Authorization", format!("Bearer {auth}"))
        .add_header("X-Org-Id", org.to_string())
        .json(&serde_json::json!({
            "op": "write", "gpio": gpio, "value": value,
        }))
        .await
}

// ─────────────────── Tests ───────────────────

/// A deployed flow writing D1 blocks the deploy of a second flow claiming
/// D1 — both device-write vs device-write and device-write vs regulator-card
/// actuator (400 `pin-already-assigned` naming the owning flow).
#[tokio::test]
#[serial]
async fn deploy_conflit_cross_flows_rejete_400() {
    with_app(|server, auth, ctx| async move {
        let org = personal_org(&server, &auth).await;
        let _dev = create_generic(&server, &auth, "gen-excl").await;

        let flow_a = create_write_flow(&server, &auth, org, "excl A", "gen-excl", "D1").await;
        seed_deployed(&ctx, flow_a).await;

        let flow_b = create_write_flow(&server, &auth, org, "excl B", "gen-excl", "D1").await;
        let res = deploy(&server, &auth, org, flow_b).await;
        assert_eq!(res.status_code(), 400, "{}", res.text());
        let body: serde_json::Value = res.json();
        let v = &body["violations"][0];
        assert_eq!(v["code"], "pin-already-assigned");
        assert_eq!(v["node_id"], "w1");
        assert_eq!(v["args"]["pin"], "d1");
        assert_eq!(v["args"]["device"], "gen-excl");
        assert_eq!(v["args"]["flow"], "excl A");

        // Regulator-card actuator vs device-write: same exclusivity.
        let res = server
            .post("/api/v1/flows")
            .add_header("Authorization", format!("Bearer {auth}"))
            .add_header("X-Org-Id", org.to_string())
            .add_header("Content-Type", "application/json")
            .json(&serde_json::json!({
                "name": "excl reg",
                "graph": {"nodes": [{
                    "id": "r1", "kind": "reg_tt_heat",
                    "config": {
                        "device_id": "gen-excl",
                        "sensor_pin": "A0",
                        "actuator_pin": "D1",
                        "setpoint": 19.0, "deadband": 0.5,
                    },
                }]},
            }))
            .await;
        res.assert_status(axum_test::http::StatusCode::CREATED);
        let flow_c = res.json::<serde_json::Value>()["id"].as_i64().unwrap();
        let res = deploy(&server, &auth, org, flow_c).await;
        assert_eq!(res.status_code(), 400, "{}", res.text());
        let body: serde_json::Value = res.json();
        assert_eq!(body["violations"][0]["code"], "pin-already-assigned");
        assert_eq!(body["violations"][0]["args"]["flow"], "excl A");
    })
    .await;
}

/// Manual write on a pin claimed by a deployed flow → 409 naming the flow
/// (reservation wins over the offline check, no session required).
#[tokio::test]
#[serial]
async fn write_manuelle_pin_reservee_409() {
    with_app(|server, auth, ctx| async move {
        let org = personal_org(&server, &auth).await;
        let dev = create_generic(&server, &auth, "gen-manuel").await;
        // Provision D1 (needs a live session), then drop the connection:
        // the reservation 409 must win over the offline 409.
        {
            let mut ws = connect(&server, &dev).await;
            announce_and_expect_provision(&mut ws).await;
            set_d1_output(&server, &auth, org, &dev).await;
            ws.close().await;
        }

        let flow_a = create_write_flow(&server, &auth, org, "manuel A", "gen-manuel", "D1").await;
        seed_deployed(&ctx, flow_a).await;

        let res = write_pin(&server, &auth, org, dev.id, 5, serde_json::json!(true)).await;
        assert_eq!(res.status_code(), 409, "{}", res.text());
        let body: serde_json::Value = res.json();
        assert_eq!(body["error"], "pin-reserved-by-flow");
        assert_eq!(body["errors"]["args"]["pin"], "D1");
        assert_eq!(body["errors"]["args"]["flow"], "manuel A");
    })
    .await;
}

/// A flow only READING a pin never blocks the manual write (Write-only
/// blocking — reads are shareable).
#[tokio::test]
#[serial]
async fn write_manuelle_pin_lue_seulement_autorisee() {
    with_app(|server, auth, ctx| async move {
        let org = personal_org(&server, &auth).await;
        let dev = create_generic(&server, &auth, "gen-lu").await;
        let mut ws = connect(&server, &dev).await;
        announce_and_expect_provision(&mut ws).await;
        set_d1_output(&server, &auth, org, &dev).await;

        let flow_d = create_read_flow(&server, &auth, org, "lect D", "gen-lu", "D1").await;
        seed_deployed(&ctx, flow_d).await;

        let res = write_pin(&server, &auth, org, dev.id, 5, serde_json::json!(true)).await;
        assert_eq!(res.status_code(), 200, "{}", res.text());
        assert_eq!(res.json::<serde_json::Value>()["sent"], true);
        ws.close().await;
    })
    .await;
}

/// Stop/undeploy releases the claim: the previously blocked deploy now goes
/// through the gate (503 = engine off, gate passed).
#[tokio::test]
#[serial]
async fn arret_flow_libere_pin() {
    with_app(|server, auth, ctx| async move {
        let org = personal_org(&server, &auth).await;
        let _dev = create_generic(&server, &auth, "gen-libere").await;

        let flow_a = create_write_flow(&server, &auth, org, "libere A", "gen-libere", "D1").await;
        seed_deployed(&ctx, flow_a).await;
        let flow_b = create_write_flow(&server, &auth, org, "libere B", "gen-libere", "D1").await;
        assert_eq!(deploy(&server, &auth, org, flow_b).await.status_code(), 400);

        unseed_deployed(&ctx, flow_a).await;
        assert_eq!(
            deploy(&server, &auth, org, flow_b).await.status_code(),
            503,
            "gate passed → engine-off 503 expected"
        );
    })
    .await;
}

/// Redeploy/rollback of the OWNING flow never conflicts with itself.
#[tokio::test]
#[serial]
async fn redeploy_owner_pas_bloque() {
    with_app(|server, auth, ctx| async move {
        let org = personal_org(&server, &auth).await;
        let _dev = create_generic(&server, &auth, "gen-owner").await;
        let flow_a = create_write_flow(&server, &auth, org, "owner A", "gen-owner", "D1").await;
        seed_deployed(&ctx, flow_a).await;
        assert_eq!(
            deploy(&server, &auth, org, flow_a).await.status_code(),
            503,
            "own claim excluded → gate passed"
        );
    })
    .await;
}

/// The pinout (and /pins) carry `reserved_by` for claimed pins, disappear
/// when the claim is released, and never appear for read-only claims.
#[tokio::test]
#[serial]
async fn pinout_reserved_by() {
    with_app(|server, auth, ctx| async move {
        let org = personal_org(&server, &auth).await;
        let dev = create_generic(&server, &auth, "gen-mark").await;
        // Provision D1 (live session): instances give the pinout its D1 row
        // (overlay pins alone may be absent on Tier-2 generic boards).
        {
            let mut ws = connect(&server, &dev).await;
            announce_and_expect_provision(&mut ws).await;
            set_d1_output(&server, &auth, org, &dev).await;
            ws.close().await;
        }

        let flow_a = create_write_flow(&server, &auth, org, "mark A", "gen-mark", "D1").await;
        seed_deployed(&ctx, flow_a).await;

        let res = server
            .get(&format!("/api/v1/devices/{}/pinout", dev.id))
            .add_header("Authorization", format!("Bearer {auth}"))
            .add_header("X-Org-Id", org.to_string())
            .await;
        res.assert_status_ok();
        let body: serde_json::Value = res.json();
        let d1 = body["pins"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["label"] == "D1")
            .expect("D1 pin");
        assert_eq!(d1["reserved_by"][0]["flow_id"], flow_a);
        assert_eq!(d1["reserved_by"][0]["flow_name"], "mark A");

        // Grandfathered double-claim: a second deployed flow writing the SAME
        // pin is listed too (oldest first) — the hint must not hide it.
        let flow_b = create_write_flow(&server, &auth, org, "mark B", "gen-mark", "D1").await;
        seed_deployed(&ctx, flow_b).await;
        let res = server
            .get(&format!("/api/v1/devices/{}/pinout", dev.id))
            .add_header("Authorization", format!("Bearer {auth}"))
            .add_header("X-Org-Id", org.to_string())
            .await;
        let body: serde_json::Value = res.json();
        let d1 = body["pins"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["label"] == "D1")
            .expect("D1 pin");
        assert_eq!(d1["reserved_by"].as_array().map(Vec::len), Some(2));
        assert_eq!(d1["reserved_by"][1]["flow_id"], flow_b);
        assert_eq!(d1["reserved_by"][1]["flow_name"], "mark B");
        unseed_deployed(&ctx, flow_b).await;

        // Read-only claim → no marker.
        let flow_r = create_read_flow(&server, &auth, org, "mark R", "gen-mark", "A0").await;
        seed_deployed(&ctx, flow_r).await;
        let res = server
            .get(&format!("/api/v1/devices/{}/pinout", dev.id))
            .add_header("Authorization", format!("Bearer {auth}"))
            .add_header("X-Org-Id", org.to_string())
            .await;
        let body: serde_json::Value = res.json();
        let a0 = body["pins"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["label"] == "A0")
            .expect("A0 pin");
        assert!(a0.get("reserved_by").is_none());

        unseed_deployed(&ctx, flow_a).await;
        let res = server
            .get(&format!("/api/v1/devices/{}/pinout", dev.id))
            .add_header("Authorization", format!("Bearer {auth}"))
            .add_header("X-Org-Id", org.to_string())
            .await;
        let body: serde_json::Value = res.json();
        let d1 = body["pins"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["label"] == "D1")
            .expect("D1 pin");
        assert!(d1.get("reserved_by").is_none(), "marker gone after release");
    })
    .await;
}

/// Flow `camera-source(cam) → video-record` — id of the created flow.
async fn create_record_flow(
    server: &axum_test::TestServer,
    auth: &str,
    org: i64,
    name: &str,
    camera: &str,
) -> i64 {
    let res = server
        .post("/api/v1/flows")
        .add_header("Authorization", format!("Bearer {auth}"))
        .add_header("X-Org-Id", org.to_string())
        .add_header("Content-Type", "application/json")
        .json(&serde_json::json!({
            "name": name,
            "graph": {"nodes": [
                {"id": "cam", "kind": "camera_source", "config": {"device_id": camera},
                 "outputs": [{"port": 0, "targets": ["rec"]}]},
                {"id": "rec", "kind": "video_record", "config": {}},
            ]},
        }))
        .await;
    res.assert_status(axum_test::http::StatusCode::CREATED);
    res.json::<serde_json::Value>()["id"]
        .as_i64()
        .expect("flow id")
}

/// One `video-record` per camera across the org (D105): a deployed flow
/// recording a camera blocks the deploy of a second recorder of the same
/// camera (400 `camera-already-recorded`), another camera passes the gate.
#[tokio::test]
#[serial]
async fn deploy_second_recorder_same_camera_rejected() {
    with_app(|server, auth, ctx| async move {
        let org = personal_org(&server, &auth).await;
        let flow_a = create_record_flow(&server, &auth, org, "rec A", "cam-one").await;
        seed_deployed(&ctx, flow_a).await;

        let flow_b = create_record_flow(&server, &auth, org, "rec B", "cam-one").await;
        let res = deploy(&server, &auth, org, flow_b).await;
        assert_eq!(res.status_code(), 400, "{}", res.text());
        let body: serde_json::Value = res.json();
        let v = &body["violations"][0];
        assert_eq!(v["code"], "camera-already-recorded");
        assert_eq!(v["node_id"], "rec");
        assert_eq!(v["args"]["camera"], "cam-one");
        assert_eq!(v["args"]["flow"], "rec A");

        // Another camera: not blocked by this gate (whatever the runtime
        // answers next, never `camera-already-recorded`).
        let flow_c = create_record_flow(&server, &auth, org, "rec C", "cam-two").await;
        let res = deploy(&server, &auth, org, flow_c).await;
        assert!(
            !res.text().contains("camera-already-recorded"),
            "{}",
            res.text()
        );
    })
    .await;
}
