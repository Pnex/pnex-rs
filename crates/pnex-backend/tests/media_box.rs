//! Capture boxes (media-ingest.md D159, D160, lot 6b): `capture_on =
//! device:<id>` accepted for the org's agents that announced
//! `media_capture`, stream list and segment upload over `/ws/media` (device
//! identity, Noise binary link), capture state reports.
//!
//! Harness of `ws_camera.rs` (PG required — TEST_DATABASE_URL).

mod common;

use loco_rs::testing::request::{RequestConfig, RequestConfigBuilder};
use pnex_backend::app::App;
use pnex_backend::models::_entities::{media_segments, media_streams};
use pnex_backend::services::media_ingest::capture::segmenter::wav_bytes;
use pnex_core::media_ingest::{encode_segment, ClockSource, MediaDownMsg, SegmentHeader};
use pnex_core::{DeviceMsg, ServerMsg};
use sea_orm::sea_query::Expr;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use serde_json::{json, Value};
use serial_test::serial;

struct Agent {
    id: i64,
    device_id: String,
    token: String,
    key: [u8; 32],
}

async fn org_of(server: &axum_test::TestServer, token: &str) -> i64 {
    server
        .get("/api/v1/user-info")
        .add_header("Authorization", format!("Bearer {token}"))
        .await
        .json::<Value>()["orgs"][0]["id"]
        .as_i64()
        .expect("personal org")
}

async fn create_agent(server: &axum_test::TestServer, auth: &str, org: i64, name: &str) -> Agent {
    let res = server
        .post("/api/v1/devices")
        .add_header("Authorization", format!("Bearer {auth}"))
        .add_header("X-Org-Id", org.to_string())
        .json(&json!({ "device_id": name, "predefined_device_name": "edge_agent" }))
        .await;
    res.assert_status(axum_test::http::StatusCode::CREATED);
    let dto: Value = res.json();
    Agent {
        id: dto["id"].as_i64().expect("id"),
        device_id: name.into(),
        token: dto["device_token"]["token"].as_str().expect("token").into(),
        key: pnex_core::frame::decode_key(dto["device_token"]["encryption_key"].as_str().unwrap())
            .expect("32-byte key"),
    }
}

async fn next_down(ws: &mut common::DevWs) -> MediaDownMsg {
    let raw = tokio::time::timeout(std::time::Duration::from_secs(5), ws.recv_plain())
        .await
        .expect("server message");
    serde_json::from_str(&raw).expect("MediaDownMsg")
}

/// Uploads one segment, returns `(ok, code)` of its acknowledgement.
async fn upload(
    ws: &mut common::DevWs,
    stream_id: &str,
    seq: i64,
    wav: &[u8],
) -> (bool, Option<String>) {
    let header = SegmentHeader {
        stream_id: stream_id.into(),
        seq,
        started_ms: 1_760_000_000_000 + seq * 10_000,
        ended_ms: 1_760_000_011_000 + seq * 10_000,
        clock: ClockSource::Host,
    };
    ws.send_sealed_bytes(&encode_segment(&header, wav)).await;
    match next_down(ws).await {
        MediaDownMsg::Ack {
            ok, code, seq: s, ..
        } => {
            assert_eq!(s, seq);
            (ok, code)
        }
        other => panic!("ack expected, got {other:?}"),
    }
}

