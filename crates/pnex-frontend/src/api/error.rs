//! Erreurs API — le message du serveur est renvoyé **tel quel** au client
//! (convention projet : pas de traduction des messages d'erreur).

/// Erreur remontée aux composants : message affichable extrait du corps
/// (detail > message > bloc error/description), renvoyé tel quel.
#[derive(Debug, Clone)]
pub struct ApiError {
    pub message: String,
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl ApiError {
    pub fn network(err: &reqwest::Error) -> Self {
        Self { message: format!("réseau : {err}") }
    }
}

/// Extrait le message d'un corps d'erreur JSON, ordre : `detail` (DRF/loco) >
/// `message` > bloc `error`/`error_description`/`description` (Keycloak,
/// ErrorDetail loco) > chaîne JSON nue > « HTTP n ».
pub fn extract_message(status: u16, body: &str) -> String {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(body) else {
        return format!("HTTP {status}");
    };
    fn as_str(v: &serde_json::Value) -> Option<String> {
        v.as_str().map(str::to_string)
    }

    if let Some(detail) = value.get("detail").and_then(as_str) {
        return detail;
    }
    if let Some(message) = value.get("message").and_then(as_str) {
        return message;
    }
    let code = value.get("error").and_then(as_str);
    let description = value
        .get("error_description")
        .and_then(as_str)
        .or_else(|| value.get("description").and_then(as_str));
    if let Some(code) = code {
        return match description {
            Some(desc) => format!("{code} : {desc}"),
            None => code,
        };
    }
    if let Some(description) = description {
        return description;
    }
    // Chaîne JSON nue (« "message" »).
    if let Some(text) = as_str(&value) {
        return text;
    }
    format!("HTTP {status}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detail_dabord() {
        let body = r#"{"detail":"vous n'êtes pas membre de cette organisation"}"#;
        assert_eq!(
            extract_message(403, body),
            "vous n'êtes pas membre de cette organisation"
        );
    }

    #[test]
    fn bloc_erreur_keycloak() {
        let body = r#"{"error":"invalid_grant","error_description":"Code expired"}"#;
        assert_eq!(extract_message(400, body), "invalid_grant : Code expired");
    }

    #[test]
    fn error_detail_loco() {
        let body = r#"{"error":"forbidden","description":"action réservée aux owners"}"#;
        assert_eq!(extract_message(403, body), "forbidden : action réservée aux owners");
    }

    #[test]
    fn code_sans_description() {
        assert_eq!(extract_message(400, r#"{"error":"upstream"}"#), "upstream");
    }

    #[test]
    fn chaine_nue_puis_fallback() {
        assert_eq!(extract_message(400, r#""oups""#), "oups");
        assert_eq!(extract_message(500, "pas du json"), "HTTP 500");
    }
}
