//! Watch checks (network-check.md D426–D431). A run of a check on a subject
//! within a scope updates a working state (`check_states`); only a run whose
//! fingerprint differs from the previous one writes objects: the open event
//! of the check closes, a new one opens in the subject's watch case (found
//! by `watch_key`, opened when absent), and the case's `watch_state` follows
//! the worst latest result of its checks. Every run is also one line of the
//! org's O2 `check_runs` stream, sent by the caller after the commit.

use std::collections::{HashMap, HashSet};

use pnex_core::err_codes::{FIELD_INVALID, FIELD_INVALID_UUID};
use pnex_core::ontology::api::{CaseAttachInput, CaseCheck, CaseEventInput, ObjectInput};
use pnex_core::ontology::check::{worst, CheckRecordAck, CheckRun};
use pnex_core::ontology::{
    CASE_STATUS, CASE_WATCH_KEY, CASE_WATCH_STATE, EVENT_NOTE, EVENT_OCCURRED, KIND_CASE,
};
use sea_orm::{
    ConnectionTrait, DatabaseConnection, DbBackend, DbErr, FromQueryResult, Statement,
    TransactionTrait,
};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::{case_of, create_event_in, invalid, links, objects, Actor, OntologyError, Result};
use crate::services::resources::containment;

/// O2 stream of the runs (system stream, like `object_changes`).
pub const CHECK_RUNS_STREAM: &str = "check_runs";

#[derive(Debug, FromQueryResult)]
struct StateRow {
    case_id: Option<Uuid>,
    fingerprint: String,
    event_id: Option<Uuid>,
    last_run_at: chrono::DateTime<chrono::FixedOffset>,
}

#[derive(Debug, FromQueryResult)]
struct CheckRow {
    check_key: String,
    result: String,
    rules: Value,
    source_ref: String,
    changed_at: chrono::DateTime<chrono::FixedOffset>,
    last_run_at: chrono::DateTime<chrono::FixedOffset>,
    runs_since_change: i64,
}

/// A recorded run: the answer, and the journal line to send.
pub struct Recorded {
    pub ack: CheckRecordAck,
    pub line: Value,
}

fn utc(t: chrono::DateTime<chrono::Utc>) -> String {
    t.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// Serializes the gestures on one key until the transaction ends.
async fn lock<C: ConnectionTrait>(db: &C, key: &str) -> Result<()> {
    db.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT pg_advisory_xact_lock(hashtextextended($1, 0))",
        [key.into()],
    ))
    .await?;
    Ok(())
}

/// Every id of the run, parsed, and live in the org (R1).
async fn checked_ids<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    run: &CheckRun,
) -> Result<(Uuid, Vec<Uuid>, Vec<Uuid>, Vec<Uuid>)> {
    let parse = |field: &str, raw: &str| {
        Uuid::parse_str(raw.trim()).map_err(|_| invalid(field, FIELD_INVALID_UUID))
    };
    let subject = parse("subject_id", &run.subject_id)?;
    let scope = run
        .scope_ids
        .iter()
        .map(|s| parse("scope_ids", s))
        .collect::<Result<Vec<_>>>()?;
    let findings = run
        .rules
        .iter()
        .filter_map(|r| r.finding_id.as_deref())
        .map(|s| parse("rules", s))
        .collect::<Result<Vec<_>>>()?;
    let context = run
        .objects
        .iter()
        .map(|s| parse("objects", s))
        .collect::<Result<Vec<_>>>()?;
    let all: HashSet<Uuid> = std::iter::once(subject)
        .chain(scope.iter().copied())
        .chain(findings.iter().copied())
        .chain(context.iter().copied())
        .collect();
    let found = links::refs(db, org_id, all.iter().copied()).await?;
    let live = |id: &Uuid| found.get(id).is_some_and(|o| o.valid_to.is_none());
    for (field, ids) in [
        ("subject_id", std::slice::from_ref(&subject)),
        ("scope_ids", &scope[..]),
        ("rules", &findings[..]),
        ("objects", &context[..]),
    ] {
        if !ids.iter().all(live) {
            return Err(invalid(field, FIELD_INVALID));
        }
    }
    let mut findings = findings;
    // Findings named by key: the ones already written join the evidence.
    if let (Some(t), Some(k)) = (&run.finding_type, &run.finding_key) {
        for r in run.rules.iter().filter_map(|r| r.finding_ref.as_deref()) {
            if let Some(o) = objects::find_by_key(db, org_id, t, k, &r.into()).await? {
                if o.valid_to.is_none() && !findings.contains(&o.id) {
                    findings.push(o.id);
                }
            }
        }
    }
    Ok((subject, scope, findings, context))
}

