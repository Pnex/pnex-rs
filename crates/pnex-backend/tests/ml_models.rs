//! Vision model registry (camera-video.md D81/D82): media `model` kind,
//! CRUD validation, and the "test with an image" endpoint.

mod common;

use loco_rs::testing::request::{RequestConfig, RequestConfigBuilder};
use pnex_backend::app::App;
use serial_test::serial;

async fn with_app<F, Fut>(f: F)
where
    F: FnOnce(axum_test::TestServer, String) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let base = common::spawn_mock_rauthy().await;
    unsafe { std::env::set_var("RAUTHY_URL", &base) };
    let alice = common::valid_token(
        &base,
        "00000000-0000-0000-0000-00000000000a",
        "alice",
        "alice@example.com",
    );
    let dir = tempfile::tempdir().expect("tmp");
    unsafe { std::env::set_var("PNEX_MEDIA_DIR", dir.path()) };
    unsafe { std::env::set_var("PNEX_MEDIA_BACKEND", "fs") };
    let config: RequestConfig = RequestConfigBuilder::new().build();
    loco_rs::testing::request::request_with_config::<App, _, _>(
        config,
        move |server, _ctx| async move {
            f(server, alice).await;
        },
    )
    .await;
    unsafe { std::env::remove_var("PNEX_MEDIA_DIR") };
    unsafe { std::env::remove_var("PNEX_MEDIA_BACKEND") };
    drop(dir);
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

async fn upload(
    server: &axum_test::TestServer,
    auth: &str,
    org: i64,
    filename: &str,
    bytes: Vec<u8>,
) -> serde_json::Value {
    let res = server
        .post(&format!("/api/v1/media?filename={filename}"))
        .add_header("Authorization", format!("Bearer {auth}"))
        .add_header("X-Org-Id", org.to_string())
        .add_header("Content-Type", "application/octet-stream")
        .bytes(bytes.into())
        .await;
    assert!(res.status_code().is_success(), "upload: {}", res.text());
    res.json()
}

/// `.onnx` uploads are sniffed as `model`; a model must reference a model
/// asset; a broken ONNX answers 422 with a machine code (never a 500).
#[tokio::test]
#[serial]
async fn registry_validation_and_broken_model() {
    with_app(|server, alice| async move {
        let org = personal_org(&server, &alice).await;
        let onnx = upload(
            &server,
            &alice,
            org,
            "broken.onnx",
            b"\x08\x07not a real onnx".to_vec(),
        )
        .await;
        assert_eq!(onnx["kind"], "model");
        let photo = upload(&server, &alice, org, "p.jpg", vec![0xFF, 0xD8, 0xFF, 0xD9]).await;

        let bad = server
            .post("/api/v1/ml/models")
            .add_header("Authorization", format!("Bearer {alice}"))
            .add_header("X-Org-Id", org.to_string())
            .json(&serde_json::json!({"name": "", "asset_id": photo["id"],
                "spec": {"family": "yolox", "input_width": 100, "input_height": 416,
                         "labels": ["a"], "score_threshold": 0.3, "nms_iou": 0.4}}))
            .await;
        bad.assert_status(axum_test::http::StatusCode::BAD_REQUEST);
        let body: serde_json::Value = bad.json();
        assert_eq!(body["name"], "required");
        assert_eq!(body["asset_id"], "invalid");
        assert!(body["input_width"].is_string());

        // D100: a model that does not load is refused at registration, with
        // the loader diagnostic as the `detail` arg (never a 500, never a
        // silently broken row).
        let refused = server
            .post("/api/v1/ml/models")
            .add_header("Authorization", format!("Bearer {alice}"))
            .add_header("X-Org-Id", org.to_string())
            .json(&serde_json::json!({"name": "broken", "asset_id": onnx["id"]}))
            .await;
        refused.assert_status(axum_test::http::StatusCode::UNPROCESSABLE_ENTITY);
        let body: serde_json::Value = refused.json();
        assert_eq!(body["error"], "ml-model-invalid");
        assert!(body["errors"]["args"]["detail"]
            .as_str()
            .is_some_and(|d| !d.is_empty()));

        let inspect = server
            .get(&format!(
                "/api/v1/ml/models/inspect?asset_id={}",
                onnx["id"].as_str().unwrap()
            ))
            .add_header("Authorization", format!("Bearer {alice}"))
            .add_header("X-Org-Id", org.to_string())
            .await;
        inspect.assert_status(axum_test::http::StatusCode::UNPROCESSABLE_ENTITY);

        let list: serde_json::Value = server
            .get("/api/v1/ml/models")
            .add_header("Authorization", format!("Bearer {alice}"))
            .add_header("X-Org-Id", org.to_string())
            .await
            .json();
        assert_eq!(list["count"], 0);
    })
    .await;
}

