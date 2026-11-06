//! Runners of the worker fabric (worker-fabric.md section 14, agent-environments.md
//! lot E0): a runner joins with a single-use token, polls out, and runs each job in
//! a pod of its own namespace. This is the contract shared by the server and the
//! `pnex-runner` binary.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub const JOIN_TOKEN_PREFIX: &str = "pnjoin_";
pub const RUNNER_TOKEN_PREFIX: &str = "pnrun_";

pub const NAME_MAX: usize = 100;
pub const CAPABILITIES_MAX: usize = 32;
pub const CAPABILITY_MAX: usize = 64;
pub const COMMAND_ARGS_MAX: usize = 32;
pub const COMMAND_ARG_MAX: usize = 1024;
pub const TIMEOUT_MAX_SECS: u32 = 3600;
pub const IMAGE_MAX: usize = 512;

/// `{"name": "..."}`: the runner this token will enrol.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct JoinTokenCreate {
    pub name: String,
}

/// Shown once, stored hashed.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct JoinTokenCreated {
    pub token: String,
    #[cfg_attr(feature = "openapi", schema(format = "date-time"))]
    pub expires_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RunnerRegister {
    pub join_token: String,
    /// What this runner offers: `executor:sandbox`, `arch:amd64`, `isolation:pod`...
    pub capabilities: Vec<String>,
}

/// The token is shown once; the runner keeps it in its own namespace.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RunnerRegistered {
    #[cfg_attr(feature = "openapi", schema(format = "uuid"))]
    pub runner_id: String,
    pub token: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RunnerView {
    #[cfg_attr(feature = "openapi", schema(format = "uuid"))]
    pub id: String,
    pub name: String,
    pub capabilities: Vec<String>,
    /// Seen within the last two minutes.
    pub online: bool,
    #[cfg_attr(feature = "openapi", schema(format = "date-time"))]
    pub last_seen_at: Option<String>,
    #[cfg_attr(feature = "openapi", schema(format = "date-time"))]
    pub created_at: String,
}

/// What a job runs. The image is always pinned by digest (D703).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RunnerJobSpec {
    pub image: String,
    #[serde(default)]
    pub command: Vec<String>,
    pub timeout_secs: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "lowercase")]
pub enum RunnerJobKind {
    /// Runs one hardened pod and reports its exit, to prove a runner works.
    Probe,
    /// Builds a version of an environment and pushes its image (D700).
    #[serde(rename = "environment_build")]
    EnvironmentBuild,
    /// Scans the image of a build and brings back the findings (D720).
    #[serde(rename = "environment_scan")]
    EnvironmentScan,
    /// Runs an agent in its sandbox (D712, D714).
    Sandbox,
}

impl RunnerJobKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Probe => "probe",
            Self::EnvironmentBuild => "environment_build",
            Self::EnvironmentScan => "environment_scan",
            Self::Sandbox => "sandbox",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        [
            Self::Probe,
            Self::EnvironmentBuild,
            Self::EnvironmentScan,
            Self::Sandbox,
        ]
        .into_iter()
        .find(|k| k.as_str() == s)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RunnerJobCreate {
    pub kind: RunnerJobKind,
    pub spec: RunnerJobSpec,
    #[serde(default)]
    pub required_capabilities: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "lowercase")]
pub enum RunnerJobState {
    Queued,
    Running,
    Succeeded,
    Failed,
    /// No compatible runner took it in time: never moved elsewhere silently (D393).
    Expired,
}

impl RunnerJobState {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "queued" => Some(Self::Queued),
            "running" => Some(Self::Running),
            "succeeded" => Some(Self::Succeeded),
            "failed" => Some(Self::Failed),
            "expired" => Some(Self::Expired),
            _ => None,
        }
    }
}

/// How a job ended, as reported by its runner or decided by the reaper.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum RunnerJobOutcome {
    Succeeded,
    /// The workload exited with an error.
    Failed,
    /// The workload went past `timeout_secs`.
    Timeout,
    /// The image could not be pulled.
    ImagePull,
    /// The runner could not create or watch the workload.
    Internal,
    /// Too many runners lost it: the reaper gave up.
    LeaseExpired,
    /// Stopped by a person (D382).
    Stopped,
}

