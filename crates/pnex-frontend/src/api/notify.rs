//! Endpoints notifications (D49–D54) — canaux (formulaires générés depuis
//! `field_spec`, secrets write-only), templates (preview), journal des
//! livraisons, tests d'envoi. Mêmes conventions que `api/flows.rs`.

use pnex_core::{
    NotifyChannel, NotifyDeliveryPage, NotifyKindInfo, NotifyTemplate, NotifyTemplateInput,
    Paginated, PreviewInput, PreviewResult, TemplateVar,
};

use crate::api::client;
use crate::api::error::ApiError;

/// `GET /api/v1/notify/kinds` — catalogue des canaux + field_spec
/// (source du formulaire dynamique, contrat `notify-kinds.v1`).
pub async fn kinds() -> Result<Vec<NotifyKindInfo>, ApiError> {
    client::request(reqwest::Method::GET, "/api/v1/notify/kinds", None).await
}

/// `GET /api/v1/notify/channels` — canaux de l'org (secrets masqués).
pub async fn list_channels() -> Result<Paginated<NotifyChannel>, ApiError> {
    client::request(reqwest::Method::GET, "/api/v1/notify/channels", None).await
}

/// `GET /api/v1/notify/channels/{id}`.
pub async fn get_channel(id: &str) -> Result<NotifyChannel, ApiError> {
    client::request(
        reqwest::Method::GET,
        &format!("/api/v1/notify/channels/{id}"),
        None,
    )
    .await
}

/// `POST /api/v1/notify/channels` — création (secrets write-only).
pub async fn create_channel(
    kind: &str,
    name: &str,
    enabled: bool,
    config: serde_json::Value,
) -> Result<NotifyChannel, ApiError> {
    client::request(
        reqwest::Method::POST,
        "/api/v1/notify/channels",
        Some(
            serde_json::json!({ "kind": kind, "name": name, "enabled": enabled, "config": config }),
        ),
    )
    .await
}

/// `PUT /api/v1/notify/channels/{id}` — secrets `null`/absents = inchangés.
pub async fn update_channel(
    id: &str,
    kind: &str,
    name: &str,
    enabled: bool,
    config: serde_json::Value,
) -> Result<NotifyChannel, ApiError> {
    client::request(
        reqwest::Method::PUT,
        &format!("/api/v1/notify/channels/{id}"),
        Some(
            serde_json::json!({ "kind": kind, "name": name, "enabled": enabled, "config": config }),
        ),
    )
    .await
}

/// `DELETE /api/v1/notify/channels/{id}` — 204.
pub async fn delete_channel(id: &str) -> Result<(), ApiError> {
    client::request(
        reqwest::Method::DELETE,
        &format!("/api/v1/notify/channels/{id}"),
        None,
    )
    .await
}

/// `POST /api/v1/notify/channels/test-draft` — test d'un brouillon avant
/// sauvegarde (réponse `{status: sent|failed, error?}`).
pub async fn test_draft(
    kind: &str,
    name: &str,
    config: serde_json::Value,
    channel_id: Option<&str>,
) -> Result<TestOutcome, ApiError> {
    client::request(
        reqwest::Method::POST,
        "/api/v1/notify/channels/test-draft",
        Some(serde_json::json!({
            "kind": kind,
            "name": name,
            "config": config,
            // Edited channel: its stored secrets fill the unchanged fields.
            "channel_id": channel_id,
        })),
    )
    .await
}

/// `POST /api/v1/notify/channels/{id}/test` — message built-in quand
/// `template_id` est `None`, template rendu sinon (les vars manquantes
/// prennent leur example déclaré côté serveur). Journalisé dans les deux
/// cas (source `test`, template_id porté au journal).
pub async fn test_channel(
    id: &str,
    template_id: Option<&str>,
    vars: std::collections::BTreeMap<String, String>,
) -> Result<TestOutcome, ApiError> {
    let body = template_id
        .map(|template_id| serde_json::json!({ "template_id": template_id, "vars": vars }));
    client::request(
        reqwest::Method::POST,
        &format!("/api/v1/notify/channels/{id}/test"),
        body,
    )
    .await
}

/// Réponse des endpoints de test.
#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
pub struct TestOutcome {
    pub status: String,
    #[serde(default)]
    pub error: Option<String>,
}

impl TestOutcome {
    pub fn ok(&self) -> bool {
        self.status == "sent"
    }
}

/// Classification des échecs de test : la config du canal est invalidée
/// (400 `{"config": …}`) ou c'est un autre problème (réseau du serveur…).
#[derive(Debug, Clone, PartialEq)]
pub enum TestError {
    /// Message sur la config (affiché sous le champ).
    Invalid(String),
    Other(String),
}

pub fn classify_test_error(err: &ApiError) -> TestError {
    match err.status {
        Some(400) => {
            let config = err
                .body
                .as_ref()
                .and_then(|b| b.get("config"))
                .and_then(|v| v.as_str())
                .map(str::to_owned);
            match config {
                Some(msg) => TestError::Invalid(msg),
                None => TestError::Other(err.message.clone()),
            }
        }
        _ => TestError::Other(err.message.clone()),
    }
}

