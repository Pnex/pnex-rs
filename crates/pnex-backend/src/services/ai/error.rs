//! Error taxonomy of the `ai` module: every variant maps to an HTTP status
//! and a machine code (`pnex_core::err_codes::AI_*`) with a canonical
//! English description; the UI renders `err-<code>` in the user's language
//! (verbatim description as fallback).

use loco_rs::prelude::*;
use pnex_core::err_codes;

/// Errors of the AI assistant module.
#[derive(Debug, thiserror::Error)]
pub enum AiError {
    /// Kill-switch `settings.ai.enabled = false`: the assistant is off.
    #[error("The assistant is disabled on this server.")]
    Disabled,
    /// The org has no default LLM provider (D119: no platform fallback).
    #[error("The assistant is not configured for this organization.")]
    NotConfigured,
    /// API key refused by the provider (401/403).
    #[error(
        "The LLM provider refused the API key: check the provider in the organization detail."
    )]
    AuthRejected(String),
    /// Provider rate limiting (429); retry-after in seconds when given.
    #[error("The LLM provider is rate-limiting requests: try again in a moment.")]
    RateLimited(Option<u64>),
    /// Any other HTTP failure of the provider (4xx/5xx, Anthropic 529 included).
    #[error("The LLM provider answered an error (HTTP {0}).")]
    Upstream(u16, String),
    /// The provider did not answer within the deadline (90 s).
    #[error("The LLM provider did not answer in time.")]
    Timeout,
    /// Transport error (DNS, connection, TLS).
    #[error("The LLM provider cannot be reached: {0}")]
    Network(String),
    /// Provider answer that cannot be decoded.
    #[error("The LLM provider's answer could not be read.")]
    Deserialize(String),
}

impl AiError {
    /// Canonical English description (verbatim fallback of the UI).
    pub fn user_message(&self) -> String {
        self.to_string()
    }

    /// Machine code resolved by the UI as `err-<code>`.
    pub fn code(&self) -> &'static str {
        match self {
            AiError::Disabled => err_codes::AI_DISABLED,
            AiError::NotConfigured => err_codes::AI_NOT_CONFIGURED,
            AiError::AuthRejected(_) => err_codes::AI_AUTH_REJECTED,
            AiError::RateLimited(_) => err_codes::AI_RATE_LIMITED,
            AiError::Upstream(_, _) => err_codes::AI_UPSTREAM,
            AiError::Timeout => err_codes::AI_TIMEOUT,
            AiError::Network(_) => err_codes::AI_NETWORK,
            AiError::Deserialize(_) => err_codes::AI_BAD_RESPONSE,
        }
    }

    /// Interpolation data of [`Self::code`] (values as strings).
    pub fn args(&self) -> Option<serde_json::Value> {
        match self {
            AiError::Upstream(status, _) => {
                Some(serde_json::json!({ "status": status.to_string() }))
            }
            AiError::Network(detail) => Some(serde_json::json!({ "detail": detail })),
            _ => None,
        }
    }

    fn status(&self) -> axum::http::StatusCode {
        use axum::http::StatusCode;
        match self {
            AiError::Disabled => StatusCode::FORBIDDEN,
            AiError::NotConfigured => StatusCode::BAD_REQUEST,
            AiError::RateLimited(_) => StatusCode::TOO_MANY_REQUESTS,
            AiError::AuthRejected(_)
            | AiError::Upstream(_, _)
            | AiError::Timeout
            | AiError::Network(_)
            | AiError::Deserialize(_) => StatusCode::BAD_GATEWAY,
        }
    }
}

/// Mapping to the Loco error (HTTP status + machine code + args).
impl From<AiError> for Error {
    fn from(e: AiError) -> Self {
        Error::CustomError(
            e.status(),
            loco_rs::controller::ErrorDetail {
                error: Some(e.code().to_string()),
                description: Some(e.user_message()),
                errors: e.args().map(|args| serde_json::json!({ "args": args })),
            },
        )
    }
}
