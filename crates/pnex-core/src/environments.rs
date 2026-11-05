//! Agent environments (agent-environments.md D702, D704): a Dockerfile and its text
//! context, versioned in the database. DTOs and checks, wasm-safe.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub const NAME_MAX: usize = 100;
pub const DESCRIPTION_MAX: usize = 2000;
/// Dockerfile, context files and build arguments together (D702).
/// Under the 1 MiB of a ConfigMap, which carries it to the build pod.
pub const SOURCE_MAX_BYTES: usize = 1_000_000;
pub const FILES_MAX: usize = 256;
pub const FILE_PATH_MAX: usize = 255;
pub const BUILD_ARGS_MAX: usize = 64;
pub const BUILD_ARG_NAME_MAX: usize = 128;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Environment {
    #[cfg_attr(feature = "openapi", schema(format = "uuid"))]
    pub id: String,
    pub name: String,
    pub description: String,
    /// The image registry its images go to.
    #[cfg_attr(feature = "openapi", schema(format = "uuid"))]
    pub registry_id: String,
    /// Number of its latest version; absent before the first one.
    pub latest_version: Option<i32>,
    /// The build its runs execute, chosen by a person.
    #[cfg_attr(feature = "openapi", schema(format = "uuid"))]
    pub active_build_id: Option<String>,
    /// `host/repository@sha256:...` of that build.
    pub active_image: Option<String>,
    pub version: i32,
    #[cfg_attr(feature = "openapi", schema(format = "date-time"))]
    pub created_at: String,
    #[cfg_attr(feature = "openapi", schema(format = "date-time"))]
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct EnvironmentInput {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[cfg_attr(feature = "openapi", schema(format = "uuid"))]
    pub registry_id: String,
    /// The version read, on an update.
    #[serde(default)]
    pub expected_version: Option<i32>,
}

impl EnvironmentInput {
    /// `(field, token)` of what is wrong.
    pub fn violation(&self) -> Option<(&'static str, String)> {
        if self.name.trim().is_empty() {
            return Some(("name", "required".into()));
        }
        if self.name.chars().count() > NAME_MAX {
            return Some(("name", format!("max_length:{NAME_MAX}")));
        }
        if self.description.chars().count() > DESCRIPTION_MAX {
            return Some(("description", format!("max_length:{DESCRIPTION_MAX}")));
        }
        None
    }
}

/// What a version builds from. Immutable once saved: a change is a new version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct EnvironmentSource {
    pub dockerfile: String,
    /// Text files of the build context, by relative path. A binary comes from the base
    /// image or from a download pinned by checksum in a `RUN` step.
    #[serde(default)]
    pub files: BTreeMap<String, String>,
    #[serde(default)]
    pub build_args: BTreeMap<String, String>,
}

/// A new version, on the latest one that was read.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct EnvironmentVersionInput {
    #[serde(flatten)]
    pub source: EnvironmentSource,
    /// The latest version read; absent for the first one.
    #[serde(default)]
    pub expected_latest: Option<i32>,
}

/// A version in a list, without its source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct EnvironmentVersionSummary {
    pub number: i32,
    /// SHA-256 of the source: what a build and a run refer to.
    pub fingerprint: String,
    #[cfg_attr(feature = "openapi", schema(format = "date-time"))]
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct EnvironmentVersion {
    #[serde(flatten)]
    pub summary: EnvironmentVersionSummary,
    #[serde(flatten)]
    pub source: EnvironmentSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "lowercase")]
pub enum EnvironmentBuildState {
    /// Waiting for a runner, or running.
    Building,
    Ready,
    Failed,
}

/// Makes a ready build the active one, on the version of the environment that was read.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct EnvironmentActivate {
    pub expected_version: i32,
}

