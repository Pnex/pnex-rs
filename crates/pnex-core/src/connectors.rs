//! Connectors (data-pipelines.md D352): saved access to an outside database, the
//! password in the vault. Only the server uses one: neither the worker nor the flow
//! runtime ever sees a connector or its secret. DTOs and checks, wasm-safe.

use serde::{Deserialize, Serialize};

use crate::SecretFieldInput;

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
}

impl ConnectorKind {
    pub fn wire(self) -> &'static str {
        match self {
            Self::Postgres => "postgres",
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
    pub host: String,
    pub port: i32,
    pub database: String,
    pub username: String,
    #[serde(default = "default_tls")]
    pub tls: bool,
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
        required("name", &self.name)
            .or_else(|| long("name", &self.name, NAME_MAX))
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
            .or_else(|| required("database", &self.database))
            .or_else(|| long("database", &self.database, FIELD_MAX))
            .or_else(|| required("username", &self.username))
            .or_else(|| long("username", &self.username, FIELD_MAX))
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
            password: None,
            expected_version: None,
        }
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
}
