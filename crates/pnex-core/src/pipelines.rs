//! Pipelines (data-pipelines.md D571): the document, its validation and the
//! DTOs of the API. Pure, wasm-safe. The YAML is parsed by the callers.

use std::collections::{BTreeMap, HashSet};

use serde::{Deserialize, Serialize};

pub const NAME_MAX: usize = 255;
pub const TRANSFORMS_MAX: usize = 50;
pub const INPUTS_MAX: usize = 32;
pub const ALIAS_MAX: usize = 63;
pub const SQL_MAX_BYTES: usize = 64 * 1024;
pub const DOCUMENT_MAX_BYTES: usize = 1024 * 1024;
pub const CHECK_COLUMNS_MAX: usize = 32;
/// Steps of one build: everything an org could chain together.
pub const BUILD_STEPS_MAX: usize = 200;

/// What a pipeline computes, transform by transform, in order. A transform may
/// read the output of an earlier one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct PipelineDocument {
    pub transforms: Vec<Transform>,
}

/// One SQL query from datasets to a dataset.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct Transform {
    /// Dataset it writes, created on the first run.
    pub output: String,
    /// SQL table name → dataset read. Nothing else is visible to the query.
    pub inputs: BTreeMap<String, String>,
    pub sql: String,
    /// Blocking checks of the result: one that fails leaves the previous version
    /// current (D576).
    #[serde(default, skip_serializing_if = "Checks::is_empty")]
    pub checks: Checks,
}

/// What a computed result must hold before it is committed (D576).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct Checks {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_rows: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_rows: Option<u64>,
    /// Columns that together identify a row.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unique: Vec<String>,
    /// Column → highest share of null or blank values, in percent.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub max_empty_pct: BTreeMap<String, u8>,
    /// Highest fall in rows against the current version, in percent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_drop_pct: Option<u8>,
    /// Refuses a result whose columns differ from the current version.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub same_columns: bool,
}

impl Checks {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    fn violations(&self, at: &str, bad: &mut impl FnMut(String, &str)) {
        if let (Some(min), Some(max)) = (self.min_rows, self.max_rows) {
            if min > max {
                bad(format!("{at}.min_rows"), "invalid");
            }
        }
        if self.unique.len() > CHECK_COLUMNS_MAX {
            bad(
                format!("{at}.unique"),
                &format!("max_items:{CHECK_COLUMNS_MAX}"),
            );
        }
        if self.max_empty_pct.len() > CHECK_COLUMNS_MAX {
            bad(
                format!("{at}.max_empty_pct"),
                &format!("max_items:{CHECK_COLUMNS_MAX}"),
            );
        }
        for (i, c) in self.unique.iter().enumerate() {
            if !valid_name(c) {
                bad(format!("{at}.unique.{i}"), "invalid");
            }
        }
        for (c, pct) in &self.max_empty_pct {
            if !valid_name(c) || *pct > 100 {
                bad(format!("{at}.max_empty_pct.{c}"), "invalid");
            }
        }
        if self.max_drop_pct.is_some_and(|p| p > 100) {
            bad(format!("{at}.max_drop_pct"), "invalid");
        }
    }
}

/// The outcome of one check, kept on the transaction (D576).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct CheckResult {
    /// `min_rows`, `max_rows`, `unique`, `max_empty_pct`, `max_drop_pct` or
    /// `same_columns`.
    pub check: String,
    /// The column concerned, when the check has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub column: Option<String>,
    pub passed: bool,
    /// What was measured, as a number or a list of columns.
    pub value: String,
    /// What was allowed.
    pub limit: String,
}

/// A problem of a document: where, and a machine token.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct PipelineViolation {
    pub path: String,
    pub token: String,
}

