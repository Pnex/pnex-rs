//! Tests du stitch serveur (Take 360 V2, plan B — worker synchrone en test) :
//! création de job, upload octet-stream des frames, assemblage Quality et
//! publication comme version n+1 de l'asset média. Isolation org, rôles,
//! kill-switch et chemins d'échec couverts.
//!
//! Nécessite PostgreSQL (base de test dédiée) — base vidée entre tests.
//!
//! ÉCART AU PLAN : l'énoncé demandait « 2 poses / frames_total=2 → succeeded ».
//! Deux frames hfov 100 ne couvrent que ~200°/360° → le contrôle de couverture
//! du stitcher (`min_band_coverage: 0.90`, plan B3) refuse le rendu : le job
//! serait `failed`, pas `succeeded`. Le test de succès utilise donc un anneau
//! complet (4 frames à 0/90/180/270°, recouvrement 20°) — le job à 2 frames
//! sert de test d'échec (couverture insuffisante).

mod common;

use loco_rs::testing::request::{RequestConfig, RequestConfigBuilder};
use pnex_backend::app::App;
use serial_test::serial;

struct Env {
    alice: String,
    bob: String,
}

/// Boot l'app avec le MediaStore fs temporaire et le stitch à 1024 px (env
/// posées AVANT le boot — école RAUTHY_URL / PNEX_MEDIA_DIR de tests/media.rs).
/// ForegroundBlocking : le worker tourne inline dans la requête du dernier
/// POST frame.
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
    unsafe {
        std::env::set_var(
            "PNEX_MEDIA_DIR",
            format!(
                "{}/pnex-stitch-tests-{}",
                std::env::temp_dir().display(),
                std::process::id()
            ),
        )
    };
    unsafe { std::env::set_var("MEDIA_BACKEND", "fs") };
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

/// Org personnelle de l'utilisateur (créée par JIT provisioning).
async fn personal_org(server: &axum_test::TestServer, token: &str) -> i64 {
    server
        .get("/api/v1/user-info")
        .add_header("Authorization", bearer(token))
        .await
        .json::<serde_json::Value>()["orgs"][0]["id"]
        .as_i64()
        .expect("org personnelle")
}

/// JPEG minimal avec segment APP1 XMP (même encodage que le sniff GPano).
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

/// Frame JPEG 64×48 valable — dégradé texturé (structure régulière, assez
/// pour l'alignement NCC sans être exigeant à cette résolution de test).
/// La même image pour toutes les frames : le stitch Quality (gains, coutures,
/// multiband) est exercé par la géométrie des poses, pas par le contenu.
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

/// Upload octet-stream d'un média (asset panorama par sniff GPano).
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
    assert_eq!(res.status_code(), 201, "upload média : {}", res.text());
    res.json::<serde_json::Value>()
}

/// Crée un job de stitch : poses = une par frame, hfov 100°.
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

/// POST octet-stream d'une frame (k).
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

/// Récupère l'état d'un job.
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

/// Un anneau complet : 4 poses hfov 100 à 0/90/180/270° (recouvrement 20°).
fn ring_poses() -> serde_json::Value {
    serde_json::json!([
        {"yaw_deg": 0.0,   "pitch_deg": 0.0, "roll_deg": 0.0},
        {"yaw_deg": 90.0,  "pitch_deg": 0.0, "roll_deg": 0.0},
        {"yaw_deg": 180.0, "pitch_deg": 0.0, "roll_deg": 0.0},
        {"yaw_deg": 270.0, "pitch_deg": 0.0, "roll_deg": 0.0}
    ])
}

// ─────────────────────────── Tests ───────────────────────────

