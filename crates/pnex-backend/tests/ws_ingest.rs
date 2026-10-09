//! Ingest WebSocket tests: base64 auth query, Noise NNpsk0 link (D156),
//! PING/PONG, key=value validation, close codes, anti-clone.
//!
//! Nécessite PostgreSQL (TEST_DATABASE_URL) — base vidée entre tests.
//! Config test : `silence_ttl_secs: 2`, `token_cache_secs: 0`
//! (revalidation à chaque frame → 4005 déterministe).

mod common;

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use loco_rs::testing::request::{RequestConfig, RequestConfigBuilder};
use pnex_backend::app::App;
use pnex_backend::services::telemetry::{self, TelemetryPoint, TelemetrySink};
use serial_test::serial;
use std::sync::{Arc, Mutex};

// ─────────────────── Client miroir (rôle firmware) ───────────────────

/// Paramètre query tel que le firmware l'envoie (base64 du texte).
fn b64_param(raw: &str) -> String {
    STANDARD.encode(raw)
}

fn key_bytes(b64: &str) -> [u8; 32] {
    STANDARD
        .decode(b64)
        .expect("clé b64")
        .try_into()
        .expect("clé 32 o")
}

/// Code d'une frame Close reçue (None si pas une close).
fn close_code(msg: axum_test::WsMessage) -> Option<u16> {
    match msg {
        axum_test::WsMessage::Close(Some(frame)) => Some(u16::from(frame.code)),
        _ => None,
    }
}

// ─────────────────── Sink enregistreur ───────────────────

#[derive(Default)]
struct RecSink(Mutex<Vec<TelemetryPoint>>);

impl TelemetrySink for RecSink {
    fn send(&self, point: TelemetryPoint) {
        self.0.lock().expect("sink").push(point);
    }
}

// ─────────────────── Harnais ───────────────────

async fn with_app<F, Fut>(f: F)
where
    F: FnOnce(axum_test::TestServer, String, loco_rs::app::AppContext) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let base = common::spawn_mock_rauthy().await;
    let _ = tracing_subscriber::fmt().with_test_writer().try_init();
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
            f(server, alice, ctx).await;
        },
    )
    .await;
}

struct Dev {
    id: i64,
    device_id: String,
    token: String,
    key: [u8; 32],
}

/// Enregistre un device via l'API (retourne token + clé pour le WS).
async fn create_device(
    server: &axum_test::TestServer,
    auth: &str,
    device_id: &str,
    predefined: &str,
) -> Dev {
    let org = personal_org(server, auth).await;
    let res = server
        .post("/api/v1/devices")
        .add_header("Authorization", format!("Bearer {auth}"))
        .add_header("X-Org-Id", org.to_string())
        .add_header("Content-Type", "application/json")
        .json(&serde_json::json!({
            "device_id": device_id,
            "predefined_device_name": predefined,
        }))
        .await;
    res.assert_status(axum_test::http::StatusCode::CREATED);
    let dto: serde_json::Value = res.json();
    Dev {
        id: dto["id"].as_i64().expect("id"),
        device_id: device_id.to_string(),
        token: dto["device_token"]["token"].as_str().expect("token").into(),
        key: key_bytes(dto["device_token"]["encryption_key"].as_str().expect("key")),
    }
}

async fn personal_org(server: &axum_test::TestServer, token: &str) -> i64 {
    server
        .get("/api/v1/user-info")
        .add_header("Authorization", format!("Bearer {token}"))
        .await
        .json::<serde_json::Value>()["orgs"][0]["id"]
        .as_i64()
        .expect("org perso")
}

/// Firmware-like connection: base64 query parameters, then the Noise
/// handshake keyed by the device key.
async fn connect(server: &axum_test::TestServer, dev: &Dev) -> common::DevWs {
    connect_with(server, dev, &dev.key).await
}

async fn connect_with(server: &axum_test::TestServer, dev: &Dev, key: &[u8; 32]) -> common::DevWs {
    connect_raw(server, &dev.token, &dev.device_id, key).await
}

