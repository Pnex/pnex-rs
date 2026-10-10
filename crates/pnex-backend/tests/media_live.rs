//! Media ingest end to end against the real stack (ignored by default):
//! real `pnex-asr` runtime, real model, real OpenObserve, real Valkey.
//!
//! ```sh
//! PNEX_O2_URL=http://localhost:5080 PNEX_O2_ROOT_PASSWORD=… \
//! PNEX_ASR_BIN=target/sherpa-only/release/pnex-asr \
//! PNEX_TEST_ASR_MODEL_DIR=~/.cache/pnex-asr-test/models/<sherpa model dir> \
//! cargo test -p pnex-backend --test media_live -- --ignored
//! ```
//!
//! The stream is the reference sample served as an HTTP file: one segment,
//! transcribed, stored in `tx_<slug>`, found by the transcript search, and
//! announced on the stream's Valkey channel.

mod common;

use std::time::Duration;

use futures_util::StreamExt;
use loco_rs::testing::request::{RequestConfig, RequestConfigBuilder};
use pnex_backend::app::App;
use pnex_backend::models::_entities::{media_segments, media_streams};
use pnex_backend::services::media_ingest::capture;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use serde_json::{json, Value};
use serial_test::serial;

/// The model directory packed as an uncompressed tar.
fn tar_dir(dir: &std::path::Path) -> Vec<u8> {
    let mut b = tar::Builder::new(Vec::new());
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_file() {
            let name = format!("model/{}", path.file_name().unwrap().to_string_lossy());
            b.append_path_with_name(&path, name).unwrap();
        }
    }
    b.into_inner().unwrap()
}

async fn serve_sample() -> String {
    let app = axum::Router::new().route(
        "/sample.wav",
        axum::routing::get(|| async {
            (
                [(axum::http::header::CONTENT_TYPE, "audio/wav")],
                pnex_asr::protocol::CHECK_WAV_FR,
            )
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://{addr}/sample.wav")
}

#[tokio::test]
#[serial]
#[ignore]
async fn live_stream_is_transcribed_into_o2_and_announced() {
    let model_dir = std::path::PathBuf::from(
        std::env::var("PNEX_TEST_ASR_MODEL_DIR").expect("PNEX_TEST_ASR_MODEL_DIR"),
    );
    assert!(std::env::var("PNEX_O2_URL").is_ok(), "PNEX_O2_URL");
    let _ = tracing_subscriber::fmt().with_test_writer().try_init();
    let base = common::spawn_mock_rauthy().await;
    unsafe { std::env::set_var("RAUTHY_URL", &base) };
    let alice = common::valid_token(
        &base,
        "00000000-0000-0000-0000-00000000000a",
        "alice",
        "alice@example.com",
    );
    let url = serve_sample().await;
    let config: RequestConfig = RequestConfigBuilder::new().build();
    loco_rs::testing::request::request_with_config::<App, _, _>(
        config,
        move |server, ctx| async move {
            common::seed_catalogue(&ctx.db).await;
            let org = server
                .get("/api/v1/user-info")
                .add_header("Authorization", format!("Bearer {alice}"))
                .await
                .json::<Value>()["orgs"][0]["id"]
                .as_i64()
                .unwrap();
            let auth = format!("Bearer {alice}");
            let call = |path: &str, body: Value| {
                server
                    .post(path)
                    .add_header("Authorization", auth.clone())
                    .add_header("X-Org-Id", org.to_string())
                    .json(&body)
            };
            let asset = server
                .post("/api/v1/media?filename=model.tar&kind=model")
                .add_header("Authorization", auth.clone())
                .add_header("X-Org-Id", org.to_string())
                .add_header("Content-Type", "application/octet-stream")
                .bytes(tar_dir(&model_dir).into())
                .await;
            assert!(asset.status_code().is_success(), "{}", asset.text());
            let asset_id = asset.json::<Value>()["id"].as_str().unwrap().to_string();
            let res = call(
                "/api/v1/asr/models",
                json!({ "name": "Live model", "asset_id": asset_id, "license": "CC-BY-4.0" }),
            )
            .await;
            assert_eq!(res.status_code(), 201, "{}", res.text());
            let model = res.json::<Value>();
            eprintln!("model check: {}", model["check"]);
            assert_eq!(model["check"]["status"], "valid", "{model}");
            assert!(model["check"]["wer"].as_f64().unwrap() < 0.3, "{model}");
            let res = call(
                "/api/v1/asr/profiles",
                json!({ "name": "FR", "asr_model_id": model["id"] }),
            )
            .await;
            let profile_id = res.json::<Value>()["id"].as_str().unwrap().to_string();
            let res = call(
                "/api/v1/media/streams",
                json!({ "name": "Lune", "kind": "http_file", "url": url, "segment_secs": 10,
                    "asr_profile_id": profile_id, "enabled": true }),
            )
            .await;
            assert_eq!(res.status_code(), 201, "{}", res.text());
            let stream_id: uuid::Uuid =
                res.json::<Value>()["id"].as_str().unwrap().parse().unwrap();

            // Listen to the stream's channel before capturing.
            let valkey = pnex_backend::services::shared_valkey::conn(&ctx.config).await;
            assert!(valkey.is_some(), "valkey");
            let url_valkey = ctx
                .config
                .settings
                .as_ref()
                .and_then(|s| s["valkey"]["url"].as_str())
                .unwrap()
                .to_string();
            let client = redis::Client::open(url_valkey).unwrap();
            let mut pubsub = client.get_async_pubsub().await.unwrap();
            pubsub
                .subscribe(pnex_core::media_ingest::transcript_channel(org, "lune"))
                .await
                .unwrap();

            let row = media_streams::Entity::find_by_id(stream_id)
                .one(&ctx.db)
                .await
                .unwrap()
                .unwrap();
            let stored = capture::capture_once(&ctx, &row, Duration::from_secs(60))
                .await
                .expect("capture");
            assert_eq!(stored, 1);
            let seg = media_segments::Entity::find()
                .filter(media_segments::Column::StreamId.eq(stream_id))
                .one(&ctx.db)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(seg.state, "transcribed", "{seg:?}");
            assert!(seg.storage_key.is_none(), "audio purged (retention none)");
            assert!(seg.asr_ms.unwrap() > 0);

            let msg = tokio::time::timeout(Duration::from_secs(5), pubsub.on_message().next())
                .await
                .expect("segment event")
                .unwrap();
            let event: Value = serde_json::from_str(&msg.get_payload::<String>().unwrap()).unwrap();
            eprintln!("event: {event}");
            assert_eq!(event["stream"], "lune");
            assert!(event["text"]
                .as_str()
                .unwrap()
                .to_lowercase()
                .contains("lune"));

            // O2 indexes asynchronously: poll the search.
            let mut found = Value::Null;
            for _ in 0..30 {
                let res = server
                    .get("/api/v1/media/transcripts?stream=lune&q=croûte")
                    .add_header("Authorization", auth.clone())
                    .add_header("X-Org-Id", org.to_string())
                    .await;
                assert_eq!(res.status_code(), 200, "{}", res.text());
                found = res.json::<Value>();
                if found["count"].as_i64().unwrap_or(0) > 0 {
                    break;
                }
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
            eprintln!("search: {found}");
            assert_eq!(found["count"], 1, "{found}");
            assert_eq!(found["results"][0]["segment_id"], seg.id.to_string());
            assert!(found["results"][0]["words"]
                .as_str()
                .unwrap()
                .starts_with('['));
        },
    )
    .await;
}
