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
        // Pre-annotation needs a detection model of the org.
        let (st, bad) = call(
            &server,
            "POST",
            &format!("/api/v1/lab/images/{image}/suggest"),
            a,
            org,
            Some(json!({"model_id": "00000000-0000-0000-0000-000000000009"})),
        )
        .await;
        assert_eq!(st, 400, "{bad}");
        assert_eq!(bad["model_id"], "invalid");
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
            (
                "POST",
                "/api/v1/lab/images/00000000-0000-0000-0000-000000000001/suggest".to_string(),
                Some(json!({"model_id": "x"})),
            ),
            (
                "POST",
                "/api/v1/lab/images/00000000-0000-0000-0000-000000000001/interpolate".to_string(),
                None,
            ),
            (
                "POST",
                "/api/v1/lab/versions/00000000-0000-0000-0000-000000000001/train".to_string(),
                Some(json!({"base_model_id": "x"})),
            ),
            (
                "POST",
                "/api/v1/lab/runs/00000000-0000-0000-0000-000000000001/cancel".to_string(),
                None,
            ),
            (
                "POST",
                "/api/v1/lab/images/00000000-0000-0000-0000-000000000001/propagate".to_string(),
                Some(json!({"model_id": "x"})),
            ),
            (
                "POST",
                format!("/api/v1/lab/projects/{pid}/import-video"),
                Some(json!({"asset_id": "x", "from_secs": 0, "to_secs": 10, "every_secs": 2})),
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

/// Frames of a camera recording become pictures of the project (D202):
/// one per interval inside the recorded spans, all of one source, each a
/// photo of the media library.
#[tokio::test]
#[serial]
async fn frames_imported_from_a_recording() {
    with_app(|server, env| async move {
        unsafe { std::env::set_var("PNEX_FLOW_RUNTIME_TOKEN", "tok-lab") };
        let org = personal_org(&server, &env.alice).await;
        let a = &env.alice;
        let res = server
            .post("/api/v1/devices")
            .add_header("Authorization", format!("Bearer {a}"))
            .add_header("X-Org-Id", org.to_string())
            .json(&json!({"device_id": "cam-lab", "predefined_device_name": "generic_esp8266"}))
            .await;
        assert_eq!(res.status_code(), 201, "{}", res.text());
        let device = res.json::<Value>()["id"].as_i64().unwrap();
        let png = base64::engine::general_purpose::STANDARD.decode(PNG_1X1).unwrap();
        let jpeg = pnex_vision::prepare_jpeg(&png, None, 64).unwrap();
        // 10 s of recording, 5 frames, then a 10 s gap, then 10 s more.
        let base = 1_790_000_000_000i64;
        for start in [base, base + 20_000] {
            let mut w = pnex_core::avi::AviWriter::new(1, 1);
            for _ in 0..5 {
                w.push(jpeg.clone());
            }
            let end = start + 10_000;
            server
                .post(&format!(
                    "/internal/flow/video-segment?org_id={org}&device_id=cam-lab&node_id=n1&stream=front\
                     &started_ms={start}&ended_ms={end}&frames=5&width=1&height=1&retention_days=7"
                ))
                .add_header("x-pnex-flow-token", "tok-lab")
                .bytes(w.finish(0.5).into())
                .await
                .assert_status_ok();
        }
        let (_, p) = call(
            &server,
            "POST",
            "/api/v1/lab/projects",
            a,
            org,
            Some(json!({"name": "Dock", "classes": ["forklift"]})),
        )
        .await;
        let pid = p["id"].as_str().unwrap().to_string();
        let at = |ms: i64| chrono::DateTime::from_timestamp_millis(ms).unwrap().to_rfc3339();
        let path = format!("/api/v1/lab/projects/{pid}/import-recording");
        let (st, too_many) = call(
            &server,
            "POST",
            &path,
            a,
            org,
            Some(json!({"device": device, "from": at(base), "to": at(base + 3_600_000), "every_secs": 1.0})),
        )
        .await;
        assert_eq!(st, 400);
        assert_eq!(too_many["every_secs"], "max_frames:200");
        let (st, imported) = call(
            &server,
            "POST",
            &path,
            a,
            org,
            Some(json!({"device": device, "from": at(base), "to": at(base + 30_000), "every_secs": 2.0})),
        )
        .await;
        assert_eq!(st, 200, "{imported}");
        // 0, 2, 4, 6, 8 s in the first segment, 20…28 s in the second.
        let list = imported.as_array().unwrap();
        assert_eq!(list.len(), 10, "{imported}");
        assert!(list.iter().all(|i| i["source"] == list[0]["source"]));
        let (_, media) = call(&server, "GET", "/api/v1/media?kind=photo", a, org, None).await;
        assert_eq!(media["count"], 10);
        let (st, none) = call(
            &server,
            "POST",
            &path,
            a,
            org,
            Some(json!({"device": device, "from": at(base + 11_000), "to": at(base + 19_000), "every_secs": 2.0})),
        )
        .await;
        assert_eq!(st, 404);
        assert_eq!(none["error"], "camera-export-empty");
        unsafe { std::env::remove_var("PNEX_FLOW_RUNTIME_TOKEN") };
    })
    .await;
}

#[tokio::test]
#[serial]
async fn frames_of_a_video_and_box_interpolation() {
    if std::process::Command::new("ffmpeg")
        .arg("-version")
        .output()
        .is_err()
    {
        eprintln!("ffmpeg missing: skipped");
        return;
    }
    // 10 s H.264 MP4, moov at the end (phone layout).
    let dir = std::env::temp_dir().join(format!("pnex-lab-video-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let clip = dir.join("clip.mp4");
    assert!(std::process::Command::new("ffmpeg")
        .args(["-y", "-loglevel", "error", "-f", "lavfi", "-i"])
        .arg("testsrc=duration=10:size=320x240:rate=10")
        .args(["-c:v", "libx264", "-pix_fmt", "yuv420p", "-f", "mp4"])
        .arg(&clip)
        .status()
        .unwrap()
        .success());
    let mp4 = std::fs::read(&clip).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    with_app(|server, env| async move {
        let org = personal_org(&server, &env.alice).await;
        let a = &env.alice;
        let res = server
            .post("/api/v1/media?name=clip&filename=clip.mp4&content_type=video%2Fmp4")
            .add_header("Authorization", format!("Bearer {a}"))
            .add_header("X-Org-Id", org.to_string())
            .add_header("Content-Type", "application/octet-stream")
            .bytes(mp4.into())
            .await;
        assert_eq!(res.status_code(), 201, "{}", res.text());
        let video = res.json::<Value>()["id"].as_str().unwrap().to_string();
        let still = upload_png(&server, a, org, "still").await;
        let (_, p) = call(
            &server,
            "POST",
            "/api/v1/lab/projects",
            a,
            org,
            Some(json!({"name": "Yard", "classes": ["cart"]})),
        )
        .await;
        let pid = p["id"].as_str().unwrap().to_string();
        let path = format!("/api/v1/lab/projects/{pid}/import-video");
        let (st, err) = call(
            &server,
            "POST",
            &path,
            a,
            org,
            Some(json!({"asset_id": still, "from_secs": 0, "to_secs": 10, "every_secs": 2})),
        )
        .await;
        assert_eq!(st, 422, "{err}");
        assert_eq!(err["error"], "lab-video-unsupported");
        let (st, imported) = call(
            &server,
            "POST",
            &path,
            a,
            org,
            Some(json!({"asset_id": video, "from_secs": 0, "to_secs": 10, "every_secs": 2})),
        )
        .await;
        assert_eq!(st, 200, "{imported}");
        let list = imported.as_array().unwrap();
        let at: Vec<i64> = list.iter().map(|i| i["frame_ms"].as_i64().unwrap()).collect();
        assert_eq!(at, [0, 2000, 4000, 6000, 8000]);
        assert_eq!(list[0]["width"], 320);
        assert!(list.iter().all(|i| i["source"] == format!("video-{video}")));
        let id = |i: usize| list[i]["id"].as_str().unwrap().to_string();
        // Nothing annotated before the first frame: no propagation.
        let (st, err) = call(
            &server,
            "POST",
            &format!("/api/v1/lab/images/{}/propagate", id(0)),
            a,
            org,
            Some(json!({"model_id": "00000000-0000-0000-0000-000000000000"})),
        )
        .await;
        assert_eq!(st, 422, "{err}");
        assert_eq!(err["error"], "lab-propagate-no-previous");
        let interp = format!("/api/v1/lab/images/{}/interpolate", id(2));
        let (st, err) = call(&server, "POST", &interp, a, org, None).await;
        assert_eq!(st, 422);
        assert_eq!(err["error"], "lab-interpolate-no-keyframes");
        for (i, x) in [(0, 0.1), (4, 0.5)] {
            let (st, saved) = call(
                &server,
                "PUT",
                &format!("/api/v1/lab/images/{}/annotations", id(i)),
                a,
                org,
                Some(json!({"boxes": [{"class": 0, "bbox": [x, 0.2, 0.2, 0.2]}], "expected_version": 1})),
            )
            .await;
            assert_eq!(st, 200, "{saved}");
        }
        let (st, sug) = call(&server, "POST", &interp, a, org, None).await;
        assert_eq!(st, 200, "{sug}");
        let x = sug[0]["bbox"][0].as_f64().unwrap();
        assert!((x - 0.3).abs() < 1e-4, "{sug}");
        assert_eq!(sug[0]["class"], 0);
        assert_eq!(sug[0]["label"], "");
        // A still picture has no position: nothing to interpolate.
        let (_, added) = call(
            &server,
            "POST",
            &format!("/api/v1/lab/projects/{pid}/images"),
            a,
            org,
            Some(json!({"asset_ids": [still]})),
        )
        .await;
        let sid = added[0]["id"].as_str().unwrap();
        let (st, _) = call(
            &server,
            "POST",
            &format!("/api/v1/lab/images/{sid}/interpolate"),
            a,
            org,
            None,
        )
        .await;
        assert_eq!(st, 422);
    })
    .await;
}

/// Synthetic picture: an orange square on a noisy background; returns the
/// PNG and the normalized box.
fn square_png(seed: u64) -> (Vec<u8>, [f32; 4]) {
    let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    let mut next = |n: u32| {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        (x % u64::from(n)) as u32
    };
    let (w, h) = (320u32, 240u32);
    let mut img = image::RgbImage::new(w, h);
    for p in img.pixels_mut() {
        let n = (next(60) + 60) as u8;
        *p = image::Rgb([n, n, n.saturating_add(20)]);
    }
    let side = 30 + next(60);
    let (x0, y0) = (next(w - side), next(h - side));
    for yy in y0..y0 + side {
        for xx in x0..x0 + side {
            img.put_pixel(xx, yy, image::Rgb([240, 120, 20]));
        }
    }
    let mut png = Vec::new();
    image::DynamicImage::ImageRgb8(img)
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .unwrap();
    let b = [
        x0 as f32 / w as f32,
        y0 as f32 / h as f32,
        side as f32 / w as f32,
        side as f32 / h as f32,
    ];
    (png, b)
}

/// Head fine-tuning end to end (D204): base YOLOX registered, synthetic
/// pictures annotated, version frozen, training queued, polled until the
/// new model (one class) is in the registry. Needs `PNEX_VISION_TEST_DIR`
/// with `yolox_nano.onnx`.
#[tokio::test]
#[serial]
async fn training_registers_a_fine_tuned_model() {
    let Ok(dir) = std::env::var("PNEX_VISION_TEST_DIR") else {
        eprintln!("PNEX_VISION_TEST_DIR missing: skipped");
        return;
    };
    let onnx = std::fs::read(std::path::Path::new(&dir).join("yolox_nano.onnx")).unwrap();
    with_app(|server, env| async move {
        let org = personal_org(&server, &env.alice).await;
        let a = &env.alice;
        let upload = |name: String, filename: String, ct: &'static str, bytes: Vec<u8>| {
            let server = &server;
            async move {
                let res = server
                    .post(&format!(
                        "/api/v1/media?name={name}&filename={filename}&content_type={ct}"
                    ))
                    .add_header("Authorization", format!("Bearer {a}"))
                    .add_header("X-Org-Id", org.to_string())
                    .add_header("Content-Type", "application/octet-stream")
                    .bytes(bytes.into())
                    .await;
                assert_eq!(res.status_code(), 201, "{}", res.text());
                res.json::<Value>()["id"].as_str().unwrap().to_string()
            }
        };
        let base_asset = upload(
            "nano".into(),
            "yolox_nano.onnx".into(),
            "application%2Foctet-stream",
            onnx,
        )
        .await;
        let (st, base) = call(
            &server,
            "POST",
            "/api/v1/ml/models",
            a,
            org,
            Some(json!({"name": "COCO nano", "asset_id": base_asset})),
        )
        .await;
        assert_eq!(st, 201, "{base}");
        let (_, p) = call(
            &server,
            "POST",
            "/api/v1/lab/projects",
            a,
            org,
            Some(json!({"name": "Squares", "classes": ["square"]})),
        )
        .await;
        let pid = p["id"].as_str().unwrap().to_string();
        let mut assets = Vec::new();
        let mut truth = Vec::new();
        for i in 0..12u64 {
            let (png, b) = square_png(i + 1);
            assets.push(upload(format!("sq{i}"), format!("sq{i}.png"), "image%2Fpng", png).await);
            truth.push(b);
        }
        let (st, added) = call(
            &server,
            "POST",
            &format!("/api/v1/lab/projects/{pid}/images"),
            a,
            org,
            Some(json!({"asset_ids": assets})),
        )
        .await;
        assert_eq!(st, 200, "{added}");
        for img in added.as_array().unwrap() {
            let i = assets
                .iter()
                .position(|x| x == img["asset_id"].as_str().unwrap())
                .unwrap();
            let (st, saved) = call(
                &server,
                "PUT",
                &format!("/api/v1/lab/images/{}/annotations", img["id"].as_str().unwrap()),
                a,
                org,
                Some(json!({"boxes": [{"class": 0, "bbox": truth[i]}], "split": "train", "expected_version": 1})),
            )
            .await;
            assert_eq!(st, 200, "{saved}");
        }
        let (st, v) = call(
            &server,
            "POST",
            &format!("/api/v1/lab/projects/{pid}/versions"),
            a,
            org,
            Some(json!({})),
        )
        .await;
        assert_eq!(st, 201, "{v}");
        let train = format!("/api/v1/lab/versions/{}/train", v["id"].as_str().unwrap());
        let body = json!({"base_model_id": base["id"], "epochs": 5});
        let (st, run) = call(&server, "POST", &train, a, org, Some(body.clone())).await;
        assert_eq!(st, 201, "{run}");
        assert_eq!(run["status"], "queued");
        let (st, busy) = call(&server, "POST", &train, a, org, Some(body)).await;
        assert_eq!(st, 409, "{busy}");
        assert_eq!(busy["error"], "lab-train-busy");
        let runs = format!("/api/v1/lab/projects/{pid}/runs");
        let mut last = Value::Null;
        for _ in 0..240 {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            let (_, list) = call(&server, "GET", &runs, a, org, None).await;
            last = list[0].clone();
            if !["queued", "running"].contains(&last["status"].as_str().unwrap_or("")) {
                break;
            }
        }
        assert_eq!(last["status"], "succeeded", "{last}");
        let model = last["model_id"].as_str().unwrap();
        let (st, m) = call(&server, "GET", &format!("/api/v1/ml/models/{model}"), a, org, None).await;
        assert_eq!(st, 200, "{m}");
        assert_eq!(m["spec"]["labels"], json!(["square"]), "{m}");
        assert_eq!(m["check"]["status"], "valid", "{m}");
    })
    .await;
}

/// Active learning (M4): a doubtful frame posted by a detection node joins
/// the review queue with its proposals (class mapped by name), leaves it
/// once annotated; foreign frame keys and missing tokens are refused.
#[tokio::test]
#[serial]
async fn doubtful_frames_join_the_review_queue() {
    let url = std::env::var("PNEX_TEST_APP_VALKEY_URL")
        .unwrap_or_else(|_| "redis://127.0.0.1:6379/14".to_string());
    let Ok(mut valkey) = redis::Client::open(url).and_then(|c| c.get_connection()) else {
        eprintln!("valkey unreachable: skipped");
        return;
    };
    with_app(|server, env| async move {
        let org = personal_org(&server, &env.alice).await;
        let a = &env.alice;
        let (_, p) = call(
            &server,
            "POST",
            "/api/v1/lab/projects",
            a,
            org,
            Some(json!({"name": "Dock", "classes": ["person"]})),
        )
        .await;
        let pid = p["id"].as_str().unwrap().to_string();
        let (png, _) = square_png(3);
        let jpeg = pnex_vision::prepare_jpeg(&png, None, 320).unwrap();
        let key = format!("pnex:cam:v1:{org}:cam-dock:f:1");
        let _: () = redis::cmd("SET")
            .arg(&key)
            .arg(jpeg)
            .arg("EX")
            .arg(60)
            .query(&mut valkey)
            .unwrap();
        let body = json!({
            "org_id": org, "project_id": pid, "frame_key": key, "source": "cam-dock",
            "ts_ms": 1_790_000_000_000i64,
            "proposals": [
                {"label": "person", "score": 0.4, "bbox": [32.0, 24.0, 64.0, 48.0]},
                {"label": "dog", "score": 0.3, "bbox": [0.0, 0.0, 10.0, 10.0]}
            ]
        });
        let denied = server.post("/internal/flow/lab-review").json(&body).await;
        assert_eq!(denied.status_code(), 401);
        unsafe { std::env::set_var("PNEX_FLOW_RUNTIME_TOKEN", "tok-lab") };
        let mut foreign = body.clone();
        foreign["frame_key"] = json!("pnex:cam:v1:999999:cam-dock:f:1");
        let res = server
            .post("/internal/flow/lab-review")
            .add_header("x-pnex-flow-token", "tok-lab")
            .json(&foreign)
            .await;
        assert_eq!(res.status_code(), 404);
        let res = server
            .post("/internal/flow/lab-review")
            .add_header("x-pnex-flow-token", "tok-lab")
            .json(&body)
            .await;
        assert_eq!(res.status_code(), 201, "{}", res.text());
        let img: Value = res.json();
        assert_eq!(img["review"], true);
        assert_eq!(img["source"], "review-cam-dock-2026-09-21");
        assert_eq!(img["proposals"][0]["class"], 0, "person maps to the class");
        assert!(img["proposals"][1]["class"].is_null());
        assert!((img["proposals"][0]["bbox"][0].as_f64().unwrap() - 0.1).abs() < 1e-4);
        let id = img["id"].as_str().unwrap();
        let (st, saved) = call(
            &server,
            "PUT",
            &format!("/api/v1/lab/images/{id}/annotations"),
            a,
            org,
            Some(json!({"boxes": [{"class": 0, "bbox": [0.1, 0.1, 0.2, 0.2]}], "expected_version": 1})),
        )
        .await;
        assert_eq!(st, 200, "{saved}");
        assert_eq!(saved["review"], false);
        unsafe { std::env::remove_var("PNEX_FLOW_RUNTIME_TOKEN") };
    })
    .await;
}
