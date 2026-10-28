//! `webhook` channel — generic JSON POST `{subject, body, meta}` (D51).
//! Covers ntfy (JSON POST), Gotify, Telegram Bot API, home automation… without
//! a per-service dependency: the target adapts or the consumer reads the JSON.
//!
//! D54: `secret_value` is a write-only secret field, sent as a header
//! (`secret_header`), never concatenated into the URL nor logged.
//!
//! D315: with `signing = hmac` the secret is never sent; the body is signed
//! instead: `x-pnex-timestamp: <unix seconds>` and
//! `x-pnex-signature: sha256=<hex HMAC-SHA256(secret, timestamp + "." + body)>`.
//! The receiver recomputes it and refuses a timestamp older than 5 minutes.

use async_trait::async_trait;
use serde_json::json;

use crate::channels::{field, str_field, CLIENT};
use crate::error::NotifyError;
use crate::{Channel, Message};

pub struct WebhookChannel;

/// `header`: the secret travels as is in `secret_header` (compatibility);
/// `hmac`: the body is signed with it (D315).
const SIGNING: &[&str] = &["header", "hmac"];
pub const TIMESTAMP_HEADER: &str = "x-pnex-timestamp";
pub const SIGNATURE_HEADER: &str = "x-pnex-signature";

/// `sha256=<hex>` of `timestamp.body` under `secret`.
pub fn signature(secret: &str, timestamp: i64, body: &[u8]) -> String {
    use hmac::{Hmac, Mac};
    let mut mac = Hmac::<sha2::Sha256>::new_from_slice(secret.as_bytes())
        .expect("HMAC takes a key of any length");
    mac.update(timestamp.to_string().as_bytes());
    mac.update(b".");
    mac.update(body);
    let hex: String = mac
        .finalize()
        .into_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    format!("sha256={hex}")
}

fn hmac_mode(config: &serde_json::Value) -> bool {
    str_field(config, "signing") == Some("hmac")
}

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
            field(
                "signing",
                "notify-field-signing",
                pnex_core::FieldType::Select,
                false,
                None,
                Some("notify-field-signing-help"),
                SIGNING,
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
        if let Some(mode) = str_field(config, "signing") {
            if !SIGNING.contains(&mode) {
                return Err("signing is header or hmac".into());
            }
        }
        if hmac_mode(config) {
            return match value {
                Some(_) => Ok(()),
                None => Err("hmac signing needs secret_value".into()),
            };
        }
        match (header, value) {
            (Some(_), Some(_)) | (None, None) => Ok(()),
            (None, Some(_)) => Err("secret_header is required to send secret_value".into()),
            (Some(_), None) => Ok(()), // header set without value: ignored at send
        }
    }

    async fn send(&self, config: &serde_json::Value, msg: &Message) -> Result<(), NotifyError> {
        let url =
            str_field(config, "url").ok_or_else(|| NotifyError::Config("missing url".into()))?;
        // Serialized once: the signed bytes are the bytes sent.
        let body = serde_json::to_vec(&json!({
            "subject": msg.subject,
            "body": msg.body,
            "meta": msg.meta,
        }))
        .map_err(|e| NotifyError::Config(e.to_string()))?;
        let mut req = CLIENT
            .post(url)
            .header(reqwest::header::CONTENT_TYPE, "application/json");
        match (hmac_mode(config), str_field(config, "secret_value")) {
            (true, Some(secret)) => {
                let ts = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0);
                req = req
                    .header(TIMESTAMP_HEADER, ts.to_string())
                    .header(SIGNATURE_HEADER, signature(secret, ts, &body));
            }
            (true, None) => return Err(NotifyError::Config("missing secret_value".into())),
            (false, value) => {
                if let Some((header, value)) = str_field(config, "secret_header").zip(value) {
                    req = req.header(header, value);
                }
            }
        }
        let req = req.body(body);
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
        crate::channels::open_egress_for_tests();
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
    async fn hmac_mode_signs_the_exact_body_and_never_sends_the_secret() {
        let seen = Arc::new(tokio::sync::Mutex::new(None::<(HeaderMap, Vec<u8>)>));
        let seen_for_handler = seen.clone();
        let app = axum::Router::new().route(
            "/hook",
            post(move |headers: HeaderMap, body: axum::body::Bytes| {
                let seen = seen_for_handler.clone();
                async move {
                    *seen.lock().await = Some((headers, body.to_vec()));
                    StatusCode::OK
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        crate::channels::open_egress_for_tests();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let cfg = json!({
            "url": format!("http://{addr}/hook"),
            "secret_header": "x-leak",
            "secret_value": "k3y",
            "signing": "hmac",
        });
        let msg = Message {
            subject: Some("Écart".into()),
            body: "à valider".into(),
            meta: json!({"kind": "ecart"}),
        };
        WebhookChannel.send(&cfg, &msg).await.unwrap();
        let (headers, body) = seen.lock().await.take().unwrap();
        assert!(headers.get("x-leak").is_none());
        let ts: i64 = headers[TIMESTAMP_HEADER].to_str().unwrap().parse().unwrap();
        assert_eq!(
            headers[SIGNATURE_HEADER].to_str().unwrap(),
            signature("k3y", ts, &body)
        );
        assert_ne!(signature("k3y", ts + 1, &body), signature("k3y", ts, &body));
        assert!(WebhookChannel
            .validate(&json!({"url": "https://e.com", "signing": "hmac"}))
            .is_err());
        assert!(WebhookChannel
            .validate(&json!({"url": "https://e.com", "signing": "rsa", "secret_value": "x"}))
            .is_err());
    }

    #[tokio::test]
    async fn http_500_erreur_retryable() {
        let app =
            axum::Router::new().route("/ko", post(|| async { StatusCode::INTERNAL_SERVER_ERROR }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        crate::channels::open_egress_for_tests();
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
        // secret_value without secret_header: refused (the secret has no
        // transmission vector).
        assert!(WebhookChannel
            .validate(&json!({"url": "https://e.com", "secret_value": "x"}))
            .is_err());
        assert!(WebhookChannel
            .validate(&json!({"url": "https://e.com", "secret_header": "h", "secret_value": "x"}))
            .is_ok());
    }
}
