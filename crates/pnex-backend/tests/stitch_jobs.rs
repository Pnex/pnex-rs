//! Server stitch tests (Take 360 V2, plan B — synchronous worker in tests):
//! job creation, octet-stream upload of the frames, Quality assembly and
//! publication as version n+1 of the media asset. Org isolation, roles,
//! kill-switch and failure paths covered.
//!
//! Requires PostgreSQL (dedicated test database) — database emptied between tests.
//!
//! DEVIATION FROM THE PLAN: the spec asked for "2 poses / frames_total=2 → succeeded".
//! Two hfov 100 frames only cover ~200°/360° → the stitcher's coverage check
//! (`min_band_coverage: 0.90`, plan B3) refuses the render: the job would be
//! `failed`, not `succeeded`. The success test therefore uses a full ring
//! (4 frames at 0/90/180/270°, 20° overlap) — the 2-frame job serves as the
//! failure test (insufficient coverage).

mod common;

use loco_rs::testing::request::{RequestConfig, RequestConfigBuilder};
use pnex_backend::app::App;
use serial_test::serial;

struct Env {
    alice: String,
    bob: String,
}

/// Boots the app on the test object store with stitching at 1024 px (env
/// set BEFORE the boot).
/// ForegroundBlocking: the worker runs inline within the request of the last
/// frame POST.
async fn with_app<F, Fut>(stitch_enabled: bool, f: F)
where
    F: FnOnce(axum_test::TestServer, Env) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    with_app_ctx(stitch_enabled, move |server, env, _ctx| f(server, env)).await
}

/// Same boot as `with_app`, also handing the app context to the test.
async fn with_app_ctx<F, Fut>(stitch_enabled: bool, f: F)
where
    F: FnOnce(axum_test::TestServer, Env, loco_rs::app::AppContext) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let base = common::spawn_mock_rauthy().await;
    let _ = tracing_subscriber::fmt().with_test_writer().try_init();
    unsafe { std::env::set_var("RAUTHY_URL", &base) };
    unsafe { std::env::set_var("PNEX_STITCH_OUT_WIDTH", "1024") };
    unsafe {
        std::env::set_var(
            "PNEX_STITCH_ENABLED",
            if stitch_enabled { "true" } else { "false" },
        )
    };
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

/// The user's personal org (created by JIT provisioning).
async fn personal_org(server: &axum_test::TestServer, token: &str) -> i64 {
    server
        .get("/api/v1/user-info")
        .add_header("Authorization", bearer(token))
        .await
        .json::<serde_json::Value>()["orgs"][0]["id"]
        .as_i64()
        .expect("org personnelle")
}

/// Minimal JPEG with an APP1 XMP segment (same encoding as the GPano sniff).
fn gpano_jpeg() -> Vec<u8> {
    let prefix = b"http://ns.adobe.com/xap/1.0/\x00";
    let xmp = r#"<rdf:Description GPano:ProjectionType="equirectangular"/>"#;
    let content = [prefix.as_slice(), xmp.as_bytes()].concat();
    let len = (content.len() + 2) as u16;
    let mut out = vec![0xFF, 0xD8];
    out.extend_from_slice(&[0xFF, 0xE1]);
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(&content);
    out.extend_from_slice(&[0xFF, 0xD9]);
    out
}

/// Valid 64×48 JPEG frame — textured gradient (regular structure, enough
/// for NCC alignment without being demanding at this test resolution).
/// The same image for every frame: the Quality stitch (gains, seams,
/// multiband) is exercised by the pose geometry, not by the content.
fn frame_jpeg() -> Vec<u8> {
    let (w, h) = (64u32, 48u32);
    let mut rgb = vec![0u8; (w * h * 3) as usize];
    for y in 0..h {
        for x in 0..w {
            let i = ((y * w + x) as usize) * 3;
            let gx = (x as f32 / w as f32 * std::f32::consts::TAU).sin();
            let gy = (y as f32 / h as f32 * 4.0 * std::f32::consts::PI).sin();
            let v = ((gx + gy) * 0.25 + 0.5).clamp(0.0, 1.0);
            rgb[i] = (v * 255.0) as u8;
            rgb[i + 1] = (((gx * 0.5 + 0.5).clamp(0.0, 1.0)) * 255.0) as u8;
            rgb[i + 2] = ((y * 255) / h) as u8;
        }
    }
    pnex_stitcher::encode_jpeg(&rgb, w, h, 85).expect("encode jpeg de test")
}

