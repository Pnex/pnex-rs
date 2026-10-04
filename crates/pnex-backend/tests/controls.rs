//! Org controls (D125–D127): CRUD, value write (validation, Valkey store +
//! publish, rate limit), batch read, viewer rights, org isolation, deploy
//! gate `control-unknown`, delete guard `control-in-use` and listeners.
//!
//! Requires PostgreSQL (TEST_DATABASE_URL) and the compose Valkey of
//! `config/test.yaml` (value tests skip when Valkey is unreachable).
//! Engine OFF: a deploy answers 503 only AFTER the gates, so 503 = "the
//! gates let the deploy through" (flows_pin_exclusivity.rs school).

mod common;

use futures_util::StreamExt;
use loco_rs::testing::request::{RequestConfig, RequestConfigBuilder};
use pnex_backend::app::App;
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
    unsafe { std::env::set_var("PNEX_FLOW_ENABLED", "false") };
    unsafe {
        std::env::set_var(
            "PNEX_FLOW_STATE_DIR",
            format!("/tmp/pnex-controls-tests-{}", std::process::id()),
        )
    };
    unsafe { std::env::set_var("PNEX_FLOW_DEBUG_TOOLS", "false") };
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

async fn personal_org(server: &axum_test::TestServer, token: &str) -> i64 {
    server
        .get("/api/v1/user-info")
        .add_header("Authorization", format!("Bearer {token}"))
        .await
        .json::<serde_json::Value>()["orgs"][0]["id"]
        .as_i64()
        .expect("personal org")
}

async fn post(
    server: &axum_test::TestServer,
    token: &str,
    org: i64,
    path: &str,
    body: serde_json::Value,
) -> axum_test::TestResponse {
    server
        .post(path)
        .add_header("Authorization", format!("Bearer {token}"))
        .add_header("X-Org-Id", org.to_string())
        .add_header("Content-Type", "application/json")
        .json(&body)
        .await
}

async fn create_control(
    server: &axum_test::TestServer,
    token: &str,
    org: i64,
    key: &str,
    spec: serde_json::Value,
) -> String {
    let res = post(
        server,
        token,
        org,
        "/api/v1/controls",
        serde_json::json!({ "key": key, "label": format!("Label {key}"), "spec": spec }),
    )
    .await;
    res.assert_status(axum_test::http::StatusCode::CREATED);
    res.json::<serde_json::Value>()["id"]
        .as_str()
        .expect("control id")
        .to_string()
}

