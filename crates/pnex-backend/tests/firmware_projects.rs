//! Custom firmware projects (custom-firmware.md D89–D94): versioned CRUD
//! (append-only revisions, hash dedup, optimistic 409), sketch guard and
//! library catalog 400s, org isolation (masked 404), compile gate (409 when
//! custom firmware is disabled), device attachment (chip guard, delete 409).
//!
//! Requires PostgreSQL (TEST_DATABASE_URL) — database reset between tests.

mod common;

use loco_rs::testing::request::{RequestConfig, RequestConfigBuilder};
use pnex_backend::app::App;
use serde_json::json;
use serial_test::serial;

struct Env {
    alice: String,
    bob: String,
}

async fn with_app<F, Fut>(f: F)
where
    F: FnOnce(axum_test::TestServer, Env) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let base = common::spawn_mock_rauthy().await;
    let _ = tracing_subscriber::fmt().with_test_writer().try_init();
    unsafe { std::env::set_var("RAUTHY_URL", &base) };
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
            f(server, env).await;
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

struct Client<'a> {
    server: &'a axum_test::TestServer,
    token: String,
    org: i64,
}

impl Client<'_> {
    async fn get(&self, path: &str) -> axum_test::TestResponse {
        self.server
            .get(path)
            .add_header("Authorization", format!("Bearer {}", self.token))
            .add_header("X-Org-Id", self.org.to_string())
            .await
    }
    async fn send(
        &self,
        method: &str,
        path: &str,
        body: serde_json::Value,
    ) -> axum_test::TestResponse {
        let req = match method {
            "POST" => self.server.post(path),
            "PATCH" => self.server.patch(path),
            "PUT" => self.server.put(path),
            _ => self.server.delete(path),
        };
        req.add_header("Authorization", format!("Bearer {}", self.token))
            .add_header("X-Org-Id", self.org.to_string())
            .add_header("Content-Type", "application/json")
            .json(&body)
            .await
    }
}

#[tokio::test]
#[serial]
async fn project_crud_and_revisions() {
    with_app(|server, env| async move {
        let alice = Client { org: personal_org(&server, &env.alice).await, token: env.alice.clone(), server: &server };

        // Catalog is served.
        let cat = alice.get("/api/v1/firmware-projects/lib-catalog").await;
        cat.assert_status_ok();
        assert!(cat.json::<serde_json::Value>()["results"].as_array().unwrap().iter().any(|e| e["id"] == "bme280"));

        // Create: starter sketch, revision 1.
        let res = alice.send("POST", "/api/v1/firmware-projects", json!({"name": "Greenhouse", "chip_family": "esp32"})).await;
        res.assert_status(axum_test::http::StatusCode::CREATED);
        let p: serde_json::Value = res.json();
        let id = p["id"].as_i64().unwrap();
        assert_eq!(p["current_revision_number"], 1);
        assert!(p["main_cpp"].as_str().unwrap().contains("pnex.loop()"));

        // New content + library → revision 2.
        let res = alice
            .send(
                "PATCH",
                &format!("/api/v1/firmware-projects/{id}"),
                json!({"expected_revision_number": 1, "main_cpp": "#include <Pnex.h>\nvoid setup(){}\nvoid loop(){}\n", "lib_deps": ["bme280"], "note": "bme"}),
            )
            .await;
        res.assert_status_ok();
        assert_eq!(res.json::<serde_json::Value>()["current_revision_number"], 2);

        // Same content → no new revision; metadata still applied.
        let res = alice
            .send("PATCH", &format!("/api/v1/firmware-projects/{id}"), json!({"expected_revision_number": 2, "name": "Greenhouse v2"}))
            .await;
        res.assert_status_ok();
        let body: serde_json::Value = res.json();
        assert_eq!((body["current_revision_number"].as_i64(), body["name"].as_str()), (Some(2), Some("Greenhouse v2")));
        assert_eq!(body["lib_deps"], json!(["bme280"]));

        // Stale expected revision → 409.
        let res = alice
            .send("PATCH", &format!("/api/v1/firmware-projects/{id}"), json!({"expected_revision_number": 1, "main_cpp": "x"}))
            .await;
        res.assert_status(axum_test::http::StatusCode::CONFLICT);

        // Guard: absolute include refused with per-line violations.
        let res = alice
            .send(
                "PATCH",
                &format!("/api/v1/firmware-projects/{id}"),
                json!({"expected_revision_number": 2, "main_cpp": "void a();\n#include \"/etc/passwd\"\n"}),
            )
            .await;
        res.assert_status_bad_request();
        let body: serde_json::Value = res.json();
        assert_eq!(body["error"], "firmware-source-refused");
        assert_eq!(body["violations"][0], json!({"line": 2, "code": "firmware-include-absolute"}));

        // Catalog: unknown / chip-incompatible library.
        let res = alice
            .send("PATCH", &format!("/api/v1/firmware-projects/{id}"), json!({"expected_revision_number": 2, "lib_deps": ["nope"]}))
            .await;
        assert_eq!(res.json::<serde_json::Value>()["error"], "firmware-lib-unknown");
        let res = alice
            .send("POST", "/api/v1/firmware-projects", json!({"name": "Servo", "chip_family": "esp8266", "lib_deps": ["esp32servo"]}))
            .await;
        assert_eq!(res.json::<serde_json::Value>()["error"], "firmware-lib-incompatible");
        let res = alice.send("POST", "/api/v1/firmware-projects", json!({"name": " ", "chip_family": "esp32"})).await;
        assert_eq!(res.json::<serde_json::Value>(), json!({"name": "required"}));
        let res = alice.send("POST", "/api/v1/firmware-projects", json!({"name": "x", "chip_family": "avr"})).await;
        assert_eq!(res.json::<serde_json::Value>(), json!({"chip_family": "invalid"}));

        // History: 2 revisions, newest first, full content.
        let hist: serde_json::Value = alice.get(&format!("/api/v1/firmware-projects/{id}/revisions")).await.json();
        assert_eq!(hist["count"], 2);
        assert_eq!(hist["results"][0]["revision_number"], 2);
        assert_eq!(hist["results"][0]["note"], "bme");
        assert!(hist["results"][1]["main_cpp"].as_str().unwrap().contains("pnex.loop()"));

        // List.
        let list: serde_json::Value = alice.get("/api/v1/firmware-projects").await.json();
        assert_eq!(list["count"], 1);
        assert_eq!(list["results"][0]["current_revision_number"], 2);

        // Org isolation: bob sees a masked 404.
        let bob = Client { org: personal_org(&server, &env.bob).await, token: env.bob.clone(), server: &server };
        bob.get(&format!("/api/v1/firmware-projects/{id}")).await.assert_status_not_found();

        // Compile gate: custom firmware is off by default (D91).
        let res = alice.send("POST", &format!("/api/v1/firmware-projects/{id}/check"), json!({})).await;
        res.assert_status(axum_test::http::StatusCode::CONFLICT);
        assert_eq!(res.json::<serde_json::Value>()["error"], "firmware-custom-disabled");

        // Delete.
        alice.send("DELETE", &format!("/api/v1/firmware-projects/{id}"), json!({})).await.assert_status(axum_test::http::StatusCode::NO_CONTENT);
        alice.get(&format!("/api/v1/firmware-projects/{id}")).await.assert_status_not_found();
    })
    .await;
}

