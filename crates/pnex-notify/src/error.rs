//! Erreurs d'envoi / rendu — [`NotifyError::retryable`] pilote la politique
//! de retry (D53 : at-least-once, backoff 1s/5s/25s).
//!
//! Règle D54 : aucun message d'erreur ne cite la config du canal ni un
//! secret (l'URL est loggable, le token jamais).

#[derive(Debug, thiserror::Error)]
pub enum NotifyError {
    #[error("invalid channel config: {0}")]
    Config(String),
    #[error("template render: {0}")]
    Render(String),
    #[error("output too large: {0} bytes (64 KiB limit)")]
    TooLarge(usize),
    #[error("HTTP {status}")]
    Http { status: u16 },
    #[error("network: {0}")]
    Network(String),
}

impl NotifyError {
    /// `true` pour les échecs transitoires (réseau, timeout, 5xx, 429) —
    /// seuls ceux-là méritent un retry ; un 4xx reproduira la même erreur.
    pub fn retryable(&self) -> bool {
        match self {
            NotifyError::Network(_) => true,
            NotifyError::Http { status } => *status >= 500 || *status == 429,
            NotifyError::Config(_) | NotifyError::Render(_) | NotifyError::TooLarge(_) => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retryable_couvre_transitoire_seulement() {
        assert!(NotifyError::Network("timeout".into()).retryable());
        assert!(NotifyError::Http { status: 500 }.retryable());
        assert!(NotifyError::Http { status: 429 }.retryable());
        assert!(!NotifyError::Http { status: 400 }.retryable());
        assert!(!NotifyError::Http { status: 404 }.retryable());
        assert!(!NotifyError::Config("missing url".into()).retryable());
        assert!(!NotifyError::Render("syntax".into()).retryable());
        assert!(!NotifyError::TooLarge(70_000).retryable());
    }
}