fn valid_alias(s: &str) -> bool {
    let mut chars = s.chars();
    s.len() <= ALIAS_MAX
        && chars
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn valid_name(s: &str) -> bool {
    !s.trim().is_empty() && s.chars().count() <= NAME_MAX
}

impl PipelineDocument {
    /// The first version of a pipeline made on a screen: a copy of one table.
    pub fn starter(input: &str, output: &str) -> Self {
        Self {
            transforms: vec![Transform {
                output: output.to_string(),
                inputs: BTreeMap::from([("source".to_string(), input.to_string())]),
                sql: "SELECT * FROM source".to_string(),
                checks: Checks::default(),
            }],
        }
    }

    /// The document as YAML, with its SQL readable. Written by hand because this crate
    /// runs in the browser, without a YAML library; a value goes as a JSON string, which
    /// is valid YAML, whenever a block could change it.
    pub fn to_yaml(&self) -> String {
        let quoted = |s: &str| serde_json::Value::String(s.to_string()).to_string();
        let block_safe = |s: &str| {
            !s.is_empty()
                && !s.chars().any(|c| c.is_control() && c != '\n')
                && s.lines()
                    .all(|l| l.is_empty() || l.trim_end() == l && !l.starts_with(' '))
                && !s.starts_with('\n')
                && !s.ends_with('\n')
        };
        let mut out = String::from("transforms:\n");
        for t in &self.transforms {
            out += &format!("  - output: {}\n", quoted(&t.output));
            let inputs = serde_json::to_string(&t.inputs).unwrap_or_else(|_| "{}".into());
            out += &format!("    inputs: {inputs}\n");
            if block_safe(&t.sql) {
                out += "    sql: |-\n";
                for line in t.sql.split('\n') {
                    match line {
                        "" => out.push('\n'),
                        l => out += &format!("      {l}\n"),
                    }
                }
            } else {
                out += &format!("    sql: {}\n", quoted(&t.sql));
            }
            if !t.checks.is_empty() {
                let checks = serde_json::to_string(&t.checks).unwrap_or_else(|_| "{}".into());
                out += &format!("    checks: {checks}\n");
            }
        }
        out
    }

    /// Empty when the document can run.
    pub fn violations(&self) -> Vec<PipelineViolation> {
        let mut out = Vec::new();
        let mut bad = |path: String, token: &str| {
            out.push(PipelineViolation {
                path,
                token: token.to_string(),
            })
        };
        if self.transforms.is_empty() {
            bad("transforms".into(), "required");
        }
        if self.transforms.len() > TRANSFORMS_MAX {
            bad("transforms".into(), &format!("max_items:{TRANSFORMS_MAX}"));
        }
        let outputs: Vec<&str> = self.transforms.iter().map(|t| t.output.as_str()).collect();
        let mut seen: HashSet<&str> = HashSet::new();
        for (i, t) in self.transforms.iter().enumerate() {
            if !valid_name(&t.output) {
                bad(format!("transforms.{i}.output"), "invalid");
            }
            if !seen.insert(&t.output) {
                bad(format!("transforms.{i}.output"), "duplicate");
            }
            if t.sql.trim().is_empty() {
                bad(format!("transforms.{i}.sql"), "required");
            }
            if t.sql.len() > SQL_MAX_BYTES {
                bad(
                    format!("transforms.{i}.sql"),
                    &format!("max_bytes:{SQL_MAX_BYTES}"),
                );
            }
            if t.inputs.is_empty() {
                bad(format!("transforms.{i}.inputs"), "required");
            }
            if t.inputs.len() > INPUTS_MAX {
                bad(
                    format!("transforms.{i}.inputs"),
                    &format!("max_items:{INPUTS_MAX}"),
                );
            }
            t.checks
                .violations(&format!("transforms.{i}.checks"), &mut bad);
            for (alias, dataset) in &t.inputs {
                let path = format!("transforms.{i}.inputs.{alias}");
                if !valid_alias(alias) || !valid_name(dataset) {
                    bad(path.clone(), "invalid");
                }
                // Reading an output written later, or its own, is a cycle (D575).
                if outputs[i..].contains(&dataset.as_str()) {
                    bad(path, "cycle");
                }
            }
        }
        out
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Pipeline {
    #[cfg_attr(feature = "openapi", schema(format = "uuid"))]
    pub id: String,
    pub name: String,
    pub version: i32,
    /// The document, in YAML.
    pub document: String,
    /// When it runs on its own, with what it depends on (D575).
    pub recurrence: Option<serde_json::Value>,
    #[cfg_attr(feature = "openapi", schema(format = "date-time"))]
    pub next_run_at: Option<String>,
    #[cfg_attr(feature = "openapi", schema(format = "date-time"))]
    pub created_at: String,
    #[cfg_attr(feature = "openapi", schema(format = "date-time"))]
    pub updated_at: String,
}

/// When a pipeline runs on its own. `null` stops it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct PipelineSchedule {
    pub recurrence: Option<serde_json::Value>,
}

/// A document to check without saving it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct PipelineDraft {
    /// The document, in YAML.
    pub document: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct PipelineInput {
    pub name: String,
    /// The document, in YAML.
    pub document: String,
}

/// A new version, written over the version that was read.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct PipelineUpdate {
    pub document: String,
    pub expected_version: i32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct PipelineVersion {
    pub version: i32,
    pub document: String,
    /// Who saved it; absent once that account is gone.
    pub author: Option<String>,
    #[cfg_attr(feature = "openapi", schema(format = "date-time"))]
    pub created_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "lowercase")]
pub enum PipelineRunState {
    Queued,
    Running,
    Done,
    Failed,
}

/// One run of a pipeline.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct PipelineRun {
    #[cfg_attr(feature = "openapi", schema(format = "uuid"))]
    pub id: String,
    /// Version of the pipeline it runs.
    pub version: i32,
    pub state: PipelineRunState,
    pub attempts: i32,
    /// Machine code of a failure.
    pub code: Option<String>,
    /// Arguments of `code`.
    pub args: Option<serde_json::Value>,
    #[cfg_attr(feature = "openapi", schema(format = "date-time"))]
    pub created_at: String,
    #[cfg_attr(feature = "openapi", schema(format = "date-time"))]
    pub started_at: Option<String>,
    #[cfg_attr(feature = "openapi", schema(format = "date-time"))]
    pub finished_at: Option<String>,
}

