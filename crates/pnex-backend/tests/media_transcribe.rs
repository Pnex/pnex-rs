//! Audio model registry and transcription chain (media-ingest.md D166–D167)
//! with a fake `pnex-asr` (tests/fixtures/asr): import of a sherpa-style
//! archive, family read from the files, check on the reference sample,
//! profile, enabled stream, capture → queued → transcribed by the worker
//! (run inline: `ForegroundBlocking`). O2 is not configured in the tests,
//! so the chain stops at the transcript write with `o2-failed`: every step
//! before it ran.
//!
//! Needs PostgreSQL (TEST_DATABASE_URL); the capture part needs ffmpeg.

mod common;

use std::time::Duration;

use loco_rs::testing::request::{RequestConfig, RequestConfigBuilder};
use pnex_backend::app::App;
use pnex_backend::models::_entities::{media_segments, media_streams};
use pnex_backend::services::media_ingest::capture::{self, decoder, CaptureSettings};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use serde_json::{json, Value};
use serial_test::serial;

fn fake_runtime() -> String {
    format!(
        "{}/tests/fixtures/asr/fake-pnex-asr.sh",
        env!("CARGO_MANIFEST_DIR")
    )
}

/// sherpa-onnx release layout, dummy weights.
fn parakeet_archive() -> Vec<u8> {
    let mut b = tar::Builder::new(Vec::new());
    for name in [
        "encoder.int8.onnx",
        "decoder.int8.onnx",
        "joiner.int8.onnx",
        "tokens.txt",
    ] {
        let mut h = tar::Header::new_gnu();
        h.set_size(4);
        h.set_mode(0o644);
        h.set_entry_type(tar::EntryType::Regular);
        b.append_data(&mut h, format!("sherpa-onnx-test/{name}"), &b"fake"[..])
            .unwrap();
    }
    let raw = b.into_inner().unwrap();
    let mut out = Vec::new();
    std::io::copy(
        &mut bzip2::read::BzEncoder::new(raw.as_slice(), bzip2::Compression::fast()),
        &mut out,
    )
    .unwrap();
    out
}

fn tone_server_wav() -> Vec<u8> {
    let samples: Vec<i16> = (0..12 * 16_000)
        .map(|i| ((i as f32 * 440.0 * std::f32::consts::TAU / 16_000.0).sin() * 8_000.0) as i16)
        .collect();
    capture::segmenter::wav_bytes(&samples)
}

