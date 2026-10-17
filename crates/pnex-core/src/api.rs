//! DTOs of the `/api/v1` API, the single source shared by backend and
//! frontend. Timestamps travel as RFC 3339 strings (no chrono here, the core
//! stays serde-only). Closed sets (roles, theme, deployment mode...) are
//! lowercase enums on the wire.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Role of a user in an org (D2, D117). Lowercase on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum OrgRole {
    Owner,
    Admin,
    Member,
    Viewer,
}

impl OrgRole {
    pub const ALL: [OrgRole; 4] = [Self::Owner, Self::Admin, Self::Member, Self::Viewer];

    /// Wire form (`owner`, `admin`, `member`, `viewer`).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Owner => "owner",
            Self::Admin => "admin",
            Self::Member => "member",
            Self::Viewer => "viewer",
        }
    }

    /// Parses the wire form; `None` for anything else.
    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|r| r.as_str() == value)
    }

    /// Content writes (flows, devices, dashboards...): owner, admin, member.
    pub fn can_write(self) -> bool {
        !matches!(self, Self::Viewer)
    }

    /// Org governance (members, vault, telemetry deletion): owner or admin.
    pub fn can_administer(self) -> bool {
        matches!(self, Self::Owner | Self::Admin)
    }
}

/// UI theme of a user profile. Lowercase on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum UiTheme {
    #[default]
    Light,
    Dark,
    Auto,
}

impl UiTheme {
    /// Wire form (`light`, `dark`, `auto`).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Light => "light",
            Self::Dark => "dark",
            Self::Auto => "auto",
        }
    }
}

/// Deployment mode of the server (`PNEX_DEPLOYMENT_MODE`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum DeploymentMode {
    #[default]
    SelfHosted,
    Saas,
}

impl DeploymentMode {
    /// Wire form (`self_hosted`, `saas`).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SelfHosted => "self_hosted",
            Self::Saas => "saas",
        }
    }
}

/// Origin of an effective telemetry retention.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum RetentionSource {
    /// Per-org override set by a platform admin.
    Override,
    /// Platform default (self-hosted).
    Global,
    /// Subscription tier (SaaS).
    Tier,
    /// Server environment fallback.
    Env,
}

impl RetentionSource {
    /// Wire form (`override`, `global`, `tier`, `env`).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Override => "override",
            Self::Global => "global",
            Self::Tier => "tier",
            Self::Env => "env",
        }
    }
}

/// Token response of the OAuth2 proxy, relayed from the identity provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[cfg_attr(
    feature = "openapi",
    schema(
        description = "Token response of the OAuth2 proxy (`/oauth2/token`, `/oauth2/refresh`), relayed from the identity provider."
    )
)]
pub struct TokenResponse {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_in: i64,
    pub token_type: String,
    /// Issued with `scope=openid`; needed for the end-session call (`id_token_hint`).
    #[serde(default)]
    pub id_token: Option<String>,
}

/// `GET /api/v1/user-info`: identity, profile, orgs and device count.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[cfg_attr(
    feature = "openapi",
    schema(
        description = "`GET /api/v1/user-info`: identity, profile, org memberships and device count."
    )
)]
pub struct UserInfo {
    pub id: i64,
    pub username: String,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub full_name: Option<String>,
    #[serde(default)]
    pub profile: Option<UserProfile>,
    #[serde(default)]
    pub orgs: Vec<OrgMembership>,
    pub device_count: DeviceCount,
    /// Platform administrator (D72): infra status + global retention.
    #[serde(default)]
    pub platform_admin: bool,
    /// Deployment mode of the server (`PNEX_DEPLOYMENT_MODE`).
    #[serde(default)]
    pub deployment_mode: DeploymentMode,
}

