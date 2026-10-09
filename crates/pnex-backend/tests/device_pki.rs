//! Device PKI (D153, lot L3): one CA per org, a certificate issued per
//! build, and the verification of a presented certificate.

mod common;

use loco_rs::testing::request::{RequestConfig, RequestConfigBuilder};
use pnex_backend::app::App;
use pnex_backend::models::_entities::{device_certificates, org_device_cas};
use pnex_backend::services::device_pki::{issue_device_cert, verify_client_cert, CertRefusal};
use pnex_backend::services::secrets::Keyring;
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, PaginatorTrait, QueryFilter, Set};
use serial_test::serial;

async fn with_app<F, Fut>(f: F)
where
    F: FnOnce(axum_test::TestServer, loco_rs::app::AppContext, String) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let base = common::spawn_mock_rauthy().await;
    let _ = tracing_subscriber::fmt().with_test_writer().try_init();
    unsafe { std::env::set_var("RAUTHY_URL", &base) };
    let alice = common::valid_token(
        &base,
        "pki-alice-000000000000000",
        "alice",
        "pki-alice@example.com",
    );
    let config: RequestConfig = RequestConfigBuilder::new().build();
    loco_rs::testing::request::request_with_config::<App, _, _>(
        config,
        move |server, ctx| async move {
            common::seed_catalogue(&ctx.db).await;
            f(server, ctx, alice).await;
        },
    )
    .await;
}

async fn create_device(server: &axum_test::TestServer, token: &str, id: &str) -> (i64, i64) {
    let org = server
        .get("/api/v1/user-info")
        .add_header("Authorization", format!("Bearer {token}"))
        .await
        .json::<serde_json::Value>()["orgs"][0]["id"]
        .as_i64()
        .unwrap();
    let res = server
        .post("/api/v1/devices")
        .add_header("Authorization", format!("Bearer {token}"))
        .add_header("X-Org-Id", org.to_string())
        .json(&serde_json::json!({"device_id": id, "predefined_device_name": "generic_esp8266"}))
        .await;
    res.assert_status(axum_test::http::StatusCode::CREATED);
    (org, res.json::<serde_json::Value>()["id"].as_i64().unwrap())
}

#[tokio::test]
#[serial]
async fn issued_certificates_identify_their_device_and_nothing_else() {
    with_app(|server, ctx, alice| async move {
        let ring = Keyring::from_config(&ctx.config).unwrap();
        let (org, pk) = create_device(&server, &alice, "pki-dev-1").await;

        let first = issue_device_cert(&ctx.db, &ring, org, pk, "pki-dev-1").await.unwrap();
        assert!(first.cert_pem.contains("BEGIN CERTIFICATE"));
        assert!(first.key_pem.contains("PRIVATE KEY"));
        assert_eq!(
            verify_client_cert(&ctx.db, &first.cert_pem).await.unwrap(),
            Ok((org, pk))
        );
        // A rebuild issues a new certificate under the same CA; both work
        // until one is revoked (a device keeps connecting until its update).
        let second = issue_device_cert(&ctx.db, &ring, org, pk, "pki-dev-1").await.unwrap();
        assert_ne!(first.fingerprint_sha256, second.fingerprint_sha256);
        assert_eq!(
            org_device_cas::Entity::find()
                .filter(org_device_cas::Column::OrgId.eq(org))
                .count(&ctx.db)
                .await
                .unwrap(),
            1
        );
        assert_eq!(
            verify_client_cert(&ctx.db, &second.cert_pem).await.unwrap(),
            Ok((org, pk))
        );

        // A look-alike certificate never issued here: unknown.
        let key = rcgen::KeyPair::generate().unwrap();
        let mut params = rcgen::CertificateParams::new(Vec::<String>::new()).unwrap();
        params
            .distinguished_name
            .push(rcgen::DnType::CommonName, "pki-dev-1");
        let forged = params.self_signed(&key).unwrap();
        assert_eq!(
            verify_client_cert(&ctx.db, &forged.pem()).await.unwrap(),
            Err(CertRefusal::Unknown)
        );
        assert_eq!(
            verify_client_cert(&ctx.db, "not a certificate").await.unwrap(),
            Err(CertRefusal::Malformed)
        );

        // Revoked: refused.
        let row = device_certificates::Entity::find()
            .filter(device_certificates::Column::FingerprintSha256.eq(&first.fingerprint_sha256))
            .one(&ctx.db)
            .await
            .unwrap()
            .unwrap();
        let mut am: device_certificates::ActiveModel = row.into();
        am.revoked_at = Set(Some(chrono::Utc::now().into()));
        am.update(&ctx.db).await.unwrap();
        assert_eq!(
            verify_client_cert(&ctx.db, &first.cert_pem).await.unwrap(),
            Err(CertRefusal::Revoked)
        );

        // The org boundary is the CA: a certificate of org B whose registry
        // row claims org A fails the chain check against A's CA.
        let org_b = server
            .post("/api/v1/orgs")
            .add_header("Authorization", format!("Bearer {alice}"))
            .json(&serde_json::json!({"name": "pki-org-b"}))
            .await
            .json::<serde_json::Value>()["id"]
            .as_i64()
            .unwrap();
        let res = server
            .post("/api/v1/devices")
            .add_header("Authorization", format!("Bearer {alice}"))
            .add_header("X-Org-Id", org_b.to_string())
            .json(&serde_json::json!({"device_id": "pki-dev-b", "predefined_device_name": "generic_esp8266"}))
            .await;
        res.assert_status(axum_test::http::StatusCode::CREATED);
        let pk_b = res.json::<serde_json::Value>()["id"].as_i64().unwrap();
        let cert_b = issue_device_cert(&ctx.db, &ring, org_b, pk_b, "pki-dev-b").await.unwrap();
        assert_eq!(
            verify_client_cert(&ctx.db, &cert_b.cert_pem).await.unwrap(),
            Ok((org_b, pk_b))
        );
        let mut moved: device_certificates::ActiveModel = device_certificates::Entity::find()
            .filter(device_certificates::Column::FingerprintSha256.eq(&cert_b.fingerprint_sha256))
            .one(&ctx.db)
            .await
            .unwrap()
            .unwrap()
            .into();
        moved.org_id = Set(org);
        moved.update(&ctx.db).await.unwrap();
        assert_eq!(
            verify_client_cert(&ctx.db, &cert_b.cert_pem).await.unwrap(),
            Err(CertRefusal::BadChain)
        );

        // The CA key is a platform secret: the org's vault never lists it.
        let secrets: serde_json::Value = server
            .get("/api/v1/secrets")
            .add_header("Authorization", format!("Bearer {alice}"))
            .add_header("X-Org-Id", org.to_string())
            .await
            .json();
        assert!(!secrets.to_string().contains("pnex-device-ca"));
    })
    .await;
}