fn error_code(res: &axum_test::TestResponse) -> String {
    res.json::<serde_json::Value>()["error"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// Test Valkey of the app (`config/test.yaml`), `None` when unreachable.
async fn valkey(ctx: &loco_rs::app::AppContext) -> Option<redis::Client> {
    pnex_backend::services::shared_valkey::conn(&ctx.config).await?;
    let url = std::env::var("PNEX_TEST_APP_VALKEY_URL")
        .unwrap_or_else(|_| "redis://127.0.0.1:6379/14".to_string());
    redis::Client::open(url).ok()
}

#[tokio::test]
#[serial]
async fn crud_key_uniqueness_and_validation() {
    with_app(|server, env, _ctx| async move {
        let org = personal_org(&server, &env.alice).await;
        let id = create_control(
            &server,
            &env.alice,
            org,
            "light.room",
            serde_json::json!({"kind": "switch"}),
        )
        .await;

        // Duplicate key: 409 control-key-taken.
        let dup = post(
            &server,
            &env.alice,
            org,
            "/api/v1/controls",
            serde_json::json!({"key": "light.room", "label": "Other", "spec": {"kind": "button"}}),
        )
        .await;
        assert_eq!(dup.status_code(), 409);
        assert_eq!(error_code(&dup), "control-key-taken");

        // Field tokens: bad key, empty label, degenerate spec.
        let bad = post(
            &server,
            &env.alice,
            org,
            "/api/v1/controls",
            serde_json::json!({"key": "bad key", "label": "x", "spec": {"kind": "switch"}}),
        )
        .await;
        assert_eq!(bad.status_code(), 400);
        assert_eq!(bad.json::<serde_json::Value>()["key"], "invalid");
        let bad = post(
            &server,
            &env.alice,
            org,
            "/api/v1/controls",
            serde_json::json!({"key": "k2", "label": " ", "spec": {"kind": "switch"}}),
        )
        .await;
        assert_eq!(bad.json::<serde_json::Value>()["label"], "required");
        let bad = post(
            &server,
            &env.alice,
            org,
            "/api/v1/controls",
            serde_json::json!({"key": "k2", "label": "Dim",
                               "spec": {"kind": "slider", "min": 10.0, "max": 5.0}}),
        )
        .await;
        assert_eq!(
            bad.json::<serde_json::Value>()["spec"],
            "control_spec_range"
        );

        // List (search on the key) + detail with an empty listener list.
        let list = server
            .get("/api/v1/controls?search=light")
            .add_header("Authorization", format!("Bearer {}", env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await
            .json::<serde_json::Value>();
        assert_eq!(list["count"], 1);
        assert_eq!(list["results"][0]["key"], "light.room");
        assert_eq!(list["results"][0]["listened_by"], serde_json::json!([]));

        // Patch: label + kind change.
        let patched = server
            .patch(&format!("/api/v1/controls/{id}"))
            .add_header("Authorization", format!("Bearer {}", env.alice))
            .add_header("X-Org-Id", org.to_string())
            .json(&serde_json::json!({"label": "Room light", "spec": {"kind": "slider"}}))
            .await;
        patched.assert_status_ok();
        let body = patched.json::<serde_json::Value>();
        assert_eq!(body["label"], "Room light");
        assert_eq!(body["spec"]["kind"], "slider");

        // Delete without listeners: 204, then 404.
        let del = server
            .delete(&format!("/api/v1/controls/{id}"))
            .add_header("Authorization", format!("Bearer {}", env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await;
        assert_eq!(del.status_code(), 204);
        let gone = server
            .get(&format!("/api/v1/controls/{id}"))
            .add_header("Authorization", format!("Bearer {}", env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await;
        assert_eq!(gone.status_code(), 404);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn write_publishes_stores_and_rate_limits() {
    with_app(|server, env, ctx| async move {
        let Some(client) = valkey(&ctx).await else {
            eprintln!("skip: test Valkey unreachable");
            return;
        };
        let org = personal_org(&server, &env.alice).await;
        let id = create_control(
            &server,
            &env.alice,
            org,
            "pwm.fan",
            serde_json::json!({"kind": "slider", "step": 0.5}),
        )
        .await;

        // Subscribe before writing: the publish is the flow trigger.
        let mut pubsub = client.get_async_pubsub().await.expect("pubsub");
        pubsub
            .subscribe(pnex_core::ui_control::control_channel(org))
            .await
            .expect("subscribe");
        let mut stream = pubsub.into_on_message();

        let res = post(
            &server,
            &env.alice,
            org,
            &format!("/api/v1/controls/{id}/value"),
            serde_json::json!({"value": 42.5, "via": "dashboard:abc"}),
        )
        .await;
        res.assert_status_ok();
        let stored = res.json::<serde_json::Value>();
        assert_eq!(stored["v"], 42.5);
        assert!(stored["by"].as_str().is_some_and(|b| !b.is_empty()));
        assert_eq!(stored["via"], "dashboard:abc");

        let msg = tokio::time::timeout(std::time::Duration::from_secs(3), stream.next())
            .await
            .expect("published in time")
            .expect("message");
        let frame: pnex_core::ui_control::ControlEvent =
            serde_json::from_str(&msg.get_payload::<String>().unwrap()).unwrap();
        assert_eq!(frame.control_id.to_string(), id);
        assert_eq!(frame.key, "pwm.fan");
        assert_eq!(frame.value.v, 42.5);

        // Immediate second write: 429 control-rate-limited.
        let fast = post(
            &server,
            &env.alice,
            org,
            &format!("/api/v1/controls/{id}/value"),
            serde_json::json!({"value": 43.0}),
        )
        .await;
        assert_eq!(fast.status_code(), 429);
        assert_eq!(error_code(&fast), "control-rate-limited");

        // Out of the spec domain: 400 with the reason token.
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        let off = post(
            &server,
            &env.alice,
            org,
            &format!("/api/v1/controls/{id}/value"),
            serde_json::json!({"value": 42.2}),
        )
        .await;
        assert_eq!(off.status_code(), 400);
        assert_eq!(error_code(&off), "control-value-invalid");
        assert_eq!(
            off.json::<serde_json::Value>()["errors"]["args"]["reason"],
            "off_step"
        );

        // Batch read: stored value, unknown id = null.
        let unknown = uuid::Uuid::new_v4().to_string();
        let values = post(
            &server,
            &env.alice,
            org,
            "/api/v1/controls/values",
            serde_json::json!({"ids": [id, unknown]}),
        )
        .await
        .json::<serde_json::Value>();
        assert_eq!(values["values"][&id]["v"], 42.5);
        assert!(values["values"][&unknown].is_null());
    })
    .await;
}

#[tokio::test]
#[serial]
async fn viewer_reads_but_never_operates_and_orgs_are_isolated() {
    with_app(|server, env, _ctx| async move {
        let org_a = personal_org(&server, &env.alice).await;
        let org_b = personal_org(&server, &env.bob).await;
        let id = create_control(
            &server,
            &env.alice,
            org_a,
            "valve",
            serde_json::json!({"kind": "switch"}),
        )
        .await;

        // Bob on his own org: masked 404 on Alice's control.
        let res = post(
            &server,
            &env.bob,
            org_b,
            &format!("/api/v1/controls/{id}/value"),
            serde_json::json!({"value": 1.0}),
        )
        .await;
        assert_eq!(res.status_code(), 404);
        let values = post(
            &server,
            &env.bob,
            org_b,
            "/api/v1/controls/values",
            serde_json::json!({"ids": [id]}),
        )
        .await
        .json::<serde_json::Value>();
        assert!(values["values"][&id].is_null());

        // Bob as viewer of org A: reads, cannot operate nor create.
        server
            .post(&format!("/api/v1/orgs/{org_a}/members"))
            .add_header("Content-Type", "application/json")
            .add_header("Authorization", format!("Bearer {}", env.alice))
            .json(&serde_json::json!({ "email": "bob@example.com", "role": "viewer" }))
            .await;
        let read = server
            .get(&format!("/api/v1/controls/{id}"))
            .add_header("Authorization", format!("Bearer {}", env.bob))
            .add_header("X-Org-Id", org_a.to_string())
            .await;
        assert_eq!(read.status_code(), 200);
        let operate = post(
            &server,
            &env.bob,
            org_a,
            &format!("/api/v1/controls/{id}/value"),
            serde_json::json!({"value": 1.0}),
        )
        .await;
        assert_eq!(operate.status_code(), 403);
        assert_eq!(error_code(&operate), "control-write-forbidden");
        let create = post(
            &server,
            &env.bob,
            org_a,
            "/api/v1/controls",
            serde_json::json!({"key": "x", "label": "x", "spec": {"kind": "button"}}),
        )
        .await;
        assert_eq!(create.status_code(), 403);
    })
    .await;
}

/// Seeds the deployed state the runtime would have acked (engine-off).
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

async fn create_listening_flow(
    server: &axum_test::TestServer,
    token: &str,
    org: i64,
    name: &str,
    control: &str,
) -> i64 {
    let res = post(
        server,
        token,
        org,
        "/api/v1/flows",
        serde_json::json!({
            "name": name,
            "graph": {"nodes": [
                {"id": "cs", "kind": "control_source",
                 "config": {"controls": [control]},
                 "outputs": [{"port": 0, "targets": ["dbg"]}]},
                {"id": "dbg", "kind": "debug"}
            ]},
        }),
    )
    .await;
    res.assert_status(axum_test::http::StatusCode::CREATED);
    res.json::<serde_json::Value>()["id"]
        .as_i64()
        .expect("flow id")
}

#[tokio::test]
#[serial]
async fn deploy_gate_listeners_and_delete_guard() {
    with_app(|server, env, ctx| async move {
        let org = personal_org(&server, &env.alice).await;

        // A control-source on a control absent from the org: 400.
        let ghost = uuid::Uuid::new_v4().to_string();
        let flow_ghost = create_listening_flow(&server, &env.alice, org, "ghost", &ghost).await;
        let res = post(
            &server,
            &env.alice,
            org,
            &format!("/api/v1/flows/{flow_ghost}/deploy"),
            serde_json::json!({}),
        )
        .await;
        assert_eq!(res.status_code(), 400);
        let v = res.json::<serde_json::Value>()["violations"][0].clone();
        assert_eq!(v["code"], "control-unknown");
        assert_eq!(v["args"]["control"], ghost);

        // An existing control passes the gate (503 = engine off, after it).
        let id = create_control(
            &server,
            &env.alice,
            org,
            "pump",
            serde_json::json!({"kind": "switch"}),
        )
        .await;
        let flow = create_listening_flow(&server, &env.alice, org, "pump flow", &id).await;
        let res = post(
            &server,
            &env.alice,
            org,
            &format!("/api/v1/flows/{flow}/deploy"),
            serde_json::json!({}),
        )
        .await;
        assert_eq!(res.status_code(), 503, "{}", res.text());

        // Deployed: listed as listener, delete refused with the flow name.
        seed_deployed(&ctx, flow).await;
        let detail = server
            .get(&format!("/api/v1/controls/{id}"))
            .add_header("Authorization", format!("Bearer {}", env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await
            .json::<serde_json::Value>();
        assert_eq!(detail["listened_by"][0]["flow_name"], "pump flow");
        let del = server
            .delete(&format!("/api/v1/controls/{id}"))
            .add_header("Authorization", format!("Bearer {}", env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await;
        assert_eq!(del.status_code(), 409);
        assert_eq!(error_code(&del), "control-in-use");
        assert_eq!(
            del.json::<serde_json::Value>()["errors"]["args"]["flow"],
            "pump flow"
        );
    })
    .await;
}

async fn patch(
    server: &axum_test::TestServer,
    token: &str,
    org: i64,
    path: &str,
    body: serde_json::Value,
) -> axum_test::TestResponse {
    server
        .patch(path)
        .add_header("Authorization", format!("Bearer {token}"))
        .add_header("X-Org-Id", org.to_string())
        .add_header("Content-Type", "application/json")
        .json(&body)
        .await
}

async fn get(
    server: &axum_test::TestServer,
    token: &str,
    org: i64,
    path: &str,
) -> serde_json::Value {
    server
        .get(path)
        .add_header("Authorization", format!("Bearer {token}"))
        .add_header("X-Org-Id", org.to_string())
        .await
        .json::<serde_json::Value>()
}

/// Desktop layout with control widgets `(id, type, title, control)`.
fn control_layout(widgets: &[(&str, &str, &str, Option<&str>)]) -> serde_json::Value {
    let widgets: Vec<serde_json::Value> = widgets
        .iter()
        .enumerate()
        .map(|(i, (id, ty, title, control))| {
            let mut options = serde_json::json!({});
            if let Some(c) = control {
                options["control"] = serde_json::json!({ "control_id": c });
            }
            serde_json::json!({
                "id": id, "type": ty, "title": title,
                "x": 40 + 300 * i as i64, "y": 40, "w": 240, "h": 120,
                "source": [], "options": options,
            })
        })
        .collect();
    serde_json::json!({
        "canvas": { "width": 1600, "height": 900 },
        "widgets": widgets,
        "wires": [],
    })
}

/// Control bound to widget `id` in a dashboard response.
fn bound_control(dashboard: &serde_json::Value, id: &str) -> String {
    dashboard["layout"]["widgets"]
        .as_array()
        .expect("widgets")
        .iter()
        .find(|w| w["id"] == id)
        .and_then(|w| w["options"]["control"]["control_id"].as_str())
        .unwrap_or_else(|| panic!("widget {id} bound: {dashboard}"))
        .to_string()
}

fn control_by_id<'a>(list: &'a serde_json::Value, id: &str) -> Option<&'a serde_json::Value> {
    list["results"]
        .as_array()
        .expect("results")
        .iter()
        .find(|c| c["id"] == id)
}

/// D131: a control widget declares its own control — provisioned at save
/// (stable id and generated key, label following the title, origin
/// resolved for the flow catalog), released when the widget or the
/// dashboard goes away (kept standalone while a flow references it), and a
/// foreign control id never crosses orgs.
#[tokio::test]
#[serial]
async fn surface_declared_controls_are_provisioned_and_released() {
    with_app(|server, env, _ctx| async move {
        let org = personal_org(&server, &env.alice).await;
        let created = post(
            &server,
            &env.alice,
            org,
            "/api/v1/dashboards",
            serde_json::json!({
                "name": "Machine room",
                "layout": control_layout(&[
                    ("w-0001", "switch", "Light", None),
                    ("w-0002", "slider", "", None),
                ]),
            }),
        )
        .await;
        created.assert_status(axum_test::http::StatusCode::CREATED);
        let dash: serde_json::Value = created.json();
        let dash_id = dash["id"].as_str().unwrap().to_string();
        let light = bound_control(&dash, "w-0001");
        let dimmer = bound_control(&dash, "w-0002");
        assert_ne!(light, dimmer);

        let list = get(&server, &env.alice, org, "/api/v1/controls").await;
        let c = control_by_id(&list, &light).expect("light provisioned");
        let short = &dash_id.replace('-', "")[..8];
        assert_eq!(c["key"], format!("dash-{short}.w-0001"));
        assert_eq!(c["label"], "Light");
        assert_eq!(c["spec"]["kind"], "switch");
        assert_eq!(c["origin"]["surface"], "dashboard");
        assert_eq!(c["origin"]["surface_id"], dash_id.as_str());
        assert_eq!(c["origin"]["surface_name"], "Machine room");
        assert_eq!(c["origin"]["item_id"], "w-0001");
        let d = control_by_id(&list, &dimmer).expect("dimmer provisioned");
        assert_eq!(d["label"], "w-0002", "untitled widget: item id as label");
        assert_eq!(d["spec"]["kind"], "slider");

        // Re-save without the ids (an editor that lost them): same controls
        // (found by origin); the title change renames the light.
        let saved = patch(
            &server,
            &env.alice,
            org,
            &format!("/api/v1/dashboards/{dash_id}"),
            serde_json::json!({
                "expected_version_number": 1,
                "layout": control_layout(&[
                    ("w-0001", "switch", "Main light", None),
                    ("w-0002", "slider", "", Some(&dimmer)),
                ]),
            }),
        )
        .await;
        saved.assert_status_ok();
        let saved: serde_json::Value = saved.json();
        assert_eq!(bound_control(&saved, "w-0001"), light);
        assert_eq!(bound_control(&saved, "w-0002"), dimmer);
        let list = get(&server, &env.alice, org, "/api/v1/controls").await;
        assert_eq!(control_by_id(&list, &light).unwrap()["label"], "Main light");

        // A flow references the light (saved, not deployed).
        create_listening_flow(&server, &env.alice, org, "Light flow", &light).await;

        // Widget removed: its unused control is deleted.
        patch(
            &server,
            &env.alice,
            org,
            &format!("/api/v1/dashboards/{dash_id}"),
            serde_json::json!({
                "expected_version_number": 2,
                "layout": control_layout(&[("w-0001", "switch", "Main light", Some(&light))]),
            }),
        )
        .await
        .assert_status_ok();
        let list = get(&server, &env.alice, org, "/api/v1/controls").await;
        assert!(
            control_by_id(&list, &dimmer).is_none(),
            "unused control deleted"
        );

        // Dashboard deleted: the light survives as a standalone control
        // (a flow uses it).
        server
            .delete(&format!("/api/v1/dashboards/{dash_id}"))
            .add_header("Authorization", format!("Bearer {}", env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await
            .assert_status(axum_test::http::StatusCode::NO_CONTENT);
        let list = get(&server, &env.alice, org, "/api/v1/controls").await;
        let kept = control_by_id(&list, &light).expect("used control kept");
        assert!(kept.get("origin").is_none(), "released: {kept}");

        // Org isolation: Bob linking Alice's control gets his own one.
        let bob_org = personal_org(&server, &env.bob).await;
        let bob_dash: serde_json::Value = post(
            &server,
            &env.bob,
            bob_org,
            "/api/v1/dashboards",
            serde_json::json!({
                "name": "Bob",
                "layout": control_layout(&[("w-0001", "switch", "", Some(&light))]),
            }),
        )
        .await
        .json();
        let bob_control = bound_control(&bob_dash, "w-0001");
        assert_ne!(bob_control, light, "foreign control never bound");
        let bob_list = get(&server, &env.bob, bob_org, "/api/v1/controls").await;
        assert!(control_by_id(&bob_list, &bob_control).is_some());
        assert!(control_by_id(&bob_list, &light).is_none());
    })
    .await;
}

/// D133: a flow whose `control-source` has no source yet saves as a draft
/// (the flow may be built before its surfaces); the deploy refuses it.
#[tokio::test]
#[serial]
async fn empty_control_source_saves_but_never_deploys() {
    with_app(|server, env, _ctx| async move {
        let org = personal_org(&server, &env.alice).await;
        let res = post(
            &server,
            &env.alice,
            org,
            "/api/v1/flows",
            serde_json::json!({
                "name": "Draft",
                "graph": {"nodes": [
                    {"id": "cs", "kind": "control_source", "config": {"controls": []},
                     "outputs": [{"port": 0, "targets": ["dbg"]}]},
                    {"id": "dbg", "kind": "debug"}
                ]},
            }),
        )
        .await;
        res.assert_status(axum_test::http::StatusCode::CREATED);
        let flow_id = res.json::<serde_json::Value>()["id"].as_i64().unwrap();
        let deploy = post(
            &server,
            &env.alice,
            org,
            &format!("/api/v1/flows/{flow_id}/deploy"),
            serde_json::json!({}),
        )
        .await;
        deploy.assert_status(axum_test::http::StatusCode::BAD_REQUEST);
        let body: serde_json::Value = deploy.json();
        assert_eq!(
            body["violations"][0]["code"], "control_source_empty",
            "{body}"
        );
        assert_eq!(body["violations"][0]["node_id"], "cs", "{body}");
    })
    .await;
}

/// D137: a select widget declares its options; the server provisions the
/// control with them, keeps them in sync at each save, refuses a value
/// outside the options and stores the option key next to the number. An
/// invalid declared domain is refused with the dashboard save.
#[tokio::test]
#[serial]
async fn declared_select_spec_is_applied_and_enforced() {
    with_app(|server, env, ctx| async move {
        let org = personal_org(&server, &env.alice).await;
        let spec =
            |opts: serde_json::Value| serde_json::json!({ "kind": "select", "options": opts });
        let modes = spec(serde_json::json!([
            {"value": 0, "key": "off"},
            {"value": 1, "key": "heat", "icon": "home-flame"},
            {"value": 2, "key": "cool", "label": "AC"},
        ]));
        let mut layout = control_layout(&[("w-0001", "select", "Mode", None)]);
        layout["widgets"][0]["options"]["control_spec"] = modes;
        let created = post(
            &server,
            &env.alice,
            org,
            "/api/v1/dashboards",
            serde_json::json!({ "name": "Home", "layout": layout }),
        )
        .await;
        created.assert_status(axum_test::http::StatusCode::CREATED);
        let dash: serde_json::Value = created.json();
        let dash_id = dash["id"].as_str().unwrap().to_string();
        let id = bound_control(&dash, "w-0001");
        let list = get(&server, &env.alice, org, "/api/v1/controls").await;
        let c = control_by_id(&list, &id).expect("provisioned");
        assert_eq!(c["spec"]["kind"], "select");
        assert_eq!(c["spec"]["options"].as_array().unwrap().len(), 3);
        assert_eq!(c["spec"]["options"][2]["label"], "AC");

        // Re-save with one more option: the own control follows.
        let mut layout = control_layout(&[("w-0001", "select", "Mode", Some(&id))]);
        layout["widgets"][0]["options"]["control_spec"] = spec(serde_json::json!([
            {"value": 0, "key": "off"},
            {"value": 1, "key": "heat"},
            {"value": 2, "key": "cool"},
            {"value": 3, "key": "auto"},
        ]));
        patch(
            &server,
            &env.alice,
            org,
            &format!("/api/v1/dashboards/{dash_id}"),
            serde_json::json!({ "expected_version_number": 1, "layout": layout }),
        )
        .await
        .assert_status_ok();
        let list = get(&server, &env.alice, org, "/api/v1/controls").await;
        let c = control_by_id(&list, &id).unwrap();
        assert_eq!(c["spec"]["options"][3]["key"], "auto");

        // Invalid declared domain (duplicate keys): the save is refused.
        let mut bad = control_layout(&[("w-0001", "select", "Mode", Some(&id))]);
        bad["widgets"][0]["options"]["control_spec"] = spec(serde_json::json!([
            {"value": 0, "key": "off"},
            {"value": 1, "key": "off"},
        ]));
        let refused = patch(
            &server,
            &env.alice,
            org,
            &format!("/api/v1/dashboards/{dash_id}"),
            serde_json::json!({ "expected_version_number": 2, "layout": bad }),
        )
        .await;
        assert_eq!(refused.status_code(), 400, "{}", refused.text());

        if valkey(&ctx).await.is_none() {
            eprintln!("skip value checks: test Valkey unreachable");
            return;
        }
        let path = format!("/api/v1/controls/{id}/value");
        let off = post(
            &server,
            &env.alice,
            org,
            &path,
            serde_json::json!({"value": 7}),
        )
        .await;
        assert_eq!(off.status_code(), 400);
        assert_eq!(
            off.json::<serde_json::Value>()["errors"]["args"]["reason"],
            "not_an_option"
        );
        let ok = post(
            &server,
            &env.alice,
            org,
            &path,
            serde_json::json!({"value": 2}),
        )
        .await;
        ok.assert_status_ok();
        let stored = ok.json::<serde_json::Value>();
        assert_eq!(stored["v"], 2.0);
        assert_eq!(stored["option"], "cool");
    })
    .await;
}

/// Home card layout: one `home_card` widget with the given options and
/// sources.
fn home_layout(options: serde_json::Value, source: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "canvas": { "width": 1600, "height": 900 },
        "widgets": [{
            "id": "w-0001", "type": "home_card", "title": "Living room",
            "x": 40, "y": 40, "w": 300, "h": 200,
            "source": source, "options": { "home": options },
        }],
        "wires": [],
    })
}

/// D138: a home card provisions one control per active role (default
/// domain of the role), binds them back by role, provisions an optional
/// role once declared and releases it when undeclared; a card missing a
/// required source is refused.
#[tokio::test]
#[serial]
async fn home_card_provisions_one_control_per_active_role() {
    with_app(|server, env, _ctx| async move {
        let org = personal_org(&server, &env.alice).await;
        let created = post(
            &server,
            &env.alice,
            org,
            "/api/v1/dashboards",
            serde_json::json!({
                "name": "Home",
                "layout": home_layout(serde_json::json!({"card": "thermostat"}), serde_json::json!([])),
            }),
        )
        .await;
        created.assert_status(axum_test::http::StatusCode::CREATED);
        let dash: serde_json::Value = created.json();
        let dash_id = dash["id"].as_str().unwrap().to_string();
        let home = &dash["layout"]["widgets"][0]["options"]["home"];
        let setpoint = home["controls"]["setpoint"]["control_id"]
            .as_str()
            .expect("setpoint bound")
            .to_string();
        assert!(home["controls"].get("mode").is_none(), "optional role off");
        let list = get(&server, &env.alice, org, "/api/v1/controls").await;
        let c = control_by_id(&list, &setpoint).expect("provisioned");
        assert_eq!(c["spec"]["kind"], "stepper");
        assert_eq!(c["spec"]["min"], 5.0);
        assert_eq!(c["spec"]["step"], 0.5);
        assert_eq!(c["label"], "Living room · setpoint");
        assert_eq!(c["origin"]["item_id"], "w-0001.setpoint");

        // Declare the optional mode role: a second control appears.
        let mode_spec = serde_json::json!({"kind": "select", "options": [
            {"value": 0, "key": "off"}, {"value": 1, "key": "heat"},
        ]});
        let saved: serde_json::Value = patch(
            &server,
            &env.alice,
            org,
            &format!("/api/v1/dashboards/{dash_id}"),
            serde_json::json!({
                "expected_version_number": 1,
                "layout": home_layout(
                    serde_json::json!({
                        "card": "thermostat",
                        "controls": {"setpoint": {"control_id": setpoint}},
                        "specs": {"mode": mode_spec},
                    }),
                    serde_json::json!([]),
                ),
            }),
        )
        .await
        .json();
        let home = &saved["layout"]["widgets"][0]["options"]["home"];
        assert_eq!(home["controls"]["setpoint"]["control_id"], setpoint.as_str());
        let mode = home["controls"]["mode"]["control_id"]
            .as_str()
            .expect("mode bound")
            .to_string();
        let list = get(&server, &env.alice, org, "/api/v1/controls").await;
        assert_eq!(control_by_id(&list, &mode).unwrap()["spec"]["kind"], "select");

        // Undeclare it: released (no flow uses it).
        patch(
            &server,
            &env.alice,
            org,
            &format!("/api/v1/dashboards/{dash_id}"),
            serde_json::json!({
                "expected_version_number": 2,
                "layout": home_layout(
                    serde_json::json!({
                        "card": "thermostat",
                        "controls": {"setpoint": {"control_id": setpoint}, "mode": {"control_id": mode}},
                    }),
                    serde_json::json!([]),
                ),
            }),
        )
        .await
        .assert_status_ok();
        let list = get(&server, &env.alice, org, "/api/v1/controls").await;
        assert!(control_by_id(&list, &mode).is_none(), "undeclared role released");
        assert!(control_by_id(&list, &setpoint).is_some());

        // A binary sensor without its state source is refused.
        let refused = post(
            &server,
            &env.alice,
            org,
            "/api/v1/dashboards",
            serde_json::json!({
                "name": "Bad",
                "layout": home_layout(serde_json::json!({"card": "binary"}), serde_json::json!([])),
            }),
        )
        .await;
        assert_eq!(refused.status_code(), 400, "{}", refused.text());
    })
    .await;
}
