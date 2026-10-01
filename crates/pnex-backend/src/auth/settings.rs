//! Configuration Rauthy lue depuis `settings` de la config Loco.

use loco_rs::config::Config;
use serde::Deserialize;

#[derive(Clone, Debug, Deserialize)]
pub struct RauthySettings {
    /// URL de base Rauthy pour les appels serveur-à-serveur (token, JWKS)
    /// (ex. `http://localhost:8080`, `https://iam.pnex.io`).
    pub base_url: String,
    /// Issuer OIDC vu par les navigateurs (optionnel, défaut = `base_url`).
    ///
    /// Différent de `base_url` quand Rauthy publie du HTTPS auto-signé pour
    /// les devices : la 302 SSO et la validation `iss` doivent pointer vers
    /// l'URL **https** (contexte sécurisé, `crypto.subtle` requis par la
    /// page login Rauthy), tandis que token/JWKS restent appelés en HTTP
    /// local — un certificat auto-signé ferait rejeter la connexion reqwest
    /// du backend.
    pub issuer_url: Option<String>,
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

    /// Issuer OIDC Rauthy **vu par les navigateurs** : `{issuer}/auth/v1/` —
    /// le slash final fait partie de l'issuer émis par Rauthy (claim `iss`) ;
    /// la validation `jsonwebtoken` est un match exact, l'omettre casserait
    /// TOUTE validation de token. `issuer_url` (optionnel) prime sur
    /// `base_url` quand Rauthy publie du HTTPS auto-signé pour les devices.
    pub fn issuer(&self) -> String {
        let base = self.issuer_url.as_deref().unwrap_or(&self.base_url);
        format!("{}/auth/v1/", base.trim_end_matches('/'))
    }

    /// Issuer « API » pour les appels serveur-à-serveur (token, JWKS) —
    /// toujours dérivé de `base_url` (HTTP local, pas de cert auto-signé).
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

    /// Endpoint de déconnexion (RP-initiated logout OIDC) — détruit la session
    /// SSO navigateur, sinon le login suivant ré-authentifie sans formulaire.
    pub fn end_session_endpoint(&self) -> String {
        format!("{}oidc/logout", self.issuer())
    }

    /// Page UI d'inscription Rauthy (« Register ») — ce n'est pas un endpoint
    /// OIDC : pas de params OAuth2, l'activation se fait par mail.
    pub fn register_page(&self) -> String {
        format!("{}/users/register", self.issuer().trim_end_matches('/'))
    }

    /// Page UI compte Rauthy — le changement de mot de passe vit dans l'IdP
    /// (équivalent du `kc_action=UPDATE_PASSWORD` Keycloak).
    pub fn account_page(&self) -> String {
        format!("{}/account", self.issuer().trim_end_matches('/'))
    }
}