/// The live case whose `watch_key` is `key`, opened with `title` when
/// absent (under `parent` when given). `None` when it was archived by hand.
async fn watch_case<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    actor: &Actor,
    key: &str,
    title: &str,
) -> Result<(Uuid, bool, bool)> {
    if let Some(c) =
        objects::find_by_key(db, org_id, KIND_CASE, CASE_WATCH_KEY, &key.into()).await?
    {
        let archived = c.properties.get(CASE_STATUS).and_then(Value::as_str) == Some("archived");
        return Ok((c.id, archived, false));
    }
    let mut props = Map::new();
    props.insert(CASE_STATUS.into(), "open".into());
    props.insert(CASE_WATCH_KEY.into(), key.into());
    let title = if title.trim().is_empty() { key } else { title };
    let case = objects::create(
        db,
        org_id,
        actor,
        &ObjectInput {
            type_key: KIND_CASE.into(),
            title: title.into(),
            properties: props,
            expected_version: None,
        },
    )
    .await?;
    let id = Uuid::parse_str(&case.id).map_err(|_| OntologyError::NotFound)?;
    Ok((id, false, true))
}

/// The watch case of `subject`, from any of its checks.
async fn subject_case<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    subject: Uuid,
) -> Result<Option<Uuid>> {
    #[derive(FromQueryResult)]
    struct Row {
        case_id: Uuid,
    }
    Ok(Row::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT case_id FROM check_states WHERE org_id = $1 AND subject_id = $2 \
         AND case_id IS NOT NULL ORDER BY changed_at DESC LIMIT 1",
        [org_id.into(), subject.into()],
    ))
    .one(db)
    .await?
    .map(|r| r.case_id))
}

/// Whether the case is gone or archived: no event is written in it.
async fn archived<C: ConnectionTrait>(db: &C, org_id: i64, case_id: Uuid) -> Result<bool> {
    match case_of(db, org_id, case_id).await {
        Ok(c) => Ok(c.properties.get(CASE_STATUS).and_then(Value::as_str) == Some("archived")),
        Err(OntologyError::NotFound) => Ok(true),
        Err(e) => Err(e),
    }
}