/// Cycle complet : asset → job 4 frames → succeeded → version 2 attachée à
/// l'asset (courante, 1024×512, GPano + note serveur), frames conservées
/// dans le store, immutabilité des frames déjà reçues.
#[tokio::test]
#[serial]
async fn cycle_job_complet() {
    with_app(true, |server, env| async move {
        let org = personal_org(&server, &env.alice).await;
        let asset = upload_media(&server, &env.alice, org).await;
        let asset_id = asset["id"].as_str().unwrap().to_string();

        // Validation : 1 pose (frames_total=1 < 2) et poses vides → 400.
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
        }

        // Asset cross-org : bob, dans SON org, vise l'asset d'alice → 404
        // masqué (école tests/media.rs::isolation_org_et_derniere_version).
        let org_bob = personal_org(&server, &env.bob).await;
        let res = create_job(&server, &env.bob, org_bob, &asset_id, ring_poses()).await;
        assert_eq!(res.status_code(), 404, "asset cross-org masqué");

        let res = create_job(&server, &env.alice, org, &asset_id, ring_poses()).await;
        assert_eq!(res.status_code(), 201, "création job : {}", res.text());
        let job: serde_json::Value = res.json();
        let job_id = job["id"].as_str().unwrap().to_string();
        assert_eq!(job["state"], "queued");
        assert_eq!(job["frames_total"], 4);
        assert_eq!(job["frames_received"], 0);

        // Frame hors bornes (k = 4) → 400, compteur intact.
        let res = upload_frame(&server, &env.alice, org, &job_id, 4, frame_jpeg()).await;
        assert_eq!(res.status_code(), 400, "k >= frames_total → 400");
        assert_eq!(
            get_job(&server, &env.alice, org, &job_id).await["frames_received"],
            0
        );

        // 3 premières frames : compteur qui monte, état toujours queued.
        for k in 0..3u32 {
            let res = upload_frame(&server, &env.alice, org, &job_id, k, frame_jpeg()).await;
            assert_eq!(res.status_code(), 200, "frame {k} : {}", res.text());
            let body: serde_json::Value = res.json();
            assert_eq!(body["frames_received"], k + 1);
            assert_eq!(body["state"], "queued", "état inchangé pendant l'upload");
        }

        // Dernière frame : worker inline (ForegroundBlocking) → succeeded.
        let res = upload_frame(&server, &env.alice, org, &job_id, 3, frame_jpeg()).await;
        assert_eq!(res.status_code(), 200, "dernière frame : {}", res.text());
        let body: serde_json::Value = res.json();
        assert_eq!(body["state"], "succeeded", "worker synchrone");
        assert_eq!(body["frames_received"], 4);
        assert_eq!(body["error"], serde_json::Value::Null);

        // Un nouveau POST frame sur le job terminal → 409.
        let res = upload_frame(&server, &env.alice, org, &job_id, 0, frame_jpeg()).await;
        assert_eq!(res.status_code(), 409, "job terminal → 409");

        // Asset : 2 versions, la 2e (serveur) courante.
        let detail: serde_json::Value = server
            .get(&format!("/api/v1/media/{asset_id}"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await
            .json();
        assert_eq!(detail["current_version_number"], 2, "v2 serveur courante");
        assert_eq!(detail["versions_count"], 2);

        // Version 2 = JPEG 1024×512 avec GPano + note d'assemblage.
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

        // Octets de la v2 : JPEG équirect 1024×512 (hauteur = largeur / 2).
        let res = server
            .get(&format!("/api/v1/media/{asset_id}/versions/2/content"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await;
        assert_eq!(res.status_code(), 200);
        let bytes = res.into_bytes().to_vec();
        let (_, w, h) = pnex_stitcher::decode_jpeg_rgb(&bytes).expect("v2 décodable");
        assert_eq!(
            (w, h),
            (1024, 512),
            "dimensions de la spec (PNEX_STITCH_OUT_WIDTH)"
        );
        // GPano toujours présent (le sniff reclasse en panorama tout seul).
        let head = &bytes[..bytes.len().min(128 * 1024)];
        assert!(head.windows(6).any(|s| s == b"GPano:"), "GPano conservé");

        // Frames conservées dans le store (re-stitch hôte futur — plan B2).
        let dir = std::env::var("PNEX_MEDIA_DIR").unwrap();
        for k in 0..4u32 {
            let frame = format!("{dir}/stitch_jobs/{job_id}/frame_{k:03}.jpg");
            assert!(
                std::path::Path::new(&frame).exists(),
                "frame conservée : {frame}"
            );
        }
    })
    .await;
}

/// Échec déterministe : 2 frames (~200° de couverture) → le contrôle
/// `min_band_coverage` refuse → job `failed` + erreur courte, aucune
/// nouvelle version sur l'asset.
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

        // Le dernier POST frame a déclenché le worker inline : échec couverture.
        let after = get_job(&server, &env.alice, org, &job_id).await;
        assert_eq!(after["state"], "failed", "couverture insuffisante → failed");
        let err = after["error"].as_str().expect("message d'erreur présent");
        assert!(!err.is_empty());
        assert!(
            !err.contains('/'),
            "pas de chemin disque dans l'erreur : {err}"
        );

        // Aucune version ajoutée : l'asset reste à sa version 1.
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

/// Isolation org : bob (autre org) ne voit ni le job ni ses frames.
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

        // GET cross-org → 404 masqué ; POST frame cross-org → 404 aussi.
        let res = server
            .get(&format!("/api/v1/stitch-jobs/{job_id}"))
            .add_header("Authorization", bearer(&env.bob))
            .add_header("X-Org-Id", org2.to_string())
            .await;
        assert_eq!(res.status_code(), 404, "GET cross-org masqué");
        let res = upload_frame(&server, &env.bob, org2, &job_id, 0, frame_jpeg()).await;
        assert_eq!(res.status_code(), 404, "POST frame cross-org masqué");

        // Sans X-Org-Id → 400 (contexte org requis).
        let res = server
            .get(&format!("/api/v1/stitch-jobs/{job_id}"))
            .add_header("Authorization", bearer(&env.alice))
            .await;
        assert_eq!(res.status_code(), 400);
    })
    .await;
}