/// A dataset linked to another by a pipeline run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct LineageLink {
    #[cfg_attr(feature = "openapi", schema(format = "uuid"))]
    pub dataset_id: String,
    pub dataset_name: String,
    #[cfg_attr(feature = "openapi", schema(format = "uuid"))]
    pub pipeline_id: String,
    pub pipeline_name: String,
}

/// What a dataset reads and what reads it, as written by the runs (D574).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct DatasetLineage {
    pub upstream: Vec<LineageLink>,
    pub downstream: Vec<LineageLink>,
    /// An input has a transaction newer than the one the current view read.
    pub stale: bool,
}

/// What a build computes (D575).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "lowercase")]
pub enum BuildTargetKind {
    Pipeline,
    Import,
}

impl BuildTargetKind {
    pub fn wire(self) -> &'static str {
        match self {
            Self::Pipeline => "pipeline",
            Self::Import => "import",
        }
    }
}

/// A pipeline or an import, as the scheduler sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphNode {
    pub kind: BuildTargetKind,
    pub id: String,
    pub name: String,
    /// Whether the server can start it: a file import without steps waits for a
    /// file, and what it wrote last is read as it is.
    pub runnable: bool,
    pub reads: Vec<String>,
    pub writes: Vec<String>,
    /// Keys of the imports it runs after. Empty on a pipeline.
    pub after: Vec<String>,
    /// Mapping key of an import.
    pub key: Option<String>,
}

/// Why no order exists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanError {
    Cycle {
        through: String,
    },
    /// `after` names an import the org does not have.
    Missing {
        name: String,
        after: String,
    },
    TooLong,
}

fn graph_edges(nodes: &[GraphNode]) -> Vec<Vec<usize>> {
    nodes
        .iter()
        .enumerate()
        .map(|(i, n)| {
            let mut deps: Vec<usize> = nodes
                .iter()
                .enumerate()
                .filter(|(j, m)| {
                    *j != i
                        && (m.writes.iter().any(|w| n.reads.contains(w))
                            || m.key.as_ref().is_some_and(|k| n.after.contains(k)))
                })
                .map(|(j, _)| j)
                .collect();
            deps.sort_unstable();
            deps
        })
        .collect()
}

