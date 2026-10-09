//! Tests de parité du domaine builds firmware (Phase 6) : POST build-firmware
//! (verification order inherited from the legacy stack: 404 device, 403 quota, 429 interval),
//! worker inline (ForegroundBlocking — le build est terminal au 201),
//! échec/timeout de toolchain via les fixtures, download proxifié — cf.
//! `docs/contracts/build.http`.
//!
//! Nécessite PostgreSQL (TEST_DATABASE_URL) — base vidée entre tests.
//! Toolchain remplacée par tests/fixtures/firmware (config test.yaml) :
//! `fail`/`sleep` en WIFI_SSID pilotent échec/timeout.

mod common;

use base64::Engine as _;
use loco_rs::testing::request::{RequestConfig, RequestConfigBuilder};
use pnex_backend::app::App;
use pnex_backend::models::_entities::build_records;
use sea_orm::EntityTrait;
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
            // Buildable devices are generic (mixed) ones: room for three.
            use sea_orm::ConnectionTrait;
            ctx.db
                .execute_unprepared(
                    "UPDATE subscription_tiers SET max_mixed_devices = 3 WHERE name = 'Free'",
                )
                .await
                .expect("tier quota");
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
        .expect("org personnelle")
}

async fn create_device(server: &axum_test::TestServer, token: &str, org_id: i64, device_id: &str) {
    let res = server
        .post("/api/v1/devices")
        .add_header("Authorization", bearer(token))
        .add_header("X-Org-Id", org_id.to_string())
        .add_header("Content-Type", "application/json")
        .json(&serde_json::json!({
            "device_id": device_id,
            "predefined_device_name": "generic_esp8266",
        }))
        .await;
    res.assert_status(axum_test::http::StatusCode::CREATED);
}

async fn post_build(
    server: &axum_test::TestServer,
    token: &str,
    org_id: i64,
    device_id: &str,
    wifi_ssid: &str,
) -> axum_test::TestResponse {
    let wifi_credential_id = wifi_entry(server, token, org_id, wifi_ssid).await;
    server
        .post("/api/v1/build-firmware")
        .add_header("Authorization", bearer(token))
        .add_header("X-Org-Id", org_id.to_string())
        .add_header("Content-Type", "application/json")
        .json(&serde_json::json!({
            "wifi_credential_id": wifi_credential_id,
            "pnex_host": "dev1.pnex.io",
            "device_id": device_id,
        }))
        .await
}

/// WiFi entry `ssid` of the org's referential (upsert), password in the
/// vault: its id.
async fn wifi_entry(server: &axum_test::TestServer, token: &str, org_id: i64, ssid: &str) -> i64 {
    server
        .post("/api/v1/edge/wifi-credentials")
        .add_header("Authorization", bearer(token))
        .add_header("X-Org-Id", org_id.to_string())
        .json(&serde_json::json!({ "ssid": ssid, "password": { "value": "pass-wifi" } }))
        .await
        .json::<serde_json::Value>()["id"]
        .as_i64()
        .expect("wifi entry id")
}

async fn records(
    server: &axum_test::TestServer,
    token: &str,
    org_id: i64,
    query: &str,
) -> serde_json::Value {
    server
        .get(&format!("/api/v1/build-records{query}"))
        .add_header("Authorization", bearer(token))
        .add_header("X-Org-Id", org_id.to_string())
        .await
        .json()
}