/// `profile` block of user-info (user preferences).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[cfg_attr(
    feature = "openapi",
    schema(
        description = "User preferences (`profile` block of user-info, body of `PATCH /api/v1/profile` answers)."
    )
)]
pub struct UserProfile {
    pub language: String,
    pub timezone: String,
    #[serde(default)]
    pub date_format: Option<String>,
    pub theme: UiTheme,
    #[serde(default)]
    pub preferences: Option<serde_json::Value>,
    #[serde(default)]
    pub grafana_url: Option<String>,
}

/// Org membership of a user, as returned by user-info.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[cfg_attr(
    feature = "openapi",
    schema(description = "Org membership of the user as returned by user-info.")
)]
pub struct OrgMembership {
    pub id: i64,
    pub name: String,
    pub role: OrgRole,
    #[serde(default)]
    pub subscription_tier: Option<TierInfo>,
}

/// Subscription tier of an org (quotas).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[cfg_attr(
    feature = "openapi",
    schema(description = "Subscription tier of an org (device quotas).")
)]
pub struct TierInfo {
    pub name: String,
    pub max_sensor_devices: i64,
    pub max_actuator_devices: i64,
    pub max_mixed_devices: i64,
}

/// Device count aggregated over the orgs of the user.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[cfg_attr(
    feature = "openapi",
    schema(description = "Device count aggregated over the orgs of the user.")
)]
pub struct DeviceCount {
    pub total: u64,
    pub active: u64,
    #[serde(default)]
    pub by_type: HashMap<String, u64>,
}

/// `GET /api/v1/orgs` row: an org the caller is a member of. `PartialEq`
/// is required by the CRUD component props.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[cfg_attr(
    feature = "openapi",
    schema(description = "An org the user is a member of.")
)]
pub struct OrgSummary {
    pub id: i64,
    pub name: String,
    pub role: OrgRole,
    #[serde(default)]
    pub subscription_tier: Option<String>,
    #[serde(default)]
    #[cfg_attr(feature = "openapi", schema(format = DateTime))]
    pub created_at: Option<String>,
}

/// `GET /api/v1/orgs/{id}`: detail with members.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[cfg_attr(
    feature = "openapi",
    schema(description = "Org detail with its members.")
)]
pub struct OrgDetail {
    pub id: i64,
    pub name: String,
    #[serde(default)]
    pub subscription_tier_id: Option<i64>,
    /// Role of the caller in this org.
    pub role: OrgRole,
    pub members: Vec<OrgMember>,
}

/// Member of an organisation.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[cfg_attr(
    feature = "openapi",
    schema(description = "Member of an organisation.")
)]
pub struct OrgMember {
    pub user_id: i64,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub full_name: Option<String>,
    pub role: OrgRole,
    #[serde(default)]
    #[cfg_attr(feature = "openapi", schema(format = DateTime))]
    pub created_at: Option<String>,
}

/// Body of `PATCH /api/v1/profile`: optional fields.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[cfg_attr(
    feature = "openapi",
    schema(description = "Body of `PATCH /api/v1/profile`: only the provided fields change.")
)]
pub struct ProfilePatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timezone: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date_format: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub theme: Option<String>,
}

/// Body of `POST /api/v1/orgs`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[cfg_attr(
    feature = "openapi",
    schema(description = "Body of `POST /api/v1/orgs`.")
)]
pub struct CreateOrg {
    pub name: String,
}

/// Body of `PATCH /api/v1/orgs/{id}`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[cfg_attr(
    feature = "openapi",
    schema(description = "Body of `PATCH /api/v1/orgs/{id}`.")
)]
pub struct UpdateOrg {
    pub name: String,
}

/// Body of `POST /api/v1/orgs/{id}/members`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[cfg_attr(
    feature = "openapi",
    schema(description = "Body of `POST /api/v1/orgs/{id}/members`.")
)]
pub struct AddMember {
    pub email: String,
    /// Default `viewer`.
    pub role: Option<OrgRole>,
}

/// Body of `PATCH /api/v1/orgs/{id}/members/{user_id}`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[cfg_attr(
    feature = "openapi",
    schema(description = "Body of `PATCH /api/v1/orgs/{id}/members/{user_id}`.")
)]
pub struct UpdateMember {
    pub role: OrgRole,
}

