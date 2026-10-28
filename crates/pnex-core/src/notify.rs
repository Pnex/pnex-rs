//! Notifications (D49–D54): DTOs shared by the backend, the frontend and
//! the flow runtime.
//!
//! [`FieldSpec`] describes the configuration form of each channel kind; the
//! frontend generates it from `GET /api/v1/notify/kinds` (the backend is the
//! single source of truth).

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Type of a channel form field ([`FieldSpec`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "lowercase")]
pub enum FieldType {
    Text,
    /// Write-only: never returned by a GET (`secrets_set` lists the set
    /// fields, the form offers "replace"), D54.
    Secret,
    Number,
    Select,
    Bool,
}

/// Field of a channel configuration form, rendered by the frontend.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct FieldSpec {
    pub id: String,
    /// Frontend i18n key (`notify-field-…`), resolved client-side.
    pub label_i18n: String,
    pub r#type: FieldType,
    pub required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placeholder: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub help_i18n: Option<String>,
    /// Choices of a `select` field (a select without options renders as
    /// text).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<String>,
}

/// A channel kind listed by `GET /api/v1/notify/kinds`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct NotifyKindInfo {
    pub kind: String,
    /// English display name; the frontend uses its own icon and
    /// `notify-kind-*` translation.
    pub label: String,
    pub field_spec: Vec<FieldSpec>,
}

/// Declared variable of a template (`{name, example}`): feeds the UI
/// picker and the preview fallback.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct TemplateVar {
    pub name: String,
    pub example: String,
}

/// Notification channel (destination). The `secret` fields of `config`
/// are **never** returned (D54): `secrets_set` lists which are set.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct NotifyChannel {
    #[cfg_attr(feature = "openapi", schema(value_type = String, format = "uuid"))]
    pub id: Uuid,
    pub org_id: i64,
    pub kind: String,
    pub name: String,
    pub enabled: bool,
    pub config: serde_json::Value,
    pub secrets_set: Vec<String>,
    /// Vault reference of each set `secret` field (`field → {secret_id,
    /// name}`, secrets.md D113) — what the form shows, never a value.
    #[serde(default)]
    pub secrets: std::collections::BTreeMap<String, crate::SecretFieldView>,
    /// Outcome of the last journaled delivery.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_status: Option<DeliveryStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "openapi", schema(format = DateTime))]
    pub last_delivery_at: Option<String>,
    #[cfg_attr(feature = "openapi", schema(format = DateTime))]
    pub created_at: String,
    #[cfg_attr(feature = "openapi", schema(format = DateTime))]
    pub updated_at: String,
}

/// Minijinja template (optional subject + body).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct NotifyTemplate {
    #[cfg_attr(feature = "openapi", schema(value_type = String, format = "uuid"))]
    pub id: Uuid,
    pub org_id: i64,
    pub name: String,
    pub subject: Option<String>,
    pub body: String,
    #[serde(default)]
    pub vars: Vec<TemplateVar>,
    #[cfg_attr(feature = "openapi", schema(format = DateTime))]
    pub created_at: String,
    #[cfg_attr(feature = "openapi", schema(format = DateTime))]
    pub updated_at: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct NotifyTemplateInput {
    pub name: String,
    #[serde(default)]
    pub subject: Option<String>,
    pub body: String,
    #[serde(default)]
    pub vars: Vec<TemplateVar>,
}

/// Preview parameters: given vars, missing ones fall back to the declared
/// `example`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PreviewInput {
    #[serde(default)]
    pub vars: std::collections::BTreeMap<String, String>,
    /// Sample payload (the `msg` context of the rendering), any object.
    #[serde(default)]
    pub payload: serde_json::Value,
}

/// Preview result (rendering only, nothing is sent).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct PreviewResult {
    pub subject: Option<String>,
    pub body: String,
}

/// OpenObserve logs stream holding the delivery journal (D86) — one stream
/// per org (the org is the O2 organization), no relational table.
pub const NOTIFY_DELIVERY_STREAM: &str = "notify_deliveries";

/// Delivery outcomes stored in the journal.
pub const DELIVERY_SENT: &str = "sent";
pub const DELIVERY_FAILED: &str = "failed";
/// Held back by the node's anti-spam window (nothing left the server).
pub const DELIVERY_BLOCKED: &str = "blocked";

/// Outcome of a delivery attempt, as read back from the journal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum DeliveryStatus {
    Sent,
    Failed,
    /// Held back by the node's anti-spam window (nothing left the server).
    Blocked,
}

impl DeliveryStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Sent => DELIVERY_SENT,
            Self::Failed => DELIVERY_FAILED,
            Self::Blocked => DELIVERY_BLOCKED,
        }
    }

    /// Journal value to status; an unknown value reads as `failed`.
    pub fn parse(raw: &str) -> Self {
        match raw {
            DELIVERY_SENT => Self::Sent,
            DELIVERY_BLOCKED => Self::Blocked,
            _ => Self::Failed,
        }
    }
}

