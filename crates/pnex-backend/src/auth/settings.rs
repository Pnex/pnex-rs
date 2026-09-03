//! Configuration Rauthy lue depuis `settings` de la config Loco.

use loco_rs::config::Config;
use serde::Deserialize;

#[derive(Clone, Debug, Deserialize)]
pub struct RauthySettings {
    /// URL de base Rauthy (ex. `http://localhost:8080`, `https://iam.pnex.io`).
    pub base_url: String,
    pub client_id: String,
}

impl RauthySettings {
    /// Extrait la section `settings.rauthy` de la config Loco.
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

    /// Issuer OIDC Rauthy : `{base}/auth/v1/` — le slash final fait partie de
    /// l'issuer émis par Rauthy (claim `iss`) ; la validation `jsonwebtoken`
    /// est un match exact, l'omettre casserait TOUTE validation de token.
    pub fn issuer(&self) -> String {
        format!("{}/auth/v1/", self.base_url.trim_end_matches('/'))
    }

    pub fn jwks_url(&self) -> String {
        format!("{}oidc/certs", self.issuer())
    }

    pub fn token_endpoint(&self) -> String {
        format!("{}oidc/token", self.issuer())
    }

    pub fn authorize_endpoint(&self) -> String {
        format!("{}oidc/authorize", self.issuer())
    }

    /// Endpoint de déconnexion (RP-initiated logout OIDC) — détruit la session
    /// SSO navigateur, sinon le login suivant ré-authentifie sans formulaire.
    pub fn end_session_endpoint(&self) -> String {
        format!("{}oidc/logout", self.issuer())
    }

    /// Page UI d'inscription Rauthy (« Register ») — ce n'est pas un endpoint
    /// OIDC : pas de params OAuth2, l'activation se fait par mail.
    pub fn register_page(&self) -> String {
        format!(
            "{}/auth/v1/users/register",
            self.base_url.trim_end_matches('/')
        )
    }

    /// Page UI compte Rauthy — le changement de mot de passe vit dans l'IdP
    /// (équivalent du `kc_action=UPDATE_PASSWORD` Keycloak).
    pub fn account_page(&self) -> String {
        format!("{}/auth/v1/account", self.base_url.trim_end_matches('/'))
    }
}
