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

/// `POST /internal/media/segment` (D160, `capture_on = worker`): service
/// token, stream owned by the org and captured by a worker, WAV bounded by
/// the stream's segment length, re-sent `seq` acknowledged once.
#[tokio::test]
#[serial]
async fn worker_segment_upload() {
    let base = common::spawn_mock_rauthy().await;
    unsafe { std::env::set_var("RAUTHY_URL", &base) };
    unsafe { std::env::set_var("PNEX_FLOW_RUNTIME_TOKEN", "tok-media-worker") };
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
                .json::<Value>()["orgs"][0]["id"]
                .as_i64()
                .unwrap();
            let auth = |r: axum_test::TestRequest| {
                r.add_header("Authorization", format!("Bearer {alice}"))
                    .add_header("X-Org-Id", org.to_string())
            };
            let created = auth(server.post("/api/v1/media/streams"))
                .json(&json!({ "name": "Remote", "kind": "icecast",
                "url": "https://icecast.radio.example/live.mp3", "segment_secs": 10 }))
                .await;
            assert_eq!(created.status_code(), 201, "{}", created.text());
            let id = created.json::<Value>()["id"].as_str().unwrap().to_string();

            let wav = capture::segmenter::wav_bytes(&vec![0i16; 11 * 16_000]);
            let path = |org: i64, seq: i64| {
                format!(
                    "/internal/media/segment?stream_id={id}&org_id={org}&seq={seq}\
                 &started_ms=1760000000000&ended_ms=1760000011000&clock=host"
                )
            };
            let post = |p: String, token: &'static str, body: Vec<u8>| {
                server
                    .post(&p)
                    .add_header("x-pnex-flow-token", token)
                    .bytes(body.into())
            };

            assert_eq!(
                post(path(org, 0), "wrong", wav.clone()).await.status_code(),
                401
            );
            // Still a server capture: no worker upload.
            assert_eq!(
                post(path(org, 0), "tok-media-worker", wav.clone())
                    .await
                    .status_code(),
                404
            );
            let patched = auth(server.patch(&format!("/api/v1/media/streams/{id}")))
                .json(&json!({ "capture_on": "worker" }))
                .await;
            assert_eq!(patched.status_code(), 200, "{}", patched.text());
            assert_eq!(
                post(path(org + 1000, 0), "tok-media-worker", wav.clone())
                    .await
                    .status_code(),
                404
            );
            assert_eq!(
                post(path(org, 0), "tok-media-worker", b"not a wav".to_vec())
                    .await
                    .status_code(),
                400
            );
            let too_long = capture::segmenter::wav_bytes(&vec![0i16; 30 * 16_000]);
            assert_eq!(
                post(path(org, 0), "tok-media-worker", too_long)
                    .await
                    .status_code(),
                400
            );

            assert_eq!(
                post(path(org, 0), "tok-media-worker", wav.clone())
                    .await
                    .status_code(),
                204
            );
            assert_eq!(
                post(path(org, 0), "tok-media-worker", wav.clone())
                    .await
                    .status_code(),
                204
            );
            let rows = media_segments::Entity::find()
                .filter(media_segments::Column::StreamId.eq(id.parse::<uuid::Uuid>().unwrap()))
                .all(&ctx.db)
                .await
                .unwrap();
            assert_eq!(rows.len(), 1, "a re-sent segment is stored once");
            assert_eq!(rows[0].org_id, org);
            assert_eq!(rows[0].state, "captured", "no profile: not queued");
            assert_eq!(rows[0].size_bytes, wav.len() as i64);
        },
    )
    .await;
    unsafe { std::env::remove_var("PNEX_FLOW_RUNTIME_TOKEN") };
}

/// Health sample from real segment rows (D160): gap since the last
/// segment, lag of the oldest pending one, coverage over the hour.
#[tokio::test]
#[serial]
async fn health_sample_reads_segment_rows() {
    use pnex_backend::services::media_ingest::{health, segments};
    use pnex_core::media_ingest::ClockSource;
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
                .json::<Value>()["orgs"][0]["id"]
                .as_i64()
                .unwrap();
            let created = server
                .post("/api/v1/media/streams")
                .add_header("Authorization", format!("Bearer {alice}"))
                .add_header("X-Org-Id", org.to_string())
                .json(&json!({ "name": "Health", "kind": "icecast",
                "url": "https://icecast.radio.example/live.mp3" }))
                .await;
            let id: uuid::Uuid = created.json::<Value>()["id"]
                .as_str()
                .unwrap()
                .parse()
                .unwrap();
            let stream = media_streams::Entity::find_by_id(id)
                .one(&ctx.db)
                .await
                .unwrap()
                .unwrap();
            let store = MediaSettings::from_config(&ctx.config).store().unwrap();
            let now = chrono::Utc::now();
            let at = |secs: i64| now - chrono::Duration::seconds(secs);
            for (seq, start) in [(0, 400), (1, 100)] {
                segments::write(
                    &ctx.db,
                    &store,
                    &stream,
                    segments::NewSegment {
                        seq,
                        started_at: at(start),
                        ended_at: at(start - 30),
                        clock_source: ClockSource::Host,
                        wav: capture::segmenter::wav_bytes(&[0i16; 16]),
                    },
                )
                .await
                .unwrap();
            }
            let first = media_segments::Entity::find()
                .filter(media_segments::Column::Seq.eq(0))
                .one(&ctx.db)
                .await
                .unwrap()
                .unwrap();
            let mut done: media_segments::ActiveModel = first.into();
            done.state = sea_orm::Set("transcribed".into());
            sea_orm::ActiveModelTrait::update(done, &ctx.db)
                .await
                .unwrap();

            // Configured long ago: the gap counts from the last segment.
            let mut stream = stream;
            stream.updated_at = at(7200).into();
            let h = health::sample(&ctx.db, &stream, now).await.unwrap();
            assert!(!h.up);
            assert!((60..=72).contains(&h.gap_secs), "{h:?}");
            assert!((100..=112).contains(&h.lag_secs), "{h:?}");
            assert!((h.coverage - 30.0 / 3600.0).abs() < 1e-9, "{h:?}");
        },
    )
    .await;
}
