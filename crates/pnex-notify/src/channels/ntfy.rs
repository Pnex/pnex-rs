//! `ntfy` channel — push to an ntfy server (ntfy.sh or self-hosted).
//!
//! Publishing **as JSON** (`POST {server}/` with `{topic, message,
//! title, priority, tags}`) and not as `X-Title` headers: the subject comes
//! from a template (arbitrary UTF-8), and a non-ASCII HTTP header would make
//! reqwest refuse/crash the request. D54: the token is a write-only secret
//! field, sent as `Authorization: Bearer`, never in the URL
//! nor logged. No size guard (D63): beyond 4096 bytes the
//! ntfy server attaches it as a file instead of rejecting.

use async_trait::async_trait;
use serde_json::json;

use crate::channels::{field, str_field, CLIENT};
use crate::error::NotifyError;
use crate::{Channel, Message};

pub struct NtfyChannel;

const PRIORITIES: &[&str] = &["min", "low", "default", "high", "max"];

#[async_trait]
impl Channel for NtfyChannel {
    fn kind(&self) -> &'static str {
        "ntfy"
    }

    fn field_spec(&self) -> Vec<pnex_core::FieldSpec> {
        vec![
            field(
                "server",
                "notify-field-server",
                pnex_core::FieldType::Text,
                true,
                Some("https://ntfy.sh"),
                None,
                &[],
            ),
            field(
                "topic",
                "notify-field-topic",
                pnex_core::FieldType::Text,
                true,
                None,
                None,
                &[],
            ),
            field(
                "token",
                "notify-field-token",
                pnex_core::FieldType::Secret,
                false,
                None,
                None,
                &[],
            ),
            field(
                "priority",
                "notify-field-priority",
                pnex_core::FieldType::Select,
                false,
                None,
                None,
                PRIORITIES,
            ),
            field(
                "tags",
                "notify-field-tags",
                pnex_core::FieldType::Text,
                false,
                Some("warning,skull"),
                None,
                &[],
            ),
        ]
    }

    fn validate(&self, config: &serde_json::Value) -> Result<(), String> {
        let server = str_field(config, "server")
            .ok_or_else(|| "the ntfy server address is required".to_string())?;
        if !(server.starts_with("http://") || server.starts_with("https://")) {
            return Err("the server must start with http:// or https:// (http accepted for a local instance)".into());
        }
        let topic =
            str_field(config, "topic").ok_or_else(|| "the topic is required".to_string())?;
        if !valid_topic(topic) {
            return Err(
                "the topic can only contain letters, digits, dashes and underscores (64 chars max)"
                    .into(),
            );
        }
        if let Some(p) = str_field(config, "priority") {
            if !PRIORITIES.contains(&p) {
                return Err(format!(
                    "invalid priority: {p} (min, low, default, high or max)"
                ));
            }
        }
        Ok(())
    }

    async fn send(&self, config: &serde_json::Value, msg: &Message) -> Result<(), NotifyError> {
        let server = str_field(config, "server")
            .ok_or_else(|| NotifyError::Config("missing ntfy server".into()))?;
        let topic = str_field(config, "topic")
            .ok_or_else(|| NotifyError::Config("missing topic".into()))?;
        let mut payload = json!({
            "topic": topic,
            "message": msg.body,
        });
        if let Some(title) = msg.subject.as_deref().filter(|s| !s.is_empty()) {
            payload["title"] = json!(title);
        }
        if let Some(level) = str_field(config, "priority").and_then(priority_level) {
            payload["priority"] = json!(level);
        }
        if let Some(tags) = str_field(config, "tags") {
            payload["tags"] = json!(tags
                .split(',')
                .map(str::trim)
                .filter(|t| !t.is_empty())
                .collect::<Vec<_>>());
        }
        let url = format!("{}/", server.trim_end_matches('/'));
        let mut req = CLIENT.post(&url).json(&payload);
        if let Some(token) = str_field(config, "token") {
            req = req.bearer_auth(token);
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

/// ntfy topic: `[-_A-Za-z0-9]{1,64}` — server constraint, checked
/// on the channel side for a displayable error message.
fn valid_topic(topic: &str) -> bool {
    !topic.is_empty()
        && topic.len() <= 64
        && topic
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// ntfy's JSON publish API only accepts the numeric priority (1–5); the
/// name ("high") is valid in headers but rejected in a JSON body (HTTP 400).
fn priority_level(name: &str) -> Option<u8> {
    PRIORITIES
        .iter()
        .position(|p| *p == name)
        .map(|i| i as u8 + 1)
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
    async fn posts_topic_title_priority_tags_and_bearer() {
        let seen = Arc::new(tokio::sync::Mutex::new(None::<(HeaderMap, Value)>));
        let seen_for_handler = seen.clone();
        let app = axum::Router::new().route(
            "/",
            post(move |headers: HeaderMap, Json(body): Json<Value>| {
                let seen = seen_for_handler.clone();
                async move {
                    *seen.lock().await = Some((headers, body));
                    StatusCode::OK
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        crate::channels::open_egress_for_tests();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let cfg = json!({
            "server": format!("http://{addr}"),
            "topic": "stitch-alertes",
            "token": "tk_test",
            "priority": "high",
            "tags": "warning, skull",
        });
        let msg = Message {
            subject: Some("Job terminé".into()),
            body: "stitch #8 ok".into(),
            meta: json!({"source": "test"}),
        };
        NtfyChannel.send(&cfg, &msg).await.unwrap();

        let (headers, body) = seen.lock().await.take().unwrap();
        assert_eq!(
            headers.get("authorization").and_then(|v| v.to_str().ok()),
            Some("Bearer tk_test")
        );
        assert_eq!(body["topic"], "stitch-alertes");
        assert_eq!(body["message"], "stitch #8 ok");
        assert_eq!(body["title"], "Job terminé");
        assert_eq!(body["priority"], 4, "JSON API: numeric priority");
        assert_eq!(body["tags"], json!(["warning", "skull"]));
    }

    #[tokio::test]
    async fn http_401_non_retryable_and_500_retryable() {
        // The channel always posts to `{server}/` — a single receiver, a
        // counter to vary the response (401 then 500).
        use std::sync::atomic::{AtomicU32, Ordering};
        let calls = Arc::new(AtomicU32::new(0));
        let calls_for_handler = calls.clone();
        let app = axum::Router::new().route(
            "/",
            post(move || {
                let calls = calls_for_handler.clone();
                async move {
                    if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                        StatusCode::UNAUTHORIZED
                    } else {
                        StatusCode::INTERNAL_SERVER_ERROR
                    }
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        crate::channels::open_egress_for_tests();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let base = format!("http://{addr}");
        let msg = Message {
            subject: None,
            body: "x".into(),
            meta: json!({}),
        };
        let e = NtfyChannel
            .send(&json!({"server": base.clone(), "topic": "t"}), &msg)
            .await
            .unwrap_err();
        assert!(matches!(e, NotifyError::Http { status: 401 }));
        assert!(!e.retryable());

        let e = NtfyChannel
            .send(&json!({"server": base, "topic": "t"}), &msg)
            .await
            .unwrap_err();
        assert!(matches!(e, NotifyError::Http { status: 500 }));
        assert!(e.retryable());
    }

    #[test]
    fn validate_requires_valid_server_topic_and_priority() {
        let ok = json!({"server": "https://ntfy.sh", "topic": "alertes"});
        assert!(NtfyChannel.validate(&ok).is_ok());
        assert!(NtfyChannel.validate(&json!({})).is_err());
        assert!(NtfyChannel
            .validate(&json!({"server": "ftp://ntfy.sh", "topic": "t"}))
            .is_err());
        // Topic: rejects space, 65 chars, unicode; accepts dash/underscore.
        for topic in ["deux mots", &"a".repeat(65), "café", "ok-topic_1"] {
            let r = NtfyChannel.validate(&json!({"server": "https://n.sh", "topic": topic}));
            assert_eq!(r.is_ok(), topic == "ok-topic_1", "topic {topic:?}");
        }
        assert!(NtfyChannel
            .validate(&json!({"server": "https://n.sh", "topic": "t", "priority": "urgent"}))
            .is_err());
        assert!(NtfyChannel
            .validate(&json!({"server": "https://n.sh", "topic": "t", "priority": "max"}))
            .is_ok());
    }
}
