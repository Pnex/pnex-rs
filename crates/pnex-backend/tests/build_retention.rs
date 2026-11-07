//! Build record retention (`services::build_retention`): the newest records
//! of a device are kept, and so are a running build, the version the device
//! runs and the target of an active OTA, with their artifacts.
//!
//! Requires PostgreSQL (TEST_DATABASE_URL).

mod common;

use loco_rs::testing::request::{RequestConfig, RequestConfigBuilder};
use pnex_backend::app::App;
use pnex_backend::models::_entities::{build_records, device_registries, ota_assignments};
use pnex_backend::services::build_retention::{prune_device_builds, KEEP_PER_DEVICE};
use pnex_firmware_builder::{ota_artifact_key, ArtifactStore, InMemoryStore};
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};
use serial_test::serial;

#[tokio::test]
#[serial]
async fn prune_keeps_newest_running_installed_and_ota_target() {
    let base = common::spawn_mock_rauthy().await;
    unsafe { std::env::set_var("RAUTHY_URL", &base) };
    let alice = common::valid_token(
        &base,
        "00000000-0000-0000-0000-00000000000a",
        "alice",
        "alice@example.com",
    );
    let config: RequestConfig = RequestConfigBuilder::new().build();
    loco_rs::testing::request::request_with_config::<App, _, _>(
        config,
        move |server, ctx| async move {
            common::seed_catalogue(&ctx.db).await;
            let org = server
                .get("/api/v1/user-info")
                .add_header("Authorization", format!("Bearer {alice}"))
                .await
                .json::<serde_json::Value>()["orgs"][0]["id"]
                .as_i64()
                .unwrap();
            let res = server
                .post("/api/v1/devices")
                .add_header("Authorization", format!("Bearer {alice}"))
                .add_header("X-Org-Id", org.to_string())
                .json(&serde_json::json!({
                    "device_id": "ret-dev", "predefined_device_name": "generic_esp8266",
                }))
                .await;
            res.assert_status(axum_test::http::StatusCode::CREATED);
            let device_pk = res.json::<serde_json::Value>()["id"].as_i64().unwrap();

            let store = InMemoryStore::default();
            let shared_bin = "org/firmware/ret-dev-firmware.bin";
            store.put(shared_bin, b"bin").await.unwrap();
            // v1 (OTA target), v2 (running), v3 (installed), v4 (doomed), v5..v9 (newest).
            let total = KEEP_PER_DEVICE + 4;
            for n in 1..=total {
                let version = format!("v{n}");
                store
                    .put(&ota_artifact_key(org, "ret-dev", &version), b"ota")
                    .await
                    .unwrap();
                build_records::ActiveModel {
                    org_id: Set(org),
                    device_id: Set(Some("ret-dev".into())),
                    build_phase: Set(if n == 2 { "running" } else { "succeeded" }.into()),
                    firmware_bin_s3_key: Set(Some(shared_bin.into())),
                    fw_version: Set(Some(version)),
                    ..Default::default()
                }
                .insert(&ctx.db)
                .await
                .unwrap();
            }
            let device = device_registries::Entity::find_by_id(device_pk)
                .one(&ctx.db)
                .await
                .unwrap()
                .unwrap();
            let mut device: device_registries::ActiveModel = device.into();
            device.fw_version = Set(Some("v3".into()));
            device.update(&ctx.db).await.unwrap();
            ota_assignments::ActiveModel {
                org_id: Set(org),
                device_registry_id: Set(device_pk),
                target_version: Set("v1".into()),
                artifact_key: Set(ota_artifact_key(org, "ret-dev", "v1")),
                sha256: Set("00".into()),
                state: Set("pending".into()),
                ..Default::default()
            }
            .insert(&ctx.db)
            .await
            .unwrap();

            let deleted = prune_device_builds(&ctx.db, &store, org, "ret-dev")
                .await
                .unwrap();
            assert_eq!(deleted, 1);
            let left: Vec<String> = build_records::Entity::find()
                .filter(build_records::Column::DeviceId.eq("ret-dev"))
                .all(&ctx.db)
                .await
                .unwrap()
                .into_iter()
                .filter_map(|r| r.fw_version)
                .collect();
            assert!(!left.contains(&"v4".to_string()), "{left:?}");
            assert_eq!(left.len(), total - 1);
            let has = |k: String| {
                let store = &store;
                async move { store.exists(&k).await.unwrap() }
            };
            assert!(!has(ota_artifact_key(org, "ret-dev", "v4")).await);
            for kept in ["v1", "v2", "v3", "v9"] {
                assert!(has(ota_artifact_key(org, "ret-dev", kept)).await, "{kept}");
            }
            assert!(has(shared_bin.to_string()).await, "shared serial bin kept");
            // Nothing more to prune.
            assert_eq!(
                prune_device_builds(&ctx.db, &store, org, "ret-dev")
                    .await
                    .unwrap(),
                0
            );
        },
    )
    .await;
}