/// Records one run. `origin` is the event origin (`flow`, `import`).
pub async fn record(
    db: &DatabaseConnection,
    org_id: i64,
    actor: &Actor,
    origin: &str,
    run: &CheckRun,
) -> Result<Recorded> {
    if let Some(field) = run.check_shape() {
        return Err(invalid(field, FIELD_INVALID));
    }
    let result = run.result();
    let fingerprint = format!("{:x}", Sha256::digest(run.canonical().as_bytes()));
    let scope_key = run.scope_key();
    let now = chrono::Utc::now();
    let mut new_parent: Option<(Uuid, Uuid)> = None;
    let ack = crate::services::ontology::changes::deferred(async {
        let txn = db.begin().await?;
        let (subject, scope, findings, context) = checked_ids(&txn, org_id, run).await?;
        // Parent first, then the case, then the state: one order everywhere.
        if let Some(p) = run.case.as_ref().and_then(|c| c.parent_key.as_deref()) {
            lock(&txn, &format!("watch-case:{org_id}:{p}")).await?;
        }
        if let Some(c) = &run.case {
            lock(&txn, &format!("watch-case:{org_id}:{}", c.key)).await?;
        }
        lock(
            &txn,
            &format!("check:{org_id}:{}:{subject}:{scope_key}", run.check),
        )
        .await?;
        let state = StateRow::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT case_id, fingerprint, event_id, last_run_at FROM check_states \
             WHERE org_id = $1 AND check_key = $2 AND subject_id = $3 AND scope_key = $4",
            [
                org_id.into(),
                run.check.clone().into(),
                subject.into(),
                scope_key.clone().into(),
            ],
        ))
        .one(&txn)
        .await?;
        if let Some(s) = state.as_ref().filter(|s| s.fingerprint == fingerprint) {
            txn.execute_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "UPDATE check_states SET last_run_at = $5, runs = runs + 1, \
                 runs_since_change = runs_since_change + 1 \
                 WHERE org_id = $1 AND check_key = $2 AND subject_id = $3 AND scope_key = $4",
                [
                    org_id.into(),
                    run.check.clone().into(),
                    subject.into(),
                    scope_key.clone().into(),
                    now.into(),
                ],
            ))
            .await?;
            txn.commit().await?;
            return Result::Ok(CheckRecordAck {
                result: result.into(),
                changed: false,
                case_id: s.case_id.map(|c| c.to_string()),
                event_id: s.event_id.map(|e| e.to_string()),
            });
        }
        // The case: the one already known, else found or opened by key.
        let case = match &run.case {
            Some(c) => {
                let (id, archived, created) =
                    watch_case(&txn, org_id, actor, &c.key, &c.title).await?;
                if created {
                    if let Some(pk) = &c.parent_key {
                        let title = c.parent_title.as_deref().unwrap_or(pk);
                        let (pid, _, _) = watch_case(&txn, org_id, actor, pk, title).await?;
                        new_parent = Some((id, pid));
                    }
                }
                Some((id, archived))
            }
            // No case named: the one the subject already has (an import, a
            // generic pack); a subject outside any watch only counts.
            None => match state.as_ref().and_then(|s| s.case_id) {
                Some(id) => Some((id, false)),
                None => subject_case(&txn, org_id, subject)
                    .await?
                    .map(|id| (id, false)),
            },
        };
        // A case archived by hand stays as is (D429).
        let case = match case {
            Some((id, false)) => Some((id, archived(&txn, org_id, id).await?)),
            other => other,
        };
        // The open event of the check closes at its last run.
        if let Some(s) = &state {
            if let Some(eid) = s.event_id {
                close_event(&txn, org_id, actor, eid, s.last_run_at).await?;
            }
        }
        let event_id = match case {
            Some((case_id, false)) => Some(
                new_event(
                    &txn,
                    org_id,
                    actor,
                    case_id,
                    origin,
                    run,
                    (subject, &scope, &findings, &context),
                    now,
                )
                .await?,
            ),
            _ => None,
        };
        let case_id = case.map(|(id, _)| id);
        txn.execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "INSERT INTO check_states (org_id, check_key, subject_id, scope_key, case_id, \
             fingerprint, result, rules, event_id, source_ref, changed_at, first_run_at, \
             last_run_at) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $11, $11) \
             ON CONFLICT (org_id, check_key, subject_id, scope_key) DO UPDATE SET \
             case_id = EXCLUDED.case_id, fingerprint = EXCLUDED.fingerprint, \
             result = EXCLUDED.result, rules = EXCLUDED.rules, event_id = EXCLUDED.event_id, \
             source_ref = EXCLUDED.source_ref, changed_at = EXCLUDED.changed_at, \
             last_run_at = EXCLUDED.last_run_at, runs = check_states.runs + 1, \
             runs_since_change = 1",
            [
                org_id.into(),
                run.check.clone().into(),
                subject.into(),
                scope_key.clone().into(),
                case_id.into(),
                fingerprint.clone().into(),
                result.into(),
                serde_json::to_value(&run.rules).unwrap_or_default().into(),
                event_id.into(),
                actor.source_ref.clone().into(),
                now.into(),
            ],
        ))
        .await?;
        if let Some((cid, false)) = case {
            sync_watch_state(&txn, org_id, actor, cid).await?;
        }
        txn.commit().await?;
        crate::services::ontology::changes::flush();
        Ok(CheckRecordAck {
            result: result.into(),
            changed: true,
            case_id: case_id.map(|c| c.to_string()),
            event_id: event_id.map(|e| e.to_string()),
        })
    })
    .await?;
    // GS16: a new case goes under its parent (outside the transaction, as
    // `cases::open` does).
    if let Some((child, parent)) = new_parent {
        containment::set_parent(
            db,
            org_id,
            KIND_CASE,
            &child.to_string(),
            Some((KIND_CASE, &parent.to_string())),
            None,
        )
        .await
        .map_err(|e| OntologyError::Db(DbErr::Custom(format!("case containment: {e:?}"))))?;
    }
    let line = json!({
        "check": run.check,
        "subject_id": run.subject_id.trim(),
        "scope": scope_key,
        "result": result,
        // O2 flattens nested objects: kept as text.
        "rules": serde_json::to_string(&run.rules).unwrap_or_default(),
        "objects": run.objects.join(","),
        "fingerprint": fingerprint,
        "changed": ack.changed,
        "case_id": ack.case_id.clone().unwrap_or_default(),
        "event_id": ack.event_id.clone().unwrap_or_default(),
        "source_ref": actor.source_ref,
    });
    Ok(Recorded { ack, line })
}

