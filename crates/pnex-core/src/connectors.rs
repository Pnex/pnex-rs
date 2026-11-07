//! Connectors (data-pipelines.md D352): saved access to an outside database, the
//! password in the vault. Only the server uses one: neither the worker nor the flow
//! runtime ever sees a connector or its secret. DTOs and checks, wasm-safe.

use serde::{Deserialize, Serialize};

use crate::SecretFieldInput;

pub mod rest;

pub const NAME_MAX: usize = 100;
pub const HOST_MAX: usize = 253;
pub const FIELD_MAX: usize = 255;
pub const QUERY_MAX_BYTES: usize = 64 * 1024;
/// Hosts on private addresses one org may reach.
pub const ALLOWED_HOSTS_MAX: usize = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "lowercase")]
pub enum ConnectorKind {
    #[default]
    Postgres,
    /// An HTTP API read with GET (connectors.md D793).
    Rest,
    /// Calls an outside system sends, signed, kept until an import reads them (D795).
    Webhook,
    /// A MySQL or MariaDB database, under TLS only (D792).
    Mysql,
}

impl ConnectorKind {
    pub fn wire(self) -> &'static str {
        match self {
            Self::Postgres => "postgres",
            Self::Rest => "rest",
            Self::Webhook => "webhook",
            Self::Mysql => "mysql",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "postgres" => Some(Self::Postgres),
            "rest" => Some(Self::Rest),
            "webhook" => Some(Self::Webhook),
            "mysql" => Some(Self::Mysql),
            _ => None,
        }
    }
}

/// How a REST connector authenticates. Every secret value is the connector's one
/// vault secret: the token, the header value, the password or the client secret.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum RestAuth {
    #[default]
    None,
    Bearer,
    /// The secret in the header named `header_name`.
    Header,
    /// `username` and the secret.
    Basic,
    /// `username` is the client id, the secret the client secret.
    Oauth2ClientCredentials,
}

/// What a REST connector adds to host, port and TLS.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RestSettings {
    /// Path every request starts from (`/api/v2`).
    #[serde(default = "root")]
    pub base_path: String,
    #[serde(default)]
    pub auth: RestAuth,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub header_name: String,
    /// OAuth2 token endpoint, absolute.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub token_url: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub scope: String,
}

fn root() -> String {
    "/".into()
}

/// How the sender of a webhook signs its calls (connectors.md D795): HMAC-SHA256 of
/// the body, or of `<timestamp>.<body>` when a timestamp header is named, with the
/// connector's secret, hex encoded after `signature_prefix`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct WebhookSettings {
    #[serde(default = "signature_header")]
    pub signature_header: String,
    #[serde(default = "signature_prefix")]
    pub signature_prefix: String,
    /// Header holding the Unix time of the call; empty = no replay window.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub timestamp_header: String,
}

impl Default for WebhookSettings {
    fn default() -> Self {
        Self {
            signature_header: signature_header(),
            signature_prefix: signature_prefix(),
            timestamp_header: String::new(),
        }
    }
}

fn signature_header() -> String {
    "X-Signature".into()
}

fn signature_prefix() -> String {
    "sha256=".into()
}

/// Seconds a signed timestamp stays valid either way.
pub const WEBHOOK_SKEW_SECS: i64 = 300;
/// Bytes of one webhook call.
pub const WEBHOOK_BODY_MAX: usize = 1024 * 1024;