// ─────────────────────────── Assistant IA ───────────────────────────

/// `GET /api/v1/ai/status` — assistant state (drives the UI visibility).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct AiStatus {
    pub enabled: bool,
    /// The org has a default provider (users bring their own LLM).
    pub configured: bool,
    /// Name of the effective provider.
    #[serde(default)]
    pub provider_name: Option<String>,
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
}

/// LLM provider of an org (secrets.md D116, D119):
/// `GET /api/v1/ai/providers`. The API key is a vault reference, never a
/// value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct LlmProvider {
    #[cfg_attr(feature = "openapi", schema(value_type = String, format = "uuid"))]
    pub id: uuid::Uuid,
    pub name: String,
    pub kind: LlmProviderKind,
    #[serde(default)]
    pub base_url: Option<String>,
    pub model: String,
    /// Vault reference of the key.
    #[serde(default)]
    pub api_key: Option<crate::SecretFieldView>,
    /// Whether a key is set.
    #[serde(default)]
    pub api_key_set: bool,
    pub is_default: bool,
    /// Usages this provider serves instead of the default (D212).
    #[serde(default)]
    pub usages: Vec<LlmUsage>,
    #[cfg_attr(feature = "openapi", schema(format = DateTime))]
    pub updated_at: String,
}

/// LLM protocol of a provider.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum LlmProviderKind {
    #[default]
    Anthropic,
    /// Any OpenAI-compatible chat completions endpoint.
    OpenaiCompat,
}

impl LlmProviderKind {
    /// Wire and stored value.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Anthropic => "anthropic",
            Self::OpenaiCompat => "openai_compat",
        }
    }

    /// From the wire or stored value; unknown values read as `anthropic`.
    pub fn parse(s: &str) -> Self {
        if s.eq_ignore_ascii_case("openai_compat") {
            Self::OpenaiCompat
        } else {
            Self::Anthropic
        }
    }
}

/// What an LLM call is for (D212): each usage may run on its own provider,
/// so the assistant and the flows never have to share one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum LlmUsage {
    /// The in-app assistant.
    Assistant,
    /// Text work of flows and file analyses: summaries, `llm_text`, fusion.
    Extraction,
    /// Image work: captions of camera frames and media.
    Vision,
}

impl LlmUsage {
    pub const ALL: [Self; 3] = [Self::Assistant, Self::Extraction, Self::Vision];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Assistant => "assistant",
            Self::Extraction => "extraction",
            Self::Vision => "vision",
        }
    }
}

/// Create / update body of a provider. `api_key: None` keeps the current
/// key on update.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct LlmProviderInput {
    pub name: String,
    pub kind: LlmProviderKind,
    #[serde(default)]
    pub base_url: Option<String>,
    pub model: String,
    #[serde(default)]
    pub api_key: Option<crate::SecretFieldInput>,
    #[serde(default)]
    pub is_default: bool,
    /// Usages to route to this provider; taken away from any other one.
    #[serde(default)]
    pub usages: Vec<LlmUsage>,
}

/// `POST …/providers/{id}/test` — one-shot ping of a provider.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct LlmProviderTest {
    pub ok: bool,
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub latency_ms: Option<u64>,
    /// Canonical English description of the failure (verbatim fallback).
    #[serde(default)]
    pub error: Option<String>,
    /// Machine code of the failure (`err_codes`), rendered as `err-<code>`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    /// Interpolation data of `code`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args: Option<serde_json::Value>,
}

/// Page the user is on when asking (current route and opened entity).
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct AiPageContext {
    #[serde(default)]
    pub page: Option<String>,
    #[serde(default)]
    pub flow_id: Option<i64>,
    #[serde(default)]
    pub device_id: Option<i64>,
}

