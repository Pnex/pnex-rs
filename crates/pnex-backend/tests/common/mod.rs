//! Outils communs aux tests d'auth : serveur JWKS mock (remplace Rauthy en
//! CI) et fabrication de tokens signés.
//!
//! La clé `tests/fixtures/jwks_test_key.pem` est une clé RSA de test, sans
//! aucune valeur hors des tests automatisés.

#![allow(dead_code)]

use axum::routing::get;
use axum::Router;
use tokio::net::TcpListener;

/// Modulus (base64url) de la clé de test — extrait du PEM ci-dessous.
pub const N_B64URL: &str = "ypJs_Sp48i4rlNHndma8e4lQ6SopZWHk3gwFMWxW4E95sfGRrV7_7j4n7XBEP3OJMIP2tTyGaNTLlCszpK3xjYChwZjTGJ51pNxHuWBrwGyVtdat5ewuWoyBEwF_KAhV7VE2pp7ak3tfpV4oJTo6BZ8nXWGOazV-kZn6YZlpMoe0vlCjMZo3lXJrw3Hk15Mf_BG8pdCT7TtL4-WKFJbCVoyH0xyzenInzeW5a_8qMQ8bRk_9NYCOMk_sWIeXY7-Re-2VGATsdp498cHfdZlmoBDjC42Kn7V4ME9809bnOFR1uNJBgrLDnzmpXgoz0pIU4-VsZsuzTe_4zVTvQaUasw";
pub const KID: &str = "test-key-1";

pub fn jwks_body() -> serde_json::Value {
    serde_json::json!({
        "keys": [{
            "kty": "RSA",
            "kid": KID,
            "alg": "RS256",
            "n": N_B64URL,
            "e": "AQAB",
        }]
    })
}

/// Serve les JWKS sur un port aléatoire de 127.0.0.1 ; retourne l'URL de base
/// (`http://127.0.0.1:{port}`) à utiliser comme `RAUTHY_URL`.
pub async fn spawn_mock_rauthy() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind mock");
    let addr = listener.local_addr().expect("addr mock");
    let app = Router::new().route(
        "/auth/v1/oidc/certs",
        get(|| async { axum::Json(jwks_body()) }),
    );
    tokio::spawn(async move {
        axum::serve(listener, app).await.expect("mock rauthy");
    });
    format!("http://{}", addr)
}

fn encoding_key() -> jsonwebtoken::EncodingKey {
    jsonwebtoken::EncodingKey::from_rsa_pem(include_bytes!("../fixtures/jwks_test_key.pem"))
        .expect("clé PEM de test")
}

pub struct TokenSpec {
    pub sub: String,
    pub preferred_username: String,
    pub email: String,
    pub given_name: String,
    pub family_name: String,
    /// Expiration (epoch secondes).
    pub exp: i64,
    pub issuer: String,
    pub audience: serde_json::Value,
    /// `email_verified` claim (Rauthy sets it; SEC-W5 relink guard).
    pub email_verified: bool,
}

impl Default for TokenSpec {
    fn default() -> Self {
        Self {
            sub: "00000000-0000-0000-000000000001".into(),
            preferred_username: "alice".into(),
            email: "alice@example.com".into(),
            given_name: "Alice".into(),
            family_name: "Martin".into(),
            exp: chrono::Utc::now().timestamp() + 3600,
            email_verified: true,
            issuer: String::new(),
            audience: serde_json::json!(["account", "pnex"]),
        }
    }
}

/// Signe un access token de test (RS256, kid `test-key-1`).
pub fn mint_token(spec: &TokenSpec) -> String {
    let mut header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256);
    header.kid = Some(KID.into());
    header.typ = Some("JWT".into());
    let claims = serde_json::json!({
        "sub": spec.sub,
        "preferred_username": spec.preferred_username,
        "email": spec.email,
        "given_name": spec.given_name,
        "family_name": spec.family_name,
        "iss": spec.issuer,
        "aud": spec.audience,
        "exp": spec.exp,
        "email_verified": spec.email_verified,
    });
    jsonwebtoken::encode(&header, &claims, &encoding_key()).expect("signature token test")
}

/// Token valide pour le mock donné + claims utilisateur standard.
pub fn valid_token(base_url: &str, sub: &str, username: &str, email: &str) -> String {
    let (given, family) = match username {
        "alice" => ("Alice", "Martin"),
        _ => ("Bob", "Dupont"),
    };
    mint_token(&TokenSpec {
        sub: sub.into(),
        preferred_username: username.into(),
        email: email.into(),
        given_name: given.into(),
        family_name: family.into(),
        // Slash final obligatoire : l'issuer Rauthy est `{base}/auth/v1/`.
        issuer: format!("{base_url}/auth/v1/"),
        ..Default::default()
    })
}

