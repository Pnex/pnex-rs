//! GET /api/v1/meta/version — public (aucun header) + forme du corps.
//!
//! App ↔ server compatibility and the LAN scan rely on this endpoint before
//! any session.

mod common;

use loco_rs::testing::request::{RequestConfig, RequestConfigBuilder};
use pnex_api_contract::ServerInfo;
use serial_test::serial;

/// Route publique (sans `Authorization`) et corps conforme au contrat :
/// identité de service, version semver courante, contrat embarqué.
#[tokio::test]
#[serial]
async fn meta_version_est_public_et_conforme() {
    let guard = VarGuard::capture("RAUTHY_URL");
    let base = common::spawn_mock_rauthy().await;
    unsafe { std::env::set_var("RAUTHY_URL", &base) };

    let config: RequestConfig = RequestConfigBuilder::new().build();
    loco_rs::testing::request::request_with_config::<pnex_backend::app::App, _, _>(
        config,
        move |server, _ctx| async move {
            let response = server.get("/api/v1/meta/version").await;
            response.assert_status_ok();
            let info: ServerInfo = response.json();
            assert_eq!(info.service, pnex_api_contract::SERVICE);
            assert_eq!(info.contract, pnex_api_contract::CONTRACT);
            assert!(
                info.version.starts_with(env!("CARGO_PKG_VERSION")),
                "version inattendue : {}",
                info.version
            );
        },
    )
    .await;

    drop(guard);
}

/// Restores one env var on drop (panic included).
struct VarGuard(&'static str, Option<String>);

impl VarGuard {
    fn capture(name: &'static str) -> Self {
        Self(name, std::env::var(name).ok())
    }
}

impl Drop for VarGuard {
    fn drop(&mut self) {
        match self.1.take() {
            Some(v) => unsafe { std::env::set_var(self.0, v) },
            None => unsafe { std::env::remove_var(self.0) },
        }
    }
}

/// `GET /api/v1/meta/ca`: public download of the edge root CA in local mode,
/// 404 without a CA file, in cloud mode, or when the file is not a PEM.
#[tokio::test]
#[serial]
async fn meta_ca_serves_local_root_only() {
    let dir = tempfile::tempdir().expect("tmp");
    let pem = "-----BEGIN CERTIFICATE-----\nMIIB\n-----END CERTIFICATE-----\n";
    let ca_file = dir.path().join("device-ca.pem");
    std::fs::write(&ca_file, pem).expect("write ca");
    let junk_file = dir.path().join("junk.pem");
    std::fs::write(&junk_file, "not a certificate").expect("write junk");

    let guard = VarGuard::capture("RAUTHY_URL");
    let _file = VarGuard::capture("PNEX_CA_CERT_FILE");
    let _mode = VarGuard::capture("PNEX_EDGE_MODE");
    let base = common::spawn_mock_rauthy().await;
    unsafe { std::env::set_var("RAUTHY_URL", &base) };

    let config: RequestConfig = RequestConfigBuilder::new().build();
    loco_rs::testing::request::request_with_config::<pnex_backend::app::App, _, _>(
        config,
        move |server, _ctx| async move {
            unsafe { std::env::remove_var("PNEX_EDGE_MODE") };
            unsafe { std::env::remove_var("PNEX_CA_CERT_FILE") };
            server
                .get(pnex_api_contract::META_CA_PATH)
                .await
                .assert_status_not_found();

            unsafe { std::env::set_var("PNEX_CA_CERT_FILE", &ca_file) };
            let response = server.get(pnex_api_contract::META_CA_PATH).await;
            response.assert_status_ok();
            assert_eq!(
                response.header("content-type").to_str().unwrap(),
                "application/x-x509-ca-cert"
            );
            assert!(response
                .header("content-disposition")
                .to_str()
                .unwrap()
                .contains("pnex-ca.crt"));
            assert_eq!(response.text(), pem);

            unsafe { std::env::set_var("PNEX_EDGE_MODE", "cloud") };
            server
                .get(pnex_api_contract::META_CA_PATH)
                .await
                .assert_status_not_found();

            unsafe { std::env::set_var("PNEX_EDGE_MODE", "local") };
            unsafe { std::env::set_var("PNEX_CA_CERT_FILE", &junk_file) };
            server
                .get(pnex_api_contract::META_CA_PATH)
                .await
                .assert_status_not_found();
        },
    )
    .await;

    drop(guard);
}
