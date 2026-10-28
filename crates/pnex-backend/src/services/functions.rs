//! "Functions" service — CRUD + append-only versioning + resolution for
//! the deploy projection + live test spawn. The backend never links
//! edgelink: it generates the text (pnex-core) and spawns the runtime binary.
//! Modelled on services/flow.rs.

use std::collections::{BTreeMap, BTreeSet};

use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder, Set,
    TransactionTrait,
};

use crate::models::_entities::{flow_versions, flows, function_versions, functions};
use crate::services::flow::FlowSettings;
use pnex_core::{
    CreateFunction, FlowGraph, FlowNodeKind, FlowViolation, FunctionExecRequest, FunctionLanguage,
    FunctionResolver, FunctionSnapshot, FunctionTestRequest, FunctionTestResponse,
    FunctionValidateRequest, FunctionValidateResponse, SaveFunctionVersion,
};

/// Service errors: 400 field by field (edge_refs/notify pattern) or
/// DB outage (500 in the controller).
#[derive(Debug)]
pub enum FunctionWriteError {
    /// (field, machine token: `required`, `max_length:200`).
    Field(String, String),
    /// Malformed `@input`/`@output` directive.
    Directive(pnex_core::DirectiveError),
    /// DB outage (turned into 500 by the controller).
    Db(String),
    /// Optimistic concurrency lost: `expected_version_number` is no longer
    /// the latest version (checked inside the save transaction).
    Conflict { current: i64 },
}

impl std::fmt::Display for FunctionWriteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FunctionWriteError::Field(field, msg) => write!(f, "{field} : {msg}"),
            FunctionWriteError::Directive(e) => write!(f, "code : {e}"),
            FunctionWriteError::Db(s) => write!(f, "{s}"),
            FunctionWriteError::Conflict { current } => {
                write!(
                    f,
                    "version conflict: the function is now at version {current}"
                )
            }
        }
    }
}

impl From<pnex_core::DirectiveError> for FunctionWriteError {
    fn from(e: pnex_core::DirectiveError) -> Self {
        FunctionWriteError::Directive(e)
    }
}

/// `"js" | "starlark"` → enum (application VARCHAR, flows.status pattern).
pub(crate) fn parse_language(s: &str) -> Option<FunctionLanguage> {
    match s {
        "js" => Some(FunctionLanguage::Js),
        "starlark" => Some(FunctionLanguage::Starlark),
        _ => None,
    }
}

/// Name: non-empty after trim, ≤ 200 (DB limit `StringLen(200)`).
fn validate_name(name: Option<&str>) -> Result<String, FunctionWriteError> {
    let name = name.unwrap_or_default().trim();
    if name.is_empty() {
        return Err(FunctionWriteError::Field(
            "name".into(),
            pnex_core::err_codes::FIELD_REQUIRED.into(),
        ));
    }
    if name.len() > 200 {
        return Err(FunctionWriteError::Field(
            "name".into(),
            format!("{}:200", pnex_core::err_codes::FIELD_MAX_LENGTH),
        ));
    }
    Ok(name.to_string())
}