/// One build of a version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct EnvironmentBuild {
    #[cfg_attr(feature = "openapi", schema(format = "uuid"))]
    pub id: String,
    pub number: i32,
    pub state: EnvironmentBuildState,
    /// `host/repository@sha256:...`, once ready: what a run executes, never a tag (D703).
    pub image: Option<String>,
    /// How its job ended, when it failed.
    pub outcome: Option<String>,
    #[cfg_attr(feature = "openapi", schema(format = "uuid"))]
    pub job_id: String,
    #[cfg_attr(feature = "openapi", schema(format = "date-time"))]
    pub created_at: String,
    #[cfg_attr(feature = "openapi", schema(format = "date-time"))]
    pub finished_at: Option<String>,
    /// Its latest scan, without the findings.
    pub scan_state: Option<EnvironmentScanState>,
    pub scan_counts: Option<ScanCounts>,
}

/// A known vulnerability in an image. Our own format, whatever tool found it (D720).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ScanFinding {
    pub id: String,
    /// Package URL of the component it touches.
    #[serde(default)]
    pub purl: Option<String>,
    pub package: String,
    pub installed: String,
    /// Version that fixes it, when one exists: what a rebuild can pick up.
    pub fixed: Option<String>,
    /// `critical`, `high`, `medium`, `low` or `unknown`.
    pub severity: String,
}

pub const SCAN_FINDINGS_MAX: usize = 5000;
pub const SCAN_COMPONENTS_MAX: usize = 20_000;
pub const SCAN_FIELD_MAX: usize = 256;
pub const SCAN_PURL_MAX: usize = 1024;
/// What an SBOM lists, by Package URL.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ScanComponent {
    pub purl: String,
    pub name: String,
    pub version: String,
}

const SEVERITIES: [&str; 5] = ["critical", "high", "medium", "low", "unknown"];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ScanReport {
    pub components: Vec<ScanComponent>,
    pub findings: Vec<ScanFinding>,
}

impl ScanReport {
    /// A runner only relays what a pod wrote: nothing out of bounds gets stored.
    pub fn is_valid(&self) -> bool {
        let short = |s: &str| s.len() <= SCAN_FIELD_MAX;
        let purl = |s: &str| s.len() <= SCAN_PURL_MAX;
        self.findings.len() <= SCAN_FINDINGS_MAX
            && self.components.len() <= SCAN_COMPONENTS_MAX
            && self
                .components
                .iter()
                .all(|c| purl(&c.purl) && short(&c.name) && short(&c.version))
            && self.findings.iter().all(|f| {
                short(&f.id)
                    && short(&f.package)
                    && short(&f.installed)
                    && f.fixed.as_deref().is_none_or(short)
                    && f.purl.as_deref().is_none_or(purl)
                    && SEVERITIES.contains(&f.severity.as_str())
            })
    }

    pub fn summary(&self) -> ScanCounts {
        let count =
            |level: &str| self.findings.iter().filter(|f| f.severity == level).count() as u32;
        ScanCounts {
            critical: count("critical"),
            high: count("high"),
            medium: count("medium"),
            low: count("low"),
            unknown: count("unknown"),
            fixable: self.findings.iter().filter(|f| f.fixed.is_some()).count() as u32,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ScanCounts {
    pub critical: u32,
    pub high: u32,
    pub medium: u32,
    pub low: u32,
    pub unknown: u32,
    /// Findings with a known fixed version.
    pub fixable: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "lowercase")]
pub enum EnvironmentScanState {
    /// Waiting for a runner, or running.
    Scanning,
    Done,
    Failed,
}

/// One scan in the follow-up of an environment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ScanHistoryEntry {
    #[cfg_attr(feature = "openapi", schema(format = "date-time"))]
    pub at: String,
    pub image: String,
    pub version: i32,
    pub components: u32,
    pub counts: ScanCounts,
}

/// The latest scan of a build.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct EnvironmentScan {
    pub state: EnvironmentScanState,
    pub counts: Option<ScanCounts>,
    pub report: Option<ScanReport>,
    #[cfg_attr(feature = "openapi", schema(format = "date-time"))]
    pub created_at: String,
}