fn header_name_ok(h: &str) -> bool {
    !h.is_empty() && h.len() <= 100 && h.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

impl WebhookSettings {
    fn violation(&self) -> Option<(&'static str, String)> {
        if !header_name_ok(&self.signature_header) {
            return Some(("webhook.signature_header", "invalid".into()));
        }
        if !self.timestamp_header.is_empty() && !header_name_ok(&self.timestamp_header) {
            return Some(("webhook.timestamp_header", "invalid".into()));
        }
        if self.signature_prefix.len() > 20
            || !self.signature_prefix.chars().all(|c| c.is_ascii_graphic())
        {
            return Some(("webhook.signature_prefix", "invalid".into()));
        }
        None
    }

    /// The bytes the sender signed.
    pub fn signed_payload(&self, timestamp: Option<&str>, body: &[u8]) -> Vec<u8> {
        match timestamp {
            Some(t) if !self.timestamp_header.is_empty() => {
                let mut out = Vec::with_capacity(t.len() + 1 + body.len());
                out.extend_from_slice(t.as_bytes());
                out.push(b'.');
                out.extend_from_slice(body);
                out
            }
            _ => body.to_vec(),
        }
    }

    /// The signature bytes of a header value, without the prefix; `None` if malformed.
    pub fn signature(&self, header: &str) -> Option<Vec<u8>> {
        let hex = header.trim().strip_prefix(self.signature_prefix.as_str())?;
        if hex.len() % 2 != 0 || hex.len() > 256 {
            return None;
        }
        (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok())
            .collect()
    }
}

/// Whether a signed Unix timestamp is within [`WEBHOOK_SKEW_SECS`] of `now`.
pub fn webhook_fresh(timestamp: &str, now: i64) -> bool {
    timestamp
        .trim()
        .parse::<i64>()
        .is_ok_and(|t| (now - t).abs() <= WEBHOOK_SKEW_SECS)
}

/// A secret PneX generated, shown once.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ConnectorSecret {
    pub secret: String,
}

/// A path piece: no scheme, query, fragment or whitespace to smuggle.
fn clean_path(p: &str) -> bool {
    p.starts_with('/')
        && p.len() <= 2048
        && !p.contains("//")
        && !p
            .chars()
            .any(|c| c.is_whitespace() || matches!(c, '?' | '#' | '\\' | '@'))
}

impl RestSettings {
    fn violation(&self, username: &str) -> Option<(&'static str, String)> {
        let invalid = |field| Some((field, "invalid".to_string()));
        let required = |field| Some((field, "required".to_string()));
        // Empty means the root.
        if !self.base_path.is_empty() && !clean_path(&self.base_path) {
            return invalid("rest.base_path");
        }
        match self.auth {
            RestAuth::Header if self.header_name.trim().is_empty() => required("rest.header_name"),
            RestAuth::Header
                if !self
                    .header_name
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-')
                    || self.header_name.len() > 100 =>
            {
                invalid("rest.header_name")
            }
            RestAuth::Basic | RestAuth::Oauth2ClientCredentials if username.trim().is_empty() => {
                required("username")
            }
            RestAuth::Oauth2ClientCredentials if self.token_url.trim().is_empty() => {
                required("rest.token_url")
            }
            RestAuth::Oauth2ClientCredentials
                if !self.token_url.starts_with("https://") || self.token_url.len() > 2048 =>
            {
                invalid("rest.token_url")
            }
            _ if self.scope.len() > FIELD_MAX => {
                Some(("rest.scope", format!("max_length:{FIELD_MAX}")))
            }
            _ => None,
        }
    }
}

/// A connector as read: never its password (R4, R16).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Connector {
    #[cfg_attr(feature = "openapi", schema(format = "uuid"))]
    pub id: String,
    pub name: String,
    pub kind: ConnectorKind,
    pub host: String,
    pub port: i32,
    pub database: String,
    pub username: String,
    /// Whether the connection requires TLS.
    pub tls: bool,
    /// Settings of a `rest` connector.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rest: Option<RestSettings>,
    /// Settings of a `webhook` connector.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub webhook: Option<WebhookSettings>,
    /// Whether a password is set.
    pub has_password: bool,
    pub version: i32,
    #[cfg_attr(feature = "openapi", schema(format = "date-time"))]
    pub created_at: String,
    #[cfg_attr(feature = "openapi", schema(format = "date-time"))]
    pub updated_at: String,
}