/// Trace of one tool call shown in the chat (collapsible bubble, deep link to the flow).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct AiToolTrace {
    pub name: String,
    pub arguments: serde_json::Value,
    pub ok: bool,
    pub summary: String,
    /// Flow the tool touched: drives the "Open in the editor" button.
    #[serde(default)]
    pub flow_id: Option<i64>,
    /// Fluent key of a successful call's summary (`ai-trace-*`), rendered
    /// with `args`; absent on traces stored before it existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary_key: Option<String>,
    /// Machine code of a coded refusal (`err_codes`), resolved by the UI
    /// as `err-<code>`; `summary` stays the verbatim fallback.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    /// Interpolation data of `code` (refusal) or `summary_key` (success).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args: Option<serde_json::Value>,
}

/// One conversation of the current user in the current org (D145); list
/// rows never carry messages nor tool traces.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct AiConversation {
    #[cfg_attr(feature = "openapi", schema(value_type = String, format = "uuid"))]
    pub id: uuid::Uuid,
    pub title: String,
    #[cfg_attr(feature = "openapi", schema(format = DateTime))]
    pub created_at: String,
    #[cfg_attr(feature = "openapi", schema(format = DateTime))]
    pub last_message_at: String,
}

/// One stored message of a conversation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct AiMessage {
    pub seq: i32,
    pub role: AiRole,
    pub content: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_trace: Vec<AiToolTrace>,
    #[cfg_attr(feature = "openapi", schema(format = DateTime))]
    pub created_at: String,
}

/// Author of a stored assistant message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum AiRole {
    User,
    Assistant,
}

/// `GET /api/v1/ai/conversations/{id}` — resume a conversation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct AiConversationDetail {
    pub conversation: AiConversation,
    pub messages: Vec<AiMessage>,
}

/// `POST /api/v1/ai/conversations` (title optional) and `PATCH` (rename).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct AiConversationWrite {
    #[serde(default)]
    pub title: Option<String>,
}

/// `POST /api/v1/ai/conversations/{id}/messages` — only the new message:
/// the history is rebuilt by the server.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct AiSendMessage {
    pub content: String,
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default)]
    pub page: Option<AiPageContext>,
}

/// Answer of one agent turn.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct AiTurnResponse {
    pub answer: String,
    pub tool_trace: Vec<AiToolTrace>,
    /// Updated conversation (title set by the first message).
    pub conversation: AiConversation,
}

/// `GET /api/v1/ai/retention` — how long inactive assistant conversations
/// are kept in the current org (D145).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct AiRetention {
    /// Effective value (days): the org value, at most the platform one.
    pub days: u32,
    /// Platform value (days).
    pub platform_days: u32,
    /// Org value (days), when the org shortened it.
    #[serde(default)]
    pub org_days: Option<u32>,
    /// The caller may change the org value (owner/admin).
    #[serde(default)]
    pub editable: bool,
    /// The caller may change the platform value (platform admin).
    #[serde(default)]
    pub platform_editable: bool,
}

/// `GET /api/v1/ai/conversations/export` — portability export.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct AiConversationsExport {
    #[cfg_attr(feature = "openapi", schema(format = DateTime))]
    pub exported_at: String,
    pub conversations: Vec<AiConversationDetail>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_info_minimal_deserialize() {
        // Forme minimale renvoyée par le backend (profile/orgs absents ou vides).
        let json = r#"{
            "id": 1,
            "username": "alice",
            "email": null,
            "full_name": null,
            "profile": null,
            "orgs": [],
            "device_count": { "total": 0, "active": 0, "by_type": {} }
        }"#;
        let info: UserInfo = serde_json::from_str(json).unwrap();
        assert_eq!(info.username, "alice");
        assert!(info.orgs.is_empty());
    }

    #[test]
    fn orgs_shapes_deserialize() {
        let list = r#"[{
            "id": 3, "name": "Atelier Co", "role": "owner",
            "subscription_tier": "Free", "created_at": "2026-08-15T10:00:00+00:00"
        }]"#;
        let orgs: Vec<OrgSummary> = serde_json::from_str(list).unwrap();
        assert_eq!(orgs[0].subscription_tier.as_deref(), Some("Free"));

        let detail = r#"{
            "id": 3, "name": "Atelier Co", "subscription_tier_id": 1, "role": "owner",
            "members": [{ "user_id": 2, "email": "bob@example.com", "full_name": null,
                          "role": "viewer", "created_at": null }]
        }"#;
        let detail: OrgDetail = serde_json::from_str(detail).unwrap();
        assert_eq!(detail.members[0].role, OrgRole::Viewer);
    }

    #[test]
    fn profile_patch_skip_none() {
        let patch = ProfilePatch {
            language: Some("fr-FR".into()),
            ..Default::default()
        };
        let json = serde_json::to_string(&patch).unwrap();
        assert_eq!(json, r#"{"language":"fr-FR"}"#);
    }
}