impl RunnerJobOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Timeout => "timeout",
            Self::ImagePull => "image_pull",
            Self::Internal => "internal",
            Self::LeaseExpired => "lease_expired",
            Self::Stopped => "stopped",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        [
            Self::Succeeded,
            Self::Failed,
            Self::Timeout,
            Self::ImagePull,
            Self::Internal,
            Self::LeaseExpired,
            Self::Stopped,
        ]
        .into_iter()
        .find(|o| o.as_str() == s)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RunnerJobView {
    #[cfg_attr(feature = "openapi", schema(format = "uuid"))]
    pub id: String,
    pub kind: RunnerJobKind,
    pub state: RunnerJobState,
    pub spec: RunnerJobSpec,
    pub required_capabilities: Vec<String>,
    #[cfg_attr(feature = "openapi", schema(format = "uuid"))]
    pub runner_id: Option<String>,
    pub attempts: i32,
    pub outcome: Option<RunnerJobOutcome>,
    pub exit_code: Option<i32>,
    #[cfg_attr(feature = "openapi", schema(format = "date-time"))]
    pub created_at: String,
    #[cfg_attr(feature = "openapi", schema(format = "date-time"))]
    pub started_at: Option<String>,
    #[cfg_attr(feature = "openapi", schema(format = "date-time"))]
    pub finished_at: Option<String>,
}

/// A job handed to a runner, which holds it for `lease_secs` from each heartbeat.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RunnerClaimedJob {
    #[cfg_attr(feature = "openapi", schema(format = "uuid"))]
    pub id: String,
    pub kind: RunnerJobKind,
    pub spec: RunnerJobSpec,
    pub lease_secs: u32,
    /// What to build, on an `environment_build` job. Holds credentials: delivered at
    /// the claim, never stored with the job.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build: Option<RunnerBuildInput>,
    /// How to read the image, on an `environment_scan` job. Holds a read secret, delivered
    /// the same way. Absent on a registry that needs no login.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scan: Option<RunnerScanInput>,
    /// What a sandbox needs, on a `sandbox` job. Holds the secrets of the run, delivered at
    /// the claim, never stored with the job.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sandbox: Option<RunnerSandboxInput>,
}

/// A repository cloned before the agent starts (D714).
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SandboxRepo {
    pub url: String,
    pub git_ref: String,
    /// Under `/workspace`.
    pub path: String,
    /// Read token, for the clone step only: the agent never gets it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
}

/// A test service beside the agent, on `localhost` (D717).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SandboxService {
    pub name: String,
    /// Pinned by digest.
    pub image: String,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// Command that succeeds once the service answers.
    pub ready_command: Vec<String>,
    /// Writable folders of its own, gone with the pod.
    #[serde(default)]
    pub data_paths: Vec<String>,
    #[serde(default)]
    pub run_as_user: Option<i64>,
}

/// One destination the sandbox may reach; everything else is refused.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SandboxEgress {
    pub cidr: String,
    pub port: u16,
}

/// What a sandbox pod needs, delivered at the claim (D705, D714, D717).
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RunnerSandboxInput {
    /// Pinned image of the clone step.
    pub init_image: String,
    #[serde(default)]
    pub repos: Vec<SandboxRepo>,
    #[serde(default)]
    pub services: Vec<SandboxService>,
    /// Runs before the agent in the image of the environment, without any secret.
    #[serde(default)]
    pub setup: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// For the agent only, through a Secret of this run: never in the pod spec.
    #[serde(default)]
    pub secrets: BTreeMap<String, String>,
    #[serde(default)]
    pub egress: Vec<SandboxEgress>,
}