async fn serve_tone() -> String {
    let wav = tone_server_wav();
    let app = axum::Router::new().route(
        "/ep.wav",
        axum::routing::get(move || {
            let wav = wav.clone();
            async move { ([(axum::http::header::CONTENT_TYPE, "audio/wav")], wav) }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://{addr}/ep.wav")
}

#[tokio::test]
#[serial]
async fn model_import_check_and_transcription_chain() {
    let models_dir =
        std::env::temp_dir().join(format!("pnex-asr-test-models-{}", std::process::id()));
    unsafe {
        std::env::set_var("PNEX_ASR_BIN", fake_runtime());
        std::env::set_var("PNEX_ASR_MODELS_DIR", &models_dir);
    }
    let base = common::spawn_mock_rauthy().await;
    unsafe { std::env::set_var("RAUTHY_URL", &base) };
    let alice = common::valid_token(
        &base,
        "00000000-0000-0000-0000-00000000000a",
        "alice",
        "alice@example.com",
    );
    let has_ffmpeg = decoder::resolve(&CaptureSettings::from_env().ffmpeg).is_some();
    let url = serve_tone().await;
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
            let post = |path: &str, body: Value| {
                server
                    .post(path)
                    .add_header("Authorization", auth.clone())
                    .add_header("X-Org-Id", org.to_string())
                    .json(&body)
            };

            // Upload: an archive is a model asset when the client says so.
            let upload = |name: &str, bytes: Vec<u8>| {
                server
                    .post(&format!("/api/v1/media?filename={name}&kind=model"))
                    .add_header("Authorization", auth.clone())
                    .add_header("X-Org-Id", org.to_string())
                    .add_header("Content-Type", "application/octet-stream")
                    .bytes(bytes.into())
            };
            let asset = upload("parakeet.tar.bz2", parakeet_archive()).await;
            assert!(asset.status_code().is_success(), "{}", asset.text());
            let asset_id = asset.json::<Value>()["id"].as_str().unwrap().to_string();
            let junk = upload("junk.tar.bz2", b"not an archive".to_vec()).await;
            let junk_id = junk.json::<Value>()["id"].as_str().unwrap().to_string();

            // Import: license required, junk refused, the family is read.
            let res = post(
                "/api/v1/asr/models",
                json!({ "name": "Parakeet", "asset_id": asset_id }),
            )
            .await;
            assert_eq!(res.status_code(), 400);
            assert_eq!(res.json::<Value>()["license"], "required");
            let res = post(
                "/api/v1/asr/models",
                json!({ "name": "Junk", "asset_id": junk_id, "license": "MIT" }),
            )
            .await;
            assert_eq!(res.status_code(), 400, "{}", res.text());
            assert_eq!(res.json::<Value>()["error"], "asr-model-unsupported");
            let res = post(
                "/api/v1/asr/models",
                json!({ "name": "Parakeet", "asset_id": asset_id, "license": "CC-BY-4.0" }),
            )
            .await;
            assert_eq!(res.status_code(), 201, "{}", res.text());
            let model = res.json::<Value>();
            assert_eq!(model["family"], "parakeet_tdt");
            assert_eq!(model["license"], "CC-BY-4.0");
            assert_eq!(model["check"]["status"], "valid", "{model}");
            assert!(model["check"]["rtf"].as_f64().unwrap() > 0.0);
            let wer = model["check"]["wer"].as_f64().unwrap();
            assert!(wer > 0.0 && wer < 1.0, "partial sentence: {wer}");
            let model_id = model["id"].as_str().unwrap().to_string();

            // Audio models stay out of the vision registry, and back.
            let vision = server
                .get("/api/v1/ml/models")
                .add_header("Authorization", auth.clone())
                .add_header("X-Org-Id", org.to_string())
                .await
                .json::<Value>();
            assert_eq!(vision["count"], 0, "{vision}");
            let check = post(&format!("/api/v1/asr/models/{model_id}/check"), json!({})).await;
            assert_eq!(check.status_code(), 200);

            // Profile + enabled stream.
            let res = post(
                "/api/v1/asr/profiles",
                json!({ "name": "FR", "asr_model_id": model_id }),
            )
            .await;
            assert_eq!(res.status_code(), 201, "{}", res.text());
            let profile_id = res.json::<Value>()["id"].as_str().unwrap().to_string();
            let res = post(
                "/api/v1/media/streams",
                json!({
                    "name": "Episode", "kind": "http_file", "url": url,
                    "segment_secs": 10, "asr_profile_id": profile_id, "enabled": true
                }),
            )
            .await;
            assert_eq!(res.status_code(), 201, "{}", res.text());
            let stream = res.json::<Value>();
            assert_eq!(stream["enabled"], true);
            let stream_id: uuid::Uuid = stream["id"].as_str().unwrap().parse().unwrap();

            // Transcript search: O2 not configured = empty page; a slug that is
            // not the org's = hidden 404.
            let res = server
                .get("/api/v1/media/transcripts?stream=episode&q=lune")
                .add_header("Authorization", auth.clone())
                .add_header("X-Org-Id", org.to_string())
                .await;
            assert_eq!(res.status_code(), 200, "{}", res.text());
            assert_eq!(res.json::<Value>()["count"], 0);
            let res = server
                .get("/api/v1/media/transcripts?stream=someone_else")
                .add_header("Authorization", auth.clone())
                .add_header("X-Org-Id", org.to_string())
                .await;
            assert_eq!(res.status_code(), 404);

            if !has_ffmpeg {
                eprintln!("ffmpeg not installed: capture part skipped");
                return;
            }
            let row = media_streams::Entity::find_by_id(stream_id)
                .one(&ctx.db)
                .await
                .unwrap()
                .unwrap();
            let stored = capture::capture_once(&ctx, &row, Duration::from_secs(60))
                .await
                .expect("capture");
            assert_eq!(stored, 2, "0–11 s and 10–12 s");
            let segs = media_segments::Entity::find()
                .filter(media_segments::Column::StreamId.eq(stream_id))
                .all(&ctx.db)
                .await
                .unwrap();
            assert_eq!(segs.len(), 2);
            for s in &segs {
                // Transcribed by the fake runtime, then stopped at the O2 write.
                assert_eq!(s.state, "failed", "{s:?}");
                assert_eq!(s.error.as_deref(), Some("o2-failed"), "{s:?}");
                assert!(
                    s.storage_key.is_some(),
                    "failed segments keep their audio (D161)"
                );
            }

            // Replay: failed segments with audio go back to the queue (run
            // inline here, they fail again at the O2 write).
            let res = post(
                &format!("/api/v1/media/streams/{stream_id}/segments/retry"),
                json!({}),
            )
            .await;
            assert_eq!(res.status_code(), 200, "{}", res.text());
            assert_eq!(res.json::<Value>()["requeued"], 2);

            // Past the replay window, the pruner deletes their audio.
            let store = pnex_backend::services::media::MediaSettings::from_config(&ctx.config)
                .store()
                .unwrap();
            let prune =
                || pnex_backend::services::media_ingest::segments::prune_audio(&ctx.db, &store);
            assert_eq!(prune().await.unwrap(), 0, "inside the window");
            let old: sea_orm::prelude::DateTimeWithTimeZone =
                (chrono::Utc::now() - chrono::Duration::hours(1)).into();
            media_segments::Entity::update_many()
                .col_expr(
                    media_segments::Column::UpdatedAt,
                    sea_orm::sea_query::Expr::value(old),
                )
                .filter(media_segments::Column::StreamId.eq(stream_id))
                .exec(&ctx.db)
                .await
                .unwrap();
            let keys: Vec<String> = segs.iter().filter_map(|s| s.storage_key.clone()).collect();
            assert_eq!(prune().await.unwrap(), 2);
            for k in keys {
                assert!(!store.exists(&k).await.unwrap(), "{k}");
            }
        },
    )
    .await;
    let _ = std::fs::remove_dir_all(models_dir);
}