/// What triggered a delivery attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum DeliverySource {
    /// A deployed flow's notify node.
    Flow,
    /// A Test button.
    Test,
    /// An OTA outcome.
    Ota,
    /// A direct server-side send.
    Direct,
}

impl DeliverySource {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Flow => "flow",
            Self::Test => "test",
            Self::Ota => "ota",
            Self::Direct => "direct",
        }
    }

    /// Journal value to source; an unknown value reads as `direct`.
    pub fn parse(raw: &str) -> Self {
        match raw {
            "flow" => Self::Flow,
            "test" => Self::Test,
            "ota" => Self::Ota,
            _ => Self::Direct,
        }
    }
}

/// One delivery attempt to journal — written by the backend (websocket
/// bus, Test buttons, OTA outcomes) and by the `pnex-notify` flow node
/// through `POST /internal/notify/journal` (other channel kinds).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct NotifyDeliveryEntry {
    pub org_id: i64,
    pub channel_id: Uuid,
    #[serde(default)]
    pub channel_kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template_id: Option<Uuid>,
    /// `flow` | `test` | `ota` | `direct`.
    pub source: String,
    /// [`DELIVERY_SENT`] | [`DELIVERY_FAILED`] | [`DELIVERY_BLOCKED`].
    pub status: String,
    /// Upstream HTTP status of a failed send, when there was one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http_status: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flow_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_id: Option<String>,
    /// Live websocket sessions reached (websocket channel only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivered: Option<i64>,
    /// SHA-256 (hex) of a recipient taken from the message (D313): the
    /// address itself is never journaled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recipient_hash: Option<String>,
}

/// Messages sent per day by one channel to recipients taken from messages.
pub const DYNAMIC_PER_CHANNEL_DAY: i32 = 200;
/// Messages sent per day to one recipient taken from messages.
pub const DYNAMIC_PER_RECIPIENT_DAY: i32 = 3;

/// `POST /internal/flow/notify-recipient`: may the node send to this
/// address now? The server checks the deployed node, the channel flag and
/// the daily caps, and counts the send when it answers yes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecipientCheck {
    pub org_id: i64,
    pub flow_id: i64,
    pub version: i64,
    pub node_id: String,
    pub channel_id: Uuid,
    pub to: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cc: Option<String>,
}

/// Journal entry as read back (`GET /api/v1/notify/deliveries`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct NotifyDelivery {
    /// OpenObserve `_timestamp` (µs), also the row key in the UI.
    pub ts_us: i64,
    #[cfg_attr(feature = "openapi", schema(value_type = String, format = "uuid"))]
    pub channel_id: Uuid,
    #[serde(default)]
    pub channel_kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "openapi", schema(value_type = Option<String>, format = "uuid"))]
    pub template_id: Option<Uuid>,
    pub source: DeliverySource,
    pub status: DeliveryStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http_status: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flow_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivered: Option<i64>,
    /// `ts_us` as RFC 3339.
    #[cfg_attr(feature = "openapi", schema(format = DateTime))]
    pub created_at: String,
}

/// Frame of the `/ws/notify` bus (JSON text): real-time push of a websocket
/// delivery to the org's subscribed clients.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NotifyItem {
    pub id: i64,
    pub subject: Option<String>,
    pub body: String,
    #[serde(default)]
    pub meta: serde_json::Value,
    pub created_at: String,
}

/// Stale notify reference found at deploy (non-blocking warning: the
/// degraded node warns and passes messages through).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct StaleNotifyNode {
    pub flow_id: i64,
    pub node_id: String,
    /// `channel_missing` | `channel_disabled` | `template_missing` |
    /// `template_changed` (template variables differ from the picked stamp).
    pub reason: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Contract `notify-kinds.v1`: FieldSpec roundtrip, `options` defaults to
    /// empty.
    #[test]
    fn field_spec_roundtrip_sans_options() {
        let json = r#"{"id":"url","label_i18n":"notify-field-url","type":"text","required":true}"#;
        let spec: FieldSpec = serde_json::from_str(json).expect("deserializable");
        assert_eq!(spec.r#type, FieldType::Text);
        assert!(spec.options.is_empty());
        let back = serde_json::to_value(&spec).unwrap();
        assert_eq!(back["type"], "text");
    }

    #[test]
    fn secret_field_roundtrip() {
        let json = r#"{"id":"bot_token","label_i18n":"notify-field-bot-token","type":"secret","required":false}"#;
        let spec: FieldSpec = serde_json::from_str(json).unwrap();
        assert_eq!(spec.r#type, FieldType::Secret);
    }
}
