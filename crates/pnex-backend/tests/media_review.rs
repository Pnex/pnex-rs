//! Review of a media file (media-review.md D272, D273): spans attributed to
//! persons of the ontology without any voice recognition — a person only,
//! merged spans of one person, optimistic concurrency, viewer refused (R2),
//! org isolation (R1), attributions shown by the transcript read.
//!
//! Needs PostgreSQL (TEST_DATABASE_URL), database emptied between tests.

mod common;

use loco_rs::testing::request::{RequestConfig, RequestConfigBuilder};
use pnex_backend::app::App;
use serde_json::{json, Value};
use serial_test::serial;

struct Env {
    alice: String,
    bob: String,
    ctx: loco_rs::app::AppContext,
}

async fn with_app<F, Fut>(f: F)
where
    F: FnOnce(axum_test::TestServer, Env) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let base = common::spawn_mock_rauthy().await;
    unsafe { std::env::set_var("RAUTHY_URL", &base) };
    unsafe { std::env::set_var("PNEX_FLOW_ENABLED", "false") };
    let alice = common::valid_token(
        &base,
        "00000000-0000-0000-0000-00000000000a",
        "alice",
        "alice@example.com",
    );
    let bob = common::valid_token(
        &base,
        "00000000-0000-0000-0000-00000000000b",
        "bob",
        "bob@example.com",
    );
    let config: RequestConfig = RequestConfigBuilder::new().build();
    loco_rs::testing::request::request_with_config::<App, _, _>(
        config,
        move |server, ctx| async move {
            common::seed_catalogue(&ctx.db).await;
            f(server, Env { alice, bob, ctx }).await;
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
        "PATCH" => server.patch(path),
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
    let text = res.text();
    (
        status,
        serde_json::from_str(&text).unwrap_or(Value::String(text)),
    )
}

/// A WAV header is enough for the library (content checked by its magic).
fn wav() -> Vec<u8> {
    let mut b = b"RIFF\x24\0\0\0WAVEfmt \x10\0\0\0\x01\0\x01\0\x80\x3e\0\0\0\x7d\0\0\x02\0\x10\0data\0\0\0\0".to_vec();
    b.resize(64, 0);
    b
}

async fn audio(server: &axum_test::TestServer, token: &str, org: i64) -> String {
    let res = server
        .post("/api/v1/media?filename=debate.wav&kind=audio")
        .add_header("Authorization", format!("Bearer {token}"))
        .add_header("X-Org-Id", org.to_string())
        .add_header("Content-Type", "application/octet-stream")
        .bytes(wav().into())
        .await;
    assert!(res.status_code().is_success(), "{}", res.text());
    res.json::<Value>()["id"].as_str().unwrap().to_string()
}

async fn object(
    server: &axum_test::TestServer,
    token: &str,
    org: i64,
    type_key: &str,
    title: &str,
) -> String {
    let (st, o) = call(
        server,
        "POST",
        "/api/v1/ontology/objects",
        token,
        org,
        Some(json!({"type_key": type_key, "title": title, "properties": {}})),
    )
    .await;
    assert_eq!(st, 201, "{o}");
    o["id"].as_str().unwrap().to_string()
}

fn spans(list: &[(i64, i64)]) -> Value {
    list.iter()
        .map(|(s, e)| json!({"start_ms": s, "end_ms": e}))
        .collect()
}

#[tokio::test]
#[serial]
async fn spans_are_attributed_to_persons_without_voice() {
    with_app(|server, env| async move {
        let org = personal_org(&server, &env.alice).await;
        let (st, p) = call(
            &server,
            "POST",
            "/api/v1/ontology/packs/investigation/install",
            &env.alice,
            org,
            None,
        )
        .await;
        assert!(st < 300, "{p}");
        let media = audio(&server, &env.alice, org).await;
        let alice_p = object(&server, &env.alice, org, "person", "Alice Martin").await;
        let bob_p = object(&server, &env.alice, org, "person", "Bob Durand").await;
        let place = object(&server, &env.alice, org, "place", "Studio").await;
        let path = format!("/api/v1/media/{media}/attributions");

        // Only a person of the org, only sound spans.
        let (st, e) = call(
            &server,
            "POST",
            &path,
            &env.alice,
            org,
            Some(json!({"object_id": place, "spans": spans(&[(0, 1_000)])})),
        )
        .await;
        assert_eq!(st, 400, "{e}");
        assert_eq!(e["errors"]["fields"]["object_id"], "invalid");
        for bad in [vec![], vec![(2_000, 1_000)], vec![(-1, 10)]] {
            let (st, e) = call(
                &server,
                "POST",
                &path,
                &env.alice,
                org,
                Some(json!({"object_id": alice_p, "spans": spans(&bad)})),
            )
            .await;
            assert_eq!(st, 400, "{bad:?}: {e}");
            assert_eq!(e["errors"]["fields"]["spans"], "invalid");
        }

        // "From 12:30 to 14:10, this is Alice"; then S3 = Bob, two turns.
        let (st, all) = call(
            &server,
            "POST",
            &path,
            &env.alice,
            org,
            Some(json!({"object_id": alice_p, "spans": spans(&[(750_000, 850_000)])})),
        )
        .await;
        assert_eq!(st, 201, "{all}");
        assert_eq!(all["count"], 1);
        let a = &all["results"][0];
        assert_eq!(a["person"], "Alice Martin");
        assert_eq!(a["origin"], "manual");
        assert_eq!(a["status"], "confirmed");
        let (st, all) = call(
            &server,
            "POST",
            &path,
            &env.alice,
            org,
            Some(json!({"object_id": bob_p, "spans": spans(&[(0, 5_000), (9_000, 12_000)])})),
        )
        .await;
        assert_eq!(st, 201, "{all}");
        assert_eq!(all["count"], 3);
        // A span of Alice touching hers joins it: one continuous span.
        let (st, all) = call(
            &server,
            "POST",
            &path,
            &env.alice,
            org,
            Some(json!({"object_id": alice_p, "spans": spans(&[(840_000, 900_000)])})),
        )
        .await;
        assert_eq!(st, 201, "{all}");
        assert_eq!(all["count"], 3, "{all}");
        let mine: Vec<&Value> = all["results"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|a| a["person"] == "Alice Martin")
            .collect();
        assert_eq!(
            (mine[0]["start_ms"].as_i64(), mine[0]["end_ms"].as_i64()),
            (Some(750_000), Some(900_000))
        );

        // The ontology knows who speaks in the media (D278): one open
        // `appears_in` per person, never one per span.
        let open_appearances = |person: String| {
            let server = &server;
            let token = env.alice.clone();
            async move {
                let (st, links) = call(
                    server,
                    "GET",
                    &format!("/api/v1/ontology/objects/{person}/links"),
                    &token,
                    org,
                    None,
                )
                .await;
                assert_eq!(st, 200, "{links}");
                links["results"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter(|l| l["link_type"] == "appears_in" && l["valid_to"].is_null())
                    .count()
            }
        };
        assert_eq!(open_appearances(alice_p.clone()).await, 1);
        assert_eq!(open_appearances(bob_p.clone()).await, 1);

        // The transcript read carries them (no run yet: no text).
        let (st, t) = call(
            &server,
            "GET",
            &format!("/api/v1/media/{media}/transcript"),
            &env.alice,
            org,
            None,
        )
        .await;
        assert_eq!(st, 200, "{t}");
        assert_eq!(t["attributions"].as_array().unwrap().len(), 3);
        assert!(t["runs"].as_array().unwrap().is_empty());

        // Optimistic concurrency, then reject and delete.
        let aid = mine[0]["id"].as_str().unwrap().to_string();
        let one = format!("{path}/{aid}");
        let (st, a) = call(
            &server,
            "PATCH",
            &one,
            &env.alice,
            org,
            Some(json!({"expected_version": 1, "end_ms": 880_000})),
        )
        .await;
        assert_eq!(st, 200, "{a}");
        assert_eq!(a["version"], 2);
        let (st, e) = call(
            &server,
            "PATCH",
            &one,
            &env.alice,
            org,
            Some(json!({"expected_version": 1, "status": "rejected"})),
        )
        .await;
        assert_eq!(st, 409, "{e}");
        assert_eq!(e["error"], "media-attribution-conflict");
        let (st, a) = call(
            &server,
            "PATCH",
            &one,
            &env.alice,
            org,
            Some(json!({"expected_version": 2, "status": "rejected"})),
        )
        .await;
        assert_eq!((st, a["status"].as_str()), (200, Some("rejected")), "{a}");
        assert_eq!(
            open_appearances(alice_p.clone()).await,
            0,
            "no confirmed span left"
        );

        // A viewer reads, never writes (R2); another org sees nothing (R1).
        let bob_org = personal_org(&server, &env.bob).await;
        let (st, _) = call(
            &server,
            "POST",
            &format!("/api/v1/orgs/{org}/members"),
            &env.alice,
            org,
            Some(json!({"email": "bob@example.com", "role": "viewer"})),
        )
        .await;
        assert!(st < 300);
        let (st, list) = call(&server, "GET", &path, &env.bob, org, None).await;
        assert_eq!((st, list["count"].as_i64()), (200, Some(3)), "{list}");
        let (st, e) = call(
            &server,
            "POST",
            &path,
            &env.bob,
            org,
            Some(json!({"object_id": bob_p, "spans": spans(&[(0, 1)])})),
        )
        .await;
        assert_eq!(st, 403, "{e}");
        assert_eq!(e["error"], "media-write-forbidden");
        let (st, _) = call(&server, "DELETE", &one, &env.bob, org, None).await;
        assert_eq!(st, 403);
        let (st, _) = call(&server, "GET", &path, &env.bob, bob_org, None).await;
        assert_eq!(st, 404, "an asset of another org");
        let (st, _) = call(&server, "DELETE", &one, &env.alice, org, None).await;
        assert_eq!(st, 204);
        let (st, _) = call(&server, "DELETE", &one, &env.alice, org, None).await;
        assert_eq!(st, 404);
    })
    .await;
}

/// Voice recognition (D275): without a ready voice profile the step is
/// offered greyed and refused; its proposals replace the previous ones and
/// never contradict a person (a confirmed span, a rejected match).
#[tokio::test]
#[serial]
async fn voice_proposals_respect_what_people_decided() {
    use pnex_backend::services::{media_attributions, voice_id::media::SpeakerMatch};
    use pnex_core::media_review::{ReviewSegment, ReviewTurn};
    with_app(|server, env| async move {
        let org = personal_org(&server, &env.alice).await;
        let (st, p) = call(
            &server,
            "POST",
            "/api/v1/ontology/packs/investigation/install",
            &env.alice,
            org,
            None,
        )
        .await;
        assert!(st < 300, "{p}");
        let media = audio(&server, &env.alice, org).await;
        let (st, state) = call(
            &server,
            "GET",
            &format!("/api/v1/media/{media}/analyses"),
            &env.alice,
            org,
            None,
        )
        .await;
        assert_eq!(st, 200, "{state}");
        let identify = state["offers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|o| o["step"] == "identify")
            .expect("identify offered");
        assert_eq!(identify["blocked"], "media-analysis-no-voice-profile");
        let (st, e) = call(
            &server,
            "POST",
            &format!("/api/v1/media/{media}/analyses"),
            &env.alice,
            org,
            Some(json!({"steps": ["identify"]})),
        )
        .await;
        assert_eq!(st, 409, "{e}");
        assert_eq!(e["error"], "media-analysis-no-voice-profile");

        let alice_p = object(&server, &env.alice, org, "person", "Alice Martin").await;
        let bob_p = object(&server, &env.alice, org, "person", "Bob Durand").await;
        let path = format!("/api/v1/media/{media}/attributions");
        // A person said 0–10 s is Bob, and that 20–30 s is not Alice.
        call(
            &server,
            "POST",
            &path,
            &env.alice,
            org,
            Some(json!({"object_id": bob_p, "spans": spans(&[(0, 10_000)])})),
        )
        .await;
        let (_, all) = call(
            &server,
            "POST",
            &path,
            &env.alice,
            org,
            Some(json!({"object_id": alice_p, "spans": spans(&[(20_000, 30_000)])})),
        )
        .await;
        let aid = all["results"]
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["person"] == "Alice Martin")
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string();
        let (st, _) = call(
            &server,
            "PATCH",
            &format!("{path}/{aid}"),
            &env.alice,
            org,
            Some(json!({"expected_version": 1, "status": "rejected"})),
        )
        .await;
        assert_eq!(st, 200);

        let turn = |start_ms, end_ms| ReviewTurn {
            label: "S1".into(),
            start_ms,
            end_ms,
        };
        let segments = [ReviewSegment {
            start_ms: 0,
            end_ms: 60_000,
            text: String::new(),
            words: Vec::new(),
            turns: vec![
                turn(2_000, 8_000),
                turn(20_000, 30_000),
                turn(40_000, 50_000),
            ],
        }];
        let s1_is_alice = [SpeakerMatch {
            label: "S1".into(),
            object_id: alice_p.parse().unwrap(),
            score: 0.71,
        }];
        let asset: uuid::Uuid = media.parse().unwrap();
        let n =
            media_attributions::propose_voices(&env.ctx.db, org, asset, &segments, &s1_is_alice)
                .await
                .unwrap();
        assert_eq!(
            n, 1,
            "only 40–50 s: 2–8 s is Bob's, 20–30 s was rejected for Alice"
        );
        // A second run replaces its own proposals, never adds to them.
        let n =
            media_attributions::propose_voices(&env.ctx.db, org, asset, &segments, &s1_is_alice)
                .await
                .unwrap();
        assert_eq!(n, 1);
        let (_, list) = call(
            &server,
            "GET",
            &format!("{path}?limit=100"),
            &env.alice,
            org,
            None,
        )
        .await;
        let proposed: Vec<&Value> = list["results"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|a| a["status"] == "proposed")
            .collect();
        assert_eq!(proposed.len(), 1, "{list}");
        assert_eq!(proposed[0]["origin"], "voice");
        assert_eq!(proposed[0]["start_ms"], 40_000);
        assert!((proposed[0]["confidence"].as_f64().unwrap() - 0.71).abs() < 1e-3);
    })
    .await;
}

/// 12 s of a steady tone (speech for the energy gate), a real WAV.
fn tone_wav() -> Vec<u8> {
    let samples: Vec<i16> = (0..12 * 16_000)
        .map(|i| ((i as f32 * 440.0 * std::f32::consts::TAU / 16_000.0).sin() * 8_000.0) as i16)
        .collect();
    pnex_backend::services::media_ingest::capture::segmenter::wav_bytes(&samples)
}

/// Enrolment from a media (D277): the confirmed passages of the person,
/// decoded from the original file, become clips of an existing consented
/// profile; without a confirmed passage nothing is enrolled.
#[tokio::test]
#[serial]
async fn confirmed_passages_enrol_a_consented_profile() {
    let models_dir =
        std::env::temp_dir().join(format!("pnex-review-models-{}", std::process::id()));
    unsafe {
        std::env::set_var(
            "PNEX_ASR_BIN",
            format!(
                "{}/tests/fixtures/asr/fake-pnex-asr.sh",
                env!("CARGO_MANIFEST_DIR")
            ),
        );
        std::env::set_var("PNEX_ASR_MODELS_DIR", &models_dir);
    }
    if pnex_backend::services::media_ingest::capture::decoder::resolve(
        &pnex_backend::services::media_ingest::capture::CaptureSettings::from_env().ffmpeg,
    )
    .is_none()
    {
        eprintln!("ffmpeg not installed: skipped");
        return;
    }
    with_app(|server, env| async move {
        let org = personal_org(&server, &env.alice).await;
        call(&server, "POST", "/api/v1/ontology/packs/investigation/install", &env.alice, org, None).await;
        let upload = |name: &str, kind: &str, bytes: Vec<u8>| {
            server
                .post(&format!("/api/v1/media?filename={name}&kind={kind}"))
                .add_header("Authorization", format!("Bearer {}", env.alice))
                .add_header("X-Org-Id", org.to_string())
                .add_header("Content-Type", "application/octet-stream")
                .bytes(bytes.into())
        };
        let emb = upload("emb.onnx", "model", {
            let (key, value) = ("framework", "3d-speaker");
            let mut entry = vec![0x0a, key.len() as u8];
            entry.extend_from_slice(key.as_bytes());
            entry.extend_from_slice(&[0x12, value.len() as u8]);
            entry.extend_from_slice(value.as_bytes());
            let mut out = vec![0x08, 0x07, 0x72, entry.len() as u8];
            out.extend(entry);
            out
        })
        .await
        .json::<Value>();
        let (st, m) = call(&server, "POST", "/api/v1/asr/models", &env.alice, org, Some(json!({"name": "emb", "asset_id": emb["id"], "license": "Apache-2.0"}))).await;
        assert_eq!(st, 201, "{m}");
        let ada = object(&server, &env.alice, org, "person", "Ada").await;
        let (st, p) = call(&server, "POST", "/api/v1/voice-id/profiles", &env.alice, org, Some(json!({
            "object_id": ada, "model_id": "", "consent_at": "2026-10-01",
            "consent_scope": "public debates", "consent_proof": "signed form #3", "consent_confirmed": true,
        }))).await;
        assert_eq!(st, 201, "{p}");
        let pid = p["id"].as_str().unwrap().to_string();
        let res = upload("debate.wav", "audio", tone_wav()).await;
        assert!(res.status_code().is_success(), "{}", res.text());
        let media = res.json::<Value>()["id"].as_str().unwrap().to_string();
        let enrol = format!("/api/v1/voice-id/profiles/{pid}/samples/from-media");

        let (st, e) = call(&server, "POST", &enrol, &env.alice, org, Some(json!({"asset_id": media}))).await;
        assert_eq!(st, 409, "no confirmed passage of Ada: {e}");
        assert_eq!(e["error"], "voice-id-not-enough-speech");
        let (st, _) = call(&server, "POST", &format!("/api/v1/media/{media}/attributions"), &env.alice, org, Some(json!({"object_id": ada, "spans": spans(&[(1_000, 11_000)])}))).await;
        assert_eq!(st, 201);
        let (st, p) = call(&server, "POST", &enrol, &env.alice, org, Some(json!({"asset_id": media}))).await;
        assert_eq!(st, 201, "{p}");
        let samples = p["samples"].as_array().unwrap();
        assert_eq!(samples.len(), 1, "{p}");
        assert_eq!(samples[0]["status"], "pending");
        assert!(samples[0]["duration_ms"].as_i64().unwrap() >= 9_000, "{p}");
    })
    .await;
}

/// Object detection on a video (D215): offered on a video only, blocked
/// and refused without a valid detection model; before any detection the
/// review reads no box.
#[tokio::test]
#[serial]
async fn detection_needs_a_model_and_starts_empty() {
    with_app(|server, env| async move {
        let org = personal_org(&server, &env.alice).await;
        let audio_id = audio(&server, &env.alice, org).await;
        let (_, state) = call(
            &server,
            "GET",
            &format!("/api/v1/media/{audio_id}/analyses"),
            &env.alice,
            org,
            None,
        )
        .await;
        assert!(
            state["offers"]
                .as_array()
                .unwrap()
                .iter()
                .all(|o| o["step"] != "detect"),
            "{state}"
        );
        let mp4 = b"\0\0\0\x18ftypisom\0\0\x02\0isomiso2mp41".to_vec();
        let res = server
            .post("/api/v1/media?filename=clip.mp4&kind=video")
            .add_header("Authorization", format!("Bearer {}", env.alice))
            .add_header("X-Org-Id", org.to_string())
            .add_header("Content-Type", "application/octet-stream")
            .bytes(mp4.into())
            .await;
        assert!(res.status_code().is_success(), "{}", res.text());
        let video = res.json::<Value>()["id"].as_str().unwrap().to_string();
        let (_, state) = call(
            &server,
            "GET",
            &format!("/api/v1/media/{video}/analyses"),
            &env.alice,
            org,
            None,
        )
        .await;
        let detect = state["offers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|o| o["step"] == "detect")
            .expect("offered on a video");
        assert_eq!(detect["blocked"], "media-analysis-no-detector");
        let (st, e) = call(
            &server,
            "POST",
            &format!("/api/v1/media/{video}/analyses"),
            &env.alice,
            org,
            Some(json!({"steps": ["detect"]})),
        )
        .await;
        assert_eq!(st, 409, "{e}");
        assert_eq!(e["error"], "media-analysis-no-detector");
        let (st, d) = call(
            &server,
            "GET",
            &format!("/api/v1/media/{video}/detections"),
            &env.alice,
            org,
            None,
        )
        .await;
        assert_eq!(st, 200, "{d}");
        assert!(
            d["model"].is_null() && d["frames"].as_array().unwrap().is_empty(),
            "{d}"
        );
        let (st, _) = call(
            &server,
            "GET",
            &format!("/api/v1/media/{}/detections", uuid::Uuid::new_v4()),
            &env.alice,
            org,
            None,
        )
        .await;
        assert_eq!(st, 404);
    })
    .await;
}