/// The first dependency cycle of a graph, by the name of a node on it. Refused at
/// save, so that a schedule always has an order.
pub fn find_cycle(nodes: &[GraphNode]) -> Option<String> {
    let edges = graph_edges(nodes);
    // 0 = unseen, 1 = on the path, 2 = done.
    fn visit(i: usize, edges: &[Vec<usize>], mark: &mut [u8]) -> Option<usize> {
        mark[i] = 1;
        for &d in &edges[i] {
            match mark[d] {
                1 => return Some(d),
                0 => {
                    if let Some(found) = visit(d, edges, mark) {
                        return Some(found);
                    }
                }
                _ => {}
            }
        }
        mark[i] = 2;
        None
    }
    let mut mark = vec![0u8; nodes.len()];
    (0..nodes.len())
        .find_map(|i| {
            (mark[i] == 0)
                .then(|| visit(i, &edges, &mut mark))
                .flatten()
        })
        .map(|i| nodes[i].name.clone())
}

/// The target and all it depends on, dependencies first, in a stable order.
pub fn plan(nodes: &[GraphNode], target: usize) -> Result<Vec<BuildStep>, PlanError> {
    if let Some(through) = find_cycle(nodes) {
        return Err(PlanError::Cycle { through });
    }
    let edges = graph_edges(nodes);
    fn visit(i: usize, edges: &[Vec<usize>], seen: &mut [bool], out: &mut Vec<usize>) {
        seen[i] = true;
        for &d in &edges[i] {
            if !seen[d] {
                visit(d, edges, seen, out);
            }
        }
        out.push(i);
    }
    let (mut seen, mut order) = (vec![false; nodes.len()], Vec::new());
    visit(target, &edges, &mut seen, &mut order);
    if order.len() > BUILD_STEPS_MAX {
        return Err(PlanError::TooLong);
    }
    for &i in &order {
        for after in &nodes[i].after {
            if !nodes.iter().any(|n| n.key.as_deref() == Some(after)) {
                return Err(PlanError::Missing {
                    name: nodes[i].name.clone(),
                    after: after.clone(),
                });
            }
        }
    }
    let position = |node: usize| order.iter().position(|o| *o == node);
    Ok(order
        .iter()
        .map(|&i| {
            let n = &nodes[i];
            BuildStep {
                kind: n.kind,
                id: n.id.clone(),
                name: n.name.clone(),
                state: if n.runnable {
                    BuildStepState::Pending
                } else {
                    BuildStepState::Skipped
                },
                after: edges[i].iter().filter_map(|d| position(*d)).collect(),
                run_id: None,
                code: None,
                retries: 0,
            }
        })
        .collect())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "lowercase")]
pub enum BuildStepState {
    Pending,
    Running,
    Done,
    Failed,
    /// An input failed: it keeps its last valid version.
    Blocked,
    /// Not runnable by the server: what it wrote last is read as it is.
    Skipped,
}

/// One pipeline or import of a build.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct BuildStep {
    pub kind: BuildTargetKind,
    #[cfg_attr(feature = "openapi", schema(format = "uuid"))]
    pub id: String,
    pub name: String,
    pub state: BuildStepState,
    /// Positions in the list of the steps it waits for.
    pub after: Vec<usize>,
    /// Its run, once started.
    #[cfg_attr(feature = "openapi", schema(format = "uuid"))]
    pub run_id: Option<String>,
    pub code: Option<String>,
    /// Times it started again because its run was lost with its server.
    #[serde(default)]
    pub retries: u8,
}

/// How many times a step starts again after its run was lost with its server. Once: an
/// import that itself brings its server down must not do it forever.
pub const STEP_RETRIES_MAX: u8 = 1;

impl BuildStep {
    /// Gives the step another run if it only stopped because its server did. Applying an
    /// import twice is safe: the second run compares with what the first one wrote.
    pub fn retry_if_lost(&mut self) -> bool {
        let lost = self.kind == BuildTargetKind::Import
            && self.state == BuildStepState::Failed
            && self.code.as_deref() == Some(crate::err_codes::IMPORT_RUN_INTERRUPTED);
        if lost && self.retries < STEP_RETRIES_MAX {
            self.retries += 1;
            self.state = BuildStepState::Pending;
            self.run_id = None;
            self.code = None;
            return true;
        }
        false
    }
}

