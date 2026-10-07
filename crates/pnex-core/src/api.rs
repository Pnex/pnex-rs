//! DTO de l'API `/api/v1` — source unique partagée backend ↔ frontend.
//!
//! Ces types reflètent les payloads des contrôleurs Phase 3 (proxy OAuth2,
//! user-info, orgs) et du `PATCH /api/v1/profile`. Les dates arrivent en
//! chaînes RFC 3339 (sérialisation SeaORM) — pas de chrono ici, le core
//! reste dépendance-free (serde uniquement).
//!
//! Rôles : strings minuscules (« owner », « admin », « viewer ») — convention
//! API (les enums SeaORM générés sérialisent en Capitalized, on mappe côté
//! backend via `role_str`/`RoleParam`).

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Réponse du proxy OAuth2 (`/oauth2/token`, `/oauth2/refresh`) — relay de
/// Keycloak (champs du grant flow standard).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenResponse {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_in: i64,
    pub token_type: String,
    /// Émis avec `scope=openid` — requis pour l'end-session Keycloak
    /// (`id_token_hint`).
    #[serde(default)]
    pub id_token: Option<String>,
}

/// `GET /api/v1/user-info` — identité + profil + orgs + comptage devices.
#[derive(Debug, Clone, Serialize, Deserialize)]
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
    /// `self_hosted` | `saas` (`PNEX_DEPLOYMENT_MODE`).
    #[serde(default = "default_deployment_mode")]
    pub deployment_mode: String,
}

fn default_deployment_mode() -> String {
    "self_hosted".to_string()
}

/// Bloc `profile` de `user-info` (préférences utilisateur).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UserProfile {
    pub language: String,
    pub timezone: String,
    #[serde(default)]
    pub date_format: Option<String>,
    /// « Light » / « Dark » / « Auto » côté storage (enum SeaORM Capitalized) ;
    /// minuscules en entrée du PATCH.
    pub theme: String,
    #[serde(default)]
    pub preferences: Option<serde_json::Value>,
    #[serde(default)]
    pub grafana_url: Option<String>,
}

/// Appartenance org d'un utilisateur, telle que renvoyée par `user-info`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrgMembership {
    pub id: i64,
    pub name: String,
    /// « owner » | « admin » | « viewer ».
    pub role: String,
    #[serde(default)]
    pub subscription_tier: Option<TierInfo>,
}

/// Tier d'abonnement d'une org (quotas).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TierInfo {
    pub name: String,
    pub max_sensor_devices: i64,
    pub max_actuator_devices: i64,
    pub max_mixed_devices: i64,
}

/// Comptage devices agrégé sur les orgs de l'utilisateur.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceCount {
    pub total: u64,
    pub active: u64,
    #[serde(default)]
    pub by_type: HashMap<String, u64>,
}

/// `GET /api/v1/orgs` — orgs dont je suis membre.
/// `PartialEq` requis par les props des composants du socle CRUD (macro
/// `#[component]` dioxus 0.7 — impl généré par champ).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrgSummary {
    pub id: i64,
    pub name: String,
    /// « owner » | « admin » | « viewer ».
    pub role: String,
    #[serde(default)]
    pub subscription_tier: Option<String>,
    #[serde(default)]
    pub created_at: Option<String>,
}

/// `GET /api/v1/orgs/{id}` — détail avec membres.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrgDetail {
    pub id: i64,
    pub name: String,
    #[serde(default)]
    pub subscription_tier_id: Option<i64>,
    /// Rôle de l'utilisateur courant dans cette org.
    pub role: String,
    pub members: Vec<OrgMember>,
}

/// Membre d'une organisation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrgMember {
    pub user_id: i64,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub full_name: Option<String>,
    /// « owner » | « admin » | « viewer ».
    pub role: String,
    #[serde(default)]
    pub created_at: Option<String>,
}

/// Corps du `PATCH /api/v1/profile` — champs optionnels, formulé par le front.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
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

/// Corps du `POST /api/v1/orgs`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateOrg {
    pub name: String,
}

/// Corps du `PATCH /api/v1/orgs/{id}`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateOrg {
    pub name: String,
}

/// Corps du `POST /api/v1/orgs/{id}/members`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AddMember {
    pub email: String,
    /// « owner » | « admin » | « viewer » (défaut « viewer » côté API).
    pub role: Option<String>,
}

/// Corps du `PATCH /api/v1/orgs/{id}/members/{user_id}`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateMember {
    /// « owner » | « admin » | « viewer ».
    pub role: String,
}

// ─────────────────────────── Assistant IA ───────────────────────────

/// `GET /api/v1/ai/status` — assistant state (drives the UI visibility).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
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
pub struct LlmProvider {
    pub id: uuid::Uuid,
    pub name: String,
    /// `anthropic` | `openai_compat`.
    pub kind: String,
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
    pub updated_at: String,
}

