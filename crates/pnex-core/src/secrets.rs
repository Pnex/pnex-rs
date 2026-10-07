//! Org secrets vault (secrets.md D110–D118) — shared DTOs, wasm-safe.
//!
//! A value never leaves the server: lists and forms only carry ids and
//! names. A consumer config holds a typed [`SecretRef`], never a `{{…}}`
//! template (D113).

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Typed reference to a vault row, as stored in a consumer config
/// (`{"secret_id": "…"}`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SecretRef {
    pub secret_id: Uuid,
}

/// Write side of a secret field in a functional form (D113): either pick an
/// existing secret, or type a value (owner/admin only) that creates or
/// replaces the field's dedicated secret.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SecretFieldInput {
    Pick { secret_id: Uuid },
    Value { value: String },
}

/// A secret field stored inside a flow graph node (lot S5, http-fetch).
///
/// Wire forms, all accepted on read: `null` / absent / `""` → [`Unset`];
/// `{"secret_id": "…"}` → [`Ref`]; a bare string or `{"value": "…"}` →
/// [`Value`]. The backend turns every `Value` into a vault reference on
/// save, so stored graphs, `flows.json` and the cluster wire only carry
/// references; a `Value` survives only in a graph about to be saved (or in
/// a pre-vault graph not taken over yet).
///
/// [`Unset`]: SecretSlot::Unset
/// [`Ref`]: SecretSlot::Ref
/// [`Value`]: SecretSlot::Value
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum SecretSlot {
    #[default]
    Unset,
    Ref(Uuid),
    Value(String),
}

impl SecretSlot {
    pub fn is_unset(&self) -> bool {
        matches!(self, Self::Unset)
    }

    pub fn secret_id(&self) -> Option<Uuid> {
        match self {
            Self::Ref(id) => Some(*id),
            _ => None,
        }
    }
}

/// A typed value; blank (whitespace only) means unset.
impl From<String> for SecretSlot {
    fn from(s: String) -> Self {
        if s.trim().is_empty() {
            Self::Unset
        } else {
            Self::Value(s)
        }
    }
}

impl From<&str> for SecretSlot {
    fn from(s: &str) -> Self {
        Self::from(s.to_string())
    }
}

impl Serialize for SecretSlot {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Unset => s.serialize_none(),
            Self::Ref(id) => SecretRef { secret_id: *id }.serialize(s),
            Self::Value(v) => s.serialize_str(v),
        }
    }
}

impl<'de> Deserialize<'de> for SecretSlot {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        // Through `Value`: robust inside internally tagged enums (buffered
        // content), and the shapes are tiny.
        let v = serde_json::Value::deserialize(d)?;
        Ok(match v {
            serde_json::Value::Null => Self::Unset,
            serde_json::Value::String(s) => Self::from(s),
            serde_json::Value::Object(map) => {
                if let Some(id) = map.get("secret_id").and_then(|v| v.as_str()) {
                    Self::Ref(Uuid::parse_str(id).map_err(serde::de::Error::custom)?)
                } else if let Some(value) = map.get("value").and_then(|v| v.as_str()) {
                    Self::from(value)
                } else {
                    return Err(serde::de::Error::custom(
                        "secret field: expected {\"secret_id\"} or {\"value\"}",
                    ));
                }
            }
            _ => {
                return Err(serde::de::Error::custom(
                    "secret field: expected a string or an object",
                ))
            }
        })
    }
}

/// Read side of a secret field: the reference and its display name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecretFieldView {
    pub secret_id: Uuid,
    pub name: String,
}

/// Kind of consumer referencing a secret (`secret_usages.consumer_kind`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SecretConsumerKind {
    NotifyChannel,
    Flow,
    Wifi,
    LlmProvider,
}

impl SecretConsumerKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NotifyChannel => "notify-channel",
            Self::Flow => "flow",
            Self::Wifi => "wifi",
            Self::LlmProvider => "llm-provider",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "notify-channel" => Self::NotifyChannel,
            "flow" => Self::Flow,
            "wifi" => Self::Wifi,
            "llm-provider" => Self::LlmProvider,
            _ => return None,
        })
    }
}