/// D100 with the real YOLOX-nano (`PNEX_VISION_TEST_DIR` with
/// `yolox_nano.onnx`): the file's 416x416 input wins over a 640 spec, a
/// wrong label count is a field error, the check is stored with a measured
/// inference time.
#[tokio::test]
#[serial]
#[ignore = "needs PNEX_VISION_TEST_DIR with yolox_nano.onnx"]
async fn registry_imposes_the_file_input_and_checks_the_model() {
    with_app(|server, alice| async move {
        let dir = std::path::PathBuf::from(std::env::var("PNEX_VISION_TEST_DIR").expect("dir"));
        let org = personal_org(&server, &alice).await;
        let asset = upload(
            &server,
            &alice,
            org,
            "yolox_nano.onnx",
            std::fs::read(dir.join("yolox_nano.onnx")).expect("model"),
        )
        .await;
        let auth = format!("Bearer {alice}");
        let info: serde_json::Value = server
            .get(&format!("/api/v1/ml/models/inspect?asset_id={}", asset["id"].as_str().unwrap()))
            .add_header("Authorization", auth.clone())
            .add_header("X-Org-Id", org.to_string())
            .await
            .json();
        assert_eq!((info["input_width"].as_u64(), info["classes"].as_u64()), (Some(416), Some(80)));

        let wrong_labels = server
            .post("/api/v1/ml/models")
            .add_header("Authorization", auth.clone())
            .add_header("X-Org-Id", org.to_string())
            .json(&serde_json::json!({"name": "nano", "asset_id": asset["id"],
                "spec": {"family": "yolox", "input_width": 416, "input_height": 416,
                         "labels": ["a"], "score_threshold": 0.3, "nms_iou": 0.4}}))
            .await;
        wrong_labels.assert_status(axum_test::http::StatusCode::BAD_REQUEST);
        assert_eq!(wrong_labels.json::<serde_json::Value>()["labels"], "label_count:80");

        let created = server
            .post("/api/v1/ml/models")
            .add_header("Authorization", auth.clone())
            .add_header("X-Org-Id", org.to_string())
            .json(&serde_json::json!({"name": "nano", "asset_id": asset["id"],
                "spec": {"family": "yolox", "input_width": 640, "input_height": 640,
                         "labels": pnex_core::vision::coco_labels(), "score_threshold": 0.3, "nms_iou": 0.4}}))
            .await;
        created.assert_status(axum_test::http::StatusCode::CREATED);
        let model: serde_json::Value = created.json();
        assert_eq!(model["spec"]["input_width"], 416, "the file size wins");
        assert_eq!(model["check"]["status"], "valid");
        assert!(model["check"]["infer_ms"].is_u64());

        let checked: serde_json::Value = server
            .post(&format!("/api/v1/ml/models/{}/check", model["id"].as_str().unwrap()))
            .add_header("Authorization", auth.clone())
            .add_header("X-Org-Id", org.to_string())
            .await
            .json();
        assert_eq!(checked["check"]["status"], "valid");
        assert!(checked["check"]["checked_at"].is_string());
    })
    .await;
}

/// Real YOLOX-nano through the API (`PNEX_VISION_TEST_DIR` with
/// `yolox_nano.onnx` + `dog.jpg`).
#[tokio::test]
#[serial]
#[ignore = "needs PNEX_VISION_TEST_DIR with yolox_nano.onnx + dog.jpg"]
async fn registry_detects_with_a_real_model() {
    with_app(|server, alice| async move {
        let dir = std::path::PathBuf::from(std::env::var("PNEX_VISION_TEST_DIR").expect("dir"));
        let org = personal_org(&server, &alice).await;
        let asset = upload(
            &server,
            &alice,
            org,
            "yolox_nano.onnx",
            std::fs::read(dir.join("yolox_nano.onnx")).expect("model"),
        )
        .await;
        let model: serde_json::Value = server
            .post("/api/v1/ml/models")
            .add_header("Authorization", format!("Bearer {alice}"))
            .add_header("X-Org-Id", org.to_string())
            .json(&serde_json::json!({"name": "yolox-nano", "asset_id": asset["id"]}))
            .await
            .json();
        let res = server
            .post(&format!(
                "/api/v1/ml/models/{}/test",
                model["id"].as_str().unwrap()
            ))
            .add_header("Authorization", format!("Bearer {alice}"))
            .add_header("X-Org-Id", org.to_string())
            .bytes(std::fs::read(dir.join("dog.jpg")).expect("image").into())
            .await;
        res.assert_status_ok();
        let body: serde_json::Value = res.json();
        let labels: Vec<&str> = body["detections"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|d| d["label"].as_str())
            .collect();
        assert!(labels.contains(&"dog"), "{labels:?}");
        assert!(labels.contains(&"bicycle"), "{labels:?}");
        assert_eq!(body["width"], 768);
    })
    .await;
}
