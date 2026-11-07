//! Runs of hosted agents (autonomous-agents.md D382, agent-environments.md D731, D732): one
//! sandbox job on the active image of an environment, a run token that opens the LLM
//! gateway only while the job runs, and usage counted by the gateway.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::runners::{RunnerJobOutcome, RunnerJobState};

pub const RUN_TOKEN_PREFIX: &str = "pnagr_";
pub const LLM_CALLS_MAX_DEFAULT: i32 = 200;
pub const LLM_CALLS_MAX: i32 = 10_000;
pub const ENV_MAX: usize = 64;
pub const ENV_VALUE_MAX: usize = 4096;
/// Largest LLM request body the gateway forwards.
pub const GATEWAY_BODY_MAX: usize = 8 * 1024 * 1024;

/// Opens the artifact routes of one run, for its transfer containers only (D729, D744).
pub const ARTIFACT_TOKEN_PREFIX: &str = "pnart_";
/// Declared inputs, and declared outputs, of one run.
pub const ARTIFACTS_MAX: usize = 16;
pub const ARTIFACT_NAME_MAX: usize = 63;
/// Header of an output upload: how its bytes read.
pub const ARTIFACT_FORM_HEADER: &str = "x-pnex-artifact-form";
/// Where the agent finds its inputs and leaves its outputs, one entry per name.
pub const ARTIFACTS_IN_DIR: &str = "/artifacts/in";
pub const ARTIFACTS_OUT_DIR: &str = "/artifacts/out";

/// Variables the run gets from PneX: the agent may not set them itself.
pub const RESERVED_ENV: [&str; 8] = [
    "PNEX_ARTIFACTS_IN",
    "PNEX_ARTIFACTS_OUT",
    "PNEX_RUN_TOKEN",
    "PNEX_GATEWAY_URL",
    "ANTHROPIC_BASE_URL",
    "ANTHROPIC_API_KEY",
    "OPENAI_BASE_URL",
    "OPENAI_API_KEY",
];

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct AgentRunCreate {
    /// The run uses the active build of this environment.
    #[cfg_attr(feature = "openapi", schema(format = "uuid"))]
    pub environment_id: String,
    /// Command of the agent; empty runs the entrypoint of the image.
    #[serde(default)]
    pub command: Vec<String>,
    /// Plain variables; a value that looks like a secret is refused (D705).
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    pub timeout_secs: u32,
    /// Calls through the gateway this run may make; [`LLM_CALLS_MAX_DEFAULT`] when absent.
    #[serde(default)]
    pub llm_calls_max: Option<i32>,
    /// Files given to the agent, each at `/artifacts/in/<name>` before it starts.
    #[serde(default)]
    pub inputs: Vec<ArtifactInput>,
    /// Files the agent leaves at `/artifacts/out/<name>`, collected even when it fails.
    #[serde(default)]
    pub outputs: Vec<ArtifactOutput>,
    /// Cloned into `/workspace/<path>` before anything else runs (D714, D758).
    #[serde(default)]
    pub repos: Vec<RunRepo>,
    /// Test services beside the agent, on `localhost` (D717, D758).
    #[serde(default)]
    pub services: Vec<RunService>,
    /// Folders kept from run to run of the same environment, such as `~/.cargo` (D730).
    #[serde(default)]
    pub caches: Vec<RunCache>,
}

/// A dependency cache: restored before setup and the agent, saved when the agent ends. Its
/// key is the fingerprint of its lock files in the workspace; without an exact match, the
/// latest archive of the same name is restored (D730).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RunCache {
    pub name: String,
    /// Absolute folder of the agent, outside `/workspace` and `/artifacts`.
    pub path: String,
    /// Files under `/workspace` whose content keys the cache (`app/Cargo.lock`).
    #[serde(default)]
    pub key_files: Vec<String>,
}

pub const CACHES_MAX: usize = 4;

fn cache_error(c: &RunCache) -> bool {
    let clean =
        |p: &str| !p.contains("..") && !p.contains("//") && !p.contains(char::is_whitespace);
    let path_ok = c.path.starts_with('/')
        && c.path.len() > 1
        && c.path.len() <= 200
        && clean(&c.path)
        && ![
            "/workspace",
            "/artifacts",
            "/caches",
            "/proc",
            "/sys",
            "/dev",
            "/etc",
        ]
        .iter()
        .any(|p| c.path == *p || c.path.starts_with(&format!("{p}/")));
    let keys_ok = c.key_files.len() <= 8
        && c.key_files
            .iter()
            .all(|k| !k.is_empty() && !k.starts_with('/') && k.len() <= 200 && clean(k));
    !(segment_is_valid(&c.name) && path_ok && keys_ok)
}