/// Cycle complet : 201 → worker inline (ForegroundBlocking) → record
/// `succeeded` + artefact dans le magasin → download proxifié avec les
/// secrets propagés (WiFi clair, host base64 — matérialisés par la fixture).
#[tokio::test]
#[serial]
async fn build_reussi_chemin_complet() {
    with_app(|server, env, ctx| async move {
        let org = personal_org(&server, &env.alice).await;
        create_device(&server, &env.alice, org, "capteur-jardin").await;

        let res = post_build(&server, &env.alice, org, "capteur-jardin", "coloc").await;
        res.assert_status(axum_test::http::StatusCode::CREATED);
        let body: serde_json::Value = res.json();
        assert_eq!(body["status"], "queued");
        let build_id = body["build_id"].as_i64().expect("build_id");

        // ForegroundBlocking : le build a tourné dans la requête — le
        // record est déjà terminal.
        let list = records(&server, &env.alice, org, "").await;
        assert_eq!(list["count"], 1);
        let record = &list["results"][0];
        assert_eq!(record["id"], build_id);
        assert_eq!(record["build_phase"], "succeeded");
        assert_eq!(
            record["firmware_bin_s3_key"],
            format!("org_{org}/firmware/capteur-jardin-firmware.bin")
        );
        // OTA (phase 1): the version is the record id, stamped into the
        // binary; the raw app image digest rides the same record (checked
        // at the source — the DTO intentionally omits the digest).
        assert_eq!(record["fw_version"], build_id.to_string());
        let row = build_records::Entity::find_by_id(build_id)
            .one(&ctx.db)
            .await
            .expect("record query")
            .expect("record exists");
        assert_eq!(row.ota_sha256.as_deref().map(str::len), Some(64));
        assert!(row.ota_size_bytes.unwrap_or(0) > 0);

        // Download : proxy + attachment + contenu de la fixture (les env du
        // sous-process y sont matérialisées ; HOST arrive en base64).
        let dl = server
            .get("/api/v1/download/firmware/capteur-jardin")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await;
        dl.assert_status(axum_test::http::StatusCode::OK);
        let disposition = dl
            .headers()
            .get("content-disposition")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_string();
        assert_eq!(
            disposition,
            "attachment; filename=\"capteur-jardin-firmware.bin\""
        );
        let content = dl.text();
        let ssid_b64 = base64::engine::general_purpose::STANDARD.encode("coloc");
        assert!(
            content.contains(&format!("fixture ssid={ssid_b64}")),
            "{content}"
        );
        let host_b64 = base64::engine::general_purpose::STANDARD.encode("dev1.pnex.io");
        assert!(content.contains(&format!("host={host_b64}")), "{content}");
    })
    .await;
}

/// Min interval: a 2nd build right after a success → 429 (exact legacy
/// error string), no extra record.
#[tokio::test]
#[serial]
async fn build_intervalle_429() {
    with_app(|server, env, _ctx| async move {
        let org = personal_org(&server, &env.alice).await;
        create_device(&server, &env.alice, org, "dev-a").await;
        create_device(&server, &env.alice, org, "dev-b").await;

        let first = post_build(&server, &env.alice, org, "dev-a", "coloc").await;
        first.assert_status(axum_test::http::StatusCode::CREATED);

        // Tier Free : min_build_interval 300 s — l'intervalle compte les
        // builds RÉUSSIS, tous devices confondus.
        let second = post_build(&server, &env.alice, org, "dev-b", "coloc").await;
        second.assert_status(axum_test::http::StatusCode::TOO_MANY_REQUESTS);
        let body: serde_json::Value = second.json();
        assert_eq!(body["error"], "build-interval-not-met", "{body}");

        let list = records(&server, &env.alice, org, "").await;
        assert_eq!(list["count"], 1, "pas de record pour le build refusé");
    })
    .await;
}

/// Device-type quota (Free: 3 mixed devices here). The device being built is already
/// registered: an org AT its quota builds normally (O36), an org OVER it
/// (tier lowered since) gets the exact legacy 403.
#[tokio::test]
#[serial]
async fn build_quota_403() {
    with_app(|server, env, ctx| async move {
        let org = personal_org(&server, &env.alice).await;
        for i in 1..=3 {
            create_device(&server, &env.alice, org, &format!("dev-{i}")).await;
        }
        let res = post_build(&server, &env.alice, org, "dev-1", "coloc").await;
        res.assert_status(axum_test::http::StatusCode::CREATED);

        use sea_orm::ConnectionTrait;
        let set_free_mixed = |n: i32| {
            let db = ctx.db.clone();
            async move {
                db.execute_unprepared(&format!(
                    "UPDATE subscription_tiers SET max_mixed_devices = {n} WHERE name = 'Free'"
                ))
                .await
                .expect("tier quota");
            }
        };
        set_free_mixed(2).await;
        let res = post_build(&server, &env.alice, org, "dev-2", "coloc").await;
        set_free_mixed(3).await;
        res.assert_status(axum_test::http::StatusCode::FORBIDDEN);
        let body: serde_json::Value = res.json();
        assert_eq!(body["error"], "device-quota-reached", "{body}");
        assert_eq!(body["errors"]["args"]["type"], "mixed");
    })
    .await;
}