#[tokio::test]
#[serial]
async fn capture_box_stream_list_and_upload() {
    let base = common::spawn_mock_rauthy().await;
    let _ = tracing_subscriber::fmt().with_test_writer().try_init();
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
    let config: RequestConfig = RequestConfigBuilder::new().build();
    loco_rs::testing::request::request_with_config::<App, _, _>(
        config,
        move |server, ctx| async move {
            common::seed_catalogue(&ctx.db).await;
            let org = org_of(&server, &alice).await;
            let bob_org = org_of(&server, &bob).await;
            let call = |method: &str, path: &str, who: &str, org: i64| {
                let req = match method {
                    "GET" => server.get(path),
                    "POST" => server.post(path),
                    _ => server.patch(path),
                };
                req.add_header("Authorization", format!("Bearer {who}"))
                    .add_header("X-Org-Id", org.to_string())
            };
            let agent = create_agent(&server, &alice, org, "box-1").await;
            let carrier = format!("device:{}", agent.id);
            let body = json!({ "name": "Tvheadend", "kind": "icecast",
            "url": "http://192.168.1.20:9981/stream/channel/fr2", "segment_secs": 10,
            "capture_on": carrier.clone() });

            // Not announced yet: same field error as an unknown id.
            let res = call("POST", "/api/v1/media/streams", &alice, org)
                .json(&body)
                .await;
            assert_eq!(res.status_code(), 400, "{}", res.text());
            assert_eq!(res.json::<Value>()["capture_on"], "invalid");

            // The agent announces `media_capture` on its device link.
            let mut ctl = common::DevWs::connect(
                &server,
                &format!("/ws/device?token={}", agent.token),
                &agent.key,
                &agent.device_id,
            )
            .await;
            let announce = DeviceMsg::Announce {
                chip: pnex_core::EDGE_AGENT_CHIP.into(),
                board: "aarch64-linux".into(),
                fw: "0.1.0".into(),
                caps: Some(vec![pnex_core::CapDesc {
                    id: "media_capture".into(),
                    family: "feature".into(),
                    unit: None,
                    version: 1,
                }]),
                pins: None,
            };
            ctl.send_plain(&serde_json::to_string(&announce).unwrap())
                .await;
            let cfg: ServerMsg = serde_json::from_str(&ctl.recv_plain().await).unwrap();
            assert!(matches!(cfg, ServerMsg::AgentConfig { .. }), "{cfg:?}");

            let boxes = call("GET", "/api/v1/media/capture-devices", &alice, org).await;
            assert_eq!(
                boxes.json::<Value>(),
                json!([{ "id": agent.id, "name": "box-1" }])
            );
            // Another org never sees nor picks the box.
            let res = call("POST", "/api/v1/media/streams", &bob, bob_org)
                .json(&body)
                .await;
            assert_eq!(res.status_code(), 400, "{}", res.text());
            // A box captures audio only.
            let mut video = body.clone();
            video["tracks"] = json!("video");
            let res = call("POST", "/api/v1/media/streams", &alice, org)
                .json(&video)
                .await;
            assert_eq!(res.json::<Value>()["tracks"], "invalid");

            let res = call("POST", "/api/v1/media/streams", &alice, org)
                .json(&body)
                .await;
            assert_eq!(res.status_code(), 201, "{}", res.text());
            let created: Value = res.json();
            let id = created["id"].as_str().unwrap().to_string();
            assert_eq!(created["capture_on"], carrier.as_str());
            assert_eq!(created["capture_device"], "box-1");
            // The URL test runs on the server: not for a box.
            let res = call(
                "POST",
                &format!("/api/v1/media/streams/{id}/test"),
                &alice,
                org,
            )
            .await;
            assert_eq!(res.json::<Value>()["error"], "media-capture-unsupported");

            // A server stream of the org and a stream of another org forged to
            // point at this box: neither is the box's.
            let res = call("POST", "/api/v1/media/streams", &alice, org)
                .json(&json!({ "name": "Server", "kind": "icecast",
                "url": "https://radio.example/live.mp3", "segment_secs": 10 }))
                .await;
            let server_id = res.json::<Value>()["id"].as_str().unwrap().to_string();
            let res = call("POST", "/api/v1/media/streams", &bob, bob_org)
                .json(&json!({ "name": "Bob", "kind": "icecast",
                "url": "https://radio.example/bob.mp3", "segment_secs": 10 }))
                .await;
            let bob_id = res.json::<Value>()["id"].as_str().unwrap().to_string();
            media_streams::Entity::update_many()
                .col_expr(
                    media_streams::Column::CaptureOn,
                    Expr::value(carrier.clone()),
                )
                .filter(media_streams::Column::Id.eq(bob_id.parse::<uuid::Uuid>().unwrap()))
                .exec(&ctx.db)
                .await
                .unwrap();

            // R9: the secret of a stream goes to its box, so only an
            // owner/admin moves a secret-holding stream onto a box.
            let res = call("POST", &format!("/api/v1/orgs/{org}/members"), &alice, org)
                .json(&json!({ "email": "bob@example.com", "role": "member" }))
                .await;
            assert!(res.status_code().is_success(), "{}", res.text());
            let res = call("POST", "/api/v1/media/streams", &alice, org)
                .json(&json!({ "name": "Private", "kind": "icecast",
                "url": "http://192.168.1.20:9981/stream/channel/private", "segment_secs": 10,
                "auth_secret": { "value": "Bearer s3cr3t" } }))
                .await;
            assert_eq!(res.status_code(), 201, "{}", res.text());
            let secret_id = res.json::<Value>()["id"].as_str().unwrap().to_string();
            let move_it = json!({ "capture_on": carrier });
            let path = format!("/api/v1/media/streams/{secret_id}");
            let res = call("PATCH", &path, &bob, org).json(&move_it).await;
            assert_eq!(res.status_code(), 403, "{}", res.text());
            assert_eq!(res.json::<Value>()["error"], "secret-destination-locked");
            let res = call("PATCH", &path, &alice, org).json(&move_it).await;
            assert_eq!(res.status_code(), 200, "{}", res.text());
            assert!(!res.text().contains("s3cr3t"), "no secret value in DTOs");

            // Media link: the stream list comes first, the box's streams only.
            let mut media = common::DevWs::connect_binary(
                &server,
                &format!("/ws/media?token={}", agent.token),
                &agent.key,
                &agent.device_id,
            )
            .await;
            match next_down(&mut media).await {
                MediaDownMsg::Streams { streams } => {
                    assert_eq!(streams.len(), 2, "{streams:?}");
                    let plain = streams.iter().find(|s| s.id == id).expect("plain stream");
                    assert_eq!(plain.segment_secs, 10);
                    assert!(!plain.enabled, "disabled stream");
                    assert!(plain.auth_secret.is_none());
                    // Its own access credential, for this box only (SEC-27).
                    let private = streams
                        .iter()
                        .find(|s| s.id == secret_id)
                        .expect("secret stream");
                    assert_eq!(private.auth_secret.as_deref(), Some("Bearer s3cr3t"));
                }
                other => panic!("stream list expected, got {other:?}"),
            }

            let wav = wav_bytes(&vec![0i16; 11 * 16_000]);
            assert_eq!(upload(&mut media, &id, 0, &wav).await, (true, None));
            // A resend is acknowledged and stored once.
            assert_eq!(upload(&mut media, &id, 0, &wav).await, (true, None));
            let not_found = (false, Some("not-found".to_string()));
            assert_eq!(upload(&mut media, &server_id, 0, &wav).await, not_found);
            assert_eq!(upload(&mut media, &bob_id, 0, &wav).await, not_found);
            let oversize = wav_bytes(&vec![0i16; 30 * 16_000]);
            assert_eq!(
                upload(&mut media, &id, 1, &oversize).await,
                (false, Some("invalid".to_string()))
            );
            assert_eq!(
                upload(&mut media, &id, 2, b"not a wav").await,
                (false, Some("invalid".to_string()))
            );
            let rows = media_segments::Entity::find()
                .filter(media_segments::Column::StreamId.eq(id.parse::<uuid::Uuid>().unwrap()))
                .all(&ctx.db)
                .await
                .unwrap();
            assert_eq!(rows.len(), 1, "a resent segment is stored once");
            assert_eq!(rows[0].org_id, org);
            for other in [&server_id, &bob_id] {
                let n = media_segments::Entity::find()
                    .filter(
                        media_segments::Column::StreamId.eq(other.parse::<uuid::Uuid>().unwrap()),
                    )
                    .all(&ctx.db)
                    .await
                    .unwrap()
                    .len();
                assert_eq!(n, 0);
            }

            // Capture state: own streams only, known codes only.
            for (sid, error) in [(&id, "unreachable"), (&bob_id, "stalled")] {
                let msg =
                    json!({ "t": "state", "stream_id": sid, "state": "backoff", "error": error });
                media.send_plain(&msg.to_string()).await;
            }
            let msg =
                json!({ "t": "state", "stream_id": id, "state": "running", "error": "<b>x</b>" });
            media.send_plain(&msg.to_string()).await;
            // PING round trip: every earlier frame was handled.
            media.send_plain("PING").await;
            assert_eq!(media.recv_plain().await, "PONG");
            let row = |sid: &str| {
                let sid: uuid::Uuid = sid.parse().unwrap();
                let db = ctx.db.clone();
                async move {
                    media_streams::Entity::find_by_id(sid)
                        .one(&db)
                        .await
                        .unwrap()
                        .unwrap()
                }
            };
            let mine = row(&id).await;
            assert_eq!(
                (mine.capture_state.as_str(), mine.capture_error),
                ("running", None)
            );
            assert_eq!(row(&bob_id).await.capture_state, "stopped");

            // A device that is not an agent has no media link.
            let res = server
                .post("/api/v1/devices")
                .add_header("Authorization", format!("Bearer {alice}"))
                .add_header("X-Org-Id", org.to_string())
                .json(&json!({ "device_id": "mcu-1", "predefined_device_name": "generic_esp8266" }))
                .await;
            let dto: Value = res.json();
            let mut mcu = common::DevWs::connect_binary(
                &server,
                &format!(
                    "/ws/media?token={}",
                    dto["device_token"]["token"].as_str().unwrap()
                ),
                &pnex_core::frame::decode_key(
                    dto["device_token"]["encryption_key"].as_str().unwrap(),
                )
                .unwrap(),
                "mcu-1",
            )
            .await;
            match mcu.receive_message().await {
                axum_test::WsMessage::Close(Some(f)) => assert_eq!(u16::from(f.code), 4004),
                other => panic!("close 4004 expected, got {other:?}"),
            }
            media.close().await;
            ctl.close().await;
        },
    )
    .await;
}
