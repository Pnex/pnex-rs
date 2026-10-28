//! Rauthy configuration read from `settings` of the Loco config.

use loco_rs::config::Config;
use serde::Deserialize;

#[derive(Clone, Debug, Deserialize)]
pub struct RauthySettings {
    /// Rauthy base URL for server-to-server calls (token, JWKS)
    /// (e.g. `http://localhost:8080`, `https://iam.pnex.io`).
    pub base_url: String,
    /// OIDC issuer seen by browsers (optional, default = `base_url`).
    ///
    /// Differs from `base_url` when Rauthy publishes self-signed HTTPS for
    /// devices: the SSO 302 and the `iss` validation must point to the
    /// **https** URL (secure context, `crypto.subtle` required by the
    /// Rauthy login page), while token/JWKS are still called over local
    /// HTTP — a self-signed certificate would make the backend's reqwest
    /// connection fail.
    pub issuer_url: Option<String>,
    pub client_id: String,
}

impl RauthySettings {
    /// Extracts the `settings.rauthy` section from the Loco config.
    pub fn from_config(config: &Config) -> loco_rs::Result<Self> {
        let value = config
            .settings
            .as_ref()
            .and_then(|s| s.get("rauthy"))
            .ok_or_else(|| {
                loco_rs::Error::Message(
                    "config settings.rauthy manquante (base_url, client_id)".into(),
                )
            })?;
        serde_json::from_value(value.clone())
            .map_err(|err| loco_rs::Error::Message(format!("settings.rauthy invalide : {err}")))
    }

    /// Rauthy OIDC issuer **seen by browsers**: `{issuer}/auth/v1/` —
    /// the trailing slash is part of the issuer emitted by Rauthy (`iss` claim);
    /// `jsonwebtoken` validation is an exact match, omitting it would break
    /// ALL token validation. `issuer_url` (optional) takes precedence over
    /// `base_url` when Rauthy publishes self-signed HTTPS for devices.
    pub fn issuer(&self) -> String {
        let base = self.issuer_url.as_deref().unwrap_or(&self.base_url);
        format!("{}/auth/v1/", base.trim_end_matches('/'))
    }

    /// "API" issuer for server-to-server calls (token, JWKS) —
    /// always derived from `base_url` (local HTTP, no self-signed cert).
    fn api_issuer(&self) -> String {
        format!("{}/auth/v1/", self.base_url.trim_end_matches('/'))
    }

    pub fn jwks_url(&self) -> String {
        format!("{}oidc/certs", self.api_issuer())
    }

    pub fn token_endpoint(&self) -> String {
        format!("{}oidc/token", self.api_issuer())
    }

    pub fn authorize_endpoint(&self) -> String {
        format!("{}oidc/authorize", self.issuer())
    }

    /// Logout endpoint (OIDC RP-initiated logout) — destroys the browser SSO
    /// session, otherwise the next login re-authenticates without a form.
    pub fn end_session_endpoint(&self) -> String {
        format!("{}oidc/logout", self.issuer())
    }

    /// Rauthy sign-up UI page ("Register") — this is not an OIDC
    /// endpoint: no OAuth2 params, activation happens by mail.
    pub fn register_page(&self) -> String {
        format!("{}/users/register", self.issuer().trim_end_matches('/'))
    }

    /// Rauthy account UI page — password change lives in the IdP
    /// (equivalent of Keycloak's `kc_action=UPDATE_PASSWORD`).
    pub fn account_page(&self) -> String {
        format!("{}/account", self.issuer().trim_end_matches('/'))
    }
}