/// Any credentials (refusal tests).
async fn connect_raw(
    server: &axum_test::TestServer,
    token: &str,
    device_id: &str,
    key: &[u8; 32],
) -> common::DevWs {
    common::DevWs::connect(
        server,
        &format!(
            "/ws/sensor/ingest?token={}&device_id={}",
            b64_param(token),
            b64_param(device_id),
        ),
        key,
        device_id.trim(),
    )
    .await
}

// ─────────────────── Tests ───────────────────

/// Cycle complet chiffré : PING/PONG, mesure ok (→ sink, scopée org),
/// erreurs de format et de validation, désalignement de clé.
#[tokio::test]
#[serial]
async fn cycle_ingest_chiffre_complet() {
    telemetry::reset_sink();
    with_app(|server, auth, ctx| async move {
        let dev = create_device(&server, &auth, "capteur-jardin", "soil_sensor").await;
        let sink = Arc::new(RecSink::default());
        telemetry::set_sink(sink.clone());
        let org = personal_org(&server, &auth).await;

        let mut ws = connect(&server, &dev).await;

        ws.send_plain("PING").await;
        assert_eq!(ws.recv_plain().await, "PONG");

        // Mesure valide (capacité du modèle) → ok + point scopé org.
        ws.send_plain("read_temperature=21.5").await;
        assert_eq!(ws.recv_plain().await, "ok");

        // Mesure hors capacités (device strict) → error:invalid_capability.
        ws.send_plain("soil_moisture=42").await;
        let err = ws.recv_plain().await;
        assert!(err.starts_with("error:invalid_capability:"), "{err}");

        // Formats invalides.
        for (frame, expected) in [
            ("sans_egal", "error:invalid_format"),
            ("=5", "error:empty_key"),
        ] {
            ws.send_plain(frame).await;
            assert_eq!(ws.recv_plain().await, expected);
        }
        let long = format!("{}=1", "x".repeat(101));
        ws.send_plain(&long).await;
        assert_eq!(ws.recv_plain().await, "error:measurement_name_too_long");

        // Unauthenticated frame (forged, or another key) → ERROR:decryption_failed,
        // the link stays usable.
        ws.send_text(STANDARD.encode([7u8; 40])).await;
        assert_eq!(ws.recv_plain().await, "ERROR:decryption_failed");

        // D16 : nom normalisé (casse/séparateurs/accents fonduus) → la
        // mesure passe la validation stricte et sort canonique.
        ws.send_plain("Read-Temperature = 19.5").await;
        assert_eq!(ws.recv_plain().await, "ok");

        // Le sink a reçu exactement les mesures valides, avec le routage org.
        let points = sink.0.lock().expect("sink").clone();
        assert_eq!(points.len(), 2);
        assert_eq!(points[1].metric_name, "read_temperature");
        let p = &points[0];
        assert_eq!(p.org_id, org);
        assert_eq!(p.device_id, "capteur-jardin");
        assert_eq!(p.metric_name, "read_temperature");
        assert_eq!(p.value, "21.5");
        assert_eq!(p.pred_dev, "soil_sensor");
        assert_eq!(p.source_type, "sensor");
        assert_eq!(p.ts_source, "server");

        // The liveness lease is held (fresh last seen, connected).
        let state = state_of(&ctx.db, dev.id).await;
        assert!(state.connected);
        assert!(state.last_seen.is_some());
        ws.close().await;
        telemetry::reset_sink();
    })
    .await;
}

