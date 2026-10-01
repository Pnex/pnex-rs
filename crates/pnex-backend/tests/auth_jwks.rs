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
        issuer_url: None,
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

// ─────────────── Refresh discipline (single-flight, cooldown, TTL) ───────────────

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// Mock JWKS endpoint that counts hits and can be switched to fail (500).
async fn spawn_counting_jwks(hits: Arc<AtomicU64>, failing: Arc<AtomicBool>) -> String {
    use axum::{http::StatusCode, response::IntoResponse, routing::get, Router};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    let app = Router::new().route(
        "/auth/v1/oidc/certs",
        get(move || {
            let hits = hits.clone();
            let failing = failing.clone();
            async move {
                hits.fetch_add(1, Ordering::SeqCst);
                if failing.load(Ordering::SeqCst) {
                    StatusCode::INTERNAL_SERVER_ERROR.into_response()
                } else {
                    axum::Json(common::jwks_body()).into_response()
                }
            }
        }),
    );
    tokio::spawn(async move {
        axum::serve(listener, app).await.expect("mock jwks");
    });
    format!("http://{addr}")
}

fn settings_for(base: &str) -> RauthySettings {
    RauthySettings {
        base_url: base.into(),
        issuer_url: None,
        client_id: "pnex".into(),
    }
}

fn token_with_kid(base: &str, kid: &str) -> String {
    let mut header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256);
    header.kid = Some(kid.into());
    let claims = serde_json::json!({
        "sub": "00000000-0000-0000-000000000001",
        "preferred_username": "alice",
        "iss": format!("{base}/auth/v1/"),
        "aud": ["pnex"],
        "exp": chrono::Utc::now().timestamp() + 3600,
    });
    jsonwebtoken::encode(
        &header,
        &claims,
        &jsonwebtoken::EncodingKey::from_rsa_pem(include_bytes!("fixtures/jwks_test_key.pem"))
            .expect("key"),
    )
    .expect("token")
}

#[tokio::test]
async fn unknown_kids_do_not_refetch_within_cooldown() {
    let hits = Arc::new(AtomicU64::new(0));
    let base = spawn_counting_jwks(hits.clone(), Arc::new(AtomicBool::new(false))).await;
    let v = JwksVerifier::with_timings(
        &settings_for(&base),
        Duration::from_secs(30),
        Duration::from_secs(3600),
    );
    let good = mint_token(&TokenSpec {
        issuer: format!("{base}/auth/v1/"),
        ..Default::default()
    });
    v.verify(&good).await.expect("valid token");
    assert_eq!(hits.load(Ordering::SeqCst), 1);

    // A flood of random kids: zero extra fetch inside the cooldown window.
    for i in 0..50 {
        let t = token_with_kid(&base, &format!("random-{i}"));
        assert!(matches!(
            v.verify(&t).await,
            Err(jwks::VerifyError::UnknownKid)
        ));
    }
    assert_eq!(hits.load(Ordering::SeqCst), 1, "no per-request refetch");
    // Known kid keeps verifying from cache.
    v.verify(&good).await.expect("still valid");
    assert_eq!(hits.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn concurrent_misses_share_a_single_fetch() {
    let hits = Arc::new(AtomicU64::new(0));
    let base = spawn_counting_jwks(hits.clone(), Arc::new(AtomicBool::new(false))).await;
    let v = Arc::new(JwksVerifier::with_timings(
        &settings_for(&base),
        Duration::from_secs(30),
        Duration::from_secs(3600),
    ));
    let good = mint_token(&TokenSpec {
        issuer: format!("{base}/auth/v1/"),
        ..Default::default()
    });
    let mut set = tokio::task::JoinSet::new();
    for _ in 0..32 {
        let v = v.clone();
        let t = good.clone();
        set.spawn(async move { v.verify(&t).await.is_ok() });
    }
    while let Some(ok) = set.join_next().await {
        assert!(ok.expect("join"), "every concurrent request verifies");
    }
    assert_eq!(hits.load(Ordering::SeqCst), 1, "single-flight fetch");
}

#[tokio::test]
async fn failed_refresh_keeps_known_keys_and_ttl_refreshes() {
    let hits = Arc::new(AtomicU64::new(0));
    let failing = Arc::new(AtomicBool::new(false));
    let base = spawn_counting_jwks(hits.clone(), failing.clone()).await;
    // Zero cooldown + tiny TTL: every request past the TTL refreshes.
    let v = JwksVerifier::with_timings(
        &settings_for(&base),
        Duration::ZERO,
        Duration::from_millis(50),
    );
    let good = mint_token(&TokenSpec {
        issuer: format!("{base}/auth/v1/"),
        ..Default::default()
    });
    v.verify(&good).await.expect("valid token");
    assert_eq!(hits.load(Ordering::SeqCst), 1);

    // TTL elapsed + IdP down: refresh attempted, cached key still served.
    failing.store(true, Ordering::SeqCst);
    tokio::time::sleep(Duration::from_millis(80)).await;
    v.verify(&good)
        .await
        .expect("cached key survives a failed refresh");
    assert_eq!(hits.load(Ordering::SeqCst), 2, "periodic refresh attempted");
    // A miss while the IdP is down does not wipe the map either.
    let unknown = token_with_kid(&base, "rotated-away");
    assert!(v.verify(&unknown).await.is_err());
    v.verify(&good)
        .await
        .expect("map intact after failed miss refresh");

    // IdP back: the TTL refresh succeeds again.
    failing.store(false, Ordering::SeqCst);
    tokio::time::sleep(Duration::from_millis(80)).await;
    let before = hits.load(Ordering::SeqCst);
    v.verify(&good).await.expect("valid after recovery");
    assert!(hits.load(Ordering::SeqCst) > before, "TTL refresh happened");
}
