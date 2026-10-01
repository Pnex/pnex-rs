//! API errors — the server message is kept as the canonical **display
//! fallback (English)**; when the body carries a registered machine code
//! (`pnex_core::err_codes::ALL`), the UI resolves the `err-<kebab>` key at
//! render time (see `api/error_i18n.rs`). Unknown codes → verbatim fallback.

/// Error surfaced to components: display message extracted from the body
/// (detail > message > error/description block), kept verbatim.
/// `PartialEq` required by the CRUD socle component props (dioxus 0.7
/// `#[component]` macro — per-field generated impl).
#[derive(Debug, Clone, PartialEq)]
pub struct ApiError {
    pub message: String,
    /// HTTP status (absent for local errors: network, unreadable body…)
    /// — lets callers tell a 409 from a 400 without parsing the message.
    pub status: Option<u16>,
    /// Decoded body when it is JSON (409 conflict, 400 `{"violations": […]}`
    /// of flows…).
    pub body: Option<serde_json::Value>,
    /// Machine code (`pnex_core::err_codes::ALL`) extracted from the server
    /// body (`"error"` field) or set locally (`ApiError::local`). `None` =
    /// verbatim display of `message`.
    pub code: Option<String>,
    /// Interpolation data (JSON object of strings) — the body's
    /// `"errors": {"args": {…}}` or set locally, consumed by the
    /// `err-<kebab>` resolution (see `api/error_i18n.rs`).
    pub args: Option<serde_json::Value>,
}

impl ApiError {
    /// Local error (network, unreadable body, empty response): no server
    /// status or body.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            status: None,
            body: None,
            code: None,
            args: None,
        }
    }

    /// Local error with a registered machine code (`err_codes::ALL`) — the
    /// English `fallback` is the non-i18n display (tests, missing key); the
    /// `err-<kebab>` key resolves at render time. Safe outside a render
    /// scope (async tasks): no i18n resolution happens here.
    pub fn local(
        code: &str,
        fallback_en: impl Into<String>,
        args: Option<serde_json::Value>,
    ) -> Self {
        Self {
            message: fallback_en.into(),
            status: None,
            body: None,
            code: Some(code.to_owned()),
            args,
        }
    }

    /// Erreur HTTP : message extrait du corps + statut et corps décodé
    /// conservés pour les appelants qui doivent réagir au code (409…).
    pub fn http(status: u16, body_text: &str) -> Self {
        let body = serde_json::from_str::<serde_json::Value>(body_text).ok();
        let code = body
            .as_ref()
            .and_then(|b| b.get("error"))
            .and_then(|v| v.as_str())
            .map(str::to_owned);
        let args = body
            .as_ref()
            .and_then(|b| b.get("errors"))
            .and_then(|errors| errors.get("args"))
            .cloned();
        Self {
            message: extract_message(status, body_text),
            status: Some(status),
            body,
            code,
            args,
        }
    }

    pub fn network(err: &reqwest::Error) -> Self {
        Self::local(
            pnex_core::err_codes::CLIENT_NETWORK,
            format!("network error: {err}"),
            None,
        )
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

/// Extracts the message from a JSON error body, order: `detail` (loco) >
/// `message` > `error`/`error_description`/`description` block (Rauthy IdP,
/// loco ErrorDetail) > bare JSON string > "HTTP n".
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
    // Single-field body (`{"name":"This field is required."}`):
    // first field with a string value.
    if let Some(field) = value
        .as_object()
        .and_then(|obj| obj.iter().find_map(|(_, v)| v.as_str().map(str::to_string)))
    {
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
    fn champ_single_field_unique() {
        assert_eq!(
            extract_message(400, r#"{"name":"This field is required."}"#),
            "This field is required."
        );
    }

    #[test]
    fn http_conserve_statut_et_corps() {
        let err = ApiError::http(
            409,
            r#"{"error":"conflict","description":"version périmée"}"#,
        );
        assert_eq!(err.status, Some(409));
        assert_eq!(err.message, "conflict : version périmée");
        assert_eq!(
            err.body
                .as_ref()
                .and_then(|b| b.get("error").and_then(|v| v.as_str())),
            Some("conflict")
        );
        // Corps non JSON : message de repli, pas de corps décodé.
        let err = ApiError::http(502, "bad gateway");
        assert_eq!(err.status, Some(502));
        assert_eq!(err.message, "HTTP 502");
        assert!(err.body.is_none());
    }

    #[test]
    fn http_extrait_code_et_args() {
        let err = ApiError::http(
            409,
            r#"{"error":"org-name-duplicate","description":"an organization already uses this name","errors":{"args":{"name":"demo"}}}"#,
        );
        assert_eq!(err.code.as_deref(), Some("org-name-duplicate"));
        assert_eq!(
            err.args
                .as_ref()
                .and_then(|a| a.get("name").and_then(|v| v.as_str())),
            Some("demo")
        );
        // Rauthy: "error" present but not a registered code → fallback.
        let err = ApiError::http(
            400,
            r#"{"error":"invalid_grant","error_description":"Code expired"}"#,
        );
        assert_eq!(err.code.as_deref(), Some("invalid_grant"));
        // Erreur locale sans corps : pas de code.
        assert_eq!(ApiError::new("boom").code, None);
    }

    #[test]
    fn local_pose_code_et_args() {
        let err = ApiError::local(
            pnex_core::err_codes::CLIENT_NETWORK,
            "network error: offline",
            None,
        );
        assert_eq!(err.code.as_deref(), Some("client-network"));
        assert_eq!(err.message, "network error: offline");
        assert!(err.status.is_none() && err.body.is_none());
    }
}
