//! Configuration Keycloak lue depuis `settings` de la config Loco.

use loco_rs::config::Config;
use serde::Deserialize;

#[derive(Clone, Debug, Deserialize)]
pub struct KeycloakSettings {
    /// URL de base Keycloak (ex. `http://localhost:8080`, `https://kc.pnex.io`).
    pub base_url: String,
    pub realm: String,
    pub client_id: String,
}

impl KeycloakSettings {
    /// Extrait la section `settings.keycloak` de la config Loco.
    pub fn from_config(config: &Config) -> loco_rs::Result<Self> {
        let value = config
            .settings
            .as_ref()
            .and_then(|s| s.get("keycloak"))
            .ok_or_else(|| {
                loco_rs::Error::Message(
                    "config settings.keycloak manquante (base_url, realm, client_id)".into(),
                )
            })?;
        serde_json::from_value(value.clone())
            .map_err(|err| loco_rs::Error::Message(format!("settings.keycloak invalide : {err}")))
    }

    pub fn issuer(&self) -> String {
        format!("{}/realms/{}", self.base_url.trim_end_matches('/'), self.realm)
    }

    pub fn jwks_url(&self) -> String {
        format!(
            "{}/protocol/openid-connect/certs",
            self.issuer()
        )
    }

    pub fn token_endpoint(&self) -> String {
        format!("{}/protocol/openid-connect/token", self.issuer())
    }

    pub fn authorize_endpoint(&self) -> String {
        format!("{}/protocol/openid-connect/auth", self.issuer())
    }

    /// Endpoint d'inscription (le « Register » de la page de login Keycloak).
    /// ≠ authorize + `kc_action=register`, ignoré silencieusement quand une
    /// session SSO existe déjà (l'utilisateur était re-logué au lieu de voir
    /// le formulaire d'inscription).
    pub fn registration_endpoint(&self) -> String {
        format!("{}/protocol/openid-connect/registrations", self.issuer())
    }

    /// Endpoint de déconnexion (RP-initiated logout OIDC) — détruit la session
    /// SSO navigateur, sinon le login suivant ré-authentifie sans formulaire.
    pub fn end_session_endpoint(&self) -> String {
        format!("{}/protocol/openid-connect/logout", self.issuer())
    }
}