#[tokio::test]
#[serial]
async fn provisioning_picks_a_custom_firmware() {
    with_app(|server, env| async move {
        let alice = Client {
            org: personal_org(&server, &env.alice).await,
            token: env.alice.clone(),
            server: &server,
        };
        let esp32 = alice
            .send("POST", "/api/v1/firmware-projects", json!({"name": "A", "chip_family": "esp32"}))
            .await;
        let esp32_id = esp32.json::<serde_json::Value>()["id"].as_i64().unwrap();
        let esp8266 = alice
            .send("POST", "/api/v1/firmware-projects", json!({"name": "B", "chip_family": "esp8266"}))
            .await;
        let esp8266_id = esp8266.json::<serde_json::Value>()["id"].as_i64().unwrap();

        // Chip mismatch refused at provisioning.
        let res = alice
            .send(
                "POST",
                "/api/v1/devices",
                json!({"device_id": "fw-a", "predefined_device_name": "generic_esp8266", "firmware_project_id": esp32_id}),
            )
            .await;
        res.assert_status_bad_request();
        assert_eq!(res.json::<serde_json::Value>()["error"], "firmware-chip-mismatch");

        // A predefined board keeps the firmware maintained by PneX.
        let res = alice
            .send(
                "POST",
                "/api/v1/devices",
                json!({"device_id": "fw-soil", "predefined_device_name": "soil_sensor", "firmware_project_id": esp8266_id}),
            )
            .await;
        res.assert_status_bad_request();
        assert_eq!(res.json::<serde_json::Value>()["error"], "firmware-family-locked");

        // Another org's project is not usable.
        let bob = Client {
            org: personal_org(&server, &env.bob).await,
            token: env.bob.clone(),
            server: &server,
        };
        let res = bob
            .send(
                "POST",
                "/api/v1/devices",
                json!({"device_id": "fw-b", "predefined_device_name": "generic_esp8266", "firmware_project_id": esp8266_id}),
            )
            .await;
        assert_eq!(res.json::<serde_json::Value>(), json!({"firmware_project_id": "invalid"}));

        // Compatible project: the device carries it; the project is in use.
        let res = alice
            .send(
                "POST",
                "/api/v1/devices",
                json!({"device_id": "fw-c", "predefined_device_name": "generic_esp8266", "firmware_project_id": esp8266_id}),
            )
            .await;
        res.assert_status(axum_test::http::StatusCode::CREATED);
        assert_eq!(res.json::<serde_json::Value>()["firmware_project_id"], esp8266_id);
        let list: serde_json::Value = alice.get("/api/v1/firmware-projects").await.json();
        let b = list["results"].as_array().unwrap().iter().find(|p| p["id"] == esp8266_id).cloned().unwrap();
        assert_eq!(b["device_count"], 1);
        let res = alice
            .send("DELETE", &format!("/api/v1/firmware-projects/{esp8266_id}"), json!({}))
            .await;
        res.assert_status(axum_test::http::StatusCode::CONFLICT);
        assert_eq!(res.json::<serde_json::Value>()["error"], "firmware-project-in-use");
    })
    .await;
}