/// Description: trim, empty → None.
fn clean_optional(value: &Option<String>) -> Option<String> {
    value
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Declared interface serialized for the `inputs`/`outputs` JSON columns.
fn signature_json(
    code: &str,
) -> Result<(serde_json::Value, serde_json::Value), FunctionWriteError> {
    let sig = pnex_core::parse_directives(code)?;
    let inputs = serde_json::to_value(&sig.inputs)
        .map_err(|e| FunctionWriteError::Db(format!("inputs serialization: {e}")))?;
    let outputs = serde_json::to_value(&sig.outputs)
        .map_err(|e| FunctionWriteError::Db(format!("outputs serialization: {e}")))?;
    Ok((inputs, outputs))
}

/// Creation: function + version 1 in one transaction (CreateFlow pattern).
pub async fn create_function(
    db: &DatabaseConnection,
    org_id: i64,
    input: &CreateFunction,
) -> Result<(functions::Model, function_versions::Model), FunctionWriteError> {
    let name = validate_name(Some(&input.name))?;
    let (inputs, outputs) = signature_json(&input.code)?;
    let description = clean_optional(&input.description);

    let txn = db
        .begin()
        .await
        .map_err(|e| FunctionWriteError::Db(format!("transaction : {e}")))?;
    let created = functions::ActiveModel {
        org_id: Set(org_id),
        name: Set(name),
        language: Set(input.language.to_string()),
        description: Set(description),
        ..Default::default()
    }
    .insert(&txn)
    .await
    .map_err(|e| FunctionWriteError::Db(format!("insert function: {e}")))?;

    let version = function_versions::ActiveModel {
        function_id: Set(created.id),
        org_id: Set(org_id),
        version_number: Set(1),
        code: Set(input.code.clone()),
        inputs: Set(inputs),
        outputs: Set(outputs),
        note: Set(clean_optional(&input.note)),
        ..Default::default()
    }
    .insert(&txn)
    .await
    .map_err(|e| FunctionWriteError::Db(format!("insert version : {e}")))?;

    // Circular FK: the current version is set once both rows exist.
    let mut active: functions::ActiveModel = created.clone().into();
    active.current_version_id = Set(Some(version.id));
    let created = active
        .update(&txn)
        .await
        .map_err(|e| FunctionWriteError::Db(format!("link current version: {e}")))?;
    txn.commit()
        .await
        .map_err(|e| FunctionWriteError::Db(format!("commit : {e}")))?;
    Ok((created, version))
}

/// Current version = highest `version_number` (append-only). One row
/// per function of the batch, one query per call.
pub(crate) async fn latest_version<C: sea_orm::ConnectionTrait>(
    db: &C,
    function_id: i64,
) -> Result<Option<function_versions::Model>, sea_orm::DbErr> {
    function_versions::Entity::find()
        .filter(function_versions::Column::FunctionId.eq(function_id))
        .order_by_desc(function_versions::Column::VersionNumber)
        .one(db)
        .await
}

/// Save: new append-only version (code provided) and/or metadata
/// update. Returns (updated function, new version if code).
pub async fn save_new_version(
    db: &DatabaseConnection,
    function_row: functions::Model,
    input: &SaveFunctionVersion,
) -> Result<(functions::Model, Option<function_versions::Model>), FunctionWriteError> {
    let name = if input.name.is_some() {
        Some(validate_name(input.name.as_deref())?)
    } else {
        None
    };
    let description = input
        .description
        .as_ref()
        .map(|_| clean_optional(&input.description));
    let sig = match &input.code {
        Some(code) => Some(signature_json(code)?),
        None => None,
    };

    let txn = db
        .begin()
        .await
        .map_err(|e| FunctionWriteError::Db(format!("transaction : {e}")))?;
    // Optimistic concurrency inside the transaction: the caller edited the
    // version it read; a save in between is a conflict, never overwritten.
    let latest_number = latest_version(&txn, function_row.id)
        .await
        .map_err(|e| FunctionWriteError::Db(format!("read max version: {e}")))?
        .map(|v| v.version_number)
        .unwrap_or(0);
    if latest_number != input.expected_version_number {
        return Err(FunctionWriteError::Conflict {
            current: latest_number,
        });
    }
    let mut active: functions::ActiveModel = function_row.clone().into();
    if let Some(n) = &name {
        active.name = Set(n.clone());
    }
    if let Some(d) = &description {
        active.description = Set(d.clone());
    }
    let updated = active
        .update(&txn)
        .await
        .map_err(|e| FunctionWriteError::Db(format!("update function: {e}")))?;

    let mut new_version = None;
    if let Some(code) = &input.code {
        let (inputs, outputs) = sig.expect("signature computed above");
        let previous = latest_version(&txn, function_row.id)
            .await
            .map_err(|e| FunctionWriteError::Db(format!("read max version: {e}")))?
            .map(|v| v.version_number)
            .unwrap_or(0);
        let version = function_versions::ActiveModel {
            function_id: Set(function_row.id),
            org_id: Set(function_row.org_id),
            version_number: Set(previous + 1),
            code: Set(code.clone()),
            inputs: Set(inputs),
            outputs: Set(outputs),
            note: Set(clean_optional(&input.note)),
            ..Default::default()
        }
        .insert(&txn)
        .await
        .map_err(|e| FunctionWriteError::Db(format!("insert version : {e}")))?;
        let mut link: functions::ActiveModel = updated.clone().into();
        link.current_version_id = Set(Some(version.id));
        link.update(&txn)
            .await
            .map_err(|e| FunctionWriteError::Db(format!("link current version: {e}")))?;
        new_version = Some(version);
    }
    txn.commit()
        .await
        .map_err(|e| FunctionWriteError::Db(format!("commit : {e}")))?;
    Ok((updated, new_version))
}

/// **Deployed** flows referencing the function (all orgs — the deletion
/// guard is cross-org for safety). Returns (flow, deployed
/// version) for the 409 `function_in_use`.
pub async fn referencing_deployed_flows(
    db: &DatabaseConnection,
    function_id: i64,
) -> Result<Vec<(flows::Model, i64)>, sea_orm::DbErr> {
    let deployed = flows::Entity::find()
        .filter(flows::Column::Status.eq(pnex_core::FLOW_STATUS_DEPLOYED))
        .all(db)
        .await?;
    let mut out = Vec::new();
    for flow in deployed {
        let Some(deployed_id) = flow.deployed_version_id else {
            continue;
        };
        let Some(version) = flow_versions::Entity::find_by_id(deployed_id)
            .one(db)
            .await?
        else {
            continue;
        };
        let Ok(graph) = serde_json::from_value::<FlowGraph>(version.graph.clone()) else {
            continue;
        };
        for node in &graph.nodes {
            if matches!(
                &node.kind,
                FlowNodeKind::PnexFunction { config } if config.function_id == function_id
            ) {
                out.push((flow, version.version_number));
                break;
            }
        }
    }
    Ok(out)
}

/// Batch resolution of the functions referenced by the `PnexFunction` nodes
/// of the batch (deployed flows + candidate): feeds `to_red_flows_json_with`.
/// A missing ref (deleted function/version, cross org) = a
/// `function_unresolved` violation located on the node — deploy answers 400, the
/// removal/self-healing paths warn and carry on (artifact with
/// no-op `comment` entries).
pub async fn resolve_for_projection(
    db: &DatabaseConnection,
    pairs: &[(flows::Model, flow_versions::Model)],
) -> Result<(FunctionResolver, Vec<FlowViolation>), sea_orm::DbErr> {
    let mut violations = Vec::new();
    // Distinct (function_id, version_number) references.
    let mut refs: BTreeSet<(i64, i64)> = BTreeSet::new();
    for (_, version) in pairs {
        let Ok(graph) = serde_json::from_value::<FlowGraph>(version.graph.clone()) else {
            continue;
        };
        for node in &graph.nodes {
            if let FlowNodeKind::PnexFunction { config } = &node.kind {
                if config.function_id > 0 && config.version_number > 0 {
                    refs.insert((config.function_id, config.version_number));
                }
            }
        }
    }
    if refs.is_empty() {
        return Ok((FunctionResolver::new(), violations));
    }

    let function_ids: Vec<i64> = refs.iter().map(|(fid, _)| *fid).collect();
    let version_numbers: Vec<i64> = refs.iter().map(|(_, vn)| *vn).collect();
    let function_rows = functions::Entity::find()
        .filter(functions::Column::Id.is_in(function_ids.clone()))
        .all(db)
        .await?;
    let version_rows = function_versions::Entity::find()
        .filter(function_versions::Column::FunctionId.is_in(function_ids))
        .filter(function_versions::Column::VersionNumber.is_in(version_numbers))
        .all(db)
        .await?;

    let mut snapshots: BTreeMap<(i64, i64), (i64, FunctionSnapshot)> = BTreeMap::new();
    for v in version_rows {
        let Some(f) = function_rows.iter().find(|f| f.id == v.function_id) else {
            continue;
        };
        let Some(lang) = parse_language(&f.language) else {
            continue;
        };
        let snapshot = FunctionSnapshot {
            function_id: v.function_id,
            version_number: v.version_number,
            language: lang,
            code: v.code.clone(),
            inputs: serde_json::from_value(v.inputs.clone()).unwrap_or_default(),
            outputs: serde_json::from_value(v.outputs.clone()).unwrap_or_default(),
        };
        snapshots.insert((v.function_id, v.version_number), (f.org_id, snapshot));
    }

    // Node by node: ref resolvable in the same org → snapshot; otherwise
    // a located violation (the node id guides the editor).
    let mut resolver = FunctionResolver::new();
    for (flow, version) in pairs {
        let Ok(graph) = serde_json::from_value::<FlowGraph>(version.graph.clone()) else {
            continue;
        };
        for node in &graph.nodes {
            let FlowNodeKind::PnexFunction { config } = &node.kind else {
                continue;
            };
            match snapshots.get(&(config.function_id, config.version_number)) {
                Some((owner_org, snapshot)) if *owner_org == flow.org_id => {
                    resolver.insert(
                        (config.function_id, config.version_number),
                        snapshot.clone(),
                    );
                }
                _ => violations.push(FlowViolation::new(
                    Some(&node.id),
                    "function_unresolved",
                    format!(
                        "function \"{}\" v{} not found",
                        config.function_name, config.version_number
                    ),
                )),
            }
        }
    }
    Ok((resolver, violations))
}

/// Live test: resolves (language, code, interface) then spawns the binary
/// `pnex-flow-runtime --test-function <req.json>` — mirror of the
/// supervisor's harness (`run_preflight`): `resolve_program` + `env_clear()` +
/// `apply_runtime_env`, stdout piped, deadline with kill. Err = infra
/// outage (503 in the controller); the script result travels in the
/// `FunctionTestResponse` (ok:false + error).
pub async fn spawn_function_test(
    settings: &FlowSettings,
    db: &DatabaseConnection,
    org_id: i64,
    function_id: i64,
    req: &FunctionTestRequest,
) -> Result<FunctionTestResponse, String> {
    // Org frame: the function must belong to the caller's org.
    let function_row = functions::Entity::find_by_id(function_id)
        .one(db)
        .await
        .map_err(|e| format!("read function: {e}"))?
        .ok_or_else(|| "function not found".to_string())?;
    if function_row.org_id != org_id {
        return Err("function not found".to_string());
    }

    // Code resolution: ad-hoc (test before save) or pinned version
    // (default = current version).
    let (language, code, inputs_decl, outputs_decl) = if let Some(adhoc) = &req.ad_hoc {
        let sig = pnex_core::parse_directives(&adhoc.code)
            .map_err(|e| format!("directives: line {}: {}", e.line, e.message))?;
        (adhoc.language, adhoc.code.clone(), sig.inputs, sig.outputs)
    } else {
        let version_number = match req.version_number {
            Some(n) => n,
            None => {
                let current = function_row
                    .current_version_id
                    .ok_or_else(|| "no version: the function is empty".to_string())?;
                function_versions::Entity::find_by_id(current)
                    .one(db)
                    .await
                    .map_err(|e| format!("read current version: {e}"))?
                    .ok_or_else(|| "current version not found".to_string())?
                    .version_number
            }
        };
        let version = function_versions::Entity::find()
            .filter(function_versions::Column::FunctionId.eq(function_id))
            .filter(function_versions::Column::VersionNumber.eq(version_number))
            .one(db)
            .await
            .map_err(|e| format!("version read: {e}"))?
            .ok_or_else(|| format!("version v{version_number} not found"))?;
        let language =
            parse_language(&function_row.language).ok_or_else(|| "unknown language".to_string())?;
        (
            language,
            version.code.clone(),
            serde_json::from_value(version.inputs).unwrap_or_default(),
            serde_json::from_value(version.outputs).unwrap_or_default(),
        )
    };

    // Effective msg: user msg + **typed form values**
    // injected at the implicit `payload.<name>` paths — without this, the
    // values typed in the test panel never reach
    // `inputs[...]` (seen in E2E: NoneType vs float comparison).
    let mut msg = req.msg.clone().unwrap_or(serde_json::json!({}));
    for (name, value) in &req.inputs {
        let path = pnex_core::default_binding(name);
        pnex_core::insert_msg_path(&mut msg, &path, value.clone());
    }
    let exec = FunctionExecRequest {
        language,
        // JS: the wrapper (helpers + code + tail) is generated here — the CLI
        // only evaluates already-prepared text.
        js_wrapper: match language {
            FunctionLanguage::Js => Some(pnex_core::build_js_wrapper(
                &code,
                &inputs_decl,
                &outputs_decl,
            )),
            FunctionLanguage::Starlark => None,
        },
        code: match language {
            FunctionLanguage::Starlark => Some(code.clone()),
            FunctionLanguage::Js => None,
        },
        inputs: inputs_decl,
        msg,
        timeout_ms: 5000,
    };

    // Temp file + one-shot spawn (scaffolding shared with the compile-only
    // check, modelled on the supervisor's harness).
    let (status, line) = run_runtime_with_request(
        settings,
        "--test-function",
        &serde_json::to_value(&exec).map_err(|e| format!("request serialization: {e}"))?,
    )
    .await?;

    // One JSON line expected (the CLI prints FunctionTestResponse with exit 0
    // even if ok:false); unreadable output = infra outage.
    match serde_json::from_str::<FunctionTestResponse>(&line) {
        Ok(resp) => Ok(resp),
        Err(_) => {
            let detail = if status.success() {
                "unreadable output".to_string()
            } else {
                format!("exit {status}")
            };
            Err(format!("test runtime: {detail}"))
        }
    }
}

/// One-shot spawn of the runtime binary in function mode (`--test-function` /
/// `--check-function`): request serialized into a temp file (modelled on
/// flows.json.candidate: the contract goes through a file, not stdin),
/// `resolve_program` + `env_clear()` + `apply_runtime_env`, stdout piped,
/// 10 s deadline with kill. Returns (exit status, first stdout line).
async fn run_runtime_with_request(
    settings: &FlowSettings,
    cli_arg: &str,
    request: &serde_json::Value,
) -> Result<(std::process::ExitStatus, String), String> {
    // Unique suffix: pid + nanos (same pattern as the supervisor's temp).
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    let request_path =
        std::env::temp_dir().join(format!("pnex-fn-run-{}-{}.json", std::process::id(), nanos));

    let program = crate::services::flow_supervisor::resolve_program(&settings.runtime_cmd);
    let mut cmd = tokio::process::Command::new(&program);
    cmd.arg(cli_arg)
        .arg(&request_path)
        .env_clear()
        .kill_on_drop(true)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    crate::services::flow_supervisor::apply_runtime_env(&mut cmd, settings);

    std::fs::write(
        &request_path,
        serde_json::to_string(request).map_err(|e| format!("request serialization: {e}"))?,
    )
    .map_err(|e| format!("request write: {e}"))?;

    let wait = async {
        let mut child = cmd.spawn().map_err(|e| format!("runtime launch: {e}"))?;
        let mut stdout = String::new();
        if let Some(pipe) = child.stdout.take() {
            use tokio::io::AsyncReadExt;
            let mut pipe = pipe;
            pipe.read_to_string(&mut stdout)
                .await
                .map_err(|e| format!("read stdout: {e}"))?;
        }
        let status = child.wait().await.map_err(|e| format!("wait: {e}"))?;
        Ok::<(std::process::ExitStatus, String), String>((status, stdout))
    };
    let (status, stdout) =
        match tokio::time::timeout(std::time::Duration::from_secs(10), wait).await {
            Ok(result) => result?,
            Err(_) => {
                let _ = std::fs::remove_file(&request_path);
                return Err(format!("runtime timed out (10 s): {cli_arg}"));
            }
        };
    let _ = std::fs::remove_file(&request_path);
    Ok((
        status,
        stdout.lines().next().unwrap_or_default().to_string(),
    ))
}

/// Compile-only validation (editor error bar): forwards the
/// request to the `pnex-flow-runtime --check-function` CLI. No DB access —
/// the body already carries the language and ad-hoc code.
pub async fn spawn_function_check(
    settings: &FlowSettings,
    req: &FunctionValidateRequest,
) -> Result<FunctionValidateResponse, String> {
    let (status, line) = run_runtime_with_request(
        settings,
        "--check-function",
        &serde_json::to_value(req).map_err(|e| format!("request serialization: {e}"))?,
    )
    .await?;
    match serde_json::from_str::<FunctionValidateResponse>(&line) {
        Ok(resp) => Ok(resp),
        Err(_) => {
            let detail = if status.success() {
                "unreadable output".to_string()
            } else {
                format!("exit {status}")
            };
            Err(format!("validation runtime: {detail}"))
        }
    }
}