impl std::fmt::Debug for RunnerSandboxInput {
    /// Names only: this holds the secrets of a run.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RunnerSandboxInput")
            .field("repos", &self.repos.len())
            .field("services", &self.services)
            .field("secrets", &self.secrets.keys().collect::<Vec<_>>())
            .finish_non_exhaustive()
    }
}

/// Login of a scan pod: read only (D703).
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RunnerScanInput {
    pub registry_host: String,
    pub username: String,
    pub secret: String,
}

impl std::fmt::Debug for RunnerScanInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RunnerScanInput")
            .field("registry_host", &self.registry_host)
            .finish_non_exhaustive()
    }
}

/// What a build pod needs (D701, D703).
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RunnerBuildInput {
    pub source: crate::environments::EnvironmentSource,
    /// `host/repository:tag`.
    pub target: String,
    /// Host of the registry, as credentials name it.
    pub registry_host: String,
    pub tls: bool,
    pub username: String,
    pub secret: String,
    /// Layer cache; absent when the store of this deployment cannot provide one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache: Option<RunnerBuildCache>,
}

/// Temporary credentials limited to the prefix of one environment (D730).
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RunnerBuildCache {
    pub endpoint: String,
    pub region: String,
    pub bucket: String,
    pub prefix: String,
    pub access_key: String,
    pub secret_key: String,
    pub session_token: String,
}

impl std::fmt::Debug for RunnerBuildInput {
    /// Never the secret, and not the source either: a log line has no use for it.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RunnerBuildInput")
            .field("target", &self.target)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RunnerJobFinish {
    pub outcome: RunnerJobOutcome,
    #[serde(default)]
    pub exit_code: Option<i32>,
    /// `sha256:<64 hex>` of the image a build pushed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_digest: Option<String>,
    /// What a scan found.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scan_report: Option<crate::environments::ScanReport>,
}

/// Bounds of the output a runner sends for one job (D719): its last lines, per container.
pub const LOG_CONTAINERS_MAX: usize = 32;
pub const LOG_CONTAINER_MAX_BYTES: usize = 256 * 1024;
pub const LOG_TAIL_LINES: usize = 4000;
/// A longer line is cut: a log line is for reading, not for carrying data.
pub const LOG_LINE_MAX_BYTES: usize = 16 * 1024;

/// The last lines of one container of a job, secrets masked by the runner.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RunnerContainerLog {
    pub container: String,
    pub text: String,
    /// Older lines were left out.
    #[serde(default)]
    pub truncated: bool,
}

/// What a runner sends of the output of a job, before finishing it (D719).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RunnerJobLogs {
    pub containers: Vec<RunnerContainerLog>,
}

impl RunnerJobLogs {
    /// `(field, token)` of what is past its bounds: the server never trusts the runner's.
    pub fn violation(&self) -> Option<(&'static str, String)> {
        if self.containers.len() > LOG_CONTAINERS_MAX {
            return Some(("containers", format!("max_items:{LOG_CONTAINERS_MAX}")));
        }
        if self
            .containers
            .iter()
            .any(|c| c.container.is_empty() || c.container.len() > 63)
        {
            return Some(("container", "max_length:63".into()));
        }
        // Masking may lengthen a little what the runner cut at the bound.
        if self
            .containers
            .iter()
            .any(|c| c.text.len() > 2 * LOG_CONTAINER_MAX_BYTES)
        {
            return Some((
                "text",
                format!("max_length:{}", 2 * LOG_CONTAINER_MAX_BYTES),
            ));
        }
        None
    }
}

/// One line of the journal of a job, oldest first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RunnerJobLogLine {
    pub container: String,
    pub line: String,
}

impl RunnerClaimedJob {
    /// Every secret delivered with this job, to mask out of its output.
    pub fn secret_values(&self) -> Vec<String> {
        let mut out = Vec::new();
        if let Some(build) = &self.build {
            out.push(build.secret.clone());
            if let Some(cache) = &build.cache {
                out.extend([cache.secret_key.clone(), cache.session_token.clone()]);
            }
        }
        if let Some(scan) = &self.scan {
            out.push(scan.secret.clone());
        }
        if let Some(sandbox) = &self.sandbox {
            out.extend(sandbox.secrets.values().cloned());
            out.extend(sandbox.repos.iter().filter_map(|r| r.token.clone()));
        }
        out
    }
}