/// A repository to clone. Its read token, if any, is a secret of the vault of the org:
/// delivered to the clone step only, never to the agent (D714).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RunRepo {
    /// `https://` only.
    pub url: String,
    #[serde(default = "default_ref")]
    pub git_ref: String,
    /// One segment under `/workspace`.
    pub path: String,
    #[serde(default)]
    #[cfg_attr(feature = "openapi", schema(format = "uuid"))]
    pub secret_id: Option<String>,
    /// Write token of the vault, limited to this repository and to creating branches: the
    /// agent's changes go out as branch `pnex/run-<run id>` and a merge request onto
    /// `git_ref` (D388). Only a push sidecar gets it, never the agent. GitHub in V1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "openapi", schema(format = "uuid"))]
    pub push_secret_id: Option<String>,
}

/// `(owner, repository)` of an `https://github.com/<owner>/<repo>[.git]` URL: the only
/// forge a branch output speaks in V1 (D752).
pub fn github_repo(url: &str) -> Option<(String, String)> {
    let rest = url.strip_prefix("https://github.com/")?;
    let rest = rest.trim_end_matches('/');
    let rest = rest.strip_suffix(".git").unwrap_or(rest);
    let (owner, repo) = rest.split_once('/')?;
    let ok = |s: &str| {
        !s.is_empty()
            && s != "."
            && s != ".."
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    };
    (ok(owner) && ok(repo)).then(|| (owner.to_string(), repo.to_string()))
}

fn default_ref() -> String {
    "main".into()
}

/// A test service. An `env` value of exactly [`GENERATED`] becomes a fresh password at each
/// claim, which the agent gets as `<NAME>_<KEY>` (D717): never a password written down.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RunService {
    pub name: String,
    /// Pinned by digest.
    pub image: String,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// Command that succeeds once the service answers.
    pub ready_command: Vec<String>,
    #[serde(default)]
    pub data_paths: Vec<String>,
    #[serde(default)]
    pub run_as_user: Option<i64>,
}

/// The `env` value of a service that asks for a generated password.
pub const GENERATED: &str = "@generated";
pub const REPOS_MAX: usize = 8;
pub const SERVICES_MAX: usize = 4;

/// The variable the agent reads a generated value of a service from.
pub fn service_env_name(service: &str, key: &str) -> String {
    format!("{}_{}", service.to_uppercase().replace('-', "_"), key)
}

fn segment_is_valid(s: &str) -> bool {
    artifact_name_is_valid(s)
}

fn repo_error(r: &RunRepo) -> bool {
    let url_ok = r.url.starts_with("https://")
        && r.url.len() <= 512
        && !r.url.contains(char::is_whitespace)
        && !r.url[8..].contains('@');
    let ref_ok = !r.git_ref.is_empty()
        && r.git_ref.len() <= 200
        && !r.git_ref.starts_with('-')
        && r.git_ref
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '/'));
    let secret_ok = r
        .secret_id
        .as_deref()
        .is_none_or(|s| uuid::Uuid::parse_str(s).is_ok());
    !(url_ok && ref_ok && segment_is_valid(&r.path) && secret_ok)
}

fn service_error(s: &RunService) -> bool {
    let path_ok = |p: &String| p.starts_with('/') && !p.contains("..") && p.len() <= 200;
    !(segment_is_valid(&s.name)
        && crate::runners::image_is_pinned(&s.image)
        && !s.ready_command.is_empty()
        && s.ready_command.len() <= crate::runners::COMMAND_ARGS_MAX
        && s.data_paths.iter().all(path_ok)
        && s.data_paths.len() <= 4
        && s.env.len() <= ENV_MAX
        && s.env
            .keys()
            .all(|k| !k.is_empty() && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))
        && s.env
            .values()
            .all(|v| v.len() <= ENV_VALUE_MAX && !crate::environments::holds_a_secret(v))
        && s.run_as_user.is_none_or(|u| u > 0))
}

