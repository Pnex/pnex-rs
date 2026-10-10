//! Service « Fonctions » — CRUD + versioning append-only + résolution pour
//! la projection deploy + spawn du test en live. Le backend ne lie jamais
//! edgelink : il génère le texte (pnex-core) et spawn le binaire runtime.
//! École services/flow.rs.

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

/// Erreurs du service : 400 champ-par-champ (école edge_refs/notify) ou
/// panne DB (500 au contrôleur).
#[derive(Debug)]
pub enum FunctionWriteError {
    /// (champ, message affichable).
    Field(String, String),
    /// Directive `@input`/`@output` malformée.
    Directive(pnex_core::DirectiveError),
    /// Panne DB (mise en 500 par le contrôleur).
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

/// `"js" | "starlark"` → enum (VARCHAR applicatif, école flows.status).
pub(crate) fn parse_language(s: &str) -> Option<FunctionLanguage> {
    match s {
        "js" => Some(FunctionLanguage::Js),
        "starlark" => Some(FunctionLanguage::Starlark),
        _ => None,
    }
}

/// Nom : trim non vide, ≤ 200 (limite DB `StringLen(200)`).
fn validate_name(name: Option<&str>) -> Result<String, FunctionWriteError> {
    let name = name.unwrap_or_default().trim();
    if name.is_empty() {
        return Err(FunctionWriteError::Field(
            "name".into(),
            "le nom est requis".into(),
        ));
    }
    if name.len() > 200 {
        return Err(FunctionWriteError::Field(
            "name".into(),
            "maximum 200 characters".into(),
        ));
    }
    Ok(name.to_string())
}

/// Description : trim, vide → None.
fn clean_optional(value: &Option<String>) -> Option<String> {
    value
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Interface déclarée sérialisée pour les colonnes JSON `inputs`/`outputs`.
fn signature_json(
    code: &str,
) -> Result<(serde_json::Value, serde_json::Value), FunctionWriteError> {
    let sig = pnex_core::parse_directives(code)?;
    let inputs = serde_json::to_value(&sig.inputs)
        .map_err(|e| FunctionWriteError::Db(format!("sérialisation inputs : {e}")))?;
    let outputs = serde_json::to_value(&sig.outputs)
        .map_err(|e| FunctionWriteError::Db(format!("sérialisation outputs : {e}")))?;
    Ok((inputs, outputs))
}

/// Création : fonction + version 1 en une transaction (école CreateFlow).
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
    .map_err(|e| FunctionWriteError::Db(format!("insert fonction : {e}")))?;

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
        .map_err(|e| FunctionWriteError::Db(format!("liaison version courante : {e}")))?;
    txn.commit()
        .await
        .map_err(|e| FunctionWriteError::Db(format!("commit : {e}")))?;
    Ok((created, version))
}

/// Version courante = plus grande `version_number` (append-only). Une ligne
/// par fonction du lot, une requête par appel.
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

/// Enregistrement : nouvelle version append-only (code fourni) et/ou mise à
/// jour de métadonnées. Retourne (fonction à jour, nouvelle version si code).
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
        .map_err(|e| FunctionWriteError::Db(format!("lecture version max : {e}")))?
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
        .map_err(|e| FunctionWriteError::Db(format!("update fonction : {e}")))?;

    let mut new_version = None;
    if let Some(code) = &input.code {
        let (inputs, outputs) = sig.expect("signature calculée plus haut");
        let previous = latest_version(&txn, function_row.id)
            .await
            .map_err(|e| FunctionWriteError::Db(format!("lecture version max : {e}")))?
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
            .map_err(|e| FunctionWriteError::Db(format!("liaison version courante : {e}")))?;
        new_version = Some(version);
    }
    txn.commit()
        .await
        .map_err(|e| FunctionWriteError::Db(format!("commit : {e}")))?;
    Ok((updated, new_version))
}

/// Flows **déployés** référençant la fonction (toutes orgs — le garde de
/// suppression est inter-org par sécurité). Retourne (flow, version
/// déployée) pour le 409 `function_in_use`.
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

