//! Voice identification (voice-id.md, cases.md K19 variant A) with the
//! fake `pnex-asr` (every clip gives the same embedding): consent and
//! person checks, roles (R2), clips kept as audio media, generation with
//! too little then enough speech, verification and identification, and
//! revocation deleting the clips' audio. Org isolation (R1).
//!
//! Needs PostgreSQL (TEST_DATABASE_URL); the audio part needs ffmpeg.

mod common;

use loco_rs::testing::request::{RequestConfig, RequestConfigBuilder};
use pnex_backend::app::App;
use pnex_backend::services::media_ingest::capture::{self, decoder, CaptureSettings};
use serde_json::{json, Value};
use serial_test::serial;

fn fake_runtime() -> String {
    format!(
        "{}/tests/fixtures/asr/fake-pnex-asr.sh",
        env!("CARGO_MANIFEST_DIR")
    )
}

fn embedding_onnx() -> Vec<u8> {
    let (key, value) = ("framework", "3d-speaker");
    let mut entry = vec![0x0a, key.len() as u8];
    entry.extend_from_slice(key.as_bytes());
    entry.extend_from_slice(&[0x12, value.len() as u8]);
    entry.extend_from_slice(value.as_bytes());
    let mut out = vec![0x08, 0x07, 0x72, entry.len() as u8];
    out.extend(entry);
    out
}

/// 12 s of a steady tone: the energy gate sees speech all along, so two
/// usable windows (8 s + 4 s).
fn tone_wav() -> Vec<u8> {
    let samples: Vec<i16> = (0..12 * 16_000)
        .map(|i| ((i as f32 * 440.0 * std::f32::consts::TAU / 16_000.0).sin() * 8_000.0) as i16)
        .collect();
    capture::segmenter::wav_bytes(&samples)
}