/// Kill-switch : PNEX_STITCH_ENABLED=false → POST /stitch-jobs répond 503
/// (les GET restent servis — un job créé avant peut finir).
#[tokio::test]
#[serial]
async fn kill_switch_desactive() {
    with_app(false, |server, env| async move {
        let org = personal_org(&server, &env.alice).await;
        let asset = upload_media(&server, &env.alice, org).await;
        let asset_id = asset["id"].as_str().unwrap().to_string();

        let res = create_job(&server, &env.alice, org, &asset_id, ring_poses()).await;
        assert_eq!(res.status_code(), 503, "stitch désactivé → 503");
        let body: serde_json::Value = res.json();
        assert_eq!(body["error"], "stitch-disabled");
    })
    .await;
}

/// `GET /by-asset/{asset_id}` — source de vérité de l'overlay UI : dernier
/// job d'un asset (état + erreur), 404 si jamais stitché, isolation org.
#[tokio::test]
#[serial]
async fn dernier_job_par_asset() {
    with_app(true, |server, env| async move {
        let org = personal_org(&server, &env.alice).await;
        let asset = upload_media(&server, &env.alice, org).await;
        let asset_id = asset["id"].as_str().unwrap().to_string();

        // Jamais stitché serveur → 404 (l'UI n'affiche pas d'overlay).
        let res = server
            .get(&format!("/api/v1/stitch-jobs/by-asset/{asset_id}"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await;
        assert_eq!(res.status_code(), 404, "aucun job → 404 : {}", res.text());

        // Job à 2 frames → failed (couverture) : le dernier job est visible
        // par asset avec son état terminal et son erreur.
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

        // Isolation org : bob, dans SON org, ne voit pas le job d'alice.
        let org_bob = personal_org(&server, &env.bob).await;
        let res = server
            .get(&format!("/api/v1/stitch-jobs/by-asset/{asset_id}"))
            .add_header("Authorization", bearer(&env.bob))
            .add_header("X-Org-Id", org_bob.to_string())
            .await;
        assert_eq!(res.status_code(), 404, "cross-org masqué");
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