/// Noise link (D156): a replayed, a tampered and a forged frame are refused
/// (no telemetry, the link stays usable), a frame captured on one
/// connection is useless on the next one.
#[tokio::test]
#[serial]
async fn noise_link_refuses_replay_tampering_and_cross_connection() {
    telemetry::reset_sink();
    with_app(|server, auth, _ctx| async move {
        let dev = create_device(&server, &auth, "capteur-noise", "soil_sensor").await;
        let sink = Arc::new(RecSink::default());
        telemetry::set_sink(sink.clone());

        let mut ws = connect(&server, &dev).await;
        let measure = ws.seal("read_temperature=21.5");
        ws.send_text(measure.clone()).await;
        assert_eq!(ws.recv_plain().await, "ok");

        // Same frame again.
        ws.send_text(measure.clone()).await;
        assert_eq!(ws.recv_plain().await, "ERROR:decryption_failed");

        // One flipped bit in a captured ciphertext (an attacker's forgery;
        // a genuine frame altered in transit would desynchronize the link,
        // which TLS prevents anyway).
        let mut wire = STANDARD.decode(&measure).unwrap();
        wire[2] ^= 0x01;
        ws.send_text(STANDARD.encode(wire)).await;
        assert_eq!(ws.recv_plain().await, "ERROR:decryption_failed");

        // The link still works after the refusals (counters untouched).
        ws.send_plain("PING").await;
        assert_eq!(ws.recv_plain().await, "PONG");
        assert_eq!(
            sink.0.lock().expect("sink").len(),
            1,
            "only the genuine measure"
        );
        ws.close().await;

        // A frame captured on the first connection, replayed on a new one.
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        let mut ws2 = connect(&server, &dev).await;
        ws2.send_text(measure).await;
        assert_eq!(ws2.recv_plain().await, "ERROR:decryption_failed");
        assert_eq!(sink.0.lock().expect("sink").len(), 1);
        ws2.close().await;
        telemetry::reset_sink();
    })
    .await;
}

/// Handshake refused (4011): wrong key, and a firmware speaking the removed
/// D8 framing (its first frame is not a Noise message, D157).
#[tokio::test]
#[serial]
async fn noise_handshake_failures_close_4011() {
    with_app(|server, auth, _ctx| async move {
        let dev = create_device(&server, &auth, "capteur-4011", "soil_sensor").await;
        let mut ws = connect_with(&server, &dev, &[1u8; 32]).await;
        assert_eq!(close_code(ws.receive_message().await), Some(4011));

        let mut old = server
            .get_websocket(&format!(
                "/ws/sensor/ingest?device_id={}",
                b64_param(&dev.device_id),
            ))
            .add_header("Authorization", format!("Bearer {}", b64_param(&dev.token)))
            .await
            .into_websocket()
            .await;
        // A D8 frame: base64(nonce ‖ ChaCha20("PING")), 16 bytes.
        old.send_text(STANDARD.encode([5u8; 16])).await;
        assert_eq!(close_code(old.receive_message().await), Some(4011));
    })
    .await;
}

/// Close codes d'auth : 4002 sans token, 4001 token inconnu, 4006 mismatch,
/// 4008 sans clé. Paramètre `\n` trailing (encodage firmware) trimé.
#[tokio::test]
#[serial]
async fn close_codes_authentification() {
    with_app(|server, auth, ctx| async move {
        let dev = create_device(&server, &auth, "dev-a", "soil_sensor").await;
        let other = create_device(&server, &auth, "dev-b", "soil_sensor").await;

        // 4002 : pas de token.
        let mut ws = server
            .get_websocket("/ws/sensor/ingest")
            .await
            .into_websocket()
            .await;
        assert_eq!(close_code(ws.receive_message().await), Some(4002));

        // 4001 : token inconnu.
        let mut ws = connect_raw(&server, "inconnu", "dev-a", &dev.key).await;
        assert_eq!(close_code(ws.receive_message().await), Some(4001));

        // 4006 : token de dev-a, device_id de dev-b.
        let mut ws = connect_raw(&server, &dev.token, &other.device_id, &dev.key).await;
        assert_eq!(close_code(ws.receive_message().await), Some(4006));

        // 4008 : clé absente.
        use pnex_backend::models::_entities::device_tokens;
        use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};
        let mut row: device_tokens::ActiveModel = device_tokens::Entity::find()
            .filter(device_tokens::Column::DeviceRegistryId.eq(other.id))
            .one(&ctx.db)
            .await
            .expect("tok")
            .expect("tok")
            .into();
        row.encryption_key = Set(None);
        row.update(&ctx.db).await.expect("key null");
        let mut ws = connect_raw(&server, &other.token, &other.device_id, &other.key).await;
        assert_eq!(close_code(ws.receive_message().await), Some(4008));

        // Trim `\n` : le firmware encode `echo | base64` (newline final)
        // — le serveur trime après décodage.
        let mut ws = connect_raw(
            &server,
            &format!("{}\n", dev.token),
            &dev.device_id,
            &dev.key,
        )
        .await;
        ws.send_plain("PING").await;
        assert_eq!(ws.recv_plain().await, "PONG");
        ws.close().await;
    })
    .await;
}