/// L4: the certificate seen by the edge (URL-encoded PEM in `X-Client-Cert`)
/// is trusted only with the edge secret, and only for its own device.
#[tokio::test]
#[serial]
async fn edge_forwarded_certificate_must_belong_to_the_device() {
    use axum::http::{HeaderMap, HeaderValue};
    use pnex_backend::controllers::ws_ingest::client_cert_matches;
    use pnex_backend::services::settings::IngestSettings;

    with_app(|server, ctx, alice| async move {
        let ring = Keyring::from_config(&ctx.config).unwrap();
        let (org, pk) = create_device(&server, &alice, "pki-edge-1").await;
        let (_, other_pk) = create_device(&server, &alice, "pki-edge-2").await;
        let issued = issue_device_cert(&ctx.db, &ring, org, pk, "pki-edge-1")
            .await
            .unwrap();
        let settings = IngestSettings {
            require_client_cert: true,
            edge_secret: Some("edge-secret-0123456789".into()),
            ..IngestSettings::default()
        };
        // nginx $ssl_client_escaped_cert: URL-encoded PEM.
        let escaped: String = percent_encoding::utf8_percent_encode(
            &issued.cert_pem,
            percent_encoding::NON_ALPHANUMERIC,
        )
        .to_string();
        let mut headers = HeaderMap::new();
        headers.insert("x-client-cert", HeaderValue::from_str(&escaped).unwrap());
        // Without the edge secret the header is ignored (direct access).
        assert!(!client_cert_matches(&ctx.db, &headers, &settings, pk)
            .await
            .unwrap());
        headers.insert(
            "x-pnex-edge",
            HeaderValue::from_static("edge-secret-0123456789"),
        );
        assert!(client_cert_matches(&ctx.db, &headers, &settings, pk)
            .await
            .unwrap());
        // Another device's link with this certificate: refused.
        assert!(!client_cert_matches(&ctx.db, &headers, &settings, other_pk)
            .await
            .unwrap());
        // No certificate on the link: refused.
        headers.remove("x-client-cert");
        assert!(!client_cert_matches(&ctx.db, &headers, &settings, pk)
            .await
            .unwrap());
        // Requirement off (test config): always accepted.
        let off = IngestSettings {
            require_client_cert: false,
            ..IngestSettings::default()
        };
        assert!(client_cert_matches(&ctx.db, &HeaderMap::new(), &off, pk)
            .await
            .unwrap());
    })
    .await;
}