/// `text` with each secret, and each word shaped like one, replaced by
/// [`crate::environments::MASK`]. Values under 8 characters are left: masking `a` would
/// blank the log and hide nothing.
pub fn mask_secrets(text: &str, secrets: &[String]) -> String {
    let mut secrets: Vec<&String> = secrets.iter().filter(|s| s.len() >= 8).collect();
    // The longest first: a secret that holds another one goes whole.
    secrets.sort_by_key(|s| std::cmp::Reverse(s.len()));
    let mut out = text.to_string();
    for secret in secrets {
        out = out.replace(secret.as_str(), crate::environments::MASK);
    }
    crate::environments::redact_tokens(&out)
}

/// Line between the SBOM and the report in the output of a scan pod.
pub const SCAN_REPORT_MARKER: &str = "---pnex-scan-report---";
/// The SBOM first, and the findings from that SBOM: what a scan reports is what the stored
/// document says, the way a later scan can read it without the image (D720, D728).
pub const SCAN_SCRIPT: &str = "set -e
trivy image --format cyclonedx --output /tmp/sbom.cdx.json --quiet --scanners vuln \"$1\"
cat /tmp/sbom.cdx.json
echo
echo ---pnex-scan-report---
trivy sbom --format json --list-all-pkgs --quiet /tmp/sbom.cdx.json
";

/// The command of a scan pod for this image.
pub fn scan_command(image: &str) -> Vec<String> {
    ["sh", "-c", SCAN_SCRIPT, "sh", image]
        .into_iter()
        .map(str::to_string)
        .collect()
}

pub fn is_scan_command(command: &[String]) -> bool {
    command.get(2).is_some_and(|script| script == SCAN_SCRIPT)
}

