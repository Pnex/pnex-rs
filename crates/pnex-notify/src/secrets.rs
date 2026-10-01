//! Vault references in channel configs (secrets.md D113, lot S4).
//!
//! A `secret` field of a stored channel config holds
//! `{"secret_id": "<uuid>"}`, never a value. The backend resolves it with
//! the keyring (Test buttons); the flow runtime fetches it from
//! `GET /internal/flow/secret/{id}` when the node first sends (D115) and
//! keeps it in memory only. A legacy plaintext string (before the boot
//! takeover) is passed through unchanged.

use pnex_core::FieldType;
use uuid::Uuid;

use crate::channels::CLIENT;
use crate::error::NotifyError;
use crate::registry::channel;

/// Header carrying the flow runtime service token.
pub const FLOW_TOKEN_HEADER: &str = "x-pnex-flow-token";

/// Vault id held by a field value (`{"secret_id": "…"}`), if any.
pub fn ref_of(value: &serde_json::Value) -> Option<Uuid> {
    value
        .get("secret_id")
        .and_then(|v| v.as_str())
        .and_then(|s| Uuid::parse_str(s).ok())
}

/// The `secret` fields of `kind`.
pub fn secret_fields(kind: &str) -> Vec<String> {
    channel(kind)
        .map(|ch| {
            ch.field_spec()
                .into_iter()
                .filter(|f| f.r#type == FieldType::Secret)
                .map(|f| f.id)
                .collect()
        })
        .unwrap_or_default()
}

/// `(field, secret id)` of every vault reference of a channel config.
pub fn secret_refs(kind: &str, config: &serde_json::Value) -> Vec<(String, Uuid)> {
    secret_fields(kind)
        .into_iter()
        .filter_map(|f| {
            let id = config.get(&f).and_then(ref_of)?;
            Some((f, id))
        })
        .collect()
}

/// `(field, value)` of every legacy plaintext secret of a channel config
/// (boot takeover input).
pub fn plaintext_secrets(kind: &str, config: &serde_json::Value) -> Vec<(String, String)> {
    secret_fields(kind)
        .into_iter()
        .filter_map(|f| {
            let v = config.get(&f)?.as_str()?.to_string();
            (!v.is_empty()).then_some((f, v))
        })
        .collect()
}

/// `config` with the given fields replaced by plain string values (the
/// sendable config, memory only).
pub fn with_values(config: &serde_json::Value, values: &[(String, String)]) -> serde_json::Value {
    let mut out = config.clone();
    if let Some(obj) = out.as_object_mut() {
        for (field, value) in values {
            obj.insert(field.clone(), serde_json::Value::String(value.clone()));
        }
    }
    out
}

/// `config` with the given fields replaced by vault references (the
/// storable config).
pub fn with_refs(config: &serde_json::Value, refs: &[(String, Uuid)]) -> serde_json::Value {
    let mut out = config.clone();
    if !out.is_object() {
        out = serde_json::Value::Object(Default::default());
    }
    if let Some(obj) = out.as_object_mut() {
        for (field, id) in refs {
            obj.insert(
                field.clone(),
                serde_json::json!({ "secret_id": id.to_string() }),
            );
        }
    }
    out
}

/// Fetches one secret value from the backend (flow runtime side).
/// `base_url` = `…/internal/flow/secret`. A 404 is answered while the flow
/// is not marked deployed yet (first deploy) or when it does not reference
/// the secret; the caller decides whether to retry.
pub async fn fetch(
    base_url: &str,
    token: &str,
    org_id: i64,
    id: Uuid,
) -> Result<String, NotifyError> {
    let resp = CLIENT
        .get(format!("{}/{id}", base_url.trim_end_matches('/')))
        .query(&[("org_id", org_id)])
        .header(FLOW_TOKEN_HEADER, token)
        .header(
            pnex_core::FLOW_WORKER_HEADER,
            pnex_core::flow_worker_fence().unwrap_or_default(),
        )
        .send()
        .await
        .map_err(|e| NotifyError::Network(format!("secret endpoint unreachable: {e}")))?;
    let status = resp.status();
    if !status.is_success() {
        return Err(NotifyError::Http {
            status: status.as_u16(),
        });
    }
    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| NotifyError::Network(format!("secret endpoint answer unreadable: {e}")))?;
    body.get("value")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .ok_or_else(|| NotifyError::Config("secret endpoint answered without a value".into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn refs_and_plaintext_are_told_apart() {
        let id = Uuid::from_u128(7);
        let cfg = json!({
            "url": "https://e.com",
            "secret_header": "x-sig",
            "secret_value": {"secret_id": id.to_string()},
        });
        assert_eq!(
            secret_refs("webhook", &cfg),
            [("secret_value".to_string(), id)]
        );
        assert!(plaintext_secrets("webhook", &cfg).is_empty());

        let legacy = json!({"bot_token": "123:ABC", "chat_id": "@c"});
        assert!(secret_refs("telegram", &legacy).is_empty());
        assert_eq!(
            plaintext_secrets("telegram", &legacy),
            [("bot_token".to_string(), "123:ABC".to_string())]
        );
        // Non-secret fields are never treated as secrets.
        assert!(secret_refs("webhook", &json!({"url": {"secret_id": id.to_string()}})).is_empty());
    }

    #[test]
    fn refs_and_values_round_trip() {
        let id = Uuid::from_u128(9);
        let stored = with_refs(&json!({"chat_id": "@c"}), &[("bot_token".into(), id)]);
        assert_eq!(stored["bot_token"]["secret_id"], id.to_string());
        assert_eq!(stored["chat_id"], "@c");
        let sendable = with_values(&stored, &[("bot_token".into(), "123:ABC".into())]);
        assert_eq!(sendable["bot_token"], "123:ABC");
        assert!(channel("telegram").unwrap().validate(&sendable).is_ok());
    }
}