/// Résolution batch des fonctions référencées par les nœuds `PnexFunction`
/// du lot (flows déployés + candidat) : alimente `to_red_flows_json_with`.
/// Une ref manquante (fonction/version supprimée, org croisée) = violation
/// `function_unresolved` localisée au nœud — le deploy répond 400, les
/// chemins de retrait/self-healing warn et poursuivent (artefact avec
/// entrées `comment` no-op).
pub async fn resolve_for_projection(
    db: &DatabaseConnection,
    pairs: &[(flows::Model, flow_versions::Model)],
) -> Result<(FunctionResolver, Vec<FlowViolation>), sea_orm::DbErr> {
    let mut violations = Vec::new();
    // Références (function_id, version_number) distinctes.
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

    // Nœud par nœud : ref résoluble dans la même org → snapshot ; sinon
    // violation localisée (l'id du nœud guide l'éditeur).
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
                        "fonction « {} » v{} introuvable",
                        config.function_name, config.version_number
                    ),
                )),
            }
        }
    }
    Ok((resolver, violations))
}

/// Test en live : résout (langage, code, interface) puis spawn le binaire
/// `pnex-flow-runtime --test-function <req.json>` — miroir du harnais du
/// superviseur (`run_preflight`) : `resolve_program` + `env_clear()` +
/// `apply_runtime_env`, stdout pipé, deadline avec kill. Err = panne
/// d'infra (503 au contrôleur) ; le résultat script voyage dans la
/// `FunctionTestResponse` (ok:false + error).
pub async fn spawn_function_test(
    settings: &FlowSettings,
    db: &DatabaseConnection,
    org_id: i64,
    function_id: i64,
    req: &FunctionTestRequest,
) -> Result<FunctionTestResponse, String> {
    // Cadre org : la fonction doit appartenir à l'org de l'appelant.
    let function_row = functions::Entity::find_by_id(function_id)
        .one(db)
        .await
        .map_err(|e| format!("lecture fonction : {e}"))?
        .ok_or_else(|| "fonction introuvable".to_string())?;
    if function_row.org_id != org_id {
        return Err("fonction introuvable".to_string());
    }

    // Résolution du code : ad-hoc (test avant save) ou version épinglée
    // (défaut = version courante).
    let (language, code, inputs_decl, outputs_decl) = if let Some(adhoc) = &req.ad_hoc {
        let sig = pnex_core::parse_directives(&adhoc.code)
            .map_err(|e| format!("directives : ligne {} : {}", e.line, e.message))?;
        (adhoc.language, adhoc.code.clone(), sig.inputs, sig.outputs)
    } else {
        let version_number = match req.version_number {
            Some(n) => n,
            None => {
                let current = function_row
                    .current_version_id
                    .ok_or_else(|| "aucune version : la fonction est vide".to_string())?;
                function_versions::Entity::find_by_id(current)
                    .one(db)
                    .await
                    .map_err(|e| format!("lecture version courante : {e}"))?
                    .ok_or_else(|| "version courante introuvable".to_string())?
                    .version_number
            }
        };
        let version = function_versions::Entity::find()
            .filter(function_versions::Column::FunctionId.eq(function_id))
            .filter(function_versions::Column::VersionNumber.eq(version_number))
            .one(db)
            .await
            .map_err(|e| format!("lecture version : {e}"))?
            .ok_or_else(|| format!("version v{version_number} introuvable"))?;
        let language =
            parse_language(&function_row.language).ok_or_else(|| "langage inconnu".to_string())?;
        (
            language,
            version.code.clone(),
            serde_json::from_value(version.inputs).unwrap_or_default(),
            serde_json::from_value(version.outputs).unwrap_or_default(),
        )
    };

    // Msg effectif : msg utilisateur + **valeurs du formulaire typé**
    // injectées aux chemins implicites `payload.<name>` — sans ça, les
    // valeurs saisies dans le panneau de test n'atteignent jamais
    // `inputs[...]` (constaté en E2E : comparaison NoneType vs float).
    let mut msg = req.msg.clone().unwrap_or(serde_json::json!({}));
    for (name, value) in &req.inputs {
        let path = pnex_core::default_binding(name);
        pnex_core::insert_msg_path(&mut msg, &path, value.clone());
    }
    let exec = FunctionExecRequest {
        language,
        // JS : le wrapper (helpers + code + tail) est généré ici — le CLI
        // n'évalue que du texte déjà préparé.
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

    // Fichier temp + spawn one-shot (scaffolding partagé avec le check
    // compile-only, école du harnais du superviseur).
    let (status, line) = run_runtime_with_request(
        settings,
        "--test-function",
        &serde_json::to_value(&exec).map_err(|e| format!("sérialisation requête : {e}"))?,
    )
    .await?;

    // Une ligne JSON attendue (le CLI imprime FunctionTestResponse en exit 0
    // même si ok:false) ; une sortie illisible = panne d'infra.
    match serde_json::from_str::<FunctionTestResponse>(&line) {
        Ok(resp) => Ok(resp),
        Err(_) => {
            let detail = if status.success() {
                "sortie illisible".to_string()
            } else {
                format!("exit {status}")
            };
            Err(format!("runtime de test : {detail}"))
        }
    }
}

/// Spawn one-shot du binaire runtime en mode function (`--test-function` /
/// `--check-function`) : requête sérialisée dans un fichier temp (école
/// flows.json.candidate : le contrat passe par un fichier, pas par stdin),
/// `resolve_program` + `env_clear()` + `apply_runtime_env`, stdout pipé,
/// deadline 10 s avec kill. Retour = (exit status, première ligne stdout).
async fn run_runtime_with_request(
    settings: &FlowSettings,
    cli_arg: &str,
    request: &serde_json::Value,
) -> Result<(std::process::ExitStatus, String), String> {
    // Suffixe unique : pid + nanos (même école que le temp du superviseur).
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
        serde_json::to_string(request).map_err(|e| format!("sérialisation requête : {e}"))?,
    )
    .map_err(|e| format!("écriture requête : {e}"))?;

    let wait = async {
        let mut child = cmd
            .spawn()
            .map_err(|e| format!("lancement du runtime : {e}"))?;
        let mut stdout = String::new();
        if let Some(pipe) = child.stdout.take() {
            use tokio::io::AsyncReadExt;
            let mut pipe = pipe;
            pipe.read_to_string(&mut stdout)
                .await
                .map_err(|e| format!("lecture stdout : {e}"))?;
        }
        let status = child.wait().await.map_err(|e| format!("attente : {e}"))?;
        Ok::<(std::process::ExitStatus, String), String>((status, stdout))
    };
    let (status, stdout) =
        match tokio::time::timeout(std::time::Duration::from_secs(10), wait).await {
            Ok(result) => result?,
            Err(_) => {
                let _ = std::fs::remove_file(&request_path);
                return Err(format!("délai du runtime dépassé (10 s) : {cli_arg}"));
            }
        };
    let _ = std::fs::remove_file(&request_path);
    Ok((
        status,
        stdout.lines().next().unwrap_or_default().to_string(),
    ))
}

/// Validation compile-only (barre d'erreurs de l'éditeur) : retransmet la
/// requête au CLI `pnex-flow-runtime --check-function`. Aucun accès DB —
/// le corps porte déjà langage et code ad-hoc.
pub async fn spawn_function_check(
    settings: &FlowSettings,
    req: &FunctionValidateRequest,
) -> Result<FunctionValidateResponse, String> {
    let (status, line) = run_runtime_with_request(
        settings,
        "--check-function",
        &serde_json::to_value(req).map_err(|e| format!("sérialisation requête : {e}"))?,
    )
    .await?;
    match serde_json::from_str::<FunctionValidateResponse>(&line) {
        Ok(resp) => Ok(resp),
        Err(_) => {
            let detail = if status.success() {
                "sortie illisible".to_string()
            } else {
                format!("exit {status}")
            };
            Err(format!("runtime de validation : {detail}"))
        }
    }
}