/// Create / update body of a provider. `api_key: None` keeps the current
/// key on update.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LlmProviderInput {
    pub name: String,
    pub kind: String,
    #[serde(default)]
    pub base_url: Option<String>,
    pub model: String,
    #[serde(default)]
    pub api_key: Option<crate::SecretFieldInput>,
    #[serde(default)]
    pub is_default: bool,
}

/// `POST …/providers/{id}/test` — one-shot ping of a provider.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
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

/// Contexte de page (route courante + entité ouverte).
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct AiPageContext {
    #[serde(default)]
    pub page: Option<String>,
    #[serde(default)]
    pub flow_id: Option<i64>,
    #[serde(default)]
    pub device_id: Option<i64>,
}

/// Trace d'outil renvoyée au front (bulle repliable ✓/✗ + deep-link flow).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AiToolTrace {
    pub name: String,
    pub arguments: serde_json::Value,
    pub ok: bool,
    pub summary: String,
    /// Flow touché — pilote le bouton « Ouvrir dans l'éditeur ».
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
pub struct AiConversation {
    pub id: uuid::Uuid,
    pub title: String,
    /// RFC 3339.
    pub created_at: String,
    pub last_message_at: String,
}

/// One stored message of a conversation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AiMessage {
    pub seq: i32,
    /// `user` | `assistant`.
    pub role: String,
    pub content: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_trace: Vec<AiToolTrace>,
    /// RFC 3339.
    pub created_at: String,
}

/// `GET /api/v1/ai/conversations/{id}` — resume a conversation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AiConversationDetail {
    pub conversation: AiConversation,
    pub messages: Vec<AiMessage>,
}

/// `POST /api/v1/ai/conversations` (title optional) and `PATCH` (rename).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AiConversationWrite {
    #[serde(default)]
    pub title: Option<String>,
}

/// `POST /api/v1/ai/conversations/{id}/messages` — only the new message:
/// the history is rebuilt by the server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiSendMessage {
    pub content: String,
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default)]
    pub page: Option<AiPageContext>,
}

/// Answer of one agent turn.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiTurnResponse {
    pub answer: String,
    pub tool_trace: Vec<AiToolTrace>,
    /// Updated conversation (title set by the first message).
    pub conversation: AiConversation,
}

/// `GET /api/v1/ai/retention` — how long inactive assistant conversations
/// are kept in the current org (D145).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
pub struct AiConversationsExport {
    /// RFC 3339.
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
        assert_eq!(detail.members[0].role, "viewer");
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
pub struct RetentionInfo {
    /// Effective retention in days (always >= 1, the O2 minimum).
    pub days: u32,
    /// `override` | `global` | `tier` | `env` — where `days` comes from.
    pub source: String,
    /// The raw value was below 1 day and has been raised to 1.
    #[serde(default)]
    pub clamped: bool,
    /// `self_hosted` | `saas`.
    pub deployment_mode: String,
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
pub struct RetentionUpdate {
    #[serde(default)]
    pub days: Option<u32>,
}

/// One O2 metrics stream of the org (`GET /api/v1/system/o2/streams`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
    pub time_min: Option<String>,
    #[serde(default)]
    pub time_max: Option<String>,
    /// Retention applied on the stream (0 = O2 instance default).
    pub retention_days: i64,
}

/// `GET /api/v1/system/o2/streams` — streams + O2 availability.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
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
pub struct OrgSystemRow {
    pub org_id: i64,
    pub name: String,
    #[serde(default)]
    pub tier_name: Option<String>,
    /// Effective retention (days) and its source.
    pub retention_days: u32,
    pub retention_source: String,
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

/// `POST /api/v1/system/o2/purge` — typed confirmation (org name).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct O2PurgeRequest {
    pub confirm: String,
}

/// `POST /api/v1/system/o2/streams/{name}/delete-range` — RFC 3339 bounds.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct O2DeleteRangeRequest {
    pub start: String,
    pub end: String,
}

/// Result of an O2 deletion (stream, purge or range).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct O2DeleteResult {
    /// Streams deleted (stream/purge) or targeted (range).
    #[serde(default)]
    pub streams: Vec<String>,
    /// Range deletion is asynchronous on O2 (compactor job).
    #[serde(default)]
    pub scheduled: bool,
    /// Effective bounds after alignment on full hours (range only).
    #[serde(default)]
    pub start: Option<String>,
    #[serde(default)]
    pub end: Option<String>,
}

/// Health of one platform component (`GET /api/v1/system/status`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ComponentStatus {
    /// Stable machine key (`database`, `rauthy`, `openobserve`, ...).
    pub key: String,
    /// `ok` | `degraded` | `down` | `not_configured`.
    pub status: String,
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
pub struct StatusMetric {
    /// Machine key, resolved by the UI to `system-metric-<key>`.
    pub key: String,
    /// `bytes` | `count` | `percent` | `seconds` | `text`.
    pub unit: String,
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
pub struct SystemStatus {
    pub server_version: String,
    pub deployment_mode: String,
    /// RFC 3339 generation time.
    pub generated_at: String,
    #[serde(default)]
    pub components: Vec<ComponentStatus>,
}
