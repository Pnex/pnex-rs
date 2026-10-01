//! Canal `slack` — incoming webhook : POST `{"text": …}`.
//!
//! D65 : l'URL du webhook **est** le secret (token dans le chemin) —
//! classée champ `secret` write-only, jamais loggée. La réponse de succès
//! est du texte plaintext (`ok`), jamais du JSON — on ne teste que le
//! statut. Pas de garde de taille (D63) : la borne réelle (~40 k) est
//! loin au-delà des 64 KiB du rendu, le 400 du serveur suffit.

use async_trait::async_trait;
use serde_json::json;

use crate::channels::{compose_text, field, str_field, CLIENT};
use crate::error::NotifyError;
use crate::{Channel, Message};

pub struct SlackChannel;

#[async_trait]
impl Channel for SlackChannel {
    fn kind(&self) -> &'static str {
        "slack"
    }

    fn field_spec(&self) -> Vec<pnex_core::FieldSpec> {
        vec![field(
            "webhook_url",
            "notify-field-webhook-url",
            pnex_core::FieldType::Secret,
            true,
            None,
            None,
            &[],
        )]
    }

    fn validate(&self, config: &serde_json::Value) -> Result<(), String> {
        let url = str_field(config, "webhook_url")
            .ok_or_else(|| "the Slack webhook URL is required".to_string())?;
        if !url.starts_with("https://hooks.slack.com/") {
            return Err(
                "the URL must start with https://hooks.slack.com/ (incoming webhook)".into(),
            );
        }
        Ok(())
    }

    async fn send(&self, config: &serde_json::Value, msg: &Message) -> Result<(), NotifyError> {
        let url = str_field(config, "webhook_url")
            .ok_or_else(|| NotifyError::Config("missing webhook_url".into()))?;
        let resp = CLIENT
            .post(url)
            .json(&json!({"text": compose_text(msg.subject.as_deref(), &msg.body)}))
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
    use axum::http::StatusCode;
    use axum::routing::post;
    use axum::Json;
    use serde_json::Value;
    use std::sync::Arc;

    #[tokio::test]
    async fn poste_text_compose_et_tolere_reponse_plaintext() {
        let seen = Arc::new(tokio::sync::Mutex::new(None::<Value>));
        let seen_for_handler = seen.clone();
        let app = axum::Router::new().route(
            "/services/x/y/z",
            // Succès Slack = 200 body plaintext « ok » — pas du JSON.
            post(move |Json(body): Json<Value>| {
                let seen = seen_for_handler.clone();
                async move {
                    *seen.lock().await = Some(body);
                    axum::response::IntoResponse::into_response((StatusCode::OK, "ok"))
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let cfg = json!({"webhook_url": format!("http://{addr}/services/x/y/z")});
        SlackChannel
            .send(
                &cfg,
                &Message {
                    subject: Some("Job terminé".into()),
                    body: "stitch #8 ok".into(),
                    meta: json!({}),
                },
            )
            .await
            .unwrap();

        let body = seen.lock().await.take().unwrap();
        assert_eq!(body["text"], "Job terminé\nstitch #8 ok");
    }

    #[tokio::test]
    async fn http_404_non_retryable() {
        let app =
            axum::Router::new().route("/services/x/y/z", post(|| async { StatusCode::NOT_FOUND }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let cfg = json!({"webhook_url": format!("http://{addr}/services/x/y/z")});
        let e = SlackChannel
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
        assert!(matches!(e, NotifyError::Http { status: 404 }));
        assert!(!e.retryable());
    }

    #[test]
    fn validate_exige_url_hooks_slack() {
        assert!(SlackChannel
            .validate(&json!({"webhook_url": "https://hooks.slack.com/services/T/B/X"}))
            .is_ok());
        assert!(SlackChannel.validate(&json!({})).is_err());
        assert!(SlackChannel
            .validate(&json!({"webhook_url": "https://evil.example/hook"}))
            .is_err());
        assert!(SlackChannel
            .validate(&json!({"webhook_url": "http://hooks.slack.com/services/T/B/X"}))
            .is_err());
    }
}