/// Blocks what depends on a failure, then gives the next step to start. `None`
/// with a step running means wait; `None` without means the build is over. One
/// step at a time: an org writes one import and runs one pipeline at once anyway.
pub fn next_step(steps: &mut [BuildStep]) -> Option<usize> {
    use BuildStepState::*;
    for i in 0..steps.len() {
        let stopped = steps[i]
            .after
            .iter()
            .any(|d| matches!(steps.get(*d).map(|s| s.state), Some(Failed | Blocked)));
        if steps[i].state == Pending && stopped {
            steps[i].state = Blocked;
        }
    }
    if steps.iter().any(|s| s.state == Running) {
        return None;
    }
    steps.iter().position(|s| s.state == Pending)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "lowercase")]
pub enum BuildState {
    Running,
    Done,
    Failed,
}

/// A target and what it depends on, computed in order (D575).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Build {
    #[cfg_attr(feature = "openapi", schema(format = "uuid"))]
    pub id: String,
    pub target_kind: BuildTargetKind,
    #[cfg_attr(feature = "openapi", schema(format = "uuid"))]
    pub target_id: String,
    pub target_name: String,
    /// `schedule` or `manual`.
    pub trigger: String,
    pub state: BuildState,
    /// A request came during this build: another one follows it.
    pub again: bool,
    pub code: Option<String>,
    pub args: Option<serde_json::Value>,
    pub steps: Vec<BuildStep>,
    #[cfg_attr(feature = "openapi", schema(format = "date-time"))]
    pub created_at: String,
    #[cfg_attr(feature = "openapi", schema(format = "date-time"))]
    pub finished_at: Option<String>,
}

/// The steps a build of a target would go through.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct BuildPlan {
    pub steps: Vec<BuildStep>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct BuildInput {
    pub target_kind: BuildTargetKind,
    #[cfg_attr(feature = "openapi", schema(format = "uuid"))]
    pub target_id: String,
}

/// Valkey pub/sub channel of an org's dataset events.
pub fn dataset_channel(org_id: i64) -> String {
    format!("pnex:ds:v1:{org_id}")
}

/// `dataset.committed`, or `dataset.synced` once a sync is applied (D578).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DatasetEvent {
    pub event: String,
    pub dataset_id: String,
    pub dataset: String,
    /// Committed version, or `<import id>@<run number>` for a sync.
    pub reference: String,
    pub counts: serde_json::Value,
    /// Ids of the objects a sync wrote, `CHANGED_IDS_MAX` at most.
    #[serde(default)]
    pub changed: Vec<String>,
    pub ts_ms: i64,
}

pub const DATASET_COMMITTED: &str = "dataset.committed";
pub const DATASET_SYNCED: &str = "dataset.synced";
pub const CHANGED_IDS_MAX: i64 = 10_000;

/// Configuration of the `dataset_trigger` flow node: one dataset by name.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DatasetTriggerConfig {
    #[serde(default)]
    pub dataset: String,
    /// `committed` (default) or `synced`.
    #[serde(default)]
    pub on: String,
}

