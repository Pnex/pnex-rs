//! Vision Lab (media-vision-studio.md D198–D200): project → images of the
//! media library → boxes (optimistic 409) → frozen version → COCO zip;
//! org isolation (R1) and viewer read-only (R2).
//!
//! Needs PostgreSQL (TEST_DATABASE_URL), database emptied between tests.

mod common;

use base64::Engine as _;
use loco_rs::testing::request::{RequestConfig, RequestConfigBuilder};
use pnex_backend::app::App;
use serde_json::{json, Value};
use serial_test::serial;

/// 1×1 PNG.
const PNG_1X1: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==";

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
    unsafe {
        std::env::set_var(
            "PNEX_MEDIA_DIR",
            format!(
                "{}/pnex-lab-tests-{}",
                std::env::temp_dir().display(),
                std::process::id()
            ),
        )
    };
    unsafe { std::env::set_var("PNEX_MEDIA_BACKEND", "fs") };
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
        move |server, _ctx| async move {
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
        .json::<Value>()["orgs"][0]["id"]
        .as_i64()
        .expect("personal org")
}

async fn call(
    server: &axum_test::TestServer,
    method: &str,
    path: &str,
    token: &str,
    org: i64,
    body: Option<Value>,
) -> (u16, Value) {
    let req = match method {
        "GET" => server.get(path),
        "POST" => server.post(path),
        "PUT" => server.put(path),
        "DELETE" => server.delete(path),
        _ => unreachable!(),
    }
    .add_header("Authorization", format!("Bearer {token}"))
    .add_header("X-Org-Id", org.to_string());
    let res = match body {
        Some(b) => req.json(&b).await,
        None => req.await,
    };
    let status = res.status_code().as_u16();
    (
        status,
        serde_json::from_str(&res.text()).unwrap_or(Value::Null),
    )
}

async fn upload_png(server: &axum_test::TestServer, token: &str, org: i64, name: &str) -> String {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(PNG_1X1)
        .unwrap();
    let res = server
        .post(&format!(
            "/api/v1/media?name={name}&filename={name}.png&content_type=image%2Fpng"
        ))
        .add_header("Authorization", format!("Bearer {token}"))
        .add_header("X-Org-Id", org.to_string())
        .add_header("Content-Type", "application/octet-stream")
        .bytes(bytes.into())
        .await;
    assert_eq!(res.status_code(), 201, "{}", res.text());
    res.json::<Value>()["id"].as_str().unwrap().to_string()
}