/// An artifact of the org given to a run: uploaded by a person, or produced by an earlier
/// run. Checked against its SHA-256 before the agent starts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ArtifactInput {
    pub name: String,
    #[cfg_attr(feature = "openapi", schema(format = "uuid"))]
    pub artifact_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ArtifactOutput {
    pub name: String,
    /// Missing at the end, it fails the run (`artifact-output-missing`).
    #[serde(default)]
    pub required: bool,
}

/// How the bytes of an artifact read: one file, or a folder as a tar archive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "lowercase")]
pub enum ArtifactForm {
    File,
    Tar,
}

impl ArtifactForm {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::File => "file",
            Self::Tar => "tar",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "file" => Some(Self::File),
            "tar" => Some(Self::Tar),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "lowercase")]
pub enum ArtifactState {
    /// Declared output, not received yet.
    Pending,
    Ready,
    /// Over a limit: refused whole, never cut.
    Refused,
    /// Declared output the run did not leave.
    Missing,
}

impl ArtifactState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Ready => "ready",
            Self::Refused => "refused",
            Self::Missing => "missing",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "pending" => Some(Self::Pending),
            "ready" => Some(Self::Ready),
            "refused" => Some(Self::Refused),
            "missing" => Some(Self::Missing),
            _ => None,
        }
    }
}

/// A file kept between runs, in the store of the org (D729). Never its content: that is
/// downloaded as an attachment, under its own permission.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct AgentArtifact {
    #[cfg_attr(feature = "openapi", schema(format = "uuid"))]
    pub id: String,
    /// The run that produced it; none for an upload.
    #[cfg_attr(feature = "openapi", schema(format = "uuid"))]
    pub run_id: Option<String>,
    pub name: String,
    pub state: ArtifactState,
    pub form: Option<ArtifactForm>,
    /// Error code when refused (`artifact-too-large`).
    pub error: Option<String>,
    /// `sha256:<hex>` once ready.
    pub sha256: Option<String>,
    pub size: Option<i64>,
    #[cfg_attr(feature = "openapi", schema(format = "date-time"))]
    pub created_at: String,
    #[cfg_attr(feature = "openapi", schema(format = "date-time"))]
    pub expires_at: String,
}

/// `[a-z0-9-]`, starting with a letter or digit: a path segment and a key part as is.
pub fn artifact_name_is_valid(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= ARTIFACT_NAME_MAX
        && !name.starts_with('-')
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

fn artifacts_error<'a>(names: impl Iterator<Item = &'a str>, count: usize) -> Option<&'static str> {
    if count > ARTIFACTS_MAX {
        return Some("too_many");
    }
    let mut seen = std::collections::BTreeSet::new();
    for name in names {
        if !artifact_name_is_valid(name) {
            return Some("invalid");
        }
        if !seen.insert(name) {
            return Some("duplicate");
        }
    }
    None
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct AgentRun {
    #[cfg_attr(feature = "openapi", schema(format = "uuid"))]
    pub id: String,
    /// What plays the run (D781): `sandbox` for a hosted agent, `internal_llm` for an
    /// internal one.
    pub engine: crate::runners::RunnerJobKind,
    /// Its environment; none for an engine that runs in the server.
    #[cfg_attr(feature = "openapi", schema(format = "uuid"))]
    pub environment_id: Option<String>,
    /// The agent it ran as, and the version of its record; none for a run started bare.
    #[cfg_attr(feature = "openapi", schema(format = "uuid"))]
    pub agent_id: Option<String>,
    pub agent_version: Option<i32>,
    /// The chain it belongs to, if any (D716).
    #[serde(default)]
    #[cfg_attr(feature = "openapi", schema(format = "uuid"))]
    pub chain_id: Option<String>,
    /// Image by digest, as run.
    pub image: String,
    #[cfg_attr(feature = "openapi", schema(format = "uuid"))]
    pub job_id: String,
    pub state: RunnerJobState,
    pub outcome: Option<RunnerJobOutcome>,
    pub exit_code: Option<i32>,
    pub llm_calls: i32,
    pub llm_calls_max: i32,
    pub input_tokens: i64,
    pub output_tokens: i64,
    /// Why PneX failed a run the agent itself ended well (`agent-run-output-missing`).
    pub error: Option<String>,
    #[cfg_attr(feature = "openapi", schema(format = "date-time"))]
    pub created_at: String,
    #[cfg_attr(feature = "openapi", schema(format = "date-time"))]
    pub started_at: Option<String>,
    #[cfg_attr(feature = "openapi", schema(format = "date-time"))]
    pub finished_at: Option<String>,
    /// Merge requests opened for its branches at the end (D388): forge URLs.
    #[serde(default)]
    pub merge_requests: Vec<String>,
}

