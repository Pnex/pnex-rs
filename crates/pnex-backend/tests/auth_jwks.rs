//! Validation JWT/JWKS : couvre les durcissements Phase 3 (iss, aud, exp,
//! RS256, kid inconnu) contre un mock JWKS — pas de Rauthy requis.

mod common;

use common::{mint_token, spawn_mock_rauthy, TokenSpec};
use pnex_backend::auth::{
    claims::Aud,
    jwks::{self, JwksVerifier},
    settings::RauthySettings,
};

async fn verifier(base_url: &str) -> std::sync::Arc<JwksVerifier> {
    let settings = RauthySettings {
        base_url: base_url.into(),
        client_id: "pnex".into(),
    };
    jwks::verifier_for(&settings).await
}

#[tokio::test]
async fn token_valide_passe_et_expose_les_claims() {
    let base = spawn_mock_rauthy().await;
    let v = verifier(&base).await;
    let token = mint_token(&TokenSpec {
        issuer: format!("{base}/auth/v1/"),
        ..Default::default()
    });
    let claims = v.verify(&token).await.expect("token valide");
    assert_eq!(claims.preferred_username.as_deref(), Some("alice"));
    assert_eq!(claims.display_name(), "Alice Martin");
    assert_eq!(claims.email.as_deref(), Some("alice@example.com"));
    assert!(matches!(&claims.aud, Some(Aud::Many(list)) if list.contains(&"pnex".to_string())));
}

#[tokio::test]
async fn mauvais_issuer_rejete() {
    let base = spawn_mock_rauthy().await;
    let v = verifier(&base).await;
    let token = mint_token(&TokenSpec {
        // Token signé par la bonne clé mais émis par un autre IdP.
        issuer: "http://evil.example/auth/v1/".into(),
        ..Default::default()
    });
    assert!(matches!(
        v.verify(&token).await,
        Err(jwks::VerifyError::BadIssuer)
    ));
}

#[tokio::test]
async fn mauvaise_audience_rejetee() {
    let base = spawn_mock_rauthy().await;
    let v = verifier(&base).await;
    let token = mint_token(&TokenSpec {
        issuer: format!("{base}/auth/v1/"),
        audience: serde_json::json!(["un-autre-client"]),
        ..Default::default()
    });
    assert!(matches!(
        v.verify(&token).await,
        Err(jwks::VerifyError::BadAudience)
    ));
}

#[tokio::test]
async fn token_expire_rejete() {
    let base = spawn_mock_rauthy().await;
    let v = verifier(&base).await;
    let token = mint_token(&TokenSpec {
        issuer: format!("{base}/auth/v1/"),
        exp: chrono::Utc::now().timestamp() - 3600,
        ..Default::default()
    });
    assert!(matches!(
        v.verify(&token).await,
        Err(jwks::VerifyError::Expired)
    ));
}

#[tokio::test]
async fn token_non_rs256_rejete() {
    let base = spawn_mock_rauthy().await;
    let v = verifier(&base).await;
    // Header HS256 avec le kid du mock : l'algorithme est refusé avant même
    // la vérification de signature.
    let mut header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::HS256);
    header.kid = Some(common::KID.into());
    let claims = serde_json::json!({
        "sub": "00000000-0000-0000-000000000001",
        "preferred_username": "alice",
        "iss": format!("{base}/auth/v1/"),
        "aud": ["pnex"],
        "exp": chrono::Utc::now().timestamp() + 3600,
    });
    let token = jsonwebtoken::encode(
        &header,
        &claims,
        &jsonwebtoken::EncodingKey::from_secret(b"secret"),
    )
    .expect("token HS256");
    assert!(v.verify(&token).await.is_err());
}

#[tokio::test]
async fn kid_inconnu_rejete_apres_rafraichissement() {
    let base = spawn_mock_rauthy().await;
    let v = verifier(&base).await;
    let mut header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256);
    header.kid = Some("kid-qui-n-existe-pas".into());
    let claims = serde_json::json!({
        "sub": "00000000-0000-0000-000000000001",
        "preferred_username": "alice",
        "iss": format!("{base}/auth/v1/"),
        "aud": ["pnex"],
        "exp": chrono::Utc::now().timestamp() + 3600,
    });
    let token = jsonwebtoken::encode(
        &header,
        &claims,
        &jsonwebtoken::EncodingKey::from_rsa_pem(include_bytes!("fixtures/jwks_test_key.pem"))
            .expect("clé"),
    )
    .expect("token kid inconnu");
    assert!(matches!(
        v.verify(&token).await,
        Err(jwks::VerifyError::UnknownKid)
    ));
}

#[tokio::test]
async fn token_malforme_rejete() {
    let base = spawn_mock_rauthy().await;
    let v = verifier(&base).await;
    assert!(matches!(
        v.verify("pas-un-jwt").await,
        Err(jwks::VerifyError::Malformed)
    ));
}
