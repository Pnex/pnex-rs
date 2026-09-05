//! Erreurs API — le message du serveur est renvoyé **tel quel** au client
//! (convention projet : pas de traduction des messages d'erreur).

/// Erreur remontée aux composants : message affichable extrait du corps
/// (detail > message > bloc error/description), renvoyé tel quel.
#[derive(Debug, Clone)]
pub struct ApiError {
    pub message: String,
    /// Code HTTP (absent pour les erreurs locales : réseau, corps illisible…)
    /// — permet aux appelants de distinguer un 409 d'un 400 sans parser le
    /// message.
    pub status: Option<u16>,
    /// Corps d'erreur décodé quand c'est du JSON (409 conflit, 400
    /// `{"violations": […]}` des flows…).
    pub body: Option<serde_json::Value>,
}

impl ApiError {
    /// Erreur locale (réseau, corps illisible, réponse vide) : pas de
    /// statut ni de corps serveur.
    pub fn new(message: impl Into<String>) -> Self {
        Self { message: message.into(), status: None, body: None }
    }

    /// Erreur HTTP : message extrait du corps + statut et corps décodé
    /// conservés pour les appelants qui doivent réagir au code (409…).
    pub fn http(status: u16, body_text: &str) -> Self {
        let body = serde_json::from_str::<serde_json::Value>(body_text).ok();
        Self { message: extract_message(status, body_text), status: Some(status), body }
    }

    pub fn network(err: &reqwest::Error) -> Self {
        Self::new(format!("réseau : {err}"))
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

/// Extrait le message d'un corps d'erreur JSON, ordre : `detail` (DRF/loco) >
/// `message` > bloc `error`/`error_description`/`description` (IdP Rauthy,
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
    // Corps DRF à champ unique (`{"name":"This field is required."}`) :
    // premier champ à valeur chaîne.
    if let Some(field) = value.as_object().and_then(|obj| {
        obj.iter().find_map(|(_, v)| v.as_str().map(str::to_string))
    }) {
        return field;
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
    fn bloc_erreur_idp() {
        let body = r#"{"error":"invalid_grant","error_description":"Code expired"}"#;
        assert_eq!(extract_message(400, body), "invalid_grant : Code expired");
    }

    #[test]
    fn error_detail_loco() {
        let body = r#"{"error":"forbidden","description":"action réservée aux owners"}"#;
        assert_eq!(
            extract_message(403, body),
            "forbidden : action réservée aux owners"
        );
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

    #[test]
    fn champ_drf_champ_unique() {
        assert_eq!(
            extract_message(400, r#"{"name":"This field is required."}"#),
            "This field is required."
        );
    }

    #[test]
    fn http_conserve_statut_et_corps() {
        let err = ApiError::http(409, r#"{"error":"conflict","description":"version périmée"}"#);
        assert_eq!(err.status, Some(409));
        assert_eq!(err.message, "conflict : version périmée");
        assert_eq!(
            err.body.as_ref().and_then(|b| b.get("error").and_then(|v| v.as_str())),
            Some("conflict")
        );
        // Corps non JSON : message de repli, pas de corps décodé.
        let err = ApiError::http(502, "bad gateway");
        assert_eq!(err.status, Some(502));
        assert_eq!(err.message, "HTTP 502");
        assert!(err.body.is_none());
    }
}
