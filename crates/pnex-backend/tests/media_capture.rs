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

        // Stream test without a profile: captured, not transcribed.
        let tested = server
            .post(&format!("/api/v1/media/streams/{id}/test"))
            .add_header("Authorization", format!("Bearer {alice}"))
            .add_header("X-Org-Id", org.to_string())
            .await;
        assert_eq!(tested.status_code(), 200, "{}", tested.text());
        let t = tested.json::<Value>();
        assert_eq!(t["audio_ms"], 10_000, "{t}");
        assert_eq!(t["transcribe_error"], "no-profile");
        let state = media_streams::Entity::find_by_id(id).one(&ctx.db).await.unwrap().unwrap();
        assert_eq!(state.capture_state, "stopped", "a test leaves the capture state alone");

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

/// Serves `audio` as an icecast stream: with `icy-metaint: 1024` and one
/// title block after each cut when the client asks `Icy-MetaData: 1` and
/// `icy` is set, as plain bytes otherwise.
async fn serve_icy(audio: Vec<u8>, icy: bool) -> String {
    use axum::response::IntoResponse;
    let app = axum::Router::new().route(
        "/live.mp3",
        axum::routing::get(move |headers: axum::http::HeaderMap| {
            let audio = audio.clone();
            async move {
                let asked = headers.get("icy-metadata").is_some_and(|v| v == "1");
                if !(icy && asked) {
                    return ([("content-type", "audio/mpeg".to_string())], audio).into_response();
                }
                let block = b"StreamTitle='Artiste - Morceau';";
                let len = block.len().div_ceil(16);
                let mut body = Vec::new();
                for chunk in audio.chunks(1024) {
                    body.extend_from_slice(chunk);
                    if chunk.len() == 1024 {
                        body.push(len as u8);
                        let mut padded = block.to_vec();
                        padded.resize(len * 16, 0);
                        body.extend_from_slice(&padded);
                    }
                }
                (
                    [
                        ("content-type", "audio/mpeg".to_string()),
                        ("icy-metaint", "1024".to_string()),
                    ],
                    body,
                )
                    .into_response()
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://{addr}/live.mp3")
}

/// D170: the icecast fetcher strips interleaved ICY blocks (only audio
/// reaches the decoder) and hands the title over once; a server without
/// `icy-metaint` is read exactly as before.
#[tokio::test]
#[serial]
async fn icecast_fetcher_strips_icy_metadata() {
    use pnex_backend::services::media_ingest::capture::fetch::Fetcher;
    use pnex_core::media_ingest::MediaStreamKind;
    // Booting the app sets the egress policy of the tests (`open`).
    let base = common::spawn_mock_rauthy().await;
    unsafe { std::env::set_var("RAUTHY_URL", &base) };
    let config: RequestConfig = RequestConfigBuilder::new().build();
    loco_rs::testing::request::request_with_config::<App, _, _>(
        config,
        |_server, _ctx| async move {
            let audio: Vec<u8> = (0..10_000u32).map(|i| (i % 251) as u8).collect();
            for icy in [true, false] {
                let url: reqwest::Url = serve_icy(audio.clone(), icy).await.parse().unwrap();
                let opened = Fetcher::new(None)
                    .unwrap()
                    .open(MediaStreamKind::Icecast, &url)
                    .await
                    .expect("opened");
                let mut chunks = opened.chunks;
                let mut got = Vec::new();
                while let Some(c) = chunks.recv().await {
                    got.extend_from_slice(&c.expect("chunk"));
                }
                assert_eq!(got, audio, "only audio reaches the decoder (icy = {icy})");
                match opened.metadata {
                    Some(mut events) => {
                        assert!(icy);
                        let first = events.recv().await.expect("one title");
                        assert_eq!(first.title, "Artiste - Morceau");
                        // Same title repeated in every block: emitted once.
                        assert!(events.recv().await.is_none());
                    }
                    None => assert!(!icy, "metadata expected"),
                }
            }
        },
    )
    .await;
}

/// 3 s of 1920x1080 H.264 + AAC in MPEG-TS, made by the system ffmpeg
/// (`None` when it has no H.264 encoder).
fn h264_ts() -> Option<Vec<u8>> {
    let dir = tempfile::tempdir().ok()?;
    let out = dir.path().join("cam.ts");
    let ok = std::process::Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "lavfi",
            "-i",
            "testsrc=size=1920x1080:rate=10",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:sample_rate=48000",
            "-t",
            "3",
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-pix_fmt",
            "yuv420p",
            "-c:a",
            "aac",
            "-f",
            "mpegts",
            "-y",
        ])
        .arg(&out)
        .status()
        .ok()?
        .success();
    ok.then(|| std::fs::read(&out).ok()).flatten()
}

/// Video track (D175): a TS file over HTTP with `tracks = video` → frames
/// sampled at `fps`, scaled to 1280 px, published as JPEG on the stream's
/// own camera bus keys (never a device key).
#[tokio::test]
#[serial]
async fn video_track_frames_reach_the_stream_bus() {
    if decoder::resolve(&CaptureSettings::from_env().ffmpeg).is_none() {
        eprintln!("ffmpeg not installed: skipped");
        return;
    }
    let Some(ts) = h264_ts() else {
        eprintln!("no H.264 encoder in the system ffmpeg: skipped");
        return;
    };
    let base = common::spawn_mock_rauthy().await;
    unsafe { std::env::set_var("RAUTHY_URL", &base) };
    let alice = common::valid_token(
        &base,
        "00000000-0000-0000-0000-00000000000a",
        "alice",
        "alice@example.com",
    );
    let app = axum::Router::new().route(
        "/cam.ts",
        axum::routing::get(move || {
            let ts = ts.clone();
            async move { ([(axum::http::header::CONTENT_TYPE, "video/mp2t")], ts) }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let url = format!("http://{addr}/cam.ts");
    let config: RequestConfig = RequestConfigBuilder::new().build();
    loco_rs::testing::request::request_with_config::<App, _, _>(
        config,
        move |server, ctx| async move {
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
                .json(
                    &json!({ "name": "Gate cam", "kind": "http_file", "url": url,
                           "tracks": "video", "fps": 2 }),
                )
                .await;
            assert_eq!(created.status_code(), 201, "{}", created.text());
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

            let valkey_url = std::env::var("PNEX_TEST_APP_VALKEY_URL")
                .unwrap_or_else(|_| "redis://127.0.0.1:6379/14".into());
            let client = redis::Client::open(valkey_url.as_str()).unwrap();
            let mut pubsub = client.get_async_pubsub().await.expect("valkey");
            let channel = pnex_core::media_ingest::frame_channel(org, &stream.slug);
            pubsub.subscribe(&channel).await.unwrap();
            // Listen while the capture runs.
            let listener = tokio::spawn(async move {
                use futures_util::StreamExt;
                let mut messages = pubsub.on_message();
                let mut metas = Vec::new();
                while let Ok(Some(msg)) =
                    tokio::time::timeout(Duration::from_secs(5), messages.next()).await
                {
                    let raw: String = msg.get_payload().unwrap();
                    metas.push(
                        serde_json::from_str::<pnex_core::camera::BusFrameMeta>(&raw).unwrap(),
                    );
                }
                metas
            });

            let stored = capture::capture_once(&ctx, &stream, Duration::from_secs(60))
                .await
                .expect("capture");
            assert_eq!(stored, 0, "no audio segment for a video-only stream");
            let metas = listener.await.unwrap();
            // 3 s at 2 fps.
            assert!((5..=7).contains(&metas.len()), "{} frames", metas.len());
            let m = &metas[0];
            assert_eq!((m.width, m.height), (1280, 720));
            assert!(m
                .key
                .starts_with(&format!("pnex:media:v1:{org}:{}:cam:f:", stream.slug)));
            let mut conn = client.get_multiplexed_async_connection().await.unwrap();
            let jpeg: Vec<u8> = redis::AsyncCommands::get(&mut conn, &m.key).await.unwrap();
            assert_eq!(&jpeg[..2], &[0xFF, 0xD8]);
            assert_eq!(jpeg.len() as u32, m.size);
        },
    )
    .await;
}