/// Octet-stream upload of a media (panorama asset via the GPano sniff).
async fn upload_media(
    server: &axum_test::TestServer,
    token: &str,
    org_id: i64,
) -> serde_json::Value {
    let res = server
        .post("/api/v1/media?filename=sphere.jpg&content_type=image%2Fjpeg")
        .add_header("Authorization", bearer(token))
        .add_header("X-Org-Id", org_id.to_string())
        .add_header("Content-Type", "application/octet-stream")
        .bytes(gpano_jpeg().into())
        .await;
    assert_eq!(res.status_code(), 201, "media upload: {}", res.text());
    res.json::<serde_json::Value>()
}

/// Creates a stitch job: one pose per frame, hfov 100°.
async fn create_job(
    server: &axum_test::TestServer,
    token: &str,
    org_id: i64,
    asset_id: &str,
    poses: serde_json::Value,
) -> axum_test::TestResponse {
    server
        .post("/api/v1/stitch-jobs")
        .add_header("Authorization", bearer(token))
        .add_header("X-Org-Id", org_id.to_string())
        .json(&serde_json::json!({
            "asset_id": asset_id,
            "frames_total": poses.as_array().map(|p| p.len()).unwrap_or(0),
            "hfov_deg": 100.0,
            "poses": poses,
        }))
        .await
}

/// Octet-stream POST of a frame (k).
async fn upload_frame(
    server: &axum_test::TestServer,
    token: &str,
    org_id: i64,
    job_id: &str,
    k: u32,
    jpeg: Vec<u8>,
) -> axum_test::TestResponse {
    server
        .post(&format!("/api/v1/stitch-jobs/{job_id}/frames/{k}"))
        .add_header("Authorization", bearer(token))
        .add_header("X-Org-Id", org_id.to_string())
        .add_header("Content-Type", "application/octet-stream")
        .bytes(jpeg.into())
        .await
}

/// Fetches the state of a job.
async fn get_job(
    server: &axum_test::TestServer,
    token: &str,
    org_id: i64,
    job_id: &str,
) -> serde_json::Value {
    server
        .get(&format!("/api/v1/stitch-jobs/{job_id}"))
        .add_header("Authorization", bearer(token))
        .add_header("X-Org-Id", org_id.to_string())
        .await
        .json::<serde_json::Value>()
}

/// A full ring: 4 hfov 100 poses at 0/90/180/270° (20° overlap).
fn ring_poses() -> serde_json::Value {
    serde_json::json!([
        {"yaw_deg": 0.0,   "pitch_deg": 0.0, "roll_deg": 0.0},
        {"yaw_deg": 90.0,  "pitch_deg": 0.0, "roll_deg": 0.0},
        {"yaw_deg": 180.0, "pitch_deg": 0.0, "roll_deg": 0.0},
        {"yaw_deg": 270.0, "pitch_deg": 0.0, "roll_deg": 0.0}
    ])
}

// ─────────────────────────── Tests ───────────────────────────