#[tokio::test]
#[serial]
async fn consented_profile_generation_test_and_revocation() {
    let models_dir =
        std::env::temp_dir().join(format!("pnex-voice-test-models-{}", std::process::id()));
    unsafe {
        std::env::set_var("PNEX_ASR_BIN", fake_runtime());
        std::env::set_var("PNEX_ASR_MODELS_DIR", &models_dir);
        std::env::set_var("PNEX_FLOW_ENABLED", "false");
    }
    let base = common::spawn_mock_rauthy().await;
    unsafe { std::env::set_var("RAUTHY_URL", &base) };
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
    let has_ffmpeg = decoder::resolve(&CaptureSettings::from_env().ffmpeg).is_some();
    let config: RequestConfig = RequestConfigBuilder::new().build();
    loco_rs::testing::request::request_with_config::<App, _, _>(
        config,
        move |server, ctx| async move {
            common::seed_catalogue(&ctx.db).await;
            let org_of = |token: String| {
                let server = &server;
                async move {
                    server
                        .get("/api/v1/user-info")
                        .add_header("Authorization", format!("Bearer {token}"))
                        .await
                        .json::<Value>()["orgs"][0]["id"]
                        .as_i64()
                        .unwrap()
                }
            };
            let org = org_of(alice.clone()).await;
            let bob_org = org_of(bob.clone()).await;
            let call = |token: &str, org: i64, method: &str, path: &str, body: Option<Value>| {
                let req = match method {
                    "GET" => server.get(path),
                    "POST" => server.post(path),
                    "PATCH" => server.patch(path),
                    _ => server.delete(path),
                }
                .add_header("Authorization", format!("Bearer {token}"))
                .add_header("X-Org-Id", org.to_string());
                async move {
                    let res = match body {
                        Some(b) => req.json(&b).await,
                        None => req.await,
                    };
                    let st = res.status_code().as_u16();
                    let text = res.text();
                    (
                        st,
                        serde_json::from_str(&text).unwrap_or(Value::String(text)),
                    )
                }
            };
            let upload = |token: &str, org: i64, path: String, bytes: Vec<u8>| {
                let req = server
                    .post(&path)
                    .add_header("Authorization", format!("Bearer {token}"))
                    .add_header("X-Org-Id", org.to_string())
                    .add_header("Content-Type", "application/octet-stream")
                    .bytes(bytes.into());
                async move {
                    let res = req.await;
                    let st = res.status_code().as_u16();
                    let text = res.text();
                    (
                        st,
                        serde_json::from_str(&text).unwrap_or(Value::String(text)),
                    )
                }
            };

            let (st, _) = call(
                &alice,
                org,
                "POST",
                "/api/v1/ontology/packs/investigation/install",
                None,
            )
            .await;
            assert_eq!(st, 200);
            let new_object = |type_key: &str, title: &str| {
                call(
                    &alice,
                    org,
                    "POST",
                    "/api/v1/ontology/objects",
                    Some(json!({"type_key": type_key, "title": title, "properties": {}})),
                )
            };
            let (st, ada) = new_object("person", "Ada").await;
            assert_eq!(st, 201, "{ada}");
            let ada_id = ada["id"].as_str().unwrap().to_string();
            let (_, acme) = new_object("organization", "ACME").await;
            let consent = |object_id: &str| {
                json!({
                    "object_id": object_id,
                    "model_id": "",
                    "consent_at": "2026-10-01",
                    "consent_scope": "internal meetings, 2 years",
                    "consent_proof": "signed form #12",
                    "consent_confirmed": true,
                })
            };

            // No speaker embedding model yet.
            let (st, e) = call(
                &alice,
                org,
                "POST",
                "/api/v1/voice-id/profiles",
                Some(consent(&ada_id)),
            )
            .await;
            assert_eq!(
                (st, e["error"].as_str()),
                (409, Some("voice-id-no-model")),
                "{e}"
            );
            let (st, asset) = upload(
                &alice,
                org,
                "/api/v1/media?filename=emb.onnx&kind=model".into(),
                embedding_onnx(),
            )
            .await;
            assert_eq!(st, 201, "{asset}");
            let (st, emb) = call(
                &alice,
                org,
                "POST",
                "/api/v1/asr/models",
                Some(json!({"name": "emb", "asset_id": asset["id"], "license": "Apache-2.0"})),
            )
            .await;
            assert_eq!(st, 201, "{emb}");
            assert_eq!(emb["check"]["status"], "valid", "{emb}");

            // Consent is mandatory, a profile only goes on a person.
            let mut body = consent(&ada_id);
            body["consent_confirmed"] = json!(false);
            let (st, e) = call(&alice, org, "POST", "/api/v1/voice-id/profiles", Some(body)).await;
            assert_eq!(
                (st, &e["errors"]["fields"]["consent_confirmed"]),
                (400, &json!("required")),
                "{e}"
            );
            let mut body = consent(&ada_id);
            body["consent_at"] = json!("2999-01-01");
            let (st, e) = call(&alice, org, "POST", "/api/v1/voice-id/profiles", Some(body)).await;
            assert_eq!((st, &e["errors"]["fields"]["consent_at"]), (400, &json!("invalid")), "{e}");
            let mut body = consent(&ada_id);
            body["consent_proof"] = json!(" ");
            let (st, e) = call(&alice, org, "POST", "/api/v1/voice-id/profiles", Some(body)).await;
            assert_eq!((st, &e["errors"]["fields"]["consent_proof"]), (400, &json!("required")), "{e}");
            let (st, e) = call(
                &alice,
                org,
                "POST",
                "/api/v1/voice-id/profiles",
                Some(consent(acme["id"].as_str().unwrap())),
            )
            .await;
            assert_eq!(
                (st, e["error"].as_str()),
                (400, Some("voice-id-not-person")),
                "{e}"
            );

            let (st, p) = call(
                &alice,
                org,
                "POST",
                "/api/v1/voice-id/profiles",
                Some(consent(&ada_id)),
            )
            .await;
            assert_eq!(st, 201, "{p}");
            assert_eq!(p["status"], "draft");
            assert_eq!(p["person"], "Ada");
            assert_eq!(p["consent_recorded_by"], "alice@example.com");
            assert!(p.get("centroid").is_none() && !p.to_string().contains("ciphertext"));
            let pid = p["id"].as_str().unwrap().to_string();
            let (st, e) = call(
                &alice,
                org,
                "POST",
                "/api/v1/voice-id/profiles",
                Some(consent(&ada_id)),
            )
            .await;
            assert_eq!(
                (st, e["error"].as_str()),
                (409, Some("voice-id-profile-exists"))
            );
            let (st, by) = call(
                &alice,
                org,
                "GET",
                &format!("/api/v1/voice-id/by-object/{ada_id}"),
                None,
            )
            .await;
            assert_eq!((st, &by["id"]), (200, &json!(pid)));
            let (st, none) = call(
                &alice,
                org,
                "GET",
                &format!(
                    "/api/v1/voice-id/by-object/{}",
                    acme["id"].as_str().unwrap()
                ),
                None,
            )
            .await;
            assert_eq!((st, none), (200, Value::Null));

            // R1: another org never sees the profile.
            let (st, _) = call(
                &bob,
                bob_org,
                "GET",
                &format!("/api/v1/voice-id/profiles/{pid}"),
                None,
            )
            .await;
            assert_eq!(st, 404);
            // R2: a viewer reads, never enrols nor tests.
            let (st, _) = call(
                &alice,
                org,
                "POST",
                &format!("/api/v1/orgs/{org}/members"),
                Some(json!({"email": "bob@example.com", "role": "viewer"})),
            )
            .await;
            assert!(st == 200 || st == 201, "{st}");
            let (st, list) = call(&bob, org, "GET", "/api/v1/voice-id/profiles", None).await;
            assert_eq!((st, list["results"].as_array().map(Vec::len)), (200, Some(1)));
            let (st, e) = upload(
                &bob,
                org,
                format!("/api/v1/voice-id/profiles/{pid}/samples?name=a.wav"),
                tone_wav(),
            )
            .await;
            assert_eq!(
                (st, e["error"].as_str()),
                (403, Some("access-denied"))
            );
            let (st, _) = upload(
                &bob,
                org,
                "/api/v1/voice-id/test?name=a.wav".into(),
                tone_wav(),
            )
            .await;
            assert_eq!(st, 403);
            let (st, _) = call(
                &bob,
                org,
                "DELETE",
                &format!("/api/v1/voice-id/profiles/{pid}"),
                None,
            )
            .await;
            assert_eq!(st, 403);

            let (st, e) = upload(
                &alice,
                org,
                format!("/api/v1/voice-id/profiles/{pid}/samples?name=notes.txt"),
                b"hello".to_vec(),
            )
            .await;
            assert_eq!(
                (st, e["error"].as_str()),
                (400, Some("voice-id-audio-unsupported"))
            );
            if !has_ffmpeg {
                return;
            }

            // One clip = two windows: not enough to generate.
            let (st, p) = upload(
                &alice,
                org,
                format!(
                    "/api/v1/voice-id/profiles/{pid}/samples?name=one.wav&content_type=audio/wav"
                ),
                tone_wav(),
            )
            .await;
            assert_eq!(st, 201, "{p}");
            assert_eq!(p["samples"][0]["windows"], 2, "{p}");
            assert_eq!(p["samples"][0]["status"], "pending");
            let (st, e) = call(
                &alice,
                org,
                "POST",
                &format!("/api/v1/voice-id/profiles/{pid}/generate"),
                None,
            )
            .await;
            assert_eq!(
                (st, e["error"].as_str()),
                (409, Some("voice-id-not-enough-speech")),
                "{e}"
            );
            assert_eq!(e["errors"]["args"]["min_windows"], "3", "{e}");
            let (st, e) = upload(
                &alice,
                org,
                format!("/api/v1/voice-id/test?name=t.wav&object_id={ada_id}"),
                tone_wav(),
            )
            .await;
            assert_eq!((st, e["error"].as_str()), (409, Some("voice-id-not-ready")));

            // A span of the recording (4 s → 12 s): only it is decoded and kept.
            let (st, p) = upload(
                &alice,
                org,
                format!(
                    "/api/v1/voice-id/profiles/{pid}/samples?name=two.wav&content_type=audio/wav&start=4&duration=8"
                ),
                tone_wav(),
            )
            .await;
            assert_eq!(st, 201, "{p}");
            let span = p["samples"]
                .as_array()
                .unwrap()
                .iter()
                .find(|s| s["name"].as_str().is_some_and(|n| n.contains("two")))
                .unwrap();
            assert_eq!((span["duration_ms"].as_i64(), span["windows"].as_i64()), (Some(8000), Some(1)), "{p}");
            assert!(span["name"].as_str().unwrap().ends_with("two.wav (0:04–0:12)"), "{p}");
            let (st, p) = call(
                &alice,
                org,
                "POST",
                &format!("/api/v1/voice-id/profiles/{pid}/generate"),
                None,
            )
            .await;
            assert_eq!(st, 200, "{p}");
            assert_eq!(p["status"], "ready");
            assert_eq!(p["windows_used"], 3);
            // Identical vectors: no spread, the threshold sits at its ceiling.
            assert!((p["threshold"].as_f64().unwrap() - 0.8).abs() < 1e-6, "{p}");
            assert!(
                p["samples"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(|s| s["status"] == "used"),
                "{p}"
            );
            let (_, media) = call(&alice, org, "GET", "/api/v1/media?kind=audio", None).await;
            assert_eq!(media["count"], 2, "{media}");

            // Verification 1:1 and identification 1:N.
            let (st, r) = upload(
                &alice,
                org,
                format!("/api/v1/voice-id/test?name=t.wav&object_id={ada_id}"),
                tone_wav(),
            )
            .await;
            assert_eq!(st, 200, "{r}");
            assert_eq!(r["verdict"], "match");
            assert_eq!(r["best"]["person"], "Ada");
            assert_eq!(r["windows"], 2);
            let (st, r) = upload(
                &alice,
                org,
                "/api/v1/voice-id/test?name=t.wav".into(),
                tone_wav(),
            )
            .await;
            assert_eq!((st, &r["verdict"]), (200, &json!("match")), "{r}");
            assert_eq!(r["candidates"].as_array().unwrap().len(), 1);
            let silent = capture::segmenter::wav_bytes(&vec![0i16; 12 * 16_000]);
            let (st, e) = upload(
                &alice,
                org,
                "/api/v1/voice-id/test?name=s.wav".into(),
                silent,
            )
            .await;
            assert_eq!(
                (st, e["error"].as_str()),
                (409, Some("voice-id-not-enough-speech"))
            );

            // Revocation deletes the profile and the clips' audio.
            let (st, _) = call(
                &alice,
                org,
                "DELETE",
                &format!("/api/v1/voice-id/profiles/{pid}"),
                None,
            )
            .await;
            assert_eq!(st, 204);
            let (_, by) = call(
                &alice,
                org,
                "GET",
                &format!("/api/v1/voice-id/by-object/{ada_id}"),
                None,
            )
            .await;
            assert_eq!(by, Value::Null);
            let (_, media) = call(&alice, org, "GET", "/api/v1/media?kind=audio", None).await;
            assert_eq!(media["count"], 0, "{media}");
        },
    )
    .await;
}
