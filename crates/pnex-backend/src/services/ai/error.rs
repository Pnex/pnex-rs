//! Taxonomie d'erreurs du module `ai` — chaque variante mappe sur un code
//! HTTP + un message **actionnable** (FR) : l'utilisateur doit savoir quoi
//! faire (vérifier le connecteur, attendre), toasté tel quel par l'UI.

use loco_rs::prelude::*;

/// Erreurs du module assistant IA.
#[derive(Debug, thiserror::Error)]
pub enum AiError {
    /// Kill-switch `settings.ai.enabled = false` — l'assistant est coupé.
    #[error("L'IA est désactivée sur ce serveur.")]
    Disabled,
    /// No provider resolves (neither an org default nor a platform default).
    #[error("L'IA n'est pas configurée pour cette organisation.")]
    NotConfigured,
    /// Clé API refusée par le fournisseur (401/403).
    #[error("Clé API refusée par le fournisseur — vérifiez le connecteur dans les réglages de l'organisation.")]
    AuthRejected(String),
    /// Saturation fournisseur (429) — retry-after en secondes si fourni.
    #[error("Fournisseur saturé (réessayez dans {0:?} s).")]
    RateLimited(Option<u64>),
    /// Autre échec HTTP du fournisseur (4xx/5xx, 529 Anthropic inclus).
    #[error("Le fournisseur IA a répondu une erreur ({0}).")]
    Upstream(u16, String),
    /// Le fournisseur n'a pas répondu dans le délai (90 s).
    #[error("Le fournisseur IA n'a pas répondu à temps.")]
    Timeout,
    /// Erreur réseau (DNS, connexion, TLS).
    #[error("Impossible de joindre le fournisseur IA : {0}")]
    Network(String),
    /// Réponse du fournisseur non décodable.
    #[error("Réponse du fournisseur illisible.")]
    Deserialize(String),
    /// Échec d'un outil de l'agent (message vu par le modèle).
    #[error("Outil en échec : {0}")]
    Tool(String),
}

impl AiError {
    /// Message utilisateur (FR, actionnable) — toasté tel quel par l'UI.
    pub fn user_message(&self) -> String {
        self.to_string()
    }
}

/// Mapping vers l'erreur Loco (code HTTP + code machine).
impl From<AiError> for Error {
    fn from(e: AiError) -> Self {
        match e {
            AiError::Disabled => Error::CustomError(
                axum::http::StatusCode::FORBIDDEN,
                loco_rs::controller::ErrorDetail::new("ai_disabled", e.user_message()),
            ),
            AiError::NotConfigured => Error::CustomError(
                axum::http::StatusCode::BAD_REQUEST,
                loco_rs::controller::ErrorDetail::new("ai_not_configured", e.user_message()),
            ),
            AiError::AuthRejected(_) => Error::CustomError(
                axum::http::StatusCode::BAD_GATEWAY,
                loco_rs::controller::ErrorDetail::new("ai_auth", e.user_message()),
            ),
            AiError::RateLimited(_) => Error::CustomError(
                axum::http::StatusCode::TOO_MANY_REQUESTS,
                loco_rs::controller::ErrorDetail::new("ai_rate_limited", e.user_message()),
            ),
            AiError::Upstream(_, _) | AiError::Timeout | AiError::Network(_) => Error::CustomError(
                axum::http::StatusCode::BAD_GATEWAY,
                loco_rs::controller::ErrorDetail::new("ai_upstream", e.user_message()),
            ),
            AiError::Deserialize(_) => Error::CustomError(
                axum::http::StatusCode::BAD_GATEWAY,
                loco_rs::controller::ErrorDetail::new("ai_bad_response", e.user_message()),
            ),
            AiError::Tool(_) => Error::BadRequest(e.user_message()),
        }
    }
}
