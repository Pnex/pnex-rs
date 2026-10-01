//! Canal `telegram` — Bot API `sendMessage` : POST JSON `{chat_id, text}`
//! vers `https://api.telegram.org/bot{token}/sendMessage`.
//!
//! D54 : le bot token est un champ secret write-only (jamais loggé). Le
//! texte est borné à 4096 chars par l'API — garde fail-loud `TooLarge`
//! (D63, pas de troncature silencieuse). L'URL d'API est codée en dur ;
//! [`send_to`] factorise le chemin HTTP pour être éprouvé contre un
//! récepteur local (aucune config de test dans field_spec).

use async_trait::async_trait;
use serde_json::json;

use crate::channels::{compose_text, field, size_guard, str_field, CLIENT};
use crate::error::NotifyError;
use crate::{Channel, Message};

pub struct TelegramChannel;

/// Borne Telegram Bot API pour `text` (chars, pas octets).
const MAX_CHARS: usize = 4096;

/// chat_id Telegram : `@nomdecanal` (sans espace) ou id numérique (négatif
/// pour les groupes/canaux).
fn chat_id_valide(chat_id: &str) -> bool {
    if let Some(name) = chat_id.strip_prefix('@') {
        return !name.is_empty() && !name.contains(char::is_whitespace);
    }
    chat_id.parse::<i64>().is_ok()
}

/// POST `sendMessage` vers `api_base` (prod : `https://api.telegram.org`).
async fn send_to(
    api_base: &str,
    token: &str,
    chat_id: &str,
    text: &str,
) -> Result<(), NotifyError> {
    let resp = CLIENT
        .post(format!("{api_base}/bot{token}/sendMessage"))
        .json(&json!({"chat_id": chat_id, "text": text}))
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

#[async_trait]
impl Channel for TelegramChannel {
    fn kind(&self) -> &'static str {
        "telegram"
    }

    fn field_spec(&self) -> Vec<pnex_core::FieldSpec> {
        vec![
            field(
                "bot_token",
                "notify-field-bot-token",
                pnex_core::FieldType::Secret,
                true,
                None,
                None,
                &[],
            ),
            field(
                "chat_id",
                "notify-field-chat-id",
                pnex_core::FieldType::Text,
                true,
                Some("-1001234567890 ou @moncanal"),
                None,
                &[],
            ),
        ]
    }

    fn validate(&self, config: &serde_json::Value) -> Result<(), String> {
        let token = str_field(config, "bot_token")
            .ok_or_else(|| "the bot token is required".to_string())?;
        if token.contains(char::is_whitespace) {
            return Err("the bot token must not contain spaces".into());
        }
        let chat_id =
            str_field(config, "chat_id").ok_or_else(|| "the chat_id is required".to_string())?;
        if !chat_id_valide(chat_id) {
            return Err("invalid chat_id: @channelname or numeric id (e.g. -1001234567890)".into());
        }
        Ok(())
    }

    async fn send(&self, config: &serde_json::Value, msg: &Message) -> Result<(), NotifyError> {
        let token = str_field(config, "bot_token")
            .ok_or_else(|| NotifyError::Config("missing bot token".into()))?;
        let chat_id = str_field(config, "chat_id")
            .ok_or_else(|| NotifyError::Config("missing chat_id".into()))?;
        let text = compose_text(msg.subject.as_deref(), &msg.body);
        size_guard(&text, MAX_CHARS)?;
        send_to("https://api.telegram.org", token, chat_id, &text).await
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
    async fn poste_chat_id_et_texte_compose() {
        let seen = Arc::new(tokio::sync::Mutex::new(None::<Value>));
        let seen_for_handler = seen.clone();
        let app = axum::Router::new().route(
            "/bot123:ABC/sendMessage",
            post(move |Json(body): Json<Value>| {
                let seen = seen_for_handler.clone();
                async move {
                    *seen.lock().await = Some(body);
                    StatusCode::OK
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        send_to(
            &format!("http://{addr}"),
            "123:ABC",
            "-1001234567890",
            "Sujet\ncorps",
        )
        .await
        .unwrap();

        let body = seen.lock().await.take().unwrap();
        assert_eq!(body["chat_id"], "-1001234567890");
        assert_eq!(body["text"], "Sujet\ncorps");
    }

    #[tokio::test]
    async fn http_401_non_retryable() {
        let app = axum::Router::new().route(
            "/botbad/sendMessage",
            post(|| async { StatusCode::UNAUTHORIZED }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let e = send_to(&format!("http://{addr}"), "bad", "@c", "x")
            .await
            .unwrap_err();
        assert!(matches!(e, NotifyError::Http { status: 401 }));
        assert!(!e.retryable());
    }

    #[test]
    fn garde_taille_sans_appel_http() {
        let text = compose_text(Some("Sujet"), "corps");
        assert_eq!(text, "Sujet\ncorps");
        let err = size_guard(&"x".repeat(MAX_CHARS + 1), MAX_CHARS).unwrap_err();
        assert!(matches!(err, NotifyError::TooLarge(n) if n == MAX_CHARS + 1));
        assert!(!err.retryable());
    }

    #[test]
    fn validate_exige_token_et_chat_id_conformes() {
        assert!(TelegramChannel
            .validate(&json!({"bot_token": "123:ABC", "chat_id": "@moncanal"}))
            .is_ok());
        assert!(TelegramChannel
            .validate(&json!({"bot_token": "123:ABC", "chat_id": "-1001234567890"}))
            .is_ok());
        assert!(TelegramChannel.validate(&json!({"chat_id": "@c"})).is_err());
        assert!(TelegramChannel
            .validate(&json!({"bot_token": "123:ABC"}))
            .is_err());
        assert!(TelegramChannel
            .validate(&json!({"bot_token": "a b", "chat_id": "@c"}))
            .is_err());
        assert!(TelegramChannel
            .validate(&json!({"bot_token": "123:ABC", "chat_id": "a b"}))
            .is_err());
        assert!(TelegramChannel
            .validate(&json!({"bot_token": "123:ABC", "chat_id": "chat+1"}))
            .is_err());
    }
}
