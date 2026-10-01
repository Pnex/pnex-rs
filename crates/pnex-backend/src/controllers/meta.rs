//! Endpoint meta PUBLIC — version du serveur + version du contrat d'API.
//!
//! Non authentifié : l'app doit pouvoir valider la compatibilité avant tout
//! login (porte de compatibilité du front au boot), et le scan LAN s'en sert
//! pour identifier un serveur PNEX sur le réseau local. Sans CORS : le front
//! web est servi same-origin par ce serveur, et l'APK fait des sockets natifs
//! (hyper) hors WebView — cf. `docs/architecture/features.md`.

use axum::http::StatusCode;
use axum::routing::get;
use loco_rs::prelude::*;
use pnex_api_contract::{ServerInfo, CONTRACT, SERVICE};

use crate::app;

#[debug_handler]
async fn version() -> Result<Response> {
    format::json(ServerInfo {
        service: SERVICE.to_string(),
        version: app::app_version(),
        contract: CONTRACT,
    })
}

/// `GET /api/v1/meta/ca` — the local root CA of the TLS edge (D70), public
/// like `/version`: a client must fetch it BEFORE it can trust the server.
/// Served as `.crt` so Android/iOS offer the certificate installer.
#[debug_handler]
async fn ca() -> Result<Response> {
    // Plain 404 (not `Error::NotFound`): no local CA is the normal state in
    // cloud mode, it must not be logged as a controller error.
    let Some(pem) = local_ca_pem() else {
        return Ok(StatusCode::NOT_FOUND.into_response());
    };
    Ok((
        StatusCode::OK,
        [
            ("content-type", "application/x-x509-ca-cert".to_string()),
            (
                "content-disposition",
                "attachment; filename=\"pnex-ca.crt\"".to_string(),
            ),
        ],
        pem,
    )
        .into_response())
}

/// Root CA written by the edge's pki-init (`PNEX_CA_CERT_FILE`, loaded from
/// `edge.env`). `None` in cloud mode (that file then holds ISRG Root X1,
/// already trusted everywhere) or when the file is missing or not a PEM
/// certificate.
pub(crate) fn local_ca_pem() -> Option<String> {
    if std::env::var("PNEX_EDGE_MODE").is_ok_and(|mode| mode == "cloud") {
        return None;
    }
    let path = std::env::var("PNEX_CA_CERT_FILE").ok()?;
    let pem = std::fs::read_to_string(path).ok()?;
    pem.contains("-----BEGIN CERTIFICATE-----").then_some(pem)
}

pub fn routes() -> Routes {
    Routes::new()
        .prefix("/api/v1/meta")
        .add("/version", get(version))
        .add("/ca", get(ca))
}
