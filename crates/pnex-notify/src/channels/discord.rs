//! Canal `discord` — webhook : POST JSON `{content, username?,
//! allowed_mentions}`. Succès = 204 No Content.
//!
//! D65 : l'URL du webhook **est** le secret (token dans le chemin) —
//! classée champ `secret` write-only, jamais loggée. `allowed_mentions`
//! vide neutralise les `@everyone` provenant d'un template. Le contenu
//! est borné à 2000 chars — garde fail-loud `TooLarge` (D63).

use async_trait::async_trait;
use serde_json::json;

use crate::channels::{compose_text, field, size_guard, str_field, CLIENT};
use crate::error::NotifyError;
use crate::{Channel, Message};

pub struct DiscordChannel;

/// Borne Discord webhook pour `content` (chars, pas octets).
const MAX_CHARS: usize = 2000;

#[async_trait]
impl Channel for DiscordChannel {
    fn kind(&self) -> &'static str {
        "discord"
    }

    fn field_spec(&self) -> Vec<pnex_core::FieldSpec> {
        vec![
            field(
                "webhook_url",
                "notify-field-webhook-url",
                pnex_core::FieldType::Secret,
                true,
                None,
                None,
                &[],
            ),
            field(
                "username",
                "notify-field-username",
                pnex_core::FieldType::Text,
                false,
                None,
                None,
                &[],
            ),
        ]
    }

    fn validate(&self, config: &serde_json::Value) -> Result<(), String> {
        let url = str_field(config, "webhook_url")
            .ok_or_else(|| "the Discord webhook URL is required".to_string())?;
        if !(url.starts_with("https://discord.com/api/webhooks/")
            || url.starts_with("https://discordapp.com/api/webhooks/"))
        {
            return Err(
                "the URL must be a Discord webhook (https://discord.com/api/webhooks/…)".into(),
            );
        }
        Ok(())
    }

    async fn send(&self, config: &serde_json::Value, msg: &Message) -> Result<(), NotifyError> {
        let url = str_field(config, "webhook_url")
            .ok_or_else(|| NotifyError::Config("missing webhook_url".into()))?;
        let content = compose_text(msg.subject.as_deref(), &msg.body);
        size_guard(&content, MAX_CHARS)?;
        let mut payload = json!({
            "content": content,
            "allowed_mentions": { "parse": [] },
        });
        if let Some(username) = str_field(config, "username") {
            payload["username"] = json!(username);
        }
        let resp = CLIENT
            .post(url)
            .json(&payload)
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
    async fn poste_content_username_et_allowed_mentions_vides() {
        let seen = Arc::new(tokio::sync::Mutex::new(None::<Value>));
        let seen_for_handler = seen.clone();
        let app = axum::Router::new().route(
            "/api/webhooks/1/tok",
            // Succès Discord = 204 No Content.
            post(move |Json(body): Json<Value>| {
                let seen = seen_for_handler.clone();
                async move {
                    *seen.lock().await = Some(body);
                    StatusCode::NO_CONTENT
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let cfg = json!({
            "webhook_url": format!("http://{addr}/api/webhooks/1/tok"),
            "username": "PNeX",
        });
        DiscordChannel
            .send(
                &cfg,
                &Message {
                    subject: Some("Alerte".into()),
                    body: "seuil dépassé".into(),
                    meta: json!({}),
                },
            )
            .await
            .unwrap();

        let body = seen.lock().await.take().unwrap();
        assert_eq!(body["content"], "Alerte\nseuil dépassé");
        assert_eq!(body["username"], "PNeX");
        assert_eq!(body["allowed_mentions"], json!({"parse": []}));
    }

    #[tokio::test]
    async fn contenu_trop_long_too_large_sans_appel_http() {
        let cfg = json!({"webhook_url": "https://discord.com/api/webhooks/1/tok"});
        let long = "x".repeat(MAX_CHARS - 1);
        let err = DiscordChannel
            .send(
                &cfg,
                &Message {
                    subject: Some(long),
                    body: "x".repeat(2),
                    meta: json!({}),
                },
            )
            .await
            .unwrap_err();
        assert!(matches!(err, NotifyError::TooLarge(_)));
    }

    #[test]
    fn validate_exige_url_discord() {
        assert!(DiscordChannel
            .validate(&json!({"webhook_url": "https://discord.com/api/webhooks/1/tok"}))
            .is_ok());
        assert!(DiscordChannel
            .validate(&json!({"webhook_url": "https://discordapp.com/api/webhooks/1/tok"}))
            .is_ok());
        assert!(DiscordChannel.validate(&json!({})).is_err());
        assert!(DiscordChannel
            .validate(&json!({"webhook_url": "https://evil.example/hook"}))
            .is_err());
    }
}
