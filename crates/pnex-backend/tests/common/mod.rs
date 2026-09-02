//! Outils communs aux tests d'auth : serveur JWKS mock (remplace Keycloak en
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

fn jwks_body() -> serde_json::Value {
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
/// (`http://127.0.0.1:{port}`) à utiliser comme `KEYCLOAK_URL`.
pub async fn spawn_mock_keycloak() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind mock");
    let addr = listener.local_addr().expect("addr mock");
    let app = Router::new().route(
        "/realms/pnex-realm/protocol/openid-connect/certs",
        get(|| async { axum::Json(jwks_body()) }),
    );
    tokio::spawn(async move {
        axum::serve(listener, app).await.expect("mock keycloak");
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
        issuer: format!("{base_url}/realms/pnex-realm"),
        ..Default::default()
    })
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
    for name in ["sensor", "actuator", "mixed"] {
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
    // Brick 0 : board esp8266 avec overlay NodeMCU (mcu_boards.details),
    // consommé par generic_esp8266 via services::provisioning.
    let overlay: pnex_core::BoardOverlay = serde_json::from_value(serde_json::json!({
        "board": "nodemcu",
        "pins": [
            {"label": "D0", "gpio": 16, "kind": "digital"},
            {"label": "D1", "gpio": 5, "kind": "digital"},
            {"label": "D2", "gpio": 4, "kind": "digital"},
            {"label": "D3", "gpio": 0, "kind": "digital"},
            {"label": "D4", "gpio": 2, "kind": "digital"},
            {"label": "D5", "gpio": 14, "kind": "digital"},
            {"label": "D6", "gpio": 12, "kind": "digital"},
            {"label": "D7", "gpio": 13, "kind": "digital"},
            {"label": "D8", "gpio": 15, "kind": "digital"},
            {"label": "A0", "gpio": 17, "kind": "analog"}
        ]
    }))
    .expect("overlay inline");
    let board8266 = mcu_boards::ActiveModel {
        name: Set("esp8266".into()),
        soc: Set("esp8266".into()),
        details: Set(Some(serde_json::to_value(&overlay).expect("overlay json"))),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("board esp8266");

    for (name, type_name, caps) in [
        ("soil_sensor", "sensor", vec!["read_temperature"]),
        ("4_chan_relay", "actuator", vec!["relay"]),
        ("custom_sensor", "sensor", vec![]),
        ("mixed_hub_v1", "mixed", vec!["read_temperature", "relay"]),
        // Brick 0 : device générique (board esp8266 + overlay).
        ("generic_esp8266", "mixed", vec![]),
    ] {
        let pd = predefined_devices::ActiveModel {
            name: Set(name.into()),
            revision: Set("v1".into()),
            device_type_id: Set(type_ids[type_name]),
            board_id: Set(if name == "generic_esp8266" { board8266.id } else { board.id }),
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