/// One "used by" entry of a secret.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecretUsage {
    pub kind: SecretConsumerKind,
    /// Consumer id as text (bigint or uuid depending on the kind).
    pub consumer_id: String,
    /// Field path inside the consumer (`token`, `nodes/<id>/auth.bearer`).
    pub field: String,
    /// Consumer display name when resolvable (channel name, flow name…).
    #[serde(default)]
    pub label: Option<String>,
}

/// `GET /api/v1/secrets` row. Never carries a value. Viewers only get
/// `id` + `name` (other fields empty).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrgSecret {
    pub id: Uuid,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub usages: Vec<SecretUsage>,
    /// Email of the last writer, when known.
    #[serde(default)]
    pub updated_by: Option<String>,
    /// RFC 3339.
    pub created_at: String,
    /// RFC 3339.
    pub updated_at: String,
}

/// Body of `POST /api/v1/secrets` (value required) and
/// `PUT /api/v1/secrets/{id}` (value optional: absent = keep it).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct OrgSecretInput {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
}

/// `POST /api/v1/system/secrets/rekey` (platform admin, secrets.md S8):
/// outcome of the master key rotation run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct SecretsRekeyReport {
    /// Rows rewritten under the write key.
    pub rewritten: u64,
    /// Rows under a key missing from the keyring (left untouched).
    pub unreadable: u64,
    /// Rows changed concurrently (already under the write key).
    pub skipped: u64,
    /// Rows still not under the write key after the run.
    pub remaining: u64,
}