/// O2 settings and database of the journal, set at boot.
static JOURNAL: std::sync::OnceLock<(
    crate::services::openobserve::OpenobserveSettings,
    DatabaseConnection,
)> = std::sync::OnceLock::new();

/// Called at boot (`after_routes`): without O2 the runs are not journaled.
pub fn init_journal(ctx: &loco_rs::prelude::AppContext) {
    if let Some(s) = crate::services::openobserve::OpenobserveSettings::from_config(&ctx.config) {
        let _ = JOURNAL.set((s, ctx.db.clone()));
    }
}

/// Sends a run's line to the org's `check_runs` stream, in the background:
/// a journal failure never fails the run it describes.
pub fn journal(org_id: i64, line: Value) {
    use crate::services::openobserve::{ensure_org_credentials, Client};
    let Some((settings, db)) = JOURNAL.get() else {
        return;
    };
    let (settings, db) = (settings.clone(), db.clone());
    tokio::spawn(async move {
        let client = Client::new(&settings);
        let mut line = line;
        line["_timestamp"] = chrono::Utc::now().timestamp_micros().into();
        let sent = match ensure_org_credentials(&db, &client, org_id).await {
            Ok(c) => {
                client
                    .ingest_json(&c.o2_org, CHECK_RUNS_STREAM, &[line], &c.email_passcode)
                    .await
            }
            Err(e) => Err(e),
        };
        if let Err(e) = sent {
            tracing::warn!(org_id, "check run journal write failed: {e}");
        }
    });
}