/// Anti-clone : 4003 pendant une session ouverte ; déconnexion propre =
/// bail libéré (reconnect immédiat accepté) ; last_seen périmé d'un crash
/// n'occupe plus le bail.
#[tokio::test]
#[serial]
async fn anti_clone_bail() {
    with_app(|server, auth, _ctx| async move {
        let dev = create_device(&server, &auth, "clone-target", "soil_sensor").await;

        // Session 1 ouverte.
        let mut ws1 = connect(&server, &dev).await;
        ws1.send_plain("PING").await;
        assert_eq!(ws1.recv_plain().await, "PONG");

        // Clone rejeté pendant la session.
        let mut ws2 = connect(&server, &dev).await;
        assert_eq!(close_code(ws2.receive_message().await), Some(4003));

        // Déconnexion propre : bail libéré, reconnect immédiat OK.
        ws1.close().await;
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        let mut ws3 = connect(&server, &dev).await;
        ws3.send_plain("PING").await;
        assert_eq!(ws3.recv_plain().await, "PONG");
        ws3.close().await;

        // Simule un crash (session non refermée) : last_seen périmé
        // (TTL test = 2 s) → le bail est expiré, connexion acceptée.
        tokio::time::sleep(std::time::Duration::from_millis(2100)).await;
        let mut ws4 = connect(&server, &dev).await;
        ws4.send_plain("PING").await;
        assert_eq!(ws4.recv_plain().await, "PONG");
        ws4.close().await;
    })
    .await;
}

/// Dynamic device (edge agent): measurement discovery, max_unique cap.
#[tokio::test]
#[serial]
async fn dynamique_decouverte_et_plafond() {
    with_app(|server, auth, ctx| async move {
        let dev = create_device(&server, &auth, "custom-1", "edge_agent").await;

        // Plafond à 2 mesures distinctes pour tester vite.
        use pnex_backend::models::_entities::device_registries;
        use sea_orm::{ActiveModelTrait, EntityTrait, Set};
        let mut row: device_registries::ActiveModel = device_registries::Entity::find_by_id(dev.id)
            .one(&ctx.db)
            .await
            .expect("dev")
            .expect("dev")
            .into();
        row.max_unique_measurements = Set(2);
        row.update(&ctx.db).await.expect("plafond");

        let mut ws = connect(&server, &dev).await;
        for (name, value) in [("pression", "1.2"), ("humidite", "88")] {
            ws.send_plain(&format!("{name}={value}")).await;
            assert_eq!(ws.recv_plain().await, "ok");
        }

        // D16 : style différent = même mesure découverte (pas de doublon —
        // le plafond de 2 n'est pas atteint).
        ws.send_plain("Pression = 1.4").await;
        assert_eq!(ws.recv_plain().await, "ok");
        ws.send_plain("tension=3.3").await;
        assert_eq!(ws.recv_plain().await, "error:too_many_measurements");

        // La découverte est persistée (JSONB, relecture au reconnect) —
        // noms canoniques (D16).
        let row = device_registries::Entity::find_by_id(dev.id)
            .one(&ctx.db)
            .await
            .expect("dev")
            .expect("dev");
        let names = row.discovered_measurements.expect("jsonb");
        assert!(names.get("pression").is_some() && names.get("humidite").is_some());

        // Nom normalisé vide → format invalide.
        ws.send_plain("---=1").await;
        assert_eq!(ws.recv_plain().await, "error:invalid_format");
        ws.close().await;
    })
    .await;
}

