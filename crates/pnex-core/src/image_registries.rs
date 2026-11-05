//! Image registries (agent-environments.md D703): saved access to an OCI registry,
//! with distinct read and write secrets in the vault. DTOs and checks, wasm-safe.

use serde::{Deserialize, Serialize};

use crate::SecretFieldInput;

pub const NAME_MAX: usize = 100;
/// A DNS name and a port.
pub const HOST_MAX: usize = 259;
pub const REPOSITORY_MAX: usize = 255;
pub const USERNAME_MAX: usize = 255;

/// A registry as read: never its secrets (R4, R16).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ImageRegistry {
    #[cfg_attr(feature = "openapi", schema(format = "uuid"))]
    pub id: String,
    pub name: String,
    /// `registry.example.org` or `registry.example.org:5000`.
    pub host: String,
    pub repository: String,
    /// Whether HTTPS is required.
    pub tls: bool,
    pub read_username: Option<String>,
    pub has_read_secret: bool,
    pub write_username: Option<String>,
    pub has_write_secret: bool,
    pub version: i32,
    #[cfg_attr(feature = "openapi", schema(format = "date-time"))]
    pub created_at: String,
    #[cfg_attr(feature = "openapi", schema(format = "date-time"))]
    pub updated_at: String,
}

/// A registry as written. An absent secret keeps the current one.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ImageRegistryInput {
    pub name: String,
    pub host: String,
    pub repository: String,
    #[serde(default = "default_tls")]
    pub tls: bool,
    #[serde(default)]
    pub read_username: Option<String>,
    #[serde(default)]
    pub read_secret: Option<SecretFieldInput>,
    #[serde(default)]
    pub write_username: Option<String>,
    #[serde(default)]
    pub write_secret: Option<SecretFieldInput>,
    /// The version read, on an update.
    #[serde(default)]
    pub expected_version: Option<i32>,
}

fn default_tls() -> bool {
    true
}

/// `name` or `name:port`. No scheme, path or credentials to smuggle.
fn host_is_valid(host: &str) -> bool {
    let name = match host.rsplit_once(':') {
        Some((name, port)) => {
            if !port.parse::<u16>().is_ok_and(|p| p > 0) {
                return false;
            }
            name
        }
        None => host,
    };
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-'))
}

/// Lowercase path components as the distribution spec names them. Each one starts and
/// ends with a letter or digit, so `..` cannot walk out of `/v2/`.
fn repository_is_valid(repository: &str) -> bool {
    let edge = |c: char| c.is_ascii_lowercase() || c.is_ascii_digit();
    repository.split('/').all(|part| {
        part.starts_with(edge)
            && part.ends_with(edge)
            && part
                .chars()
                .all(|c| edge(c) || matches!(c, '.' | '_' | '-'))
    })
}

impl ImageRegistryInput {
    /// `(field, token)` of what is wrong.
    pub fn violation(&self) -> Option<(&'static str, String)> {
        let long = |field, s: &str, max: usize| {
            (s.chars().count() > max).then(|| (field, format!("max_length:{max}")))
        };
        let required =
            |field, s: &str| s.trim().is_empty().then(|| (field, "required".to_string()));
        // A colon would end the user of a basic authorization.
        let username = |field, u: &Option<String>| {
            let u = u.as_deref()?;
            long(field, u, USERNAME_MAX).or_else(|| {
                (u.contains(':') || u.chars().any(char::is_control))
                    .then(|| (field, "invalid".to_string()))
            })
        };
        required("name", &self.name)
            .or_else(|| long("name", &self.name, NAME_MAX))
            .or_else(|| required("host", &self.host))
            .or_else(|| long("host", &self.host, HOST_MAX))
            .or_else(|| (!host_is_valid(self.host.trim())).then(|| ("host", "invalid".to_string())))
            .or_else(|| required("repository", &self.repository))
            .or_else(|| long("repository", &self.repository, REPOSITORY_MAX))
            .or_else(|| {
                (!repository_is_valid(self.repository.trim()))
                    .then(|| ("repository", "invalid".to_string()))
            })
            .or_else(|| username("read_username", &self.read_username))
            .or_else(|| username("write_username", &self.write_username))
    }
}

/// Outcome of one check: no text from the outside server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ImageRegistryCheck {
    pub ok: bool,
    /// Machine code of the failure.
    pub code: Option<String>,
}

/// Outcome of a registry test.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ImageRegistryTest {
    /// Pull right on the repository.
    pub read: ImageRegistryCheck,
    /// Push right on the repository; absent without a write secret.
    pub write: Option<ImageRegistryCheck>,
}

/// `realm` and `service` of a `WWW-Authenticate: Bearer` challenge.
pub fn bearer_challenge(header: &str) -> Option<(String, Option<String>)> {
    let (scheme, params) = header.trim().split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("bearer") {
        return None;
    }
    let (mut realm, mut service) = (None, None);
    let mut rest = params.trim();
    while let Some((key, after)) = rest.split_once('=') {
        let after = after.strip_prefix('"')?;
        let (value, after) = after.split_once('"')?;
        match key.trim().to_ascii_lowercase().as_str() {
            "realm" => realm = Some(value.to_string()),
            "service" => service = Some(value.to_string()),
            _ => {}
        }
        rest = after.trim_start_matches([',', ' ']);
    }
    Some((realm?, service))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input() -> ImageRegistryInput {
        ImageRegistryInput {
            name: "agents".into(),
            host: "registry.example.org:5000".into(),
            repository: "team/agent-base".into(),
            tls: true,
            read_username: Some("reader".into()),
            read_secret: None,
            write_username: None,
            write_secret: None,
            expected_version: None,
        }
    }

    #[test]
    fn a_registry_names_a_host_and_a_repository_nothing_else() {
        assert!(input().violation().is_none());
        for bad in [
            "https://r.example",
            "r.example/x",
            "u@r.example",
            "r.example:0",
            "r.example:x",
            "r example",
            ":5000",
        ] {
            let r = ImageRegistryInput {
                host: bad.into(),
                ..input()
            };
            assert_eq!(r.violation().unwrap().0, "host", "{bad}");
        }
        for bad in [
            "../other", "a/../b", "a//b", "/a", "a/", "A/b", "a?n=1", "a b", "a-",
        ] {
            let r = ImageRegistryInput {
                repository: bad.into(),
                ..input()
            };
            assert_eq!(r.violation().unwrap().0, "repository", "{bad}");
        }
        let r = ImageRegistryInput {
            read_username: Some("a:b".into()),
            ..input()
        };
        assert_eq!(r.violation().unwrap().0, "read_username");
    }

    #[test]
    fn the_challenge_gives_realm_and_service() {
        assert_eq!(
            bearer_challenge(
                r#"Bearer realm="https://auth.example/token",service="reg,one",scope="a:b:pull""#
            ),
            Some((
                "https://auth.example/token".to_string(),
                Some("reg,one".to_string())
            ))
        );
        assert_eq!(
            bearer_challenge(r#"bearer realm="https://a""#),
            Some(("https://a".to_string(), None))
        );
        assert_eq!(bearer_challenge(r#"Basic realm="x""#), None);
        assert_eq!(bearer_challenge(r#"Bearer service="x""#), None);
        assert_eq!(bearer_challenge(r#"Bearer realm="unclosed"#), None);
    }
}