/// Longest secret name (column `varchar(255)`).
pub const SECRET_NAME_MAX: usize = 255;
/// Longest secret value accepted (bytes) — tokens, passwords, URLs.
pub const SECRET_VALUE_MAX: usize = 16 * 1024;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn field_input_is_untagged() {
        let id = Uuid::from_u128(7);
        let pick: SecretFieldInput =
            serde_json::from_str(&format!(r#"{{"secret_id":"{id}"}}"#)).unwrap();
        assert_eq!(pick, SecretFieldInput::Pick { secret_id: id });
        let value: SecretFieldInput = serde_json::from_str(r#"{"value":"x"}"#).unwrap();
        assert_eq!(value, SecretFieldInput::Value { value: "x".into() });
        assert!(serde_json::from_str::<SecretFieldInput>("{}").is_err());
    }

    #[test]
    fn secret_ref_wire_shape() {
        let r = SecretRef {
            secret_id: Uuid::from_u128(1),
        };
        assert_eq!(
            serde_json::to_string(&r).unwrap(),
            r#"{"secret_id":"00000000-0000-0000-0000-000000000001"}"#
        );
    }

    #[test]
    fn consumer_kind_round_trip() {
        for k in [
            SecretConsumerKind::NotifyChannel,
            SecretConsumerKind::Flow,
            SecretConsumerKind::Wifi,
            SecretConsumerKind::LlmProvider,
        ] {
            assert_eq!(SecretConsumerKind::parse(k.as_str()), Some(k));
            assert_eq!(
                serde_json::to_string(&k).unwrap(),
                format!("\"{}\"", k.as_str())
            );
        }
    }
}

#[cfg(test)]
mod slot_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn secret_slot_reads_every_wire_form() {
        let id = Uuid::from_u128(5);
        let read = |v: serde_json::Value| serde_json::from_value::<SecretSlot>(v).unwrap();
        assert_eq!(read(json!(null)), SecretSlot::Unset);
        assert_eq!(read(json!("")), SecretSlot::Unset);
        assert_eq!(read(json!("  ")), SecretSlot::Unset);
        assert_eq!(read(json!("tok")), SecretSlot::Value("tok".into()));
        assert_eq!(
            read(json!({"value": "tok"})),
            SecretSlot::Value("tok".into())
        );
        assert_eq!(
            read(json!({"secret_id": id.to_string()})),
            SecretSlot::Ref(id)
        );
        // The editor's read model ({secret_id, name}) is a reference too.
        assert_eq!(
            read(json!({"secret_id": id.to_string(), "name": "n"})),
            SecretSlot::Ref(id)
        );
        assert!(serde_json::from_value::<SecretSlot>(json!(3)).is_err());
        assert!(serde_json::from_value::<SecretSlot>(json!({"x": 1})).is_err());
    }

    #[test]
    fn secret_slot_writes_references_and_values() {
        let id = Uuid::from_u128(5);
        assert_eq!(
            serde_json::to_value(SecretSlot::Ref(id)).unwrap(),
            json!({"secret_id": id.to_string()})
        );
        assert_eq!(
            serde_json::to_value(SecretSlot::Unset).unwrap(),
            json!(null)
        );
        assert_eq!(
            serde_json::to_value(SecretSlot::from("v")).unwrap(),
            json!("v")
        );
    }

    #[test]
    fn http_fetch_auth_carries_slots_inside_its_tagged_enum() {
        let id = Uuid::from_u128(8);
        let auth: crate::HttpFetchAuth = serde_json::from_value(
            json!({"mode": "bearer", "token": {"secret_id": id.to_string()}}),
        )
        .unwrap();
        assert_eq!(
            auth,
            crate::HttpFetchAuth::Bearer {
                token: SecretSlot::Ref(id)
            }
        );
        let legacy: crate::HttpFetchAuth =
            serde_json::from_value(json!({"mode": "basic", "username": "u", "password": "p"}))
                .unwrap();
        assert_eq!(
            legacy,
            crate::HttpFetchAuth::Basic {
                username: "u".into(),
                password: "p".into()
            }
        );
    }
}

/// Destination a secret is sent to, as compared by R9 (SEC-W2): the
/// lowercased origin `scheme://host:port` of `url`, default port made
/// explicit. Path, query and credentials are ignored: the secret stays
/// with the same server. `None` = not a URL with a host.
pub fn destination_of_url(raw: &str) -> Option<String> {
    let url = url::Url::parse(raw.trim()).ok()?;
    let host = url.host_str()?.to_ascii_lowercase();
    let port = url.port_or_known_default()?;
    Some(format!("{}://{host}:{port}", url.scheme()))
}

/// Destination key of a URL field that carries a secret: its origin, or
/// the raw trimmed text when it does not parse (then only the very same
/// text matches — an unparsable URL never sends anything anyway).
pub fn destination_key(raw: &str) -> String {
    destination_of_url(raw).unwrap_or_else(|| format!("raw:{}", raw.trim()))
}

#[cfg(test)]
mod destination_tests {
    use super::*;

    #[test]
    fn origin_ignores_path_query_and_credentials() {
        assert_eq!(
            destination_of_url("https://API.example.com/v1/x?a=1").as_deref(),
            Some("https://api.example.com:443")
        );
        assert_eq!(
            destination_of_url("https://user:pw@api.example.com:443/other"),
            destination_of_url("https://api.example.com")
        );
        assert_eq!(
            destination_of_url("http://192.168.1.20:8080/a").as_deref(),
            Some("http://192.168.1.20:8080")
        );
    }

    #[test]
    fn host_port_or_scheme_change_is_another_destination() {
        let base = destination_of_url("https://api.example.com/x");
        assert_ne!(base, destination_of_url("https://evil.example.net/x"));
        assert_ne!(base, destination_of_url("https://api.example.com:8443/x"));
        assert_ne!(base, destination_of_url("http://api.example.com/x"));
        // Userinfo tricks resolve to the real host.
        assert_eq!(
            destination_of_url("https://api.example.com@evil.example.net/").as_deref(),
            Some("https://evil.example.net:443")
        );
    }

    #[test]
    fn unparsable_urls_only_match_themselves() {
        assert_eq!(destination_of_url("not a url"), None);
        assert_eq!(destination_key(" not a url "), "raw:not a url");
    }
}