/// 4005 : token désactivé en cours de session (cache test = 0 s → la frame
/// suivante revalide et coupe).
#[tokio::test]
#[serial]
async fn revalidation_token_desactive() {
    with_app(|server, auth, ctx| async move {
        let dev = create_device(&server, &auth, "ephemere", "soil_sensor").await;
        let mut ws = connect(&server, &dev).await;
        ws.send_plain("PING").await;
        assert_eq!(ws.recv_plain().await, "PONG");

        use pnex_backend::models::_entities::device_tokens;
        use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};
        let mut row: device_tokens::ActiveModel = device_tokens::Entity::find()
            .filter(device_tokens::Column::DeviceRegistryId.eq(dev.id))
            .one(&ctx.db)
            .await
            .expect("tok")
            .expect("tok")
            .into();
        row.is_active = Set(false);
        row.update(&ctx.db).await.expect("désactivation");

        ws.send_plain("PING").await;
        assert_eq!(close_code(ws.receive_message().await), Some(4005));
    })
    .await;
}

/// Reaper : `active` suit la fraîcheur du bail — frais → true, silence ou
/// absence de state → false (sole writer, legacy handle_sensors parity).
#[tokio::test]
#[serial]
async fn reaper_active_suit_la_fraicheur() {
    with_app(|server, auth, ctx| async move {
        let dev = create_device(&server, &auth, "reaper-target", "soil_sensor").await;
        let active_of = |db: &sea_orm::DatabaseConnection, id: i64| {
            let db = db.clone();
            async move {
                use pnex_backend::models::_entities::device_registries;
                use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
                device_registries::Entity::find()
                    .filter(device_registries::Column::Id.eq(id))
                    .one(&db)
                    .await
                    .expect("dev")
                    .expect("dev")
                    .active
            }
        };

        use pnex_backend::services::device_liveness::{deactivate_stale, forget_epoch, mark_seen};
        let now = chrono::Utc::now;

        // Created inactive, never seen: the reaper leaves it inactive.
        deactivate_stale(&ctx.db, 2).await.expect("reaper");
        assert!(!active_of(&ctx.db, dev.id).await);

        // Fresh sign of life → activated.
        mark_seen(dev.id, now()).await.expect("seen");
        deactivate_stale(&ctx.db, 2).await.expect("reaper");
        assert!(active_of(&ctx.db, dev.id).await);

        // Valkey restart (epoch lost) + stale score: deactivation is held
        // back during the restart grace — the fleet does not flap offline.
        forget_epoch().await.expect("epoch");
        let old = now() - chrono::TimeDelta::seconds(60);
        mark_seen(dev.id, old).await.expect("seen");
        let (_, off) = deactivate_stale(&ctx.db, 2).await.expect("reaper");
        assert_eq!(off, 0, "restart grace");
        assert!(active_of(&ctx.db, dev.id).await);

        // Grace over (silence TTL elapsed): deactivated, last seen moved to
        // the Postgres cold record, no lease.
        tokio::time::sleep(std::time::Duration::from_millis(2100)).await;
        mark_seen(dev.id, old).await.expect("seen");
        let (on, off) = deactivate_stale(&ctx.db, 2).await.expect("reaper");
        assert_eq!((on, off), (0, 1));
        assert!(!active_of(&ctx.db, dev.id).await);
        use pnex_backend::models::_entities::device_states;
        use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
        let cold = device_states::Entity::find()
            .filter(device_states::Column::DeviceRegistryId.eq(dev.id))
            .one(&ctx.db)
            .await
            .expect("state")
            .expect("cold record");
        assert_eq!(cold.last_seen_at.timestamp(), old.timestamp());
        let st = state_of(&ctx.db, dev.id).await;
        assert!(!st.connected);
        assert_eq!(st.last_seen.map(|t| t.timestamp()), Some(old.timestamp()));
    })
    .await;
}