/// Self-hosted deployment: no tier applies, even to an org that still
/// carries one (O37) — no device quota, no minimum build interval.
#[tokio::test]
#[serial]
async fn self_hosted_ignores_subscription_tiers() {
    with_app(|server, env, _ctx| async move {
        unsafe { std::env::set_var("PNEX_DEPLOYMENT_MODE", "self_hosted") };
        let org = personal_org(&server, &env.alice).await;
        for i in 1..=4 {
            create_device(&server, &env.alice, org, &format!("dev-{i}")).await;
        }
        let first = post_build(&server, &env.alice, org, "dev-1", "coloc").await;
        let second = post_build(&server, &env.alice, org, "dev-4", "coloc").await;
        unsafe { std::env::set_var("PNEX_DEPLOYMENT_MODE", "saas") };
        first.assert_status(axum_test::http::StatusCode::CREATED);
        second.assert_status(axum_test::http::StatusCode::CREATED);
    })
    .await;
}

/// Device not found in the org → 404, exact legacy error string.
#[tokio::test]
#[serial]
async fn build_device_inconnu_404() {
    with_app(|server, env, _ctx| async move {
        let org = personal_org(&server, &env.alice).await;
        let res = post_build(&server, &env.alice, org, "fantome", "coloc").await;
        res.assert_status(axum_test::http::StatusCode::NOT_FOUND);
        let body: serde_json::Value = res.json();
        assert_eq!(body["error"], "device-not-found");
        assert_eq!(body["errors"]["args"]["device_id"], "fantome");
    })
    .await;
}

/// Field-by-field validation (errors keyed by field name), unknown WiFi
/// entry, and the strict contract: any legacy field is refused.
#[tokio::test]
#[serial]
async fn validation_400() {
    with_app(|server, env, _ctx| async move {
        let org = personal_org(&server, &env.alice).await;
        create_device(&server, &env.alice, org, "dev-val").await;
        let wifi = wifi_entry(&server, &env.alice, org, "coloc").await;

        let cases = [
            (
                serde_json::json!({
                    "wifi_credential_id": wifi,
                    "pnex_host": "dev1.pnex.io",
                    "device_id": "",
                }),
                "device_id",
                "required",
            ),
            (
                serde_json::json!({
                    "wifi_credential_id": wifi,
                    "pnex_host": "dev1.pnex.io",
                    "device_id": "x".repeat(101),
                }),
                "device_id",
                "max_length:100",
            ),
            (
                serde_json::json!({
                    "wifi_credential_id": 999_999,
                    "pnex_host": "dev1.pnex.io",
                    "device_id": "dev-val",
                }),
                "wifi_credential_id",
                "WiFi entry not found in this organization.",
            ),
        ];
        for (body, field, msg) in cases {
            let res = server
                .post("/api/v1/build-firmware")
                .add_header("Authorization", bearer(&env.alice))
                .add_header("X-Org-Id", org.to_string())
                .add_header("Content-Type", "application/json")
                .json(&body)
                .await;
            res.assert_status(axum_test::http::StatusCode::BAD_REQUEST);
            let out: serde_json::Value = res.json();
            assert_eq!(out[field], msg, "{out}");
        }

        // Legacy typed WiFi / scheme fields: refused, not ignored.
        let legacy = server
            .post("/api/v1/build-firmware")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .json(&serde_json::json!({
                "wifi_ssid": "coloc",
                "wifi_password": "p",
                "pnex_host": "dev1.pnex.io",
                "device_id": "dev-val",
                "ws_ssl": true,
            }))
            .await;
        assert!(legacy.status_code().is_client_error(), "{}", legacy.text());
    })
    .await;
}

/// Échec de toolchain (fixture `fail`) : 201 quand même, record `failed`,
/// download 404.
#[tokio::test]
#[serial]
async fn build_echec_outil() {
    with_app(|server, env, _ctx| async move {
        let org = personal_org(&server, &env.alice).await;
        create_device(&server, &env.alice, org, "dev-fail").await;

        let res = post_build(&server, &env.alice, org, "dev-fail", "fail").await;
        res.assert_status(axum_test::http::StatusCode::CREATED);

        let list = records(&server, &env.alice, org, "").await;
        let record = &list["results"][0];
        assert_eq!(record["build_phase"], "failed");
        // Failure reason (O4): code + compiler output, token masked.
        assert_eq!(record["failure_code"], "build_compile", "{record}");
        let detail = record["failure_detail"].as_str().unwrap_or_default();
        assert!(detail.contains("erreur de compilation simulée"), "{detail}");
        assert!(detail.contains("-DTOKEN=***"), "token not masked: {detail}");

        let dl = server
            .get("/api/v1/download/firmware/dev-fail")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await;
        dl.assert_status(axum_test::http::StatusCode::NOT_FOUND);
    })
    .await;
}