/// Tokens of one LLM answer, read from its body or its stream.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct LlmUsageCount {
    pub input: i64,
    pub output: i64,
}

impl LlmUsageCount {
    fn add_anthropic(&mut self, usage: &serde_json::Value) {
        let n = |k: &str| usage[k].as_i64().unwrap_or(0);
        self.input +=
            n("input_tokens") + n("cache_creation_input_tokens") + n("cache_read_input_tokens");
        self.output += n("output_tokens");
    }

    /// From a whole JSON answer, Anthropic or OpenAI.
    pub fn of_body(body: &serde_json::Value) -> Self {
        let mut out = Self::default();
        let usage = &body["usage"];
        if usage.get("prompt_tokens").is_some() {
            out.input = usage["prompt_tokens"].as_i64().unwrap_or(0);
            out.output = usage["completion_tokens"].as_i64().unwrap_or(0);
        } else {
            out.add_anthropic(usage);
        }
        out
    }

    /// From one `data:` event of a stream. Anthropic gives the input in `message_start` and
    /// the running output total in `message_delta`; OpenAI gives both in its last chunk.
    pub fn read_event(&mut self, event: &serde_json::Value) {
        match event["type"].as_str() {
            Some("message_start") => self.add_anthropic(&event["message"]["usage"]),
            Some("message_delta") => {
                if let Some(n) = event["usage"]["output_tokens"].as_i64() {
                    self.output = n;
                }
            }
            _ => {
                if let Some(n) = event["usage"]["prompt_tokens"].as_i64() {
                    self.input = n;
                    self.output = event["usage"]["completion_tokens"].as_i64().unwrap_or(0);
                }
            }
        }
    }
}