/// `sha256:<64 hex>`.
pub fn digest_is_valid(digest: &str) -> bool {
    digest.strip_prefix("sha256:").is_some_and(|hex| {
        hex.len() == 64
            && hex
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}

/// `registry/repo@sha256:<64 hex>`: a tag is never a reference to run (D703).
pub fn image_is_pinned(image: &str) -> bool {
    if image.len() > IMAGE_MAX || image.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return false;
    }
    let Some((name, digest)) = image.rsplit_once("@sha256:") else {
        return false;
    };
    !name.is_empty()
        && !name.starts_with('-')
        && digest.len() == 64
        && digest
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// `arch:amd64`, `vram>=8g`, `isolation:pod`: lowercase tokens, bounded.
pub fn capabilities_are_valid(capabilities: &[String]) -> bool {
    capabilities.len() <= CAPABILITIES_MAX
        && capabilities.iter().all(|c| {
            !c.is_empty()
                && c.len() <= CAPABILITY_MAX
                && c.bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b":._>=-".contains(&b))
        })
}

/// The first invalid field of a job spec, with its error token.
pub fn spec_error(spec: &RunnerJobSpec) -> Option<(&'static str, &'static str)> {
    if !image_is_pinned(&spec.image) {
        return Some(("spec.image", "unpinned"));
    }
    if spec.command.len() > COMMAND_ARGS_MAX
        || spec
            .command
            .iter()
            .any(|a| a.len() > COMMAND_ARG_MAX || a.contains('\0'))
    {
        return Some(("spec.command", "invalid"));
    }
    if spec.timeout_secs == 0 || spec.timeout_secs > TIMEOUT_MAX_SECS {
        return Some(("spec.timeout_secs", "invalid"));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const DIGEST: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    #[test]
    fn only_a_digest_pins_an_image() {
        assert!(image_is_pinned(&format!("cgr.dev/x/y@sha256:{DIGEST}")));
        assert!(image_is_pinned(&format!(
            "registry:5000/a/b:1.2@sha256:{DIGEST}"
        )));
        for bad in [
            "cgr.dev/x/y:latest".to_string(),
            "cgr.dev/x/y".to_string(),
            format!("@sha256:{DIGEST}"),
            format!("-x@sha256:{DIGEST}"),
            format!("a b@sha256:{DIGEST}"),
            format!("x@sha256:{}", DIGEST.to_uppercase()),
            format!("x@sha256:{}", &DIGEST[1..]),
            format!("{}@sha256:{DIGEST}", "a".repeat(IMAGE_MAX)),
        ] {
            assert!(!image_is_pinned(&bad), "{bad}");
        }
    }

    #[test]
    fn delivered_secrets_are_masked_out_of_a_log() {
        let job = RunnerClaimedJob {
            id: "j".into(),
            kind: RunnerJobKind::Sandbox,
            spec: RunnerJobSpec {
                image: format!("x@sha256:{DIGEST}"),
                command: vec![],
                timeout_secs: 60,
            },
            lease_secs: 120,
            build: None,
            scan: None,
            sandbox: Some(RunnerSandboxInput {
                init_image: String::new(),
                repos: vec![SandboxRepo {
                    url: "https://git.example/r".into(),
                    git_ref: "main".into(),
                    path: "r".into(),
                    token: Some("clone-token-value".into()),
                }],
                services: vec![],
                setup: vec![],
                env: BTreeMap::new(),
                secrets: BTreeMap::from([
                    ("RUN".to_string(), "run-secret-value".to_string()),
                    ("SHORT".to_string(), "abc".to_string()),
                ]),
                egress: vec![],
            }),
        };
        let secrets = job.secret_values();
        let out = mask_secrets(
            "run-secret-value clone-token-value abc ghp_000000000000000000000000000000",
            &secrets,
        );
        let m = crate::environments::MASK;
        assert_eq!(out, format!("{m} {m} abc {m}"));

        let mut logs = RunnerJobLogs::default();
        assert_eq!(logs.violation(), None);
        logs.containers = vec![
            RunnerContainerLog {
                container: "job".into(),
                text: String::new(),
                truncated: false,
            };
            LOG_CONTAINERS_MAX + 1
        ];
        assert_eq!(logs.violation().map(|v| v.0), Some("containers"));
    }

    #[test]
    fn capabilities_are_bounded_tokens() {
        let ok = |s: &str| capabilities_are_valid(&[s.to_string()]);
        assert!(ok("arch:amd64") && ok("vram>=8g") && ok("gpu.vulkan"));
        assert!(!ok("") && !ok("Arch") && !ok("a b") && !ok(&"a".repeat(65)));
        assert!(!capabilities_are_valid(&vec!["a".to_string(); 33]));
    }

    #[test]
    fn spec_errors_name_the_field() {
        let good = RunnerJobSpec {
            image: format!("x@sha256:{DIGEST}"),
            command: vec!["true".into()],
            timeout_secs: 60,
        };
        assert_eq!(spec_error(&good), None);
        let mut s = good.clone();
        s.image = "x:latest".into();
        assert_eq!(spec_error(&s), Some(("spec.image", "unpinned")));
        let mut s = good.clone();
        s.timeout_secs = 0;
        assert_eq!(spec_error(&s), Some(("spec.timeout_secs", "invalid")));
        let mut s = good.clone();
        s.timeout_secs = TIMEOUT_MAX_SECS + 1;
        assert_eq!(spec_error(&s), Some(("spec.timeout_secs", "invalid")));
        let mut s = good;
        s.command = vec!["a\0b".into()];
        assert_eq!(spec_error(&s), Some(("spec.command", "invalid")));
    }

    #[test]
    fn outcomes_round_trip() {
        for o in [
            RunnerJobOutcome::Succeeded,
            RunnerJobOutcome::ImagePull,
            RunnerJobOutcome::LeaseExpired,
        ] {
            assert_eq!(RunnerJobOutcome::parse(o.as_str()), Some(o));
        }
        assert_eq!(RunnerJobOutcome::parse("nope"), None);
    }
}