/// Access token Rauthy « lean » (géométrie réelle D19) : SANS
/// preferred_username/given_name/family_name — ces claims ne vivent que dans
/// l'id_token. Sert à figer le fallback `username` de user-info.
pub fn lean_token(base_url: &str, sub: &str, email: &str) -> String {
    let mut header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256);
    header.kid = Some(KID.into());
    header.typ = Some("JWT".into());
    let claims = serde_json::json!({
        "sub": sub,
        "email": email,
        // Slash final obligatoire : issuer Rauthy = `{base}/auth/v1/`.
        "iss": format!("{base_url}/auth/v1/"),
        "aud": ["account", "pnex"],
        "exp": chrono::Utc::now().timestamp() + 3600,
    });
    jsonwebtoken::encode(&header, &claims, &encoding_key()).expect("signature token test")
}

/// Catalogue minimal pour les tests. Tier Free : 3 sensors / 1 actuator /
/// 0 mixed (les quotas s'y testent vite).
pub async fn seed_catalogue(db: &sea_orm::DatabaseConnection) {
    use pnex_backend::models::_entities::{
        device_capabilities, device_types, mcu_boards, predefined_device_capabilities,
        predefined_devices, sea_orm_active_enums::CapabilityMode, subscription_tiers as tiers,
    };
    use sea_orm::{ActiveModelTrait, Set};

    tiers::ActiveModel {
        name: Set("Free".into()),
        max_sensor_devices: Set(3),
        max_actuator_devices: Set(1),
        // Brick 0 : 1 mixed autorisé en Free (device générique).
        max_mixed_devices: Set(1),
        min_build_interval_secs: Set(300),
        data_retention_secs: Set(Some(86_400)),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("tier Free");

    let mut type_ids = std::collections::HashMap::new();
    for name in ["sensor", "actuator", "mixed", "agent"] {
        let t = device_types::ActiveModel {
            name: Set(name.into()),
            ..Default::default()
        }
        .insert(db)
        .await
        .expect("device type");
        type_ids.insert(name, t.id);
    }

    let mut cap_ids = std::collections::HashMap::new();
    for (name, mode) in [
        ("read_temperature", CapabilityMode::Input),
        ("relay", CapabilityMode::Output),
    ] {
        let c = device_capabilities::ActiveModel {
            name: Set(name.into()),
            mode: Set(mode),
            ..Default::default()
        }
        .insert(db)
        .await
        .expect("capability");
        cap_ids.insert(name, c.id);
    }

    let board = mcu_boards::ActiveModel {
        name: Set("esp32".into()),
        soc: Set("esp32".into()),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("board");
    // Brick 0 : board esp8266 with the catalogue NodeMCU v2 profile
    // (mcu_boards.details), used by generic_esp8266 via services::provisioning.
    let profile = (Some(&pnex_core::catalog::boards::ESP8266_NODEMCU)
        .and_then(|b| b.profile)
        .expect("catalogue nodemcu profile"))();
    let board8266 = mcu_boards::ActiveModel {
        name: Set("esp8266".into()),
        soc: Set("esp8266".into()),
        details: Set(Some(serde_json::to_value(profile).expect("profile json"))),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("board esp8266");

    for (name, type_name, caps) in [
        ("soil_sensor", "sensor", vec!["read_temperature"]),
        ("relay_1ch", "actuator", vec!["relay"]),
        // Generic ESP32 on the overlay-less board: custom firmware
        // admission (the sketch owns the pins).
        ("generic_esp32", "mixed", vec![]),
        ("mixed_hub_v1", "mixed", vec!["read_temperature", "relay"]),
        // Brick 0 : device générique (board esp8266 + overlay).
        ("generic_esp8266", "mixed", vec![]),
        // Edge agent (D95): no board, no pins.
        ("edge_agent", "agent", vec![]),
    ] {
        let pd = predefined_devices::ActiveModel {
            name: Set(name.into()),
            revision: Set("v1".into()),
            device_type_id: Set(type_ids[type_name]),
            board_id: Set(if name == "generic_esp8266" {
                board8266.id
            } else {
                board.id
            }),
            ..Default::default()
        }
        .insert(db)
        .await
        .expect("predefined device");
        for cap in caps {
            predefined_device_capabilities::ActiveModel {
                predefined_device_id: Set(pd.id),
                device_capability_id: Set(cap_ids[cap]),
                ..Default::default()
            }
            .insert(db)
            .await
            .expect("lien capability");
        }
    }
}

/// Device side of a `/ws/*` connection, Noise NNpsk0 link included (D156):
/// [`DevWs::connect`] sends the first Noise message and reads the server's
/// answer, like the firmware. When the server refuses the connection
/// instead (close code), the close frame is kept for `receive_message`.
pub struct DevWs {
    pub ws: axum_test::TestWebSocket,
    link: Option<pnex_core::frame::Link>,
    pending: Option<axum_test::WsMessage>,
}

impl DevWs {
    /// Text link (`/ws/device`, `/ws/sensor/ingest`).
    pub async fn connect(
        server: &axum_test::TestServer,
        url: &str,
        key: &[u8; 32],
        device_id: &str,
    ) -> Self {
        Self::start(server, url, key, device_id, false).await
    }

    /// Binary link (`/ws/camera`).
    pub async fn connect_binary(
        server: &axum_test::TestServer,
        url: &str,
        key: &[u8; 32],
        device_id: &str,
    ) -> Self {
        Self::start(server, url, key, device_id, true).await
    }

    /// Text link with the device token in the `Authorization` header
    /// (D154) instead of the URL.
    pub async fn connect_with_header(
        server: &axum_test::TestServer,
        url: &str,
        token: &str,
        key: &[u8; 32],
        device_id: &str,
    ) -> Self {
        let ws = server
            .get_websocket(url)
            .add_header("Authorization", format!("Bearer {token}"))
            .await
            .into_websocket()
            .await;
        Self::handshake(ws, key, device_id, false).await
    }

    async fn start(
        server: &axum_test::TestServer,
        url: &str,
        key: &[u8; 32],
        device_id: &str,
        binary: bool,
    ) -> Self {
        // Tests still write `?token=…` in their URLs: it is moved to the
        // `Authorization` header, where the server reads it (D154).
        let (url, token) = split_token(url);
        let request = server.get_websocket(&url);
        let request = match token {
            Some(t) => request.add_header("Authorization", format!("Bearer {t}")),
            None => request,
        };
        let ws = request.await.into_websocket().await;
        Self::handshake(ws, key, device_id, binary).await
    }

    async fn handshake(
        mut ws: axum_test::TestWebSocket,
        key: &[u8; 32],
        device_id: &str,
        binary: bool,
    ) -> Self {
        let (init, msg1) = pnex_core::frame::Initiator::start(key, device_id);
        if binary {
            ws.send_message(axum_test::WsMessage::Binary(msg1.into()))
                .await;
        } else {
            ws.send_text(pnex_core::frame::encode_handshake(&msg1))
                .await;
        }
        let answer = ws.receive_message().await;
        let msg2 = match &answer {
            axum_test::WsMessage::Text(t) => pnex_core::frame::decode_handshake(t.as_str()),
            axum_test::WsMessage::Binary(b) => Some(b.to_vec()),
            _ => None,
        };
        match msg2 {
            Some(msg2) => Self {
                ws,
                link: Some(init.finish(&msg2).expect("Noise handshake")),
                pending: None,
            },
            None => Self {
                ws,
                link: None,
                pending: Some(answer),
            },
        }
    }

    fn link(&mut self) -> &mut pnex_core::frame::Link {
        self.link
            .as_mut()
            .expect("connection refused by the server")
    }

    /// Seals a text frame without sending it (replay / tampering tests).
    pub fn seal(&mut self, plain: &str) -> String {
        self.link().seal_text(plain)
    }

    /// Opens a text frame of the server.
    pub fn open(&mut self, raw: &str) -> Option<String> {
        self.link().open_text(raw)
    }

    pub async fn send_plain(&mut self, plain: &str) {
        let wire = self.seal(plain);
        self.ws.send_text(wire).await;
    }

    pub async fn recv_plain(&mut self) -> String {
        let raw = self.ws.receive_text().await;
        self.open(&raw).expect("authenticated server frame")
    }

    /// Seals and sends a binary frame (camera uplink).
    pub async fn send_sealed_bytes(&mut self, plain: &[u8]) {
        let wire = self.link().seal(plain);
        self.ws
            .send_message(axum_test::WsMessage::Binary(wire.into()))
            .await;
    }

    pub async fn send_text(&mut self, raw: String) {
        self.ws.send_text(raw).await;
    }

    pub async fn send_message(&mut self, msg: axum_test::WsMessage) {
        self.ws.send_message(msg).await;
    }

    pub async fn receive_text(&mut self) -> String {
        self.ws.receive_text().await
    }

    /// Next message, the refusal close frame first when the server
    /// refused the connection.
    pub async fn receive_message(&mut self) -> axum_test::WsMessage {
        match self.pending.take() {
            Some(m) => m,
            None => self.ws.receive_message().await,
        }
    }

    pub async fn close(self) {
        self.ws.close().await;
    }
}

/// Splits the `token` parameter out of a device URL: (URL without it,
/// its value). D154 moved the device token to the `Authorization` header.
pub fn split_token(url: &str) -> (String, Option<String>) {
    let Some((path, query)) = url.split_once('?') else {
        return (url.to_string(), None);
    };
    let mut token = None;
    let rest: Vec<&str> = query
        .split('&')
        .filter(|kv| match kv.strip_prefix("token=") {
            Some(v) => {
                token = Some(v.to_string());
                false
            }
            None => true,
        })
        .collect();
    let url = if rest.is_empty() {
        path.to_string()
    } else {
        format!("{path}?{}", rest.join("&"))
    };
    (url, token)
}

/// One-time ticket opening a browser websocket for `token` in `org`.
pub async fn ws_ticket(server: &axum_test::TestServer, token: &str, org: i64) -> String {
    server
        .post("/api/v1/ws-ticket")
        .add_header("Authorization", format!("Bearer {token}"))
        .add_header("X-Org-Id", org.to_string())
        .await
        .json::<serde_json::Value>()["ticket"]
        .as_str()
        .expect("ws ticket")
        .to_string()
}