impl DatasetTriggerConfig {
    fn wanted(&self) -> &'static str {
        match self.on.as_str() {
            "synced" => DATASET_SYNCED,
            _ => DATASET_COMMITTED,
        }
    }

    pub fn matches(&self, ev: &DatasetEvent) -> bool {
        ev.dataset == self.dataset && ev.event == self.wanted()
    }

    /// Save validation: an unset node is a valid draft.
    pub fn check(&self) -> Option<(&'static str, String)> {
        let on_ok = matches!(self.on.as_str(), "" | "committed" | "synced");
        if !on_ok || (!self.dataset.is_empty() && !valid_name(&self.dataset)) {
            return Some((
                "dataset_trigger_invalid",
                "invalid dataset or event".to_string(),
            ));
        }
        None
    }

    /// Deploy gate and runtime build: the dataset is named.
    pub fn check_deployable(&self) -> Option<(&'static str, String)> {
        if self.dataset.trim().is_empty() {
            return Some(("dataset_trigger_unset", "pick a dataset".to_string()));
        }
        self.check()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(transforms: Vec<(&str, Vec<(&str, &str)>, &str)>) -> PipelineDocument {
        PipelineDocument {
            transforms: transforms
                .into_iter()
                .map(|(output, inputs, sql)| Transform {
                    output: output.into(),
                    inputs: inputs
                        .into_iter()
                        .map(|(a, d)| (a.to_string(), d.to_string()))
                        .collect(),
                    sql: sql.into(),
                    checks: Checks::default(),
                })
                .collect(),
        }
    }

    fn tokens(d: &PipelineDocument) -> Vec<String> {
        d.violations()
            .into_iter()
            .map(|v| format!("{}={}", v.path, v.token))
            .collect()
    }

    #[test]
    fn a_chain_is_valid() {
        let d = doc(vec![
            ("clean", vec![("r", "raw")], "SELECT * FROM r"),
            ("final", vec![("c", "clean")], "SELECT * FROM c"),
        ]);
        assert!(d.violations().is_empty());
    }

    #[test]
    fn an_output_cannot_be_read_by_itself_or_before_it_is_written() {
        let own = doc(vec![("a", vec![("x", "a")], "SELECT 1")]);
        assert_eq!(tokens(&own), ["transforms.0.inputs.x=cycle"]);
        let later = doc(vec![
            ("a", vec![("x", "b")], "SELECT 1"),
            ("b", vec![("y", "raw")], "SELECT 1"),
        ]);
        assert_eq!(tokens(&later), ["transforms.0.inputs.x=cycle"]);
    }

    #[test]
    fn bad_documents_name_their_path() {
        assert_eq!(tokens(&doc(vec![])), ["transforms=required"]);
        let d = doc(vec![("a", vec![], " ")]);
        assert_eq!(
            tokens(&d),
            ["transforms.0.sql=required", "transforms.0.inputs=required"]
        );
        let d = doc(vec![("a", vec![("not valid", "raw")], "SELECT 1")]);
        assert_eq!(tokens(&d), ["transforms.0.inputs.not valid=invalid"]);
        let d = doc(vec![
            ("a", vec![("x", "raw")], "SELECT 1"),
            ("a", vec![("x", "raw")], "SELECT 1"),
        ]);
        assert_eq!(tokens(&d), ["transforms.1.output=duplicate"]);
    }

    fn node(name: &str, reads: &[&str], writes: &[&str], after: &[&str]) -> GraphNode {
        GraphNode {
            kind: BuildTargetKind::Import,
            id: name.into(),
            name: name.into(),
            runnable: true,
            reads: reads.iter().map(|s| s.to_string()).collect(),
            writes: writes.iter().map(|s| s.to_string()).collect(),
            after: after.iter().map(|s| s.to_string()).collect(),
            key: Some(name.into()),
        }
    }

    fn names(steps: &[BuildStep]) -> Vec<&str> {
        steps.iter().map(|s| s.name.as_str()).collect()
    }

    #[test]
    fn a_plan_runs_dependencies_first_and_leaves_the_rest_out() {
        let g = [
            node("target", &["clean", "other"], &[], &[]),
            node("clean", &["raw"], &["clean"], &[]),
            node("raw", &[], &["raw"], &[]),
            node("other", &[], &["other"], &["raw"]),
            node("unrelated", &[], &["u"], &[]),
        ];
        let steps = plan(&g, 0).unwrap();
        assert_eq!(names(&steps), ["raw", "clean", "other", "target"]);
        assert_eq!(steps[3].after, [1, 2]);
    }

    #[test]
    fn a_cycle_is_named_and_refused_even_through_after() {
        let g = [
            node("a", &["b"], &["a"], &[]),
            node("b", &[], &["b"], &["a"]),
        ];
        assert!(find_cycle(&g).is_some());
        assert!(matches!(plan(&g, 0), Err(PlanError::Cycle { .. })));
        assert!(find_cycle(&[node("a", &["raw"], &["b"], &[])]).is_none());
    }

    #[test]
    fn an_unknown_import_in_after_stops_the_plan() {
        let g = [node("a", &[], &[], &["gone"])];
        assert_eq!(
            plan(&g, 0),
            Err(PlanError::Missing {
                name: "a".into(),
                after: "gone".into()
            })
        );
    }

    #[test]
    fn a_failure_blocks_what_follows_and_lets_the_rest_run() {
        let g = [
            node("t", &["a", "b"], &[], &[]),
            node("a", &["c"], &["a"], &[]),
            node("b", &[], &["b"], &[]),
            node("c", &[], &["c"], &[]),
        ];
        let mut steps = plan(&g, 0).unwrap();
        let at = |s: &[BuildStep], n: &str| s.iter().position(|x| x.name == n).unwrap();
        let (c, a, b, t) = (
            at(&steps, "c"),
            at(&steps, "a"),
            at(&steps, "b"),
            at(&steps, "t"),
        );
        assert_eq!(next_step(&mut steps), Some(c));
        steps[c].state = BuildStepState::Running;
        assert_eq!(next_step(&mut steps), None, "one step at a time");
        steps[c].state = BuildStepState::Failed;
        assert_eq!(next_step(&mut steps), Some(b));
        assert_eq!(steps[a].state, BuildStepState::Blocked);
        steps[b].state = BuildStepState::Done;
        assert_eq!(next_step(&mut steps), None);
        assert_eq!(steps[t].state, BuildStepState::Blocked);
    }

    #[test]
    fn only_a_run_lost_with_its_server_starts_again_and_only_once() {
        let mut step = BuildStep {
            kind: BuildTargetKind::Import,
            id: "i".into(),
            name: "i".into(),
            state: BuildStepState::Failed,
            after: vec![],
            run_id: Some("r".into()),
            code: Some(crate::err_codes::IMPORT_RUN_INTERRUPTED.into()),
            retries: 0,
        };
        assert!(step.retry_if_lost());
        assert_eq!(step.state, BuildStepState::Pending);
        assert_eq!(step.run_id, None);
        assert_eq!(step.code, None);

        step.state = BuildStepState::Failed;
        step.code = Some(crate::err_codes::IMPORT_RUN_INTERRUPTED.into());
        assert!(!step.retry_if_lost(), "once");
        assert_eq!(step.state, BuildStepState::Failed);

        let mut refused = BuildStep {
            code: Some(crate::err_codes::IMPORT_SCHEMA_CHANGED.into()),
            retries: 0,
            ..step.clone()
        };
        assert!(!refused.retry_if_lost(), "a real failure stays one");
        let mut pipeline = BuildStep {
            kind: BuildTargetKind::Pipeline,
            code: Some(crate::err_codes::IMPORT_RUN_INTERRUPTED.into()),
            retries: 0,
            ..step
        };
        assert!(!pipeline.retry_if_lost(), "the worker retries its own jobs");
    }

    #[test]
    fn a_step_not_runnable_is_skipped_and_does_not_block() {
        let mut waits = node("file", &[], &["f"], &[]);
        waits.runnable = false;
        let g = [node("t", &["f"], &[], &[]), waits];
        let mut steps = plan(&g, 0).unwrap();
        assert_eq!(steps[0].state, BuildStepState::Skipped);
        assert_eq!(next_step(&mut steps), Some(1));
    }

    #[test]
    fn checks_are_validated_with_their_path() {
        let mut d = doc(vec![("a", vec![("x", "raw")], "SELECT 1")]);
        d.transforms[0].checks = Checks {
            min_rows: Some(2),
            max_rows: Some(1),
            max_drop_pct: Some(101),
            max_empty_pct: BTreeMap::from([("c".to_string(), 101)]),
            ..Default::default()
        };
        assert_eq!(
            tokens(&d),
            [
                "transforms.0.checks.min_rows=invalid",
                "transforms.0.checks.max_empty_pct.c=invalid",
                "transforms.0.checks.max_drop_pct=invalid",
            ]
        );
    }

    #[test]
    fn a_trigger_matches_its_dataset_and_event() {
        let ev = |event: &str| DatasetEvent {
            event: event.into(),
            dataset_id: "1".into(),
            dataset: "clean".into(),
            reference: "t".into(),
            counts: serde_json::Value::Null,
            changed: vec![],
            ts_ms: 0,
        };
        let cfg = DatasetTriggerConfig {
            dataset: "clean".into(),
            on: String::new(),
        };
        assert!(cfg.matches(&ev(DATASET_COMMITTED)));
        assert!(!cfg.matches(&ev(DATASET_SYNCED)));
        assert!(DatasetTriggerConfig::default().check().is_none());
        assert_eq!(
            DatasetTriggerConfig::default()
                .check_deployable()
                .unwrap()
                .0,
            "dataset_trigger_unset"
        );
        let bad = DatasetTriggerConfig {
            dataset: "clean".into(),
            on: "other".into(),
        };
        assert_eq!(bad.check().unwrap().0, "dataset_trigger_invalid");
    }
}