/// Field and token of the first problem of a create body.
pub fn create_error(body: &AgentRunCreate) -> Option<(&'static str, &'static str)> {
    if body.timeout_secs == 0 || body.timeout_secs > crate::runners::TIMEOUT_MAX_SECS {
        return Some(("timeout_secs", "invalid"));
    }
    if body.command.len() > crate::runners::COMMAND_ARGS_MAX
        || body
            .command
            .iter()
            .any(|a| a.len() > crate::runners::COMMAND_ARG_MAX || a.contains('\0'))
    {
        return Some(("command", "invalid"));
    }
    if let Some(n) = body.llm_calls_max {
        if !(1..=LLM_CALLS_MAX).contains(&n) {
            return Some(("llm_calls_max", "invalid"));
        }
    }
    if body.env.len() > ENV_MAX {
        return Some(("env", "too_many"));
    }
    for (k, v) in &body.env {
        let name_ok = !k.is_empty()
            && k.len() <= 128
            && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
            && !k.starts_with(|c: char| c.is_ascii_digit());
        if !name_ok || RESERVED_ENV.contains(&k.as_str()) {
            return Some(("env", "invalid"));
        }
        if v.len() > ENV_VALUE_MAX || v.contains('\0') {
            return Some(("env", "invalid"));
        }
        if crate::environments::holds_a_secret(v) {
            return Some(("env", "secret"));
        }
    }
    if let Some(token) = artifacts_error(
        body.inputs.iter().map(|i| i.name.as_str()),
        body.inputs.len(),
    ) {
        return Some(("inputs", token));
    }
    if body
        .inputs
        .iter()
        .any(|i| uuid::Uuid::parse_str(&i.artifact_id).is_err())
    {
        return Some(("inputs", "invalid"));
    }
    if let Some(token) = artifacts_error(
        body.outputs.iter().map(|o| o.name.as_str()),
        body.outputs.len(),
    ) {
        return Some(("outputs", token));
    }
    if body.repos.len() > REPOS_MAX {
        return Some(("repos", "too_many"));
    }
    if body.repos.iter().any(repo_error) {
        return Some(("repos", "invalid"));
    }
    if body.repos.iter().any(|r| {
        r.push_secret_id
            .as_deref()
            .is_some_and(|id| uuid::Uuid::parse_str(id).is_err() || github_repo(&r.url).is_none())
    }) {
        return Some(("repos", "push_unsupported"));
    }
    let mut paths = std::collections::BTreeSet::new();
    if !body.repos.iter().all(|r| paths.insert(r.path.as_str())) {
        return Some(("repos", "duplicate"));
    }
    if body.services.len() > SERVICES_MAX {
        return Some(("services", "too_many"));
    }
    if body.services.iter().any(service_error) {
        return Some(("services", "invalid"));
    }
    let mut names = std::collections::BTreeSet::new();
    if !body.services.iter().all(|s| names.insert(s.name.as_str())) {
        return Some(("services", "duplicate"));
    }
    if body.caches.len() > CACHES_MAX {
        return Some(("caches", "too_many"));
    }
    if body.caches.iter().any(cache_error) {
        return Some(("caches", "invalid"));
    }
    let mut names = std::collections::BTreeSet::new();
    let mut paths = std::collections::BTreeSet::new();
    if !body
        .caches
        .iter()
        .all(|c| names.insert(c.name.as_str()) && paths.insert(c.path.as_str()))
    {
        return Some(("caches", "duplicate"));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn usage_is_read_from_bodies_and_streams() {
        let anthropic = json!({ "usage": { "input_tokens": 10, "cache_read_input_tokens": 5, "output_tokens": 7 } });
        assert_eq!(
            LlmUsageCount::of_body(&anthropic),
            LlmUsageCount {
                input: 15,
                output: 7
            }
        );
        let openai = json!({ "usage": { "prompt_tokens": 3, "completion_tokens": 4 } });
        assert_eq!(
            LlmUsageCount::of_body(&openai),
            LlmUsageCount {
                input: 3,
                output: 4
            }
        );
        let mut s = LlmUsageCount::default();
        s.read_event(&json!({ "type": "message_start", "message": { "usage": { "input_tokens": 12, "output_tokens": 1 } } }));
        s.read_event(&json!({ "type": "content_block_delta" }));
        s.read_event(&json!({ "type": "message_delta", "usage": { "output_tokens": 40 } }));
        assert_eq!(
            s,
            LlmUsageCount {
                input: 12,
                output: 40
            }
        );
        let mut o = LlmUsageCount::default();
        o.read_event(&json!({ "choices": [] }));
        o.read_event(
            &json!({ "choices": [], "usage": { "prompt_tokens": 9, "completion_tokens": 2 } }),
        );
        assert_eq!(
            o,
            LlmUsageCount {
                input: 9,
                output: 2
            }
        );
    }

    #[test]
    fn the_gateway_variables_stay_with_pnex() {
        let mut body = AgentRunCreate {
            environment_id: "x".into(),
            timeout_secs: 60,
            ..Default::default()
        };
        assert_eq!(create_error(&body), None);
        body.env.insert("ANTHROPIC_API_KEY".into(), "mine".into());
        assert_eq!(create_error(&body), Some(("env", "invalid")));
        body.env.clear();
        body.env.insert("1BAD".into(), "x".into());
        assert_eq!(create_error(&body), Some(("env", "invalid")));
        body.env.clear();
        body.timeout_secs = 0;
        assert_eq!(create_error(&body), Some(("timeout_secs", "invalid")));
    }

    #[test]
    fn repos_and_services_are_checked_before_any_pod() {
        let mut body = AgentRunCreate {
            environment_id: "x".into(),
            timeout_secs: 60,
            ..Default::default()
        };
        let repo = |url: &str, path: &str| RunRepo {
            url: url.into(),
            git_ref: "main".into(),
            path: path.into(),
            secret_id: None,
            push_secret_id: None,
        };
        body.repos = vec![repo("https://github.com/acme/app.git", "app")];
        assert_eq!(create_error(&body), None);
        for bad in [
            repo("http://github.com/acme/app.git", "app"),
            repo("https://user:pass@github.com/acme/app.git", "app"),
            repo("https://github.com/acme/app.git", "../etc"),
            repo("ssh://github.com/acme/app.git", "app"),
        ] {
            body.repos = vec![bad.clone()];
            assert_eq!(create_error(&body), Some(("repos", "invalid")), "{bad:?}");
        }
        let mut dash = repo("https://github.com/acme/app.git", "app");
        dash.git_ref = "--upload-pack=x".into();
        body.repos = vec![dash];
        assert_eq!(create_error(&body), Some(("repos", "invalid")));
        body.repos = vec![
            repo("https://github.com/a/b.git", "app"),
            repo("https://github.com/a/c.git", "app"),
        ];
        assert_eq!(create_error(&body), Some(("repos", "duplicate")));
        // A branch output: a GitHub repository and a vault id only (D388, D752).
        let uuid = "6f9619ff-8b86-d011-b42d-00cf4fc964ff";
        let mut push = repo("https://github.com/acme/app.git", "app");
        push.push_secret_id = Some(uuid.into());
        body.repos = vec![push.clone()];
        assert_eq!(create_error(&body), None);
        for (url, id) in [
            ("https://gitlab.com/acme/app.git", uuid),
            ("https://github.com/acme/app.git", "x"),
        ] {
            let mut bad = push.clone();
            bad.url = url.into();
            bad.push_secret_id = Some(id.into());
            body.repos = vec![bad];
            assert_eq!(
                create_error(&body),
                Some(("repos", "push_unsupported")),
                "{url} {id}"
            );
        }
        body.repos = vec![];

        let digest = "0".repeat(64);
        let service = RunService {
            name: "postgres".into(),
            image: format!("cgr.dev/chainguard/postgres@sha256:{digest}"),
            env: BTreeMap::from([("POSTGRES_PASSWORD".to_string(), GENERATED.to_string())]),
            ready_command: vec!["pg_isready".into()],
            data_paths: vec!["/var/lib/postgresql/data".into()],
            run_as_user: None,
        };
        body.services = vec![service.clone()];
        assert_eq!(create_error(&body), None);
        let mut unpinned = service.clone();
        unpinned.image = "postgres:16".into();
        let mut written = service.clone();
        written
            .env
            .insert("TOKEN".into(), format!("ghp_{}", "a".repeat(36)));
        for bad in [unpinned, written] {
            body.services = vec![bad];
            assert_eq!(create_error(&body), Some(("services", "invalid")));
        }
        assert_eq!(service_env_name("pg-main", "PASSWORD"), "PG_MAIN_PASSWORD");
        body.services = vec![];

        let cache = |path: &str, key: &str| RunCache {
            name: "cargo".into(),
            path: path.into(),
            key_files: vec![key.into()],
        };
        body.caches = vec![cache("/tmp/.cargo", "app/Cargo.lock")];
        assert_eq!(create_error(&body), None);
        for bad in [
            cache("/workspace/target", "app/Cargo.lock"),
            cache("/", "app/Cargo.lock"),
            cache("/tmp/../etc", "app/Cargo.lock"),
            cache("/tmp/.cargo", "/etc/passwd"),
            cache("/tmp/.cargo", "../x"),
            cache("/artifacts/out", "a"),
        ] {
            body.caches = vec![bad.clone()];
            assert_eq!(create_error(&body), Some(("caches", "invalid")), "{bad:?}");
        }
    }

    #[test]
    fn artifact_names_are_path_segments_declared_once() {
        let mut body = AgentRunCreate {
            environment_id: "x".into(),
            timeout_secs: 60,
            ..Default::default()
        };
        let input = |name: &str| ArtifactInput {
            name: name.into(),
            artifact_id: uuid::Uuid::nil().to_string(),
        };
        body.inputs = vec![input("logs"), input("report-2")];
        assert_eq!(create_error(&body), None);
        for bad in ["", "../x", "A", "-x", "a/b", &"a".repeat(64)] {
            body.inputs = vec![input(bad)];
            assert_eq!(create_error(&body), Some(("inputs", "invalid")), "{bad}");
        }
        body.inputs = vec![input("a"), input("a")];
        assert_eq!(create_error(&body), Some(("inputs", "duplicate")));
        body.inputs = vec![ArtifactInput {
            name: "a".into(),
            artifact_id: "nope".into(),
        }];
        assert_eq!(create_error(&body), Some(("inputs", "invalid")));
        body.inputs = vec![];
        body.outputs = (0..=ARTIFACTS_MAX)
            .map(|i| ArtifactOutput {
                name: format!("o{i}"),
                required: false,
            })
            .collect();
        assert_eq!(create_error(&body), Some(("outputs", "too_many")));
        body.outputs.truncate(1);
        body.env.insert("PNEX_ARTIFACTS_OUT".into(), "/tmp".into());
        assert_eq!(create_error(&body), Some(("env", "invalid")));
    }
}