/// Relative, inside the context, and not the Dockerfile itself.
fn path_is_valid(path: &str) -> bool {
    !path.is_empty()
        && path != "Dockerfile"
        && path.chars().count() <= FILE_PATH_MAX
        && !path.starts_with('/')
        && !path.chars().any(|c| c.is_control() || c == '\\')
        && path
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..")
}

fn build_arg_is_valid(name: &str) -> bool {
    name.len() <= BUILD_ARG_NAME_MAX
        && name.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Shapes of well-known credentials: prefix, and the least token characters after it.
/// shortcut: known shapes only, so a password in plain words passes; add an entropy
/// check if a real case shows the need.
const TOKEN_SHAPES: &[(&str, usize)] = &[
    ("AKIA", 16),
    ("ASIA", 16),
    ("ghp_", 30),
    ("gho_", 30),
    ("ghs_", 30),
    ("github_pat_", 30),
    ("glpat-", 16),
    ("xoxb-", 16),
    ("xoxp-", 16),
    ("sk-ant-", 16),
    ("sk-", 32),
    ("hf_", 30),
    ("pnrun_", 16),
    ("pnjoin_", 16),
];

/// Whether `text` holds what looks like a credential.
pub fn holds_a_secret(text: &str) -> bool {
    if text.contains("PRIVATE KEY-----") {
        return true;
    }
    let token_char = |c: char| c.is_ascii_alphanumeric() || matches!(c, '_' | '-');
    TOKEN_SHAPES.iter().any(|(prefix, least)| {
        text.match_indices(prefix).any(|(at, _)| {
            // Inside a longer word, this is no prefix: `task-...` is not `sk-...`.
            let starts_a_token = !text[..at].chars().next_back().is_some_and(token_char);
            let after = &text[at + prefix.len()..];
            starts_a_token && after.chars().take_while(|c| token_char(*c)).count() >= *least
        })
    })
}

impl EnvironmentSource {
    /// `(field, token)` of what is wrong.
    pub fn violation(&self) -> Option<(&'static str, String)> {
        if self.dockerfile.trim().is_empty() {
            return Some(("dockerfile", "required".into()));
        }
        if self.files.len() > FILES_MAX {
            return Some(("files", format!("max_items:{FILES_MAX}")));
        }
        if self.files.keys().any(|path| !path_is_valid(path)) {
            return Some(("files", "invalid".into()));
        }
        if self.build_args.len() > BUILD_ARGS_MAX {
            return Some(("build_args", format!("max_items:{BUILD_ARGS_MAX}")));
        }
        if self.build_args.keys().any(|name| !build_arg_is_valid(name)) {
            return Some(("build_args", "invalid".into()));
        }
        let size = |map: &BTreeMap<String, String>| -> usize {
            map.iter().map(|(k, v)| k.len() + v.len()).sum()
        };
        if self.dockerfile.len() + size(&self.files) + size(&self.build_args) > SOURCE_MAX_BYTES {
            return Some(("dockerfile", format!("max_length:{SOURCE_MAX_BYTES}")));
        }
        None
    }

    /// Where a credential was found: `dockerfile`, or the name of the file or build
    /// argument. Never the value.
    pub fn secret_at(&self) -> Option<String> {
        if holds_a_secret(&self.dockerfile) {
            return Some("dockerfile".into());
        }
        self.files
            .iter()
            .chain(&self.build_args)
            .find(|(_, value)| holds_a_secret(value))
            .map(|(name, _)| name.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source() -> EnvironmentSource {
        EnvironmentSource {
            dockerfile: "FROM cgr.dev/chainguard/wolfi-base\nCOPY setup.sh /\n".into(),
            files: BTreeMap::from([("setup.sh".to_string(), "echo ok".to_string())]),
            build_args: BTreeMap::from([("VERSION".to_string(), "1".to_string())]),
        }
    }

    #[test]
    fn the_context_stays_inside_itself() {
        assert!(source().violation().is_none());
        for bad in [
            "/etc/passwd",
            "../x",
            "a/../../x",
            "a//b",
            "./a",
            "a\\b",
            "",
            "Dockerfile",
        ] {
            let mut s = source();
            s.files.insert(bad.into(), "x".into());
            assert_eq!(s.violation().unwrap().0, "files", "{bad:?}");
        }
        let mut s = source();
        s.files.insert("scripts/.config/ok.sh".into(), "x".into());
        assert!(s.violation().is_none());
        for bad in ["1A", "A-B", "A B", ""] {
            let mut s = source();
            s.build_args.insert(bad.into(), "x".into());
            assert_eq!(s.violation().unwrap().0, "build_args", "{bad:?}");
        }
    }

    #[test]
    fn a_report_is_bounded_and_counted() {
        let finding = |severity: &str, fixed: Option<&str>| ScanFinding {
            id: "CVE-1".into(),
            purl: None,
            package: "p".into(),
            installed: "1".into(),
            fixed: fixed.map(str::to_string),
            severity: severity.into(),
        };
        let report = ScanReport {
            components: vec![ScanComponent {
                purl: "pkg:apk/a@1".into(),
                name: "a".into(),
                version: "1".into(),
            }],
            findings: vec![finding("critical", Some("2")), finding("low", None)],
        };
        assert!(report.is_valid());
        let counts = report.summary();
        assert_eq!((counts.critical, counts.low, counts.fixable), (1, 1, 1));
        for bad in [
            vec![finding("CRITICAL", None)],
            vec![finding("nope", None)],
            vec![ScanFinding {
                id: "x".repeat(SCAN_FIELD_MAX + 1),
                ..finding("low", None)
            }],
            vec![finding("low", None); SCAN_FINDINGS_MAX + 1],
        ] {
            let r = ScanReport {
                components: vec![],
                findings: bad,
            };
            assert!(!r.is_valid());
        }
    }

    #[test]
    fn the_source_size_is_capped_as_a_whole() {
        let mut s = source();
        s.files.insert("big".into(), "x".repeat(SOURCE_MAX_BYTES));
        assert_eq!(
            s.violation().unwrap(),
            ("dockerfile", format!("max_length:{SOURCE_MAX_BYTES}"))
        );
    }

    #[test]
    fn known_credentials_are_found_and_named_without_their_value() {
        let token = format!("ghp_{}", "a".repeat(36));
        for text in [
            token.clone(),
            format!("ENV TOKEN={token}"),
            format!("AKIA{}", "A".repeat(16)),
            format!("x sk-ant-{} y", "b".repeat(40)),
            "-----BEGIN OPENSSH PRIVATE KEY-----".to_string(),
            format!("pnrun_{}", "c".repeat(32)),
        ] {
            assert!(holds_a_secret(&text), "{text}");
        }
        for text in [
            "RUN task-runner --flag",
            "ENV NAME=ghp_short",
            "RUN apt-get install gitlab-runner",
            "disk-partition-with-a-very-long-name-that-goes-on-and-on",
            "RUN curl -fsSL https://example.org/AKIA",
        ] {
            assert!(!holds_a_secret(text), "{text}");
        }
        let mut s = source();
        assert_eq!(s.secret_at(), None);
        s.build_args.insert("TOKEN".into(), token.clone());
        assert_eq!(s.secret_at().as_deref(), Some("TOKEN"));
        s.files.insert("env.sh".into(), token.clone());
        assert_eq!(s.secret_at().as_deref(), Some("env.sh"));
        s.dockerfile = format!("FROM x\nENV T={token}");
        assert_eq!(s.secret_at().as_deref(), Some("dockerfile"));
    }
}