/// After an import wrote: one run `import:<name>` for each object it
/// updated that is the subject of a watch case (D431). The fingerprint
/// holds the values written, so a run only counts when they change.
/// Objects outside any watch cost one query for the whole run.
pub async fn record_import(
    db: &DatabaseConnection,
    org_id: i64,
    actor: &Actor,
    import_name: &str,
    updated: &[pnex_core::ontology::import::DiffUpdate],
) {
    #[derive(FromQueryResult)]
    struct Row {
        subject_id: Uuid,
    }
    let ids: Vec<Uuid> = updated
        .iter()
        .filter_map(|u| Uuid::parse_str(&u.id).ok())
        .collect();
    if ids.is_empty() {
        return;
    }
    let watched = Row::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT DISTINCT subject_id FROM check_states WHERE org_id = $1 \
         AND case_id IS NOT NULL AND subject_id = ANY($2)",
        [org_id.into(), ids.into()],
    ))
    .all(db)
    .await;
    let watched: HashSet<Uuid> = match watched {
        Ok(rows) => rows.into_iter().map(|r| r.subject_id).collect(),
        Err(e) => {
            tracing::warn!(org_id, error = %e, "import check runs skipped");
            return;
        }
    };
    let slug: String = import_name
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    let check: String = format!("import:{slug}").chars().take(128).collect();
    for u in updated {
        let Some(id) = Uuid::parse_str(&u.id).ok().filter(|i| watched.contains(i)) else {
            continue;
        };
        let values = serde_json::to_string(&u.changes).unwrap_or_default();
        let digest = format!("{:x}", Sha256::digest(values.as_bytes()));
        let run = CheckRun {
            check: check.clone(),
            subject_id: id.to_string(),
            rules: vec![pnex_core::ontology::check::CheckRule {
                code: "import".into(),
                version: 1,
                result: "ok".into(),
                reason: Some(format!("values-{}", &digest[..12])),
                finding_id: None,
                finding_ref: None,
            }],
            ..Default::default()
        };
        match record(db, org_id, actor, "import", &run).await {
            Ok(r) => journal(org_id, r.line),
            Err(e) => tracing::warn!(org_id, object = %id, error = ?e, "import check run failed"),
        }
    }
}

/// Sets `occurred.to` of the open event to `last` (a period, `range`).
async fn close_event<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    actor: &Actor,
    event_id: Uuid,
    last: chrono::DateTime<chrono::FixedOffset>,
) -> Result<()> {
    let Ok(e) = objects::find(db, org_id, event_id).await else {
        return Ok(());
    };
    if e.valid_to.is_some() {
        return Ok(());
    }
    let mut props = e.properties.as_object().cloned().unwrap_or_default();
    let from = props
        .get(EVENT_OCCURRED)
        .and_then(|o| o.get("from"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| utc(last.with_timezone(&chrono::Utc)));
    let to = utc(last.with_timezone(&chrono::Utc));
    // A run in the same second leaves an empty period: still a range.
    let to = if to < from { from.clone() } else { to };
    props.insert(
        EVENT_OCCURRED.into(),
        json!({"from": from, "to": to, "precision": "range"}),
    );
    objects::update_in(
        db,
        org_id,
        actor,
        event_id,
        &ObjectInput {
            type_key: e.type_key.clone(),
            title: e.title.clone(),
            properties: props,
            expected_version: Some(e.version),
        },
    )
    .await?;
    Ok(())
}

/// The event of a changed result: from this run, open (`to` = `from` until
/// the next change closes it).
#[allow(clippy::too_many_arguments)]
async fn new_event<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    actor: &Actor,
    case_id: Uuid,
    origin: &str,
    run: &CheckRun,
    (subject, scope, findings, context): (Uuid, &[Uuid], &[Uuid], &[Uuid]),
    now: chrono::DateTime<chrono::Utc>,
) -> Result<Uuid> {
    let result = run.result();
    let flagged: Vec<&str> = run
        .rules
        .iter()
        .filter(|r| r.result != "ok")
        .map(|r| r.code.as_str())
        .collect();
    // shortcut: a machine-shaped title (check · result · codes), not
    // translated at display; a UI label per check code can come later.
    let title = if flagged.is_empty() {
        format!("{} · {result}", run.check)
    } else {
        format!("{} · {result} ({})", run.check, flagged.join(", "))
    };
    let note = run
        .rules
        .iter()
        .map(|r| {
            let reason = r
                .reason
                .as_deref()
                .map(|x| format!(" ({x})"))
                .unwrap_or_default();
            format!("{} v{}: {}{reason}", r.code, r.version, r.result)
        })
        .collect::<Vec<_>>()
        .join("\n");
    let mut seen = HashSet::new();
    let mut attach = Vec::new();
    let roles = std::iter::once((subject, "actor"))
        .chain(scope.iter().map(|id| (*id, "evidence")))
        .chain(findings.iter().map(|id| (*id, "evidence")))
        .chain(context.iter().map(|id| (*id, "context")));
    for (id, role) in roles {
        if id != case_id && seen.insert(id) {
            attach.push(CaseAttachInput {
                object_id: id.to_string(),
                role: Some(role.into()),
                ..Default::default()
            });
        }
    }
    let at = utc(now);
    let mut props = Map::new();
    props.insert(
        EVENT_NOTE.into(),
        note.chars().take(4000).collect::<String>().into(),
    );
    create_event_in(
        db,
        org_id,
        actor,
        case_id,
        &CaseEventInput {
            title,
            occurred: json!({"from": at, "to": at, "precision": "range"}),
            origin: Some(origin.into()),
            attach,
        },
        props,
    )
    .await
}