/// Timeout dur (fixture `sleep`, budget test 2 s) : build `failed` en
/// temps borné.
#[tokio::test]
#[serial]
async fn build_timeout() {
    with_app(|server, env, _ctx| async move {
        let org = personal_org(&server, &env.alice).await;
        create_device(&server, &env.alice, org, "dev-slow").await;

        let started = std::time::Instant::now();
        let res = post_build(&server, &env.alice, org, "dev-slow", "sleep").await;
        res.assert_status(axum_test::http::StatusCode::CREATED);
        // Inline : le timeout a déjà couru dans la requête (2 s budget).
        assert!(
            started.elapsed().as_secs() >= 2,
            "le timeout doit avoir couru"
        );

        let list = records(&server, &env.alice, org, "").await;
        assert_eq!(list["results"][0]["build_phase"], "failed");
        assert_eq!(list["results"][0]["failure_code"], "build_timeout");
        assert!(started.elapsed().as_secs() < 30, "test borné");
    })
    .await;
}

/// Lift the org's subscription tier (no min build interval) so several
/// successful builds can run back to back.
async fn lift_build_interval(ctx: &loco_rs::app::AppContext, org: i64) {
    use sea_orm::ConnectionTrait;
    ctx.db
        .execute_unprepared(&format!(
            "UPDATE organizations SET subscription_tier_id = NULL WHERE id = {org}"
        ))
        .await
        .expect("tier lift");
}

/// Rebuild = a NEW record every time: new id = new, strictly increasing
/// firmware version, the previous versioned OTA artifact is kept.
#[tokio::test]
#[serial]
async fn rebuild_creates_a_new_record_with_a_higher_version() {
    with_app(|server, env, ctx| async move {
        let org = personal_org(&server, &env.alice).await;
        create_device(&server, &env.alice, org, "dev-re").await;

        // 1. A failure (does not consume the interval: only successes count).
        let first = post_build(&server, &env.alice, org, "dev-re", "fail").await;
        first.assert_status(axum_test::http::StatusCode::CREATED);
        let first_body: serde_json::Value = first.json();
        let first_id = first_body["build_id"].as_i64().expect("id");

        // 2. Immediate rebuild accepted, on a new record.
        let second = post_build(&server, &env.alice, org, "dev-re", "coloc").await;
        second.assert_status(axum_test::http::StatusCode::CREATED);
        let second_body: serde_json::Value = second.json();
        let second_id = second_body["build_id"].as_i64().expect("id");
        assert!(second_id > first_id);

        // 3. A second successful build: yet another record and version.
        lift_build_interval(&ctx, org).await;
        let third = post_build(&server, &env.alice, org, "dev-re", "coloc").await;
        third.assert_status(axum_test::http::StatusCode::CREATED);
        let third_id = third.json::<serde_json::Value>()["build_id"]
            .as_i64()
            .expect("id");
        assert!(third_id > second_id);

        let list = records(&server, &env.alice, org, "?device_id=dev-re").await;
        assert_eq!(list["count"], 3);
        assert_eq!(list["results"][0]["id"], third_id);
        assert_eq!(list["results"][0]["fw_version"], third_id.to_string());
        assert_eq!(list["results"][1]["fw_version"], second_id.to_string());
        assert_eq!(list["results"][2]["build_phase"], "failed");

        // Both OTA artifacts live under their own versioned key.
        let store = pnex_backend::services::firmware::FirmwareSettings::from_config(&ctx.config)
            .store(&ctx.db)
            .expect("store");
        for id in [second_id, third_id] {
            let key = pnex_firmware_builder::ota_artifact_key(org, "dev-re", &id.to_string());
            assert!(store.exists(&key).await.expect("exists"), "{key}");
        }

        // The device DTO exposes the newest deployable version.
        let devices: serde_json::Value = server
            .get("/api/v1/devices")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await
            .json();
        let latest = &devices["results"][0]["latest_build"];
        assert_eq!(latest["fw_version"], third_id.to_string(), "{latest}");
        assert_eq!(
            latest["deployable_version"],
            third_id.to_string(),
            "{latest}"
        );
    })
    .await;
}