/// Full cycle: asset → 4-frame job → succeeded → version 2 attached to
/// the asset (current, 1024×512, GPano + server note), frames kept
/// in the store, frames already received are immutable.
#[tokio::test]
#[serial]
async fn cycle_job_complet() {
    with_app(true, |server, env| async move {
        let org = personal_org(&server, &env.alice).await;
        let asset = upload_media(&server, &env.alice, org).await;
        let asset_id = asset["id"].as_str().unwrap().to_string();

        // Validation: 1 pose (frames_total=1 < 2) and empty poses → 400.
        for bad in [
            serde_json::json!([{ "yaw_deg": 0.0, "pitch_deg": 0.0, "roll_deg": 0.0 }]),
            serde_json::json!([]),
        ] {
            let res = create_job(&server, &env.alice, org, &asset_id, bad).await;
            assert_eq!(
                res.status_code(),
                400,
                "job invalide → 400 : {}",
                res.text()
            );
            let body: serde_json::Value = res.json();
            assert_eq!(body["error"], "field-invalid", "{body}");
            assert_eq!(body["errors"]["fields"]["frames_total"], "range:2..64");
        }

        // Cross-org asset: bob, in HIS org, targets alice's asset → masked
        // 404 (as in tests/media.rs::isolation_org_et_derniere_version).
        let org_bob = personal_org(&server, &env.bob).await;
        let res = create_job(&server, &env.bob, org_bob, &asset_id, ring_poses()).await;
        assert_eq!(res.status_code(), 404, "cross-org asset masked");

        let res = create_job(&server, &env.alice, org, &asset_id, ring_poses()).await;
        assert_eq!(res.status_code(), 201, "job creation: {}", res.text());
        let job: serde_json::Value = res.json();
        let job_id = job["id"].as_str().unwrap().to_string();
        assert_eq!(job["state"], "queued");
        assert_eq!(job["frames_total"], 4);
        assert_eq!(job["frames_received"], 0);

        // Out-of-bounds frame (k = 4) → 400, counter intact.
        let res = upload_frame(&server, &env.alice, org, &job_id, 4, frame_jpeg()).await;
        assert_eq!(res.status_code(), 400, "k >= frames_total → 400");
        assert_eq!(
            res.json::<serde_json::Value>()["errors"]["fields"]["k"],
            "invalid"
        );
        assert_eq!(
            get_job(&server, &env.alice, org, &job_id).await["frames_received"],
            0
        );

        // First 3 frames: counter goes up, state still queued.
        for k in 0..3u32 {
            let res = upload_frame(&server, &env.alice, org, &job_id, k, frame_jpeg()).await;
            assert_eq!(res.status_code(), 200, "frame {k} : {}", res.text());
            let body: serde_json::Value = res.json();
            assert_eq!(body["frames_received"], k + 1);
            assert_eq!(body["state"], "queued", "state unchanged during the upload");
        }

        // Last frame: inline worker (ForegroundBlocking) → succeeded.
        let res = upload_frame(&server, &env.alice, org, &job_id, 3, frame_jpeg()).await;
        assert_eq!(res.status_code(), 200, "last frame: {}", res.text());
        let body: serde_json::Value = res.json();
        assert_eq!(body["state"], "succeeded", "worker synchrone");
        assert_eq!(body["frames_received"], 4);
        assert_eq!(body["error"], serde_json::Value::Null);

        // A new POST frame on the terminal job → 409.
        let res = upload_frame(&server, &env.alice, org, &job_id, 0, frame_jpeg()).await;
        assert_eq!(res.status_code(), 409, "job terminal → 409");

        // Asset: 2 versions, the 2nd (server) current.
        let detail: serde_json::Value = server
            .get(&format!("/api/v1/media/{asset_id}"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await
            .json();
        assert_eq!(detail["current_version_number"], 2, "v2 serveur courante");
        assert_eq!(detail["versions_count"], 2);

        // Version 2 = 1024×512 JPEG with GPano + assembly note.
        let versions: serde_json::Value = server
            .get(&format!("/api/v1/media/{asset_id}/versions"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await
            .json();
        assert_eq!(versions["count"], 2);
        let v2 = &versions["results"][1];
        assert_eq!(v2["version_number"], 2);
        assert_eq!(v2["filename"], "panorama-serveur-1024.jpg");
        assert_eq!(v2["note"], "assemblage serveur 1024 (Take 360 V2)");
        assert!(v2["current"].as_bool().unwrap(), "v2 courante");

        // v2 bytes: equirect JPEG 1024×512 (height = width / 2).
        let res = server
            .get(&format!("/api/v1/media/{asset_id}/versions/2/content"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await;
        assert_eq!(res.status_code(), 200);
        let bytes = res.into_bytes().to_vec();
        let (_, w, h) = pnex_stitcher::decode_jpeg_rgb(&bytes).expect("v2 decodable");
        assert_eq!(
            (w, h),
            (1024, 512),
            "spec dimensions (PNEX_STITCH_OUT_WIDTH)"
        );
        // GPano still present (the sniff reclassifies as panorama by itself).
        let head = &bytes[..bytes.len().min(128 * 1024)];
        assert!(head.windows(6).any(|s| s == b"GPano:"), "GPano kept");

        // Frames kept in the store (future host re-stitch — plan B2).
        let store = common::media_store();
        for k in 0..4u32 {
            let frame = format!("stitch_jobs/{job_id}/frame_{k:03}.jpg");
            assert!(store.exists(&frame).await.unwrap(), "frame kept: {frame}");
        }
    })
    .await;
}

/// Deterministic failure: 2 frames (~200° of coverage) → the
/// `min_band_coverage` check refuses → job `failed` + short error, no
/// new version on the asset.
#[tokio::test]
#[serial]
async fn job_echec_couverture() {
    with_app(true, |server, env| async move {
        let org = personal_org(&server, &env.alice).await;
        let asset = upload_media(&server, &env.alice, org).await;
        let asset_id = asset["id"].as_str().unwrap().to_string();

        let res = create_job(
            &server,
            &env.alice,
            org,
            &asset_id,
            serde_json::json!([
                {"yaw_deg": 0.0, "pitch_deg": 0.0, "roll_deg": 0.0},
                {"yaw_deg": 90.0, "pitch_deg": 0.0, "roll_deg": 0.0}
            ]),
        )
        .await;
        assert_eq!(res.status_code(), 201);
        let job: serde_json::Value = res.json();
        let job_id = job["id"].as_str().unwrap().to_string();

        for k in 0..2u32 {
            let res = upload_frame(&server, &env.alice, org, &job_id, k, frame_jpeg()).await;
            assert_eq!(res.status_code(), 200, "frame {k} : {}", res.text());
        }

        // The last frame POST triggered the inline worker: coverage failure.
        let after = get_job(&server, &env.alice, org, &job_id).await;
        assert_eq!(after["state"], "failed", "couverture insuffisante → failed");
        let err = after["error"].as_str().expect("error message present");
        assert!(!err.is_empty());
        assert!(!err.contains('/'), "no disk path in the error: {err}");

        // No version added: the asset stays at its version 1.
        let detail: serde_json::Value = server
            .get(&format!("/api/v1/media/{asset_id}"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await
            .json();
        assert_eq!(detail["current_version_number"], 1);
        assert_eq!(detail["versions_count"], 1);
    })
    .await;
}

/// Org isolation: bob (another org) sees neither the job nor its frames.
#[tokio::test]
#[serial]
async fn isolation_org() {
    with_app(true, |server, env| async move {
        let org1 = personal_org(&server, &env.alice).await;
        let org2 = personal_org(&server, &env.bob).await;
        let asset = upload_media(&server, &env.alice, org1).await;
        let asset_id = asset["id"].as_str().unwrap().to_string();

        let res = create_job(&server, &env.alice, org1, &asset_id, ring_poses()).await;
        assert_eq!(res.status_code(), 201);
        let job_id = res.json::<serde_json::Value>()["id"]
            .as_str()
            .unwrap()
            .to_string();

        // Cross-org GET → masked 404; cross-org frame POST → 404 too.
        let res = server
            .get(&format!("/api/v1/stitch-jobs/{job_id}"))
            .add_header("Authorization", bearer(&env.bob))
            .add_header("X-Org-Id", org2.to_string())
            .await;
        assert_eq!(res.status_code(), 404, "cross-org GET masked");
        let res = upload_frame(&server, &env.bob, org2, &job_id, 0, frame_jpeg()).await;
        assert_eq!(res.status_code(), 404, "cross-org frame POST masked");

        // Without X-Org-Id → 400 (org context required).
        let res = server
            .get(&format!("/api/v1/stitch-jobs/{job_id}"))
            .add_header("Authorization", bearer(&env.alice))
            .await;
        assert_eq!(res.status_code(), 400);
    })
    .await;
}

/// Kill-switch: PNEX_STITCH_ENABLED=false → POST /stitch-jobs answers 503
/// (GETs are still served — a job created earlier can finish).
#[tokio::test]
#[serial]
async fn kill_switch_desactive() {
    with_app(false, |server, env| async move {
        let org = personal_org(&server, &env.alice).await;
        let asset = upload_media(&server, &env.alice, org).await;
        let asset_id = asset["id"].as_str().unwrap().to_string();

        let res = create_job(&server, &env.alice, org, &asset_id, ring_poses()).await;
        assert_eq!(res.status_code(), 503, "stitch disabled → 503");
        let body: serde_json::Value = res.json();
        assert_eq!(body["error"], "stitch-disabled");
    })
    .await;
}

/// `GET /by-asset/{asset_id}` — source of truth of the UI overlay: last
/// job of an asset (state + error), 404 if never stitched, org isolation.
#[tokio::test]
#[serial]
async fn dernier_job_par_asset() {
    with_app(true, |server, env| async move {
        let org = personal_org(&server, &env.alice).await;
        let asset = upload_media(&server, &env.alice, org).await;
        let asset_id = asset["id"].as_str().unwrap().to_string();

        // Never stitched on the server → 404 (the UI shows no overlay).
        let res = server
            .get(&format!("/api/v1/stitch-jobs/by-asset/{asset_id}"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await;
        assert_eq!(res.status_code(), 404, "no job → 404: {}", res.text());

        // 2-frame job → failed (coverage): the last job is visible
        // per asset with its terminal state and its error.
        let res = create_job(
            &server,
            &env.alice,
            org,
            &asset_id,
            serde_json::json!([
                {"yaw_deg": 0.0, "pitch_deg": 0.0, "roll_deg": 0.0},
                {"yaw_deg": 90.0, "pitch_deg": 0.0, "roll_deg": 0.0}
            ]),
        )
        .await;
        assert_eq!(res.status_code(), 201);
        let job_id = res.json::<serde_json::Value>()["id"]
            .as_str()
            .unwrap()
            .to_string();
        for k in 0..2u32 {
            let res = upload_frame(&server, &env.alice, org, &job_id, k, frame_jpeg()).await;
            assert_eq!(res.status_code(), 200, "frame {k}");
        }

        let by_asset: serde_json::Value = server
            .get(&format!("/api/v1/stitch-jobs/by-asset/{asset_id}"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await
            .json();
        assert_eq!(by_asset["id"], job_id);
        assert_eq!(by_asset["asset_id"], asset_id);
        assert_eq!(by_asset["state"], "failed");
        assert!(by_asset["error"].as_str().is_some_and(|e| !e.is_empty()));

        // Org isolation: bob, in HIS org, doesn't see alice's job.
        let org_bob = personal_org(&server, &env.bob).await;
        let res = server
            .get(&format!("/api/v1/stitch-jobs/by-asset/{asset_id}"))
            .add_header("Authorization", bearer(&env.bob))
            .add_header("X-Org-Id", org_bob.to_string())
            .await;
        assert_eq!(res.status_code(), 404, "cross-org masked");
    })
    .await;
}

/// A duplicate delivery of an already succeeded job (queue reaper re-queue,
/// double enqueue) is a no-op: no second stitch, no extra asset version.
#[tokio::test]
#[serial]
async fn duplicate_delivery_is_a_noop() {
    use loco_rs::bgworker::BackgroundWorker;
    use pnex_backend::workers::stitch_panorama::{StitchPanoramaArgs, StitchPanoramaWorker};
    with_app_ctx(true, |server, env, ctx| async move {
        let org = personal_org(&server, &env.alice).await;
        let asset = upload_media(&server, &env.alice, org).await;
        let asset_id = asset["id"].as_str().unwrap().to_string();
        let res = create_job(&server, &env.alice, org, &asset_id, ring_poses()).await;
        assert_eq!(res.status_code(), 201, "job: {}", res.text());
        let job_id = res.json::<serde_json::Value>()["id"]
            .as_str()
            .unwrap()
            .to_string();
        for k in 0..4u32 {
            let res = upload_frame(&server, &env.alice, org, &job_id, k, frame_jpeg()).await;
            assert_eq!(res.status_code(), 200, "frame {k}: {}", res.text());
        }
        assert_eq!(
            get_job(&server, &env.alice, org, &job_id).await["state"],
            "succeeded"
        );

        StitchPanoramaWorker::build(&ctx)
            .perform(StitchPanoramaArgs {
                job_id: uuid::Uuid::parse_str(&job_id).unwrap(),
                org_id: org,
            })
            .await
            .expect("duplicate delivery");

        assert_eq!(
            get_job(&server, &env.alice, org, &job_id).await["state"],
            "succeeded"
        );
        let detail: serde_json::Value = server
            .get(&format!("/api/v1/media/{asset_id}"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await
            .json();
        assert_eq!(detail["versions_count"], 2, "no second stitch version");
    })
    .await;
}