/// `watch_state` of the case = the worst latest result of its checks,
/// written only when it changes.
async fn sync_watch_state<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    actor: &Actor,
    case_id: Uuid,
) -> Result<()> {
    let rows = checks_of(db, org_id, case_id).await?;
    let state = worst(rows.iter().map(|r| r.result.as_str()));
    let case = case_of(db, org_id, case_id).await?;
    if case
        .properties
        .get(CASE_WATCH_STATE)
        .and_then(Value::as_str)
        == Some(state)
    {
        return Ok(());
    }
    let mut props = case.properties.as_object().cloned().unwrap_or_default();
    props.insert(CASE_WATCH_STATE.into(), state.into());
    objects::update_in(
        db,
        org_id,
        actor,
        case_id,
        &ObjectInput {
            type_key: KIND_CASE.into(),
            title: case.title.clone(),
            properties: props,
            expected_version: Some(case.version),
        },
    )
    .await?;
    Ok(())
}

async fn checks_of<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    case_id: Uuid,
) -> std::result::Result<Vec<CheckRow>, DbErr> {
    CheckRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT check_key, result, rules, source_ref, changed_at, last_run_at, \
         runs_since_change FROM check_states WHERE org_id = $1 AND case_id = $2 \
         ORDER BY check_key, subject_id, scope_key",
        [org_id.into(), case_id.into()],
    ))
    .all(db)
    .await
}

/// The checks of a case for its workspace (D430), one per check and rule,
/// read from `check_states` (no O2 query when the case opens).
pub(super) async fn case_checks<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    case_id: Uuid,
) -> Result<Vec<CaseCheck>> {
    let mut out: Vec<CaseCheck> = Vec::new();
    let mut by_check: HashMap<String, usize> = HashMap::new();
    for row in checks_of(db, org_id, case_id).await? {
        let rules: Vec<pnex_core::ontology::check::CheckRule> =
            serde_json::from_value(row.rules).unwrap_or_default();
        let i = *by_check.entry(row.check_key.clone()).or_insert_with(|| {
            out.push(CaseCheck {
                check: row.check_key.clone(),
                result: "ok".into(),
                ..Default::default()
            });
            out.len() - 1
        });
        let c = &mut out[i];
        c.result = worst([c.result.as_str(), row.result.as_str()]).into();
        let last = row.last_run_at.with_timezone(&chrono::Utc);
        if c.last_run_at.as_deref().is_none_or(|l| *l < *utc(last)) {
            c.last_run_at = Some(utc(last));
            c.source_ref = row.source_ref.clone();
        }
        c.changed_at = Some(utc(row.changed_at.with_timezone(&chrono::Utc)));
        c.runs_since_change = c.runs_since_change.max(row.runs_since_change);
        c.rules.extend(rules);
    }
    Ok(out)
}
