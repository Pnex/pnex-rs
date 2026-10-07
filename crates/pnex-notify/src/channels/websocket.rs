//! Canal `websocket` — bus interne du serveur (D51, amendé 2026-09-14 :
//! pas de centre de notif dans le front, le WS est un canal configurable
//! que tout client externe authentifié consomme via `/ws/notify`).
//!
//! Particularité : la config DB du canal est `{}` — les coordonnées de
//! livraison (`deliver_url`, `token`) sont **injectées** :
//! - runtime de flows : env `PNEX_NOTIFY_DELIVER_URL` / `PNEX_NOTIFY_DELIVER_TOKEN`
//!   (posées par le supervisor via `apply_runtime_env`) ;
//! - backend (boutons Test) : `in_app_runtime_config(settings)`.
//!   Aucun secret notify dans flows.json, même au sens permissif D54.

use async_trait::async_trait;
use serde_json::json;

use crate::channels::{str_field, INTERNAL_CLIENT};
use crate::error::NotifyError;
use crate::{Channel, Message};

/// POST vers l'endpoint interne `deliver` du backend — le backend journalise
/// (source `flow`) puis diffuse la frame aux sessions WS de l'org.
pub struct WebSocketChannel;

#[async_trait]
impl Channel for WebSocketChannel {
    fn kind(&self) -> &'static str {
        "websocket"
    }

    /// Aucun champ éditable — le front rend un bloc informatif.
    fn field_spec(&self) -> Vec<pnex_core::FieldSpec> {
        vec![]
    }

    fn validate(&self, _config: &serde_json::Value) -> Result<(), String> {
        // Config DB toujours vide ; les coords injectées sont validées au
        // send (Config error) et au build du nœud (BadFlowsJson si env
        // absente).
        Ok(())
    }

    async fn send(&self, config: &serde_json::Value, msg: &Message) -> Result<(), NotifyError> {
        let deliver_url = str_field(config, "deliver_url").ok_or_else(|| {
            NotifyError::Config("missing deliver_url (env PNEX_NOTIFY_DELIVER_URL)".into())
        })?;
        let token = str_field(config, "token").ok_or_else(|| {
            NotifyError::Config("missing token (env PNEX_NOTIFY_DELIVER_TOKEN)".into())
        })?;
        let org_id = config
            .get("org_id")
            .and_then(|v| v.as_i64())
            .ok_or_else(|| {
                NotifyError::Config("missing org_id in the websocket channel config".into())
            })?;
        let channel_id = str_field(config, "channel_id").ok_or_else(|| {
            NotifyError::Config("missing channel_id in the websocket channel config".into())
        })?;

        let payload = json!({
            "org_id": org_id,
            "channel_id": channel_id,
            "subject": msg.subject,
            "body": msg.body,
            "meta": msg.meta,
        });
        // Le token ne transite QUE dans le header (D54 : jamais dans une
        // URL, qui est loggable).
        let resp = INTERNAL_CLIENT
            .post(deliver_url)
            .bearer_auth(token)
            // Fencing identity of the flow worker (D106), empty elsewhere.
            .header(
                pnex_core::FLOW_WORKER_HEADER,
                pnex_core::flow_worker_fence().unwrap_or_default(),
            )
            .json(&payload)
            .send()
            .await
            .map_err(|e| NotifyError::Network(format!("deliver inatteignable : {e}")))?;
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
    use crate::Message;
    use axum::http::{HeaderMap, StatusCode};
    use axum::routing::post;
    use axum::Json;
    use serde_json::Value;
    use std::sync::Arc;

    #[tokio::test]
    async fn poste_le_payload_et_le_bearer_vers_deliver() {
        let seen = Arc::new(tokio::sync::Mutex::new(None::<(HeaderMap, Value)>));
        let seen_for_handler = seen.clone();
        let app = axum::Router::new().route(
            "/internal/notify/deliver",
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
            "deliver_url": format!("http://{addr}/internal/notify/deliver"),
            "token": "tok-secret",
            "org_id": 7,
            "channel_id": "3f2ecfe9-1111-2222-3333-444455556666",
        });
        let msg = Message {
            subject: Some("Alerte".into()),
            body: "seuil dépassé".into(),
            meta: json!({"flow": 12}),
        };
        WebSocketChannel.send(&cfg, &msg).await.unwrap();

        let (headers, body) = seen.lock().await.take().unwrap();
        assert_eq!(
            headers.get("authorization").and_then(|v| v.to_str().ok()),
            Some("Bearer tok-secret")
        );
        assert_eq!(body["org_id"], 7);
        assert_eq!(body["channel_id"], "3f2ecfe9-1111-2222-3333-444455556666");
        assert_eq!(body["subject"], "Alerte");
        assert_eq!(body["body"], "seuil dépassé");
        assert_eq!(body["meta"]["flow"], 12);
    }

    #[tokio::test]
    async fn coords_injectees_absentes_erreur_config() {
        let e = WebSocketChannel
            .send(
                &json!({}),
                &Message {
                    subject: None,
                    body: "x".into(),
                    meta: json!({}),
                },
            )
            .await
            .unwrap_err();
        assert!(matches!(e, NotifyError::Config(_)));
    }
}