// ─────────────────────────── System (D72) ───────────────────────────

/// `GET /api/v1/system/retention` — effective O2 retention of the org.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RetentionInfo {
    /// Effective retention in days (always >= 1, the O2 minimum).
    pub days: u32,
    /// Where `days` comes from.
    pub source: RetentionSource,
    /// The raw value was below 1 day and has been raised to 1.
    #[serde(default)]
    pub clamped: bool,
    pub deployment_mode: DeploymentMode,
    /// Per-org override (days), if any.
    #[serde(default)]
    pub org_override_days: Option<u32>,
    /// Platform default (days), self-hosted only.
    #[serde(default)]
    pub global_default_days: Option<u32>,
    /// Subscription tier name and its retention (days, rounded up).
    #[serde(default)]
    pub tier_name: Option<String>,
    #[serde(default)]
    pub tier_days: Option<u32>,
    /// The caller may change the override / global default.
    #[serde(default)]
    pub editable: bool,
}

/// `PUT /api/v1/system/retention/{org,default}` — `None` clears the value.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RetentionUpdate {
    #[serde(default)]
    pub days: Option<u32>,
}

/// One O2 metrics stream of the org (`GET /api/v1/system/o2/streams`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct O2StreamInfo {
    pub name: String,
    #[serde(default)]
    pub doc_num: Option<i64>,
    /// Bytes (O2 reports MB, converted server-side).
    #[serde(default)]
    pub storage_bytes: Option<u64>,
    #[serde(default)]
    pub compressed_bytes: Option<u64>,
    /// RFC 3339 bounds of the stored data.
    #[serde(default)]
    #[cfg_attr(feature = "openapi", schema(format = DateTime))]
    pub time_min: Option<String>,
    #[serde(default)]
    #[cfg_attr(feature = "openapi", schema(format = DateTime))]
    pub time_max: Option<String>,
    /// Retention applied on the stream (0 = O2 instance default).
    pub retention_days: i64,
}

/// `GET /api/v1/system/o2/streams` — streams + O2 availability.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct O2StreamList {
    /// O2 configured on the server.
    pub configured: bool,
    /// The org already has an O2 org (first telemetry ingested).
    pub provisioned: bool,
    #[serde(default)]
    pub streams: Vec<O2StreamInfo>,
    /// On-disk usage of the org (sum of O2 compressed sizes, bytes).
    #[serde(default)]
    pub used_bytes: u64,
    /// Subscription quota (bytes) — SaaS only; `None` = unlimited.
    #[serde(default)]
    pub quota_bytes: Option<u64>,
    /// Usage above the quota (informative: ingestion is never blocked).
    #[serde(default)]
    pub over_quota: bool,
}