/// Retention: after a successful build only the 5 newest records of the
/// device survive, plus the one the device runs and the target of an
/// active OTA; pruned records lose their OTA artifact, the shared serial
/// artifact stays.
#[tokio::test]
#[serial]
async fn successful_build_prunes_old_records_and_artifacts() {
    use pnex_backend::models::_entities::{device_registries, ota_assignments};
    use sea_orm::{ActiveModelTrait, ColumnTrait, QueryFilter, QueryOrder, Set};
    with_app(|server, env, ctx| async move {
        let org = personal_org(&server, &env.alice).await;
        create_device(&server, &env.alice, org, "dev-ret").await;
        lift_build_interval(&ctx, org).await;
        let store = pnex_backend::services::firmware::FirmwareSettings::from_config(&ctx.config)
            .store(&ctx.db)
            .expect("store");
        let serial_key = format!("org_{org}/firmware/dev-ret-firmware.bin");

        // 7 older successful builds, each with its OTA artifact.
        let mut old_ids = Vec::new();
        for _ in 0..7 {
            let row = build_records::ActiveModel {
                device_id: Set(Some("dev-ret".into())),
                build_phase: Set("succeeded".into()),
                firmware_bin_s3_key: Set(Some(serial_key.clone())),
                org_id: Set(org),
                ota_sha256: Set(Some("0".repeat(64))),
                ota_size_bytes: Set(Some(4)),
                ..Default::default()
            }
            .insert(&ctx.db)
            .await
            .expect("insert");
            let version = row.id.to_string();
            let mut stamp: build_records::ActiveModel = row.clone().into();
            stamp.fw_version = Set(Some(version.clone()));
            stamp.update(&ctx.db).await.expect("stamp");
            let key = pnex_firmware_builder::ota_artifact_key(org, "dev-ret", &version);
            store.put(&key, b"old!").await.expect("put");
            old_ids.push(row.id);
        }

        // The device runs the oldest one; an active OTA targets the 2nd.
        let device = device_registries::Entity::find()
            .filter(device_registries::Column::OrgId.eq(org))
            .filter(device_registries::Column::DeviceId.eq("dev-ret"))
            .one(&ctx.db)
            .await
            .expect("query")
            .expect("device");
        let mut running: device_registries::ActiveModel = device.clone().into();
        running.fw_version = Set(Some(old_ids[0].to_string()));
        running.update(&ctx.db).await.expect("fw_version");
        ota_assignments::ActiveModel {
            org_id: Set(org),
            device_registry_id: Set(device.id),
            target_version: Set(old_ids[1].to_string()),
            artifact_key: Set(pnex_firmware_builder::ota_artifact_key(
                org,
                "dev-ret",
                &old_ids[1].to_string(),
            )),
            sha256: Set("0".repeat(64)),
            state: Set("pending".into()),
            ..Default::default()
        }
        .insert(&ctx.db)
        .await
        .expect("ota row");

        // 8th build, real (fixture toolchain) → retention runs.
        let res = post_build(&server, &env.alice, org, "dev-ret", "coloc").await;
        res.assert_status(axum_test::http::StatusCode::CREATED);
        let new_id = res.json::<serde_json::Value>()["build_id"]
            .as_i64()
            .expect("id");

        let left: Vec<i64> = build_records::Entity::find()
            .filter(build_records::Column::OrgId.eq(org))
            .filter(build_records::Column::DeviceId.eq("dev-ret"))
            .order_by_asc(build_records::Column::Id)
            .all(&ctx.db)
            .await
            .expect("query")
            .into_iter()
            .map(|r| r.id)
            .collect();
        // Kept: running (0), OTA target (1), 4 newest old ones (3..7) + new.
        let mut expected = vec![old_ids[0], old_ids[1]];
        expected.extend_from_slice(&old_ids[3..]);
        expected.push(new_id);
        assert_eq!(left, expected);

        let pruned =
            pnex_firmware_builder::ota_artifact_key(org, "dev-ret", &old_ids[2].to_string());
        assert!(
            !store.exists(&pruned).await.expect("exists"),
            "pruned artifact"
        );
        for id in [old_ids[0], old_ids[1], old_ids[6], new_id] {
            let key = pnex_firmware_builder::ota_artifact_key(org, "dev-ret", &id.to_string());
            assert!(store.exists(&key).await.expect("exists"), "{key}");
        }
        assert!(
            store.exists(&serial_key).await.expect("exists"),
            "serial artifact kept"
        );
    })
    .await;
}