/// A connector as written. An absent `password` keeps the current one.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ConnectorInput {
    pub name: String,
    #[serde(default)]
    pub kind: ConnectorKind,
    /// Empty for a webhook.
    #[serde(default)]
    pub host: String,
    #[serde(default)]
    pub port: i32,
    #[serde(default)]
    pub database: String,
    #[serde(default)]
    pub username: String,
    #[serde(default = "default_tls")]
    pub tls: bool,
    /// Required for a `rest` connector, ignored otherwise.
    #[serde(default)]
    pub rest: Option<RestSettings>,
    /// For a `webhook` connector; absent = the defaults.
    #[serde(default)]
    pub webhook: Option<WebhookSettings>,
    #[serde(default)]
    pub password: Option<SecretFieldInput>,
    /// The version read, on an update.
    #[serde(default)]
    pub expected_version: Option<i32>,
}

fn default_tls() -> bool {
    true
}

impl ConnectorInput {
    /// `(field, token)` of what is wrong.
    pub fn violation(&self) -> Option<(&'static str, String)> {
        let long = |field, s: &str, max: usize| {
            (s.chars().count() > max).then(|| (field, format!("max_length:{max}")))
        };
        let required =
            |field, s: &str| s.trim().is_empty().then(|| (field, "required".to_string()));
        let named = required("name", &self.name).or_else(|| long("name", &self.name, NAME_MAX));
        // A webhook is called, it calls nothing: no host, port or user.
        if self.kind == ConnectorKind::Webhook {
            return named.or_else(|| self.webhook.clone().unwrap_or_default().violation());
        }
        named
            .or_else(|| required("host", &self.host))
            .or_else(|| long("host", &self.host, HOST_MAX))
            .or_else(|| {
                // A host only: no scheme, path, credentials or port to smuggle.
                let ok = self
                    .host
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':' | '_'));
                (!ok).then(|| ("host", "invalid".to_string()))
            })
            .or_else(|| {
                (!(1..=65535).contains(&self.port)).then(|| ("port", "invalid".to_string()))
            })
            .or_else(|| long("database", &self.database, FIELD_MAX))
            .or_else(|| long("username", &self.username, FIELD_MAX))
            .or_else(|| match self.kind {
                ConnectorKind::Postgres => required("database", &self.database)
                    .or_else(|| required("username", &self.username)),
                // Without TLS, sqlx encrypts the password with RSA (D792).
                ConnectorKind::Mysql => required("database", &self.database)
                    .or_else(|| required("username", &self.username))
                    .or_else(|| (!self.tls).then(|| ("tls", "required".to_string()))),
                ConnectorKind::Rest => match &self.rest {
                    None => Some(("rest", "required".to_string())),
                    Some(r) => r.violation(&self.username),
                },
                ConnectorKind::Webhook => None,
            })
    }
}

/// Outcome of a connection test: no data, and no text from the server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ConnectorTest {
    pub ok: bool,
    /// Machine code of the failure.
    pub code: Option<String>,
}

/// Hosts on private addresses an org may reach, set by the platform admin (Q-P2).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ConnectorHosts {
    pub hosts: Vec<String>,
}