/// Delivery journal filters (`GET /api/v1/notify/deliveries`, D86).
#[derive(Debug, Default, Clone, PartialEq)]
pub struct DeliveryFilters {
    pub channel_id: Option<String>,
    pub status: Option<String>,
    pub source: Option<String>,
    /// Full-text search.
    pub q: Option<String>,
    /// RFC 3339 bounds.
    pub from: Option<String>,
    pub to: Option<String>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

impl DeliveryFilters {
    pub(crate) fn to_query(&self) -> String {
        let mut pairs: Vec<String> = Vec::new();
        for (key, value) in [
            ("channel_id", &self.channel_id),
            ("status", &self.status),
            ("source", &self.source),
            ("q", &self.q),
            ("from", &self.from),
            ("to", &self.to),
        ] {
            if let Some(v) = value.as_deref().filter(|v| !v.trim().is_empty()) {
                pairs.push(format!("{key}={}", crate::api::media::urlencode(v.trim())));
            }
        }
        if let Some(limit) = self.limit {
            pairs.push(format!("limit={limit}"));
        }
        if let Some(offset) = self.offset {
            pairs.push(format!("offset={offset}"));
        }
        if pairs.is_empty() {
            String::new()
        } else {
            format!("?{}", pairs.join("&"))
        }
    }
}

/// `GET /api/v1/notify/deliveries` — every delivery attempt of the org.
pub async fn list_deliveries(filters: &DeliveryFilters) -> Result<NotifyDeliveryPage, ApiError> {
    client::request(
        reqwest::Method::GET,
        &format!("/api/v1/notify/deliveries{}", filters.to_query()),
        None,
    )
    .await
}

/// `GET /api/v1/notify/templates`.
pub async fn list_templates() -> Result<Paginated<NotifyTemplate>, ApiError> {
    client::request(reqwest::Method::GET, "/api/v1/notify/templates", None).await
}

/// `POST /api/v1/notify/templates`.
pub async fn create_template(input: NotifyTemplateInput) -> Result<NotifyTemplate, ApiError> {
    client::request(
        reqwest::Method::POST,
        "/api/v1/notify/templates",
        Some(serde_json::to_value(input).unwrap_or_default()),
    )
    .await
}

/// `PUT /api/v1/notify/templates/{id}`.
pub async fn update_template(
    id: &str,
    input: NotifyTemplateInput,
) -> Result<NotifyTemplate, ApiError> {
    client::request(
        reqwest::Method::PUT,
        &format!("/api/v1/notify/templates/{id}"),
        Some(serde_json::to_value(input).unwrap_or_default()),
    )
    .await
}

/// `DELETE /api/v1/notify/templates/{id}` — 204.
pub async fn delete_template(id: &str) -> Result<(), ApiError> {
    client::request(
        reqwest::Method::DELETE,
        &format!("/api/v1/notify/templates/{id}"),
        None,
    )
    .await
}

/// `POST /api/v1/notify/templates/{id}/preview` — rendu seul, jamais d'envoi.
pub async fn preview_template(
    id: &str,
    vars: std::collections::BTreeMap<String, String>,
    payload: serde_json::Value,
) -> Result<PreviewResult, ApiError> {
    client::request(
        reqwest::Method::POST,
        &format!("/api/v1/notify/templates/{id}/preview"),
        Some(serde_json::to_value(PreviewInput { vars, payload }).unwrap_or_default()),
    )
    .await
}

/// Convertit les lignes `{name, example}` de l'éditeur en `TemplateVar`
/// (trim ; lignes vides ignorées).
pub fn vars_from_rows(rows: Vec<(String, String)>) -> Vec<TemplateVar> {
    rows.into_iter()
        .filter(|(name, _)| !name.trim().is_empty())
        .map(|(name, example)| TemplateVar {
            name: name.trim().to_string(),
            example,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::error::ApiError;

    fn api_error(status: u16, body: serde_json::Value) -> ApiError {
        ApiError {
            status: Some(status),
            message: "msg".into(),
            body: Some(body),
            code: None,
            args: None,
        }
    }

    #[test]
    fn classify_400_config_vers_invalid() {
        let err = api_error(
            400,
            serde_json::json!({"config": "l'URL doit commencer par http://"}),
        );
        assert_eq!(
            classify_test_error(&err),
            TestError::Invalid("l'URL doit commencer par http://".into())
        );
    }

    #[test]
    fn classify_autres_vers_other() {
        let err = api_error(502, serde_json::json!({"status": "failed"}));
        assert!(matches!(classify_test_error(&err), TestError::Other(_)));
        let err = api_error(400, serde_json::json!({"name": "requis"}));
        assert!(matches!(classify_test_error(&err), TestError::Other(_)));
    }

    #[test]
    fn vars_from_rows_ignore_les_vides() {
        let vars = vars_from_rows(vec![
            ("seuil".into(), "80".into()),
            ("  ".into(), "x".into()),
            ("device".into(), "{{ msg.device }}".into()),
        ]);
        assert_eq!(vars.len(), 2);
        assert_eq!(vars[0].name, "seuil");
        assert_eq!(vars[1].example, "{{ msg.device }}");
    }
}
