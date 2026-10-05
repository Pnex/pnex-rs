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
async fn version(State(ctx): State<AppContext>) -> Result<Response> {
    let issuer = crate::auth::settings::RauthySettings::from_config(&ctx.config)
        .ok()
        .and_then(|s| s.issuer_url);
    format::json(ServerInfo {
        service: SERVICE.to_string(),
        version: app::app_version(),
        contract: CONTRACT,
        origin: public_origin(issuer.as_deref()),
    })
}

/// Canonical origin announced to native apps: the browser-facing issuer
/// base (`RAUTHY_ISSUER_URL`, `https://<public host>` behind the edge),
/// reduced to `scheme://host[:port]`. HTTPS only — it is what OIDC redirect
/// URIs and the certificate are issued for. Already public (OIDC discovery).
fn public_origin(issuer_url: Option<&str>) -> Option<String> {
    let url = reqwest::Url::parse(issuer_url?.trim()).ok()?;
    if url.scheme() != "https" {
        return None;
    }
    let host = url.host_str()?;
    Some(match url.port() {
        Some(port) => format!("https://{host}:{port}"),
        None => format!("https://{host}"),
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

#[cfg(test)]
mod tests {
    use super::public_origin;

    #[test]
    fn public_origin_keeps_scheme_host_and_port_only() {
        assert_eq!(
            public_origin(Some("https://pnex.local")).as_deref(),
            Some("https://pnex.local")
        );
        assert_eq!(
            public_origin(Some("https://pnex.local/")).as_deref(),
            Some("https://pnex.local")
        );
        assert_eq!(
            public_origin(Some("https://192.168.1.185:8443")).as_deref(),
            Some("https://192.168.1.185:8443")
        );
    }

    #[test]
    fn public_origin_is_https_only() {
        assert_eq!(public_origin(None), None);
        assert_eq!(public_origin(Some("http://localhost:8080")), None);
        assert_eq!(public_origin(Some("not a url")), None);
    }
}
