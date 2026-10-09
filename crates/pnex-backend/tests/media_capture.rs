//! Media capture chain (media-ingest.md D160–D162) end to end on a local
//! source: HTTP fetcher → confined ffmpeg → segmenter → MediaStore +
//! `media_segments`. Skipped when ffmpeg is not installed.
//!
//! Needs PostgreSQL (TEST_DATABASE_URL), database emptied between tests.

mod common;

use std::time::Duration;

use loco_rs::testing::request::{RequestConfig, RequestConfigBuilder};
use pnex_backend::app::App;
use pnex_backend::models::_entities::{media_segments, media_streams};
use pnex_backend::services::media::MediaSettings;
use pnex_backend::services::media_ingest::capture::{self, decoder, CaptureSettings};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder};
use serde_json::{json, Value};
use serial_test::serial;

/// 25 s of a 440 Hz tone, 16 kHz mono WAV.
fn tone_wav() -> Vec<u8> {
    let samples: Vec<i16> = (0..25 * 16_000)
        .map(|i| ((i as f32 * 440.0 * std::f32::consts::TAU / 16_000.0).sin() * 8_000.0) as i16)
        .collect();
    pnex_backend::services::media_ingest::capture::segmenter::wav_bytes(&samples)
}

/// Serves the tone on 127.0.0.1 (tests run with egress `open`).
async fn serve_tone() -> String {
    let wav = tone_wav();
    let app = axum::Router::new().route(
        "/episode.wav",
        axum::routing::get(move || {
            let wav = wav.clone();
            async move { ([(axum::http::header::CONTENT_TYPE, "audio/wav")], wav) }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://{addr}/episode.wav")
}

#[tokio::test]
#[serial]
async fn http_file_is_cut_into_overlapping_segments() {
    if decoder::resolve(&CaptureSettings::from_env().ffmpeg).is_none() {
        eprintln!("ffmpeg not installed: skipped");
        return;
    }
    let base = common::spawn_mock_rauthy().await;
    unsafe { std::env::set_var("RAUTHY_URL", &base) };
    let alice = common::valid_token(
        &base,
        "00000000-0000-0000-0000-00000000000a",
        "alice",
        "alice@example.com",
    );
    let url = serve_tone().await;
    let config: RequestConfig = RequestConfigBuilder::new().build();
    loco_rs::testing::request::request_with_config::<App, _, _>(config, move |server, ctx| async move {
        common::seed_catalogue(&ctx.db).await;
        let org = server
            .get("/api/v1/user-info")
            .add_header("Authorization", format!("Bearer {alice}"))
            .await
            .json::<Value>()["orgs"][0]["id"]
            .as_i64()
            .unwrap();
        let created = server
            .post("/api/v1/media/streams")
            .add_header("Authorization", format!("Bearer {alice}"))
            .add_header("X-Org-Id", org.to_string())
            .json(&json!({ "name": "Episode", "kind": "http_file", "url": url, "segment_secs": 10 }))
            .await;
        assert_eq!(created.status_code(), 201, "{}", created.text());
        let id: uuid::Uuid = created.json::<Value>()["id"].as_str().unwrap().parse().unwrap();
        let stream = media_streams::Entity::find_by_id(id).one(&ctx.db).await.unwrap().unwrap();

        let stored = capture::capture_once(&ctx, &stream, Duration::from_secs(60))
            .await
            .expect("capture");
        assert_eq!(stored, 3, "0–11 s, 10–21 s, 20–25 s");

        let rows = media_segments::Entity::find()
            .filter(media_segments::Column::StreamId.eq(id))
            .order_by_asc(media_segments::Column::Seq)
            .all(&ctx.db)
            .await
            .unwrap();
        assert_eq!(rows.len(), 3);
        let secs = |r: &media_segments::Model| (r.ended_at - r.started_at).num_milliseconds();
        assert_eq!(secs(&rows[0]), 11_000);
        assert_eq!(secs(&rows[1]), 11_000);
        assert_eq!(secs(&rows[2]), 5_000);
        assert_eq!((rows[1].started_at - rows[0].started_at).num_seconds(), 10);
        let store = MediaSettings::from_config(&ctx.config).store().unwrap();
        for r in &rows {
            assert_eq!(r.state, "captured");
            assert_eq!(r.clock_source, "host");
            assert_eq!(r.org_id, org);
            let key = r.storage_key.as_deref().unwrap();
            assert!(key.starts_with(&format!("org_{org}/media/{id}/")), "{key}");
            assert!(store.exists(key).await.unwrap());
        }
        assert_eq!(rows[0].size_bytes, 44 + 11 * 16_000 * 2);
    })
    .await;
}