/// One organization in the platform-admin overview
/// (`GET /api/v1/system/orgs`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct OrgSystemRow {
    pub org_id: i64,
    pub name: String,
    #[serde(default)]
    pub tier_id: Option<i64>,
    #[serde(default)]
    pub tier_name: Option<String>,
    /// Effective retention (days) and its source.
    pub retention_days: u32,
    pub retention_source: RetentionSource,
    #[serde(default)]
    pub org_override_days: Option<u32>,
    /// O2 space created (first telemetry ingested).
    pub provisioned: bool,
    /// On-disk O2 usage (bytes); `None` when O2 could not be reached.
    #[serde(default)]
    pub used_bytes: Option<u64>,
    #[serde(default)]
    pub stream_count: Option<usize>,
    /// Subscription quota (bytes) — SaaS only.
    #[serde(default)]
    pub quota_bytes: Option<u64>,
}

/// `GET /api/v1/system/tiers` — subscription tiers an org can be put on
/// (platform admin).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct TierOption {
    pub id: i64,
    pub name: String,
}

/// `PUT /api/v1/system/orgs/{id}/tier` — platform admin; `None` removes
/// the tier (no quota).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct OrgTierUpdate {
    #[serde(default)]
    pub tier_id: Option<i64>,
}

/// `POST /api/v1/system/o2/purge` — typed confirmation (org name).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct O2PurgeRequest {
    pub confirm: String,
}

/// `POST /api/v1/system/o2/streams/{name}/delete-range` — RFC 3339 bounds.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct O2DeleteRangeRequest {
    #[cfg_attr(feature = "openapi", schema(format = DateTime))]
    pub start: String,
    #[cfg_attr(feature = "openapi", schema(format = DateTime))]
    pub end: String,
}

/// Result of an O2 deletion (stream, purge or range).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct O2DeleteResult {
    /// Streams deleted (stream/purge) or targeted (range).
    #[serde(default)]
    pub streams: Vec<String>,
    /// Range deletion is asynchronous on O2 (compactor job).
    #[serde(default)]
    pub scheduled: bool,
    /// Effective bounds after alignment on full hours (range only).
    #[serde(default)]
    #[cfg_attr(feature = "openapi", schema(format = DateTime))]
    pub start: Option<String>,
    #[serde(default)]
    #[cfg_attr(feature = "openapi", schema(format = DateTime))]
    pub end: Option<String>,
}

/// Health state of a platform component.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum ComponentState {
    Ok,
    Degraded,
    Down,
    NotConfigured,
}

impl ComponentState {
    /// Wire form (`ok`, `degraded`, `down`, `not_configured`).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Degraded => "degraded",
            Self::Down => "down",
            Self::NotConfigured => "not_configured",
        }
    }
}

/// Unit of a status metric; the UI formats the value by it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum MetricUnit {
    Bytes,
    Count,
    Percent,
    Seconds,
    Text,
}

/// Health of one platform component (`GET /api/v1/system/status`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ComponentStatus {
    /// Stable machine key (`database`, `rauthy`, `openobserve`, ...).
    pub key: String,
    pub status: ComponentState,
    #[serde(default)]
    pub latency_ms: Option<u64>,
    /// Runtime diagnostic, verbatim (English, not translated).
    #[serde(default)]
    pub detail: Option<String>,
    #[serde(default)]
    pub metrics: Vec<StatusMetric>,
}

/// A labelled measure; the UI formats it by `unit`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct StatusMetric {
    /// Machine key, resolved by the UI to `system-metric-<key>`.
    pub key: String,
    pub unit: MetricUnit,
    #[serde(default)]
    pub value: Option<f64>,
    #[serde(default)]
    pub text: Option<String>,
    /// Upper bound for gauges (bytes total for a disk, ...).
    #[serde(default)]
    pub total: Option<f64>,
    /// Free-form qualifier shown verbatim (path, table, org name, ...).
    #[serde(default)]
    pub label: Option<String>,
}

/// `GET /api/v1/system/status` — platform-wide status (platform admin).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SystemStatus {
    pub server_version: String,
    pub deployment_mode: DeploymentMode,
    /// RFC 3339 generation time.
    #[cfg_attr(feature = "openapi", schema(format = DateTime))]
    pub generated_at: String,
    #[serde(default)]
    pub components: Vec<ComponentStatus>,
}