/// One active build per device: a request while a build of the device is
/// queued/running → 409 build-in-progress, no new record.
#[tokio::test]
#[serial]
async fn second_build_while_one_is_in_flight_is_refused() {
    use sea_orm::{ActiveModelTrait, Set};
    with_app(|server, env, ctx| async move {
        let org = personal_org(&server, &env.alice).await;
        create_device(&server, &env.alice, org, "dev-busy").await;
        // Without a tier interval (whose in-flight check would answer 429).
        lift_build_interval(&ctx, org).await;
        build_records::ActiveModel {
            device_id: Set(Some("dev-busy".into())),
            build_phase: Set("running".into()),
            org_id: Set(org),
            ..Default::default()
        }
        .insert(&ctx.db)
        .await
        .expect("insert");

        let res = post_build(&server, &env.alice, org, "dev-busy", "coloc").await;
        res.assert_status(axum_test::http::StatusCode::CONFLICT);
        let body: serde_json::Value = res.json();
        assert_eq!(body["error"], "build-in-progress", "{body}");
        let list = records(&server, &env.alice, org, "").await;
        assert_eq!(list["count"], 1);
    })
    .await;
}

/// Deleting the device cleans its build records; there is no record
/// deletion route.
#[tokio::test]
#[serial]
async fn device_deletion_cleans_its_records() {
    with_app(|server, env, _ctx| async move {
        let org = personal_org(&server, &env.alice).await;
        create_device(&server, &env.alice, org, "dev-del").await;
        let res = post_build(&server, &env.alice, org, "dev-del", "fail").await;
        res.assert_status(axum_test::http::StatusCode::CREATED);
        let id = res.json::<serde_json::Value>()["build_id"]
            .as_i64()
            .expect("id");
        let no_route = server
            .delete(&format!("/api/v1/build-records/{id}"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await;
        assert!(no_route.status_code().is_client_error());

        let devices: serde_json::Value = server
            .get("/api/v1/devices")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await
            .json();
        let device_pk = devices["results"][0]["id"].as_i64().expect("pk");
        server
            .delete(&format!("/api/v1/devices/{device_pk}"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await
            .assert_status(axum_test::http::StatusCode::NO_CONTENT);
        let after: serde_json::Value = records(&server, &env.alice, org, "").await;
        assert_eq!(after["count"], 0, "records cleaned with the device");
    })
    .await;
}

/// Liste : enveloppe D14 + filtres device_id/build_phase. (Deux builds en
/// échec : un succès bloquerait le second via l'intervalle 429.)
#[tokio::test]
#[serial]
async fn liste_enveloppe_et_filtres() {
    with_app(|server, env, _ctx| async move {
        let org = personal_org(&server, &env.alice).await;
        create_device(&server, &env.alice, org, "dev-f1").await;
        create_device(&server, &env.alice, org, "dev-f2").await;

        for device in ["dev-f1", "dev-f2"] {
            let res = post_build(&server, &env.alice, org, device, "fail").await;
            res.assert_status(axum_test::http::StatusCode::CREATED);
        }

        let all = records(&server, &env.alice, org, "?limit=1&offset=1").await;
        assert_eq!(all["count"], 2);
        assert!(all["next"].is_null(), "dernière page : {all}");
        assert!(all["previous"].is_string(), "lien previous présent : {all}");
        assert_eq!(all["results"].as_array().map(Vec::len), Some(1));

        let by_device = records(&server, &env.alice, org, "?device_id=dev-f1").await;
        assert_eq!(by_device["count"], 1);
        assert_eq!(by_device["results"][0]["device_id"], "dev-f1");

        let by_failed = records(&server, &env.alice, org, "?build_phase=failed").await;
        assert_eq!(by_failed["count"], 2);
        let by_success = records(&server, &env.alice, org, "?build_phase=succeeded").await;
        assert_eq!(by_success["count"], 0);
    })
    .await;
}

/// Isolation org : bob ne voit ni ne touche les builds d'alice ; sans
/// token → 401.
#[tokio::test]
#[serial]
async fn isolation_org_et_auth() {
    with_app(|server, env, _ctx| async move {
        let alice_org = personal_org(&server, &env.alice).await;
        let bob_org = personal_org(&server, &env.bob).await;
        create_device(&server, &env.alice, alice_org, "dev-iso").await;
        post_build(&server, &env.alice, alice_org, "dev-iso", "coloc").await;

        // Bob : liste vide, download 404, build sur le device
        // d'alice → 404 (le device n'existe pas dans SON org).
        let bob_list = records(&server, &env.bob, bob_org, "").await;
        assert_eq!(bob_list["count"], 0);

        server
            .get("/api/v1/download/firmware/dev-iso")
            .add_header("Authorization", bearer(&env.bob))
            .add_header("X-Org-Id", bob_org.to_string())
            .await
            .assert_status(axum_test::http::StatusCode::NOT_FOUND);

        let cross = post_build(&server, &env.bob, bob_org, "dev-iso", "coloc").await;
        cross.assert_status(axum_test::http::StatusCode::NOT_FOUND);

        // Sans token → 401 sur tous les endpoints builds.
        for (method, path) in [
            ("POST", "/api/v1/build-firmware"),
            ("GET", "/api/v1/build-records"),
            ("GET", "/api/v1/download/firmware/dev-iso"),
        ] {
            let res = match method {
                "POST" => server.post(path).json(&serde_json::json!({})).await,
                _ => server.get(path).await,
            };
            res.assert_status(axum_test::http::StatusCode::UNAUTHORIZED);
        }
    })
    .await;
}

/// Vault (secrets.md S6): a build referencing a WiFi entry compiles the
/// password decrypted by the worker; the legacy typed request lands in
/// the referential (and the vault) first.
#[tokio::test]
#[serial]
async fn build_with_a_wifi_entry_uses_the_vault_password() {
    with_app(|server, env, ctx| async move {
        let org = personal_org(&server, &env.alice).await;
        create_device(&server, &env.alice, org, "capteur-vault").await;

        let entry: serde_json::Value = server
            .post("/api/v1/edge/wifi-credentials")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .json(&serde_json::json!({ "ssid": "atelier", "password": { "value": "vault-pass" } }))
            .await
            .json();
        let entry_id = entry["id"].as_i64().expect("entry id");
        assert!(!entry.to_string().contains("vault-pass"), "{entry}");

        let res = server
            .post("/api/v1/build-firmware")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .json(&serde_json::json!({
                "wifi_credential_id": entry_id,
                "pnex_host": "dev1.pnex.io",
                "device_id": "capteur-vault",
            }))
            .await;
        res.assert_status(axum_test::http::StatusCode::CREATED);
        let dl = server
            .get("/api/v1/download/firmware/capteur-vault")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await;
        dl.assert_status(axum_test::http::StatusCode::OK);
        let content = dl.text();
        let b64 = |v: &str| base64::engine::general_purpose::STANDARD.encode(v);
        assert!(
            content.contains(&format!("ssid={}", b64("atelier"))),
            "{content}"
        );
        assert!(
            content.contains(&format!("pass={}", b64("vault-pass"))),
            "{content}"
        );

        // Another org's entry is not usable.
        let other = personal_org(&server, &env.bob).await;
        let res = server
            .post("/api/v1/build-firmware")
            .add_header("Authorization", bearer(&env.bob))
            .add_header("X-Org-Id", other.to_string())
            .json(&serde_json::json!({
                "wifi_credential_id": entry_id,
                "pnex_host": "dev1.pnex.io",
                "device_id": "capteur-vault",
            }))
            .await;
        res.assert_status(axum_test::http::StatusCode::BAD_REQUEST);

        // Every entry holds its value in the vault.
        let row = pnex_backend::models::_entities::wifi_credentials::Entity::find()
            .all(&ctx.db)
            .await
            .unwrap();
        assert!(row.iter().all(|r| r.secret_id.is_some()));
    })
    .await;
}
