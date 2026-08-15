//! Validation locale des JWT Keycloak via JWKS (RS256), sans introspection —
//! parité Django, **avec les durcissements** décidés en Phase 3 (rapport
//! Phase 0 §3.4-3.5) : vérification explicite de l'`iss`, audience restreinte
//! à `{client_id, "account"}`, algorithme RS256 uniquement.
//!
//! Les JWKS sont mises en cache en mémoire et rafraîchies quand un `kid`
//! inconnu apparaît (rotation de clés Keycloak) — Django cachait 1 h sans
//! rafraîchissement sur `kid` inconnu.

use std::collections::HashMap;
use std::sync::Arc;

use jsonwebtoken::{DecodingKey, Validation, decode};
use serde::Deserialize;
use tokio::sync::{Mutex, RwLock};

use super::claims::Claims;
use super::settings::KeycloakSettings;

#[derive(Debug, thiserror::Error)]
pub enum VerifyError {
    #[error("token absent ou malformé")]
    Malformed,
    #[error("signature invalide ou algorithme non autorisé")]
    BadSignature,
    #[error("clé de signature inconnue (kid) après rafraîchissement JWKS")]
    UnknownKid,
    #[error("token expiré")]
    Expired,
    #[error("émetteur (iss) invalide")]
    BadIssuer,
    #[error("audience (aud) invalide")]
    BadAudience,
    #[error("Keycloak injoignable pour les JWKS : {0}")]
    JwksUnreachable(String),
}

#[derive(Deserialize)]
struct Jwks {
    keys: Vec<Jwk>,
}

#[derive(Deserialize)]
struct Jwk {
    kid: String,
    kty: String,
    #[serde(default)]
    alg: Option<String>,
    n: String,
    e: String,
}

pub struct JwksVerifier {
    issuer: String,
    /// Audience acceptée : le client PNEX + "account" (mapper par défaut
    /// Keycloak). Un token émis pour un autre client du realm est rejeté.
    audiences: Vec<String>,
    jwks_url: String,
    http: reqwest::Client,
    keys: RwLock<HashMap<String, Arc<DecodingKey>>>,
}

impl JwksVerifier {
    pub fn new(settings: &KeycloakSettings) -> Self {
        Self {
            issuer: settings.issuer(),
            audiences: vec![settings.client_id.clone(), "account".into()],
            jwks_url: settings.jwks_url(),
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(5))
                .build()
                .expect("reqwest client"),
            keys: RwLock::new(HashMap::new()),
        }
    }

    /// Valide un access token : signature RS256 (JWKS), `iss`, `aud`, `exp`.
    pub async fn verify(&self, token: &str) -> Result<Claims, VerifyError> {
        let header = jsonwebtoken::decode_header(token).map_err(|_| VerifyError::Malformed)?;
        let kid = header.kid.ok_or(VerifyError::Malformed)?;

        // NB : borner le guard AVANT le match — sinon il vit pendant tout le
        // match et `refresh()` (write) deadlock sur le même RwLock.
        let cached = self.keys.read().await.get(&kid).cloned();
        let key = match cached {
            Some(key) => Some(key),
            None => {
                // Rotation de clés : on rafraîchit une fois puis on retente.
                self.refresh().await?;
                self.keys.read().await.get(&kid).cloned()
            }
        };
        let key = key.ok_or(VerifyError::UnknownKid)?;

        let mut validation = Validation::new(jsonwebtoken::Algorithm::RS256);
        validation.set_issuer(&[&self.issuer]);
        validation.set_audience(&self.audiences);
        validation.validate_exp = true;

        let data = decode::<Claims>(token, &key, &validation).map_err(|err| match err.kind() {
            jsonwebtoken::errors::ErrorKind::ExpiredSignature => VerifyError::Expired,
            jsonwebtoken::errors::ErrorKind::InvalidIssuer => VerifyError::BadIssuer,
            jsonwebtoken::errors::ErrorKind::InvalidAudience => VerifyError::BadAudience,
            _ => VerifyError::BadSignature,
        })?;
        Ok(data.claims)
    }

    async fn refresh(&self) -> Result<(), VerifyError> {
        let jwks: Jwks = self
            .http
            .get(&self.jwks_url)
            .send()
            .await
            .and_then(|resp| resp.error_for_status())
            .map_err(|err| VerifyError::JwksUnreachable(err.to_string()))?
            .json()
            .await
            .map_err(|err| VerifyError::JwksUnreachable(err.to_string()))?;

        let mut keys = self.keys.write().await;
        keys.clear();
        for jwk in jwks.keys {
            if jwk.kty != "RSA" {
                continue;
            }
            if let Some(alg) = &jwk.alg {
                if alg != "RS256" {
                    continue;
                }
            }
            if let Ok(key) = DecodingKey::from_rsa_components(&jwk.n, &jwk.e) {
                keys.insert(jwk.kid, Arc::new(key));
            }
        }
        Ok(())
    }
}

/// Registre process-global : un vérifieur par issuer (dev/test/prod peuvent
/// pointer vers des realms différents dans le même process de test).
static VERIFIERS: std::sync::OnceLock<Mutex<HashMap<String, Arc<JwksVerifier>>>> =
    std::sync::OnceLock::new();

pub async fn verifier_for(settings: &KeycloakSettings) -> Arc<JwksVerifier> {
    let registry = VERIFIERS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut guard = registry.lock().await;
    guard
        .entry(settings.issuer())
        .or_insert_with(|| Arc::new(JwksVerifier::new(settings)))
        .clone()
}