#[tokio::test]
#[serial]
async fn project_images_boxes_versions_export() {
    with_app(|server, env| async move {
        let org = personal_org(&server, &env.alice).await;
        let a = &env.alice;
        let (st, bad) = call(&server, "POST", "/api/v1/lab/projects", a, org, Some(json!({"name": " "}))).await;
        assert_eq!(st, 400);
        assert_eq!(bad["name"], "required");
        let (st, p) = call(
            &server,
            "POST",
            "/api/v1/lab/projects",
            a,
            org,
            Some(json!({"name": "Valves", "classes": ["red_valve", "helmet"]})),
        )
        .await;
        assert_eq!(st, 201, "{p}");
        let pid = p["id"].as_str().unwrap().to_string();

        let img1 = upload_png(&server, a, org, "v1").await;
        let img2 = upload_png(&server, a, org, "v2").await;
        let (st, added) = call(
            &server,
            "POST",
            &format!("/api/v1/lab/projects/{pid}/images"),
            a,
            org,
            Some(json!({"asset_ids": [img1, img2]})),
        )
        .await;
        assert_eq!(st, 200, "{added}");
        assert_eq!(added.as_array().unwrap().len(), 2);
        assert_eq!(added[0]["width"], 1);
        assert_eq!(added[0]["source"], added[1]["source"], "one import = one source");
        // Adding the same picture again is a no-op.
        let (_, again) = call(
            &server,
            "POST",
            &format!("/api/v1/lab/projects/{pid}/images"),
            a,
            org,
            Some(json!({"asset_ids": [img1]})),
        )
        .await;
        assert_eq!(again, json!([]));

        let image = added[0]["id"].as_str().unwrap().to_string();
        let path = format!("/api/v1/lab/images/{image}/annotations");
        let boxes = json!({"boxes": [{"class": 1, "bbox": [0.1, 0.1, 0.5, 0.5]}], "negative": false, "split": "auto", "expected_version": 1});
        let (st, saved) = call(&server, "PUT", &path, a, org, Some(boxes.clone())).await;
        assert_eq!(st, 200, "{saved}");
        assert_eq!(saved["version"], 2);
        let (st, stale) = call(&server, "PUT", &path, a, org, Some(boxes)).await;
        assert_eq!(st, 409);
        assert_eq!(stale["error"], "lab-annotations-conflict");
        let (st, out) = call(
            &server,
            "PUT",
            &path,
            a,
            org,
            Some(json!({"boxes": [{"class": 5, "bbox": [0.1, 0.1, 0.5, 0.5]}], "expected_version": 2})),
        )
        .await;
        assert_eq!(st, 400);
        assert_eq!(out["error"], "lab-box-invalid");
        // The second picture: a negative example.
        let neg = added[1]["id"].as_str().unwrap();
        let (st, _) = call(
            &server,
            "PUT",
            &format!("/api/v1/lab/images/{neg}/annotations"),
            a,
            org,
            Some(json!({"boxes": [], "negative": true, "expected_version": 1})),
        )
        .await;
        assert_eq!(st, 200);

        // "helmet" (index 1) has a box: it cannot be removed.
        let (st, inuse) = call(
            &server,
            "PUT",
            &format!("/api/v1/lab/projects/{pid}"),
            a,
            org,
            Some(json!({"name": "Valves", "classes": ["red_valve"]})),
        )
        .await;
        assert_eq!(st, 409);
        assert_eq!(inuse["error"], "lab-class-in-use");

        let (_, stats) = call(&server, "GET", &format!("/api/v1/lab/projects/{pid}/stats"), a, org, None).await;
        assert_eq!(stats["annotated"], 2);
        assert_eq!(stats["negatives"], 1);
        assert_eq!(stats["boxes_per_class"], json!([0, 1]));

        let (st, v) = call(
            &server,
            "POST",
            &format!("/api/v1/lab/projects/{pid}/versions"),
            a,
            org,
            Some(json!({"note": "first"})),
        )
        .await;
        assert_eq!(st, 201, "{v}");
        assert_eq!(v["version"], 1);
        let (_, list) = call(&server, "GET", "/api/v1/lab/projects", a, org, None).await;
        assert_eq!(list[0]["version_count"], 1);
        assert_eq!(list[0]["annotated_count"], 2);

        let res = server
            .get(&format!("/api/v1/lab/versions/{}/export", v["id"].as_str().unwrap()))
            .add_header("Authorization", format!("Bearer {a}"))
            .add_header("X-Org-Id", org.to_string())
            .await;
        assert_eq!(res.status_code(), 200);
        let zip = zip::ZipArchive::new(std::io::Cursor::new(res.as_bytes().to_vec())).expect("zip");
        let names: Vec<String> = zip.file_names().map(str::to_string).collect();
        assert!(names.iter().any(|n| n == "annotations/train.json"), "{names:?}");
        assert!(names.iter().any(|n| n == "classes.txt"));
        assert_eq!(names.iter().filter(|n| n.starts_with("images/")).count(), 2);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn org_isolation_and_viewer_read_only() {
    with_app(|server, env| async move {
        let org_a = personal_org(&server, &env.alice).await;
        let org_b = personal_org(&server, &env.bob).await;
        let (_, p) = call(
            &server,
            "POST",
            "/api/v1/lab/projects",
            &env.alice,
            org_a,
            Some(json!({"name": "Helmets", "classes": ["helmet"]})),
        )
        .await;
        let pid = p["id"].as_str().unwrap().to_string();
        // Bob in his own org: alice's project does not exist.
        let (st, e) = call(
            &server,
            "GET",
            &format!("/api/v1/lab/projects/{pid}"),
            &env.bob,
            org_b,
            None,
        )
        .await;
        assert_eq!(st, 404);
        assert_eq!(e["error"], "lab-project-not-found");

        // Bob as a viewer of alice's org: reads, never writes.
        let (st, _) = call(
            &server,
            "POST",
            &format!("/api/v1/orgs/{org_a}/members"),
            &env.alice,
            org_a,
            Some(json!({"email": "bob@example.com", "role": "viewer"})),
        )
        .await;
        assert!(st < 300, "member add: {st}");
        let (st, _) = call(
            &server,
            "GET",
            &format!("/api/v1/lab/projects/{pid}"),
            &env.bob,
            org_a,
            None,
        )
        .await;
        assert_eq!(st, 200);
        let writes = [
            (
                "POST",
                "/api/v1/lab/projects".to_string(),
                Some(json!({"name": "x"})),
            ),
            (
                "PUT",
                format!("/api/v1/lab/projects/{pid}"),
                Some(json!({"name": "x"})),
            ),
            ("DELETE", format!("/api/v1/lab/projects/{pid}"), None),
            (
                "POST",
                format!("/api/v1/lab/projects/{pid}/images"),
                Some(json!({"asset_ids": []})),
            ),
            (
                "POST",
                format!("/api/v1/lab/projects/{pid}/versions"),
                Some(json!({})),
            ),
            (
                "PUT",
                "/api/v1/lab/images/00000000-0000-0000-0000-000000000001/annotations".to_string(),
                Some(json!({"boxes": [], "expected_version": 1})),
            ),
            (
                "DELETE",
                "/api/v1/lab/images/00000000-0000-0000-0000-000000000001".to_string(),
                None,
            ),
        ];
        for (m, path, body) in writes {
            let (st, e) = call(&server, m, &path, &env.bob, org_a, body).await;
            assert_eq!(st, 403, "{m} {path}");
            assert_eq!(e["error"], "lab-write-forbidden");
        }
    })
    .await;
}
