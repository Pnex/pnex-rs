//! Canal `webhook` — POST JSON générique `{subject, body, meta}` (D51).
//! Couvre ntfy (POST JSON), Gotify, Telegram Bot API, domotique… sans
//! dépendance par service : la cible adapte ou le consommateur lit le JSON.
//!
//! D54 : `secret_value` est un champ secret write-only, transmis en header
//! (`secret_header`), jamais concaténé dans l'URL ni loggé.

use async_trait::async_trait;
use serde_json::json;

use crate::channels::{field, str_field, CLIENT};
use crate::error::NotifyError;
use crate::{Channel, Message};

pub struct WebhookChannel;

#[async_trait]
impl Channel for WebhookChannel {
    fn kind(&self) -> &'static str {
        "webhook"
    }

    fn field_spec(&self) -> Vec<pnex_core::FieldSpec> {
        vec![
            field(
                "url",
                "notify-field-url",
                pnex_core::FieldType::Text,
                true,
                None,
                None,
                &[],
            ),
            field(
                "secret_header",
                "notify-field-secret-header",
                pnex_core::FieldType::Text,
                false,
                None,
                None,
                &[],
            ),
            field(
                "secret_value",
                "notify-field-secret-value",
                pnex_core::FieldType::Secret,
                false,
                None,
                None,
                &[],
            ),
        ]
    }

    fn validate(&self, config: &serde_json::Value) -> Result<(), String> {
        let url =
            str_field(config, "url").ok_or_else(|| "the webhook URL is required".to_string())?;
        if !(url.starts_with("http://") || url.starts_with("https://")) {
            return Err("the URL must start with http:// or https://".into());
        }
        let header = str_field(config, "secret_header");
        let value = str_field(config, "secret_value");
        match (header, value) {
            (Some(_), Some(_)) | (None, None) => Ok(()),
            (None, Some(_)) => Err("secret_header is required to send secret_value".into()),
            (Some(_), None) => Ok(()), // header posé sans valeur : ignoré au send
        }
    }

    async fn send(&self, config: &serde_json::Value, msg: &Message) -> Result<(), NotifyError> {
        let url =
            str_field(config, "url").ok_or_else(|| NotifyError::Config("missing url".into()))?;
        let mut req = CLIENT.post(url).json(&json!({
            "subject": msg.subject,
            "body": msg.body,
            "meta": msg.meta,
        }));
        if let Some((header, value)) =
            str_field(config, "secret_header").zip(str_field(config, "secret_value"))
        {
            req = req.header(header, value);
        }
        let resp = req
            .send()
            .await
            .map_err(|e| NotifyError::Network(e.to_string()))?;
        let status = resp.status();
        if !status.is_success() {
            return Err(NotifyError::Http {
                status: status.as_u16(),
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::{HeaderMap, StatusCode};
    use axum::routing::post;
    use axum::Json;
    use serde_json::Value;
    use std::sync::Arc;

    #[tokio::test]
    async fn poste_subject_body_meta_et_header_secret() {
        let seen = Arc::new(tokio::sync::Mutex::new(None::<(HeaderMap, Value)>));
        let seen_for_handler = seen.clone();
        let app = axum::Router::new().route(
            "/hook",
            post(move |headers: HeaderMap, Json(body): Json<Value>| {
                let seen = seen_for_handler.clone();
                async move {
                    *seen.lock().await = Some((headers, body));
                    StatusCode::OK
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let cfg = json!({
            "url": format!("http://{addr}/hook"),
            "secret_header": "x-webhook-signature",
            "secret_value": "s3cr3t",
        });
        let msg = Message {
            subject: Some("Job terminé".into()),
            body: "stitch #8 ok".into(),
            meta: json!({"source": "test"}),
        };
        WebhookChannel.send(&cfg, &msg).await.unwrap();

        let (headers, body) = seen.lock().await.take().unwrap();
        assert_eq!(
            headers
                .get("x-webhook-signature")
                .and_then(|v| v.to_str().ok()),
            Some("s3cr3t")
        );
        assert_eq!(body["subject"], "Job terminé");
        assert_eq!(body["body"], "stitch #8 ok");
        assert_eq!(body["meta"]["source"], "test");
    }

    #[tokio::test]
    async fn http_500_erreur_retryable() {
        let app =
            axum::Router::new().route("/ko", post(|| async { StatusCode::INTERNAL_SERVER_ERROR }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let cfg = json!({"url": format!("http://{addr}/ko")});
        let e = WebhookChannel
            .send(
                &cfg,
                &Message {
                    subject: None,
                    body: "x".into(),
                    meta: json!({}),
                },
            )
            .await
            .unwrap_err();
        assert!(matches!(e, NotifyError::Http { status: 500 }));
        assert!(e.retryable());
    }

    #[test]
    fn validate_exige_url_http_et_paire_de_secrets() {
        assert!(WebhookChannel
            .validate(&json!({"url": "https://example.com/hook"}))
            .is_ok());
        assert!(WebhookChannel.validate(&json!({})).is_err());
        assert!(WebhookChannel
            .validate(&json!({"url": "ftp://example.com"}))
            .is_err());
        // secret_value sans secret_header : refusé (le secret n'a pas de
        // vecteur de transmission).
        assert!(WebhookChannel
            .validate(&json!({"url": "https://e.com", "secret_value": "x"}))
            .is_err());
        assert!(WebhookChannel
            .validate(&json!({"url": "https://e.com", "secret_header": "h", "secret_value": "x"}))
            .is_ok());
    }
}