/// Liveness of a device as seen by the API.
async fn state_of(
    db: &sea_orm::DatabaseConnection,
    id: i64,
) -> pnex_backend::services::device_liveness::Seen {
    pnex_backend::services::device_liveness::seen_of(db, id)
        .await
        .expect("state")
}

/// Lease claim is atomic across pods (concurrent claims: exactly one
/// winner), refused while a live session holds it, granted when released
/// or stale; touch and release are owner-checked — a superseded session can
/// neither refresh nor release the newer session's lease.
#[tokio::test]
#[serial]
async fn lease_claim_is_atomic_and_release_is_owner_checked() {
    use pnex_backend::services::device_liveness::{claim, release, touch_owned};
    with_app(|server, auth, ctx| async move {
        let dev = create_device(&server, &auth, "lease-owner", "soil_sensor").await;
        let db = &ctx.db;

        // Concurrent admissions on a free lease: exactly one wins.
        let mut tasks = Vec::new();
        for i in 0..8 {
            tasks.push(tokio::spawn(async move {
                let session = format!("race-{i}");
                let won = claim(dev.id, &session, 60).await.expect("claim");
                (session, won)
            }));
        }
        let mut winners = Vec::new();
        for t in tasks {
            let (session, won) = t.await.expect("join");
            if won {
                winners.push(session);
            }
        }
        assert_eq!(winners.len(), 1, "one admission only");
        let holder = winners.remove(0);
        assert!(state_of(db, dev.id).await.connected);

        // Fresh live lease: another session is refused, the holder re-claims.
        assert!(!claim(dev.id, "other", 60).await.expect("claim"));
        assert!(claim(dev.id, &holder, 1).await.expect("same session"));

        // Expired lease (1 s TTL elapsed): a new session takes over.
        tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
        assert!(claim(dev.id, "newer", 60).await.expect("claim"));
        // The superseded session can neither touch nor release.
        assert!(!touch_owned(dev.id, &holder, 60).await.expect("touch"));
        assert!(!release(db, dev.id, &holder).await.expect("release"));
        assert!(
            state_of(db, dev.id).await.connected,
            "late close of the old session keeps the new lease"
        );
        assert!(!claim(dev.id, &holder, 60).await.expect("claim"));

        // The owner refreshes, then releases: immediate reconnect accepted.
        assert!(touch_owned(dev.id, "newer", 60).await.expect("touch"));
        assert!(release(db, dev.id, "newer").await.expect("release"));
        assert!(!state_of(db, dev.id).await.connected);
        assert!(claim(dev.id, "after-release", 60).await.expect("claim"));
    })
    .await;
}

/// A half-open session (silent beyond the silence TTL) does not lock the
/// device out: the reconnection is admitted, the stale session is
/// superseded and closed, and its late teardown never marks the device
/// offline.
#[tokio::test]
#[serial]
async fn stale_ingest_session_is_superseded_without_releasing_the_new_lease() {
    with_app(|server, auth, ctx| async move {
        let dev = create_device(&server, &auth, "stale-ingest", "soil_sensor").await;
        let ws1 = {
            let mut ws1 = connect(&server, &dev).await;
            ws1.send_plain("PING").await;
            assert_eq!(ws1.recv_plain().await, "PONG");
            ws1
        };
        // ws1 goes silent (half-open): lease stale after the 2 s test TTL.
        tokio::time::sleep(std::time::Duration::from_millis(2100)).await;
        let mut ws2 = connect(&server, &dev).await;
        ws2.send_plain("PING").await;
        assert_eq!(ws2.recv_plain().await, "PONG");
        // Let the superseded session tear down (owner-checked release).
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        let st = state_of(&ctx.db, dev.id).await;
        assert!(st.connected, "new session still holds the lease");
        // A clone is still rejected while ws2 is live.
        let mut ws3 = connect(&server, &dev).await;
        assert_eq!(close_code(ws3.receive_message().await), Some(4003));
        ws2.send_plain("PING").await;
        assert_eq!(ws2.recv_plain().await, "PONG");
        drop(ws1);
        ws2.close().await;
    })
    .await;
}