/// The rule for a connection: private addresses stay refused unless the platform
/// admin listed the host, and what no policy may reach (loopback, cloud metadata)
/// stays refused even then.
pub fn egress_policy(
    listed: bool,
    process: crate::egress::EgressPolicy,
) -> crate::egress::EgressPolicy {
    use crate::egress::EgressPolicy::{Lan, Open, Public};
    match (process, listed) {
        (Open, _) => Open,
        (_, true) => Lan,
        (_, false) => Public,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::egress::EgressPolicy::{Lan, Open, Public};

    fn input() -> ConnectorInput {
        ConnectorInput {
            name: "erp".into(),
            kind: ConnectorKind::Postgres,
            host: "db.example.org".into(),
            port: 5432,
            database: "erp".into(),
            username: "reader".into(),
            tls: true,
            rest: None,
            webhook: None,
            password: None,
            expected_version: None,
        }
    }

    #[test]
    fn a_rest_connector_names_a_path_and_its_auth() {
        let rest = |r: RestSettings| ConnectorInput {
            kind: ConnectorKind::Rest,
            database: String::new(),
            username: String::new(),
            port: 443,
            rest: Some(r),
            ..input()
        };
        assert!(rest(RestSettings::default()).violation().is_none());
        let none = ConnectorInput {
            rest: None,
            ..rest(RestSettings::default())
        };
        assert_eq!(none.violation().unwrap().0, "rest");
        for bad in ["api", "/a?x=1", "/a#f", "//evil.org/x", "/a b", "/u@h"] {
            let c = rest(RestSettings {
                base_path: bad.into(),
                ..Default::default()
            });
            assert_eq!(c.violation().unwrap().0, "rest.base_path", "{bad}");
        }
        let header = rest(RestSettings {
            auth: RestAuth::Header,
            header_name: "X-Api-Key\r\nHost".into(),
            ..Default::default()
        });
        assert_eq!(header.violation().unwrap().0, "rest.header_name");
        let basic = rest(RestSettings {
            auth: RestAuth::Basic,
            ..Default::default()
        });
        assert_eq!(basic.violation().unwrap().0, "username");
        let oauth = ConnectorInput {
            username: "client".into(),
            ..rest(RestSettings {
                auth: RestAuth::Oauth2ClientCredentials,
                token_url: "http://idp.example.org/token".into(),
                ..Default::default()
            })
        };
        assert_eq!(oauth.violation().unwrap().0, "rest.token_url");
    }

    #[test]
    fn a_connector_names_a_host_nothing_else() {
        assert!(input().violation().is_none());
        for bad in [
            "postgres://u:p@db/x",
            "db/x",
            "u@db",
            "db example",
            "db;x",
            "db?sslmode=disable",
        ] {
            let c = ConnectorInput {
                host: bad.into(),
                ..input()
            };
            assert_eq!(c.violation().unwrap().0, "host", "{bad}");
        }
        let c = ConnectorInput { port: 0, ..input() };
        assert_eq!(c.violation().unwrap().0, "port");
        let c = ConnectorInput {
            name: " ".into(),
            ..input()
        };
        assert_eq!(c.violation().unwrap(), ("name", "required".to_string()));
    }

    #[test]
    fn a_private_address_needs_the_list() {
        assert_eq!(egress_policy(false, Lan), Public);
        assert_eq!(egress_policy(true, Lan), Lan);
        assert_eq!(egress_policy(true, Public), Lan);
        assert_eq!(egress_policy(false, Public), Public);
        assert_eq!(egress_policy(false, Open), Open);
    }

    #[test]
    fn a_webhook_checks_its_signature_headers() {
        let hook = ConnectorInput {
            kind: ConnectorKind::Webhook,
            host: String::new(),
            port: 0,
            database: String::new(),
            username: String::new(),
            ..input()
        };
        assert!(hook.violation().is_none());
        let bad = ConnectorInput {
            webhook: Some(WebhookSettings {
                signature_header: "X-Sig\r\nHost".into(),
                ..Default::default()
            }),
            ..hook.clone()
        };
        assert_eq!(bad.violation().unwrap().0, "webhook.signature_header");

        let w = WebhookSettings::default();
        assert_eq!(w.signature("sha256=0aff"), Some(vec![0x0a, 0xff]));
        assert_eq!(w.signature("0aff"), None);
        assert_eq!(w.signature("sha256=0af"), None);
        assert_eq!(w.signature("sha256=zz"), None);
        assert_eq!(w.signed_payload(Some("17"), b"{}"), b"{}");
        let ts = WebhookSettings {
            timestamp_header: "X-Timestamp".into(),
            ..w
        };
        assert_eq!(ts.signed_payload(Some("17"), b"{}"), b"17.{}");
        assert!(webhook_fresh("1000", 1200) && !webhook_fresh("1000", 1400));
        assert!(!webhook_fresh("soon", 1000));
    }

    #[test]
    fn a_mysql_connector_requires_tls() {
        let my = ConnectorInput {
            kind: ConnectorKind::Mysql,
            port: 3306,
            ..input()
        };
        assert!(my.violation().is_none());
        let clear = ConnectorInput { tls: false, ..my };
        assert_eq!(clear.violation().unwrap(), ("tls", "required".to_string()));
    }
}
