//! A person entered by a sales rep, matched with their LinkedIn profile
//! (agent-unification.md D788). The search itself is the pack pipeline
//! `prospecting_b2b/person_match_linkedin` (pipeline-engine.md D996): one
//! query of the ladder per navigation, scored by the pack's function, the
//! best profile attached, the plausible ones proposed, else the next query
//! asked. What stays here are the route's guards and inputs: the person's
//! objection, the daily searches, the subject (folded names, the ladder, the
//! thresholds); the view of a person's searches; and an hourly sweep that
//! starts the matches of targets and prospects and follows the proposals a
//! human decided (an accepted profile's text is read onto the person, a
//! ladder whose proposals were all rejected goes on).

use loco_rs::app::AppContext;
use pnex_core::err_codes;
use pnex_core::fold::fold_words;
use pnex_core::person_match::{
    self as pm, MatchCandidate, MatchState, PersonMatch, Subject, Verdict,
};
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::services::extension::ExtensionError;
use crate::services::pipeline_engine::store;
use crate::services::profiles;
use crate::services::site_recipes::purpose;

/// The pack pipeline that searches a person (D996).
pub const PIPELINE: &str = "prospecting_b2b/person_match_linkedin";
/// `origin` of the proposals it files.
const ORIGIN: &str = "prospecting_b2b.person_match";

const SWEEP_INTERVAL_SECS: u64 = 3600;
/// Matches started per org and sweep, whatever is left of the day.
const STARTS_PER_SWEEP: i64 = 10;

/// Settings of `prospecting_b2b` read here (D788), with their defaults.
const SET_WRITE: (&str, u32) = ("prospecting_b2b.match_write_threshold", 80);
const SET_PROPOSE: (&str, u32) = ("prospecting_b2b.match_propose_threshold", 40);
const SET_PER_DAY: (&str, u32) = ("prospecting_b2b.match_searches_per_day", 20);
const SET_RETRY: (&str, u32) = ("prospecting_b2b.match_retry_days", 90);

fn stmt(sql: &str, values: Vec<sea_orm::Value>) -> Statement {
    Statement::from_sql_and_values(DatabaseBackend::Postgres, sql, values)
}

async fn setting(db: &DatabaseConnection, org_id: i64, (key, default): (&str, u32)) -> u32 {
    crate::services::ontology::packs::setting_value(db, org_id, key)
        .await
        .ok()
        .flatten()
        .and_then(|v| v.as_f64())
        .map(|v| v.max(0.0) as u32)
        .unwrap_or(default)
}

/// The row asked of the pipeline for the query `step` of the ladder: the
/// subject folded as the scoring function compares it.
fn asked_row(
    person_id: &str,
    subject: &Value,
    step: usize,
) -> Option<serde_json::Map<String, Value>> {
    let query = subject["queries"].get(step)?.as_str()?.to_string();
    Some(serde_json::Map::from_iter([
        ("person_id".to_string(), Value::String(person_id.to_string())),
        ("query".to_string(), Value::String(query)),
        ("step".to_string(), Value::String(step.to_string())),
        ("subject".to_string(), Value::String(subject.to_string())),
    ]))
}

/// The subject as the pipeline reads it: folded words, the ladder and the
/// two thresholds.
fn folded(s: &Subject, queries: &[String], (write_at, propose_at): (u32, u32)) -> Value {
    let names = |list: &[String]| -> Vec<Vec<String>> {
        list.iter()
            .map(|n| pm::company_words(n))
            .filter(|w| !w.is_empty())
            .collect()
    };
    json!({
        "name": fold_words(&s.name),
        "orgs": names(&s.org_names),
        "groups": names(&s.group_names),
        "cities": s.cities.iter().map(|c| fold_words(c)).collect::<Vec<_>>(),
        "queries": queries,
        "write_at": write_at,
        "propose_at": propose_at,
    })
}

/// The searches of the person, newest first: each ladder from its first
/// query, with what its runs and proposals tell.
pub async fn list(
    db: &DatabaseConnection,
    org_id: i64,
    person_id: Uuid,
) -> Result<Vec<PersonMatch>, ExtensionError> {
    profiles::person(db, org_id, person_id).await?;
    let person = person_id.to_string();
    let mut runs = purpose::pipeline_runs(db, org_id, PIPELINE, "person_id", &person).await?;
    runs.reverse();
    // A ladder starts at its first query; the runs after it are its next ones.
    let mut ladders: Vec<Vec<_>> = Vec::new();
    for r in runs {
        match ladders.last_mut() {
            Some(l) if r.1.get("step").is_some_and(|s| s != "0") => l.push(r),
            _ => ladders.push(vec![r]),
        }
    }
    let proposals = db
        .query_all_raw(stmt(
            "SELECT id, status, evidence, created_at FROM proposals
             WHERE org_id = $1 AND origin = $2 AND evidence ->> 'person_id' = $3
             ORDER BY created_at",
            vec![org_id.into(), ORIGIN.into(), person.clone().into()],
        ))
        .await?;
    let linked = db
        .query_all_raw(stmt(
            "SELECT i.properties ->> 'value' AS url, l.valid_from FROM resource_edges l
               JOIN objects i ON i.id = l.target_object_id AND i.valid_to IS NULL
             WHERE l.org_id = $1 AND l.relation = 'uses_identifier' AND l.source_object_id = $2
               AND l.valid_to IS NULL AND i.properties ->> 'value' LIKE '%linkedin.com/in/%'",
            vec![org_id.into(), person_id.into()],
        ))
        .await?;
    let at = |r: &sea_orm::QueryResult, col: &str| {
        r.try_get::<chrono::DateTime<chrono::FixedOffset>>("", col)
            .ok()
    };
    let instant = |t: &str| chrono::DateTime::parse_from_rfc3339(t).ok();
    let mut out = Vec::new();
    for (i, runs) in ladders.iter().enumerate() {
        let (first, last) = (&runs[0], &runs[runs.len() - 1]);
        let from = instant(&first.4);
        let until = ladders.get(i + 1).and_then(|l| instant(&l[0].4));
        let within = |t: Option<chrono::DateTime<chrono::FixedOffset>>| {
            t.is_some_and(|t| from.is_none_or(|f| t >= f) && until.is_none_or(|u| t < u))
        };
        let subject: Value = first
            .1
            .get("subject")
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or(Value::Null);
        let queries: Vec<String> =
            serde_json::from_value(subject["queries"].clone()).unwrap_or_default();
        let step: u32 = last.1.get("step").and_then(|s| s.parse().ok()).unwrap_or(0);
        let mut candidates = Vec::new();
        let mut statuses = Vec::new();
        for p in proposals.iter().filter(|p| within(at(p, "created_at"))) {
            let ev: Value = p.try_get("", "evidence").unwrap_or(Value::Null);
            candidates.push(MatchCandidate {
                url: ev["url"].as_str().unwrap_or_default().to_string(),
                query: ev["step"].as_u64().unwrap_or(0) as u32,
                score: ev["score"].as_u64().map(|s| s as u32),
                detail: serde_json::from_value(ev["detail"].clone()).unwrap_or_default(),
                left_company: ev["left_company"].as_bool().unwrap_or(false),
                verdict: Some(Verdict::Propose),
                asset_id: None,
                proposal_id: p.try_get::<Uuid>("", "id").ok().map(|u| u.to_string()),
            });
            statuses.push(p.try_get::<String>("", "status").unwrap_or_default());
        }
        let attached = linked.iter().find(|l| within(at(l, "valid_from")));
        if let Some(l) = attached {
            let url: String = l.try_get("", "url").unwrap_or_default();
            if !candidates.iter().any(|c| c.url == url) {
                candidates.push(MatchCandidate {
                    url,
                    query: step,
                    score: None,
                    detail: Vec::new(),
                    left_company: false,
                    verdict: Some(Verdict::Write),
                    asset_id: None,
                    proposal_id: None,
                });
            }
        }
        let (state, state_code) = match last.2.as_str() {
            "queued" | "running" => (MatchState::Searching, None),
            "succeeded" if attached.is_some() || statuses.iter().any(|s| s == "accepted") => {
                (MatchState::Matched, None)
            }
            "succeeded" if statuses.iter().any(|s| s == "proposed") => (MatchState::Proposed, None),
            // The pipeline, or the sweep after a rejection, asks the next query.
            "succeeded" if (step as usize) + 1 < queries.len() => (MatchState::Searching, None),
            "succeeded" => (MatchState::NotFound, None),
            _ => (MatchState::Stopped, last.3.clone()),
        };
        out.push(PersonMatch {
            id: first.0.to_string(),
            person_id: person.clone(),
            organization_id: None,
            state,
            step,
            queries,
            candidates,
            state_code,
            created_at: first.4.clone(),
            updated_at: last.4.clone(),
        });
    }
    out.reverse();
    Ok(out)
}

/// What is known of the person and the company they work for: its names,
/// its group, the cities of its sites. `None` without a current company.
async fn subject(
    db: &DatabaseConnection,
    org_id: i64,
    person_id: Uuid,
) -> Result<Option<(Subject, Uuid)>, ExtensionError> {
    let Some(person) = db
        .query_one_raw(stmt(
            "SELECT title FROM objects WHERE org_id = $1 AND id = $2 AND valid_to IS NULL",
            vec![org_id.into(), person_id.into()],
        ))
        .await?
    else {
        return Ok(None);
    };
    let name: String = person.try_get("", "title").unwrap_or_default();
    let Some(company) = db
        .query_one_raw(stmt(
            "SELECT o.id, o.title, o.properties FROM resource_edges l
               JOIN objects o ON o.id = l.source_object_id AND o.valid_to IS NULL
             WHERE l.org_id = $1 AND l.relation = 'employs' AND l.target_object_id = $2
               AND l.valid_to IS NULL
             ORDER BY l.valid_from DESC LIMIT 1",
            vec![org_id.into(), person_id.into()],
        ))
        .await?
    else {
        return Ok(None);
    };
    let company_id: Uuid = company.try_get("", "id").map_err(ExtensionError::Db)?;
    let title: String = company.try_get("", "title").unwrap_or_default();
    let props: Value = company.try_get("", "properties").unwrap_or(Value::Null);
    let mut org_names = vec![title];
    for key in ["press_names", "aliases"] {
        if let Some(more) = props.get(key).and_then(Value::as_str) {
            org_names.extend(
                more.split([',', ';', '|', '\n'])
                    .map(str::trim)
                    .filter(|n| !n.is_empty())
                    .map(str::to_string),
            );
        }
    }
    // Parent first, then subsidiaries: the names a profile may carry.
    let group = db
        .query_all_raw(stmt(
            "SELECT o.title FROM resource_edges l JOIN objects o ON o.valid_to IS NULL AND
               o.id = CASE WHEN l.source_object_id = $2 THEN l.target_object_id
                           ELSE l.source_object_id END
             WHERE l.org_id = $1 AND l.relation = 'subsidiary_of' AND l.valid_to IS NULL
               AND (l.source_object_id = $2 OR l.target_object_id = $2)
             ORDER BY (l.source_object_id = $2) DESC LIMIT 5",
            vec![org_id.into(), company_id.into()],
        ))
        .await?;
    let group_names = group
        .iter()
        .filter_map(|r| r.try_get::<String>("", "title").ok())
        .collect();
    let sites = db
        .query_all_raw(stmt(
            "SELECT o.properties ->> 'address' AS address FROM resource_edges l
               JOIN objects o ON o.id = l.source_object_id AND o.valid_to IS NULL
             WHERE l.org_id = $1 AND l.relation = 'establishment_of' AND l.target_object_id = $2
               AND l.valid_to IS NULL
             ORDER BY (o.properties ->> 'head_office') = 'true' DESC NULLS LAST LIMIT 20",
            vec![org_id.into(), company_id.into()],
        ))
        .await?;
    let mut cities: Vec<String> = Vec::new();
    for r in &sites {
        let city = r
            .try_get::<Option<String>>("", "address")
            .ok()
            .flatten()
            .and_then(|a| pm::city_of_address(&a));
        if let Some(c) = city.filter(|c| !cities.contains(c)) {
            cities.push(c);
        }
    }
    cities.truncate(3);
    Ok(Some((
        Subject {
            name,
            org_names,
            group_names,
            cities,
        },
        company_id,
    )))
}

/// Searches started by the org's person matches over the last 24 h.
async fn searches_today(db: &DatabaseConnection, org_id: i64) -> Result<i64, ExtensionError> {
    Ok(db
        .query_one_raw(stmt(
            "SELECT count(*) AS n FROM agent_runs WHERE org_id = $1
               AND plan -> 'purpose' ->> 'pipeline' = $2
               AND created_at > now() - interval '24 hours'",
            vec![org_id.into(), PIPELINE.into()],
        ))
        .await?
        .and_then(|r| r.try_get::<i64>("", "n").ok())
        .unwrap_or(0))
}

/// Starts a match of the person (D788), or returns the one still running.
/// `auto` is the sweep: it spends the org's daily searches; a human asking
/// from the person's page is bounded by the compiled ceiling only.
pub async fn request(
    ctx: &AppContext,
    org_id: i64,
    created_by: Option<i64>,
    person_id: Uuid,
    auto: bool,
) -> Result<PersonMatch, ExtensionError> {
    let db = &ctx.db;
    profiles::person(db, org_id, person_id).await?;
    if profiles::opposed_any(db, org_id, &[format!("person:{person_id}")]).await? {
        return Err(ExtensionError::Opposed);
    }
    if let Some(open) = list(db, org_id, person_id)
        .await?
        .into_iter()
        .next()
        .filter(|m| m.state.open())
    {
        return Ok(open);
    }
    let Some((subject, _)) = subject(db, org_id, person_id).await? else {
        return Err(ExtensionError::PersonWithoutCompany);
    };
    let queries = pm::queries(&subject);
    if queries.is_empty() {
        return Err(ExtensionError::Invalid {
            field: "title".into(),
            token: err_codes::FIELD_REQUIRED.into(),
        });
    }
    let max = if auto {
        (setting(db, org_id, SET_PER_DAY).await as i64).min(pm::SEARCHES_PER_DAY_MAX)
    } else {
        pm::SEARCHES_PER_DAY_MAX
    };
    if searches_today(db, org_id).await? >= max {
        return Err(ExtensionError::MatchDailyCap { max });
    }
    let thresholds = (
        setting(db, org_id, SET_WRITE).await,
        setting(db, org_id, SET_PROPOSE).await,
    );
    let subject = folded(&subject, &queries, thresholds);
    let row = asked_row(&person_id.to_string(), &subject, 0).ok_or(ExtensionError::MissionNotFound)?;
    let out = store::run_asked(ctx, org_id, created_by, PIPELINE, vec![row])
        .await
        .map_err(|e| ExtensionError::Db(sea_orm::DbErr::Custom(format!("person match: {e:?}"))))?;
    profiles::asked_view(&out, PIPELINE)?;
    list(db, org_id, person_id)
        .await?
        .into_iter()
        .next()
        .ok_or(ExtensionError::MissionNotFound)
}

/// Follows the proposals a human decided (D788), each once: an accepted
/// profile's text is read onto the person; a ladder whose proposals were
/// all rejected goes on with its next query.
async fn follow_proposals(ctx: &AppContext) -> Result<u64, ExtensionError> {
    let db = &ctx.db;
    let decided = db
        .query_all_raw(stmt(
            "SELECT p.org_id, p.evidence ->> 'person_id' AS person, array_agg(p.id) AS ids,
               bool_or(p.status = 'proposed') AS open,
               (array_agg(p.evidence ->> 'url') FILTER (WHERE p.status = 'accepted'))[1] AS accepted,
               max((p.evidence ->> 'step')::int) AS step
             FROM proposals p
             WHERE p.origin = $1 AND NOT coalesce((p.evidence ->> 'followed')::bool, false)
             GROUP BY p.org_id, p.evidence ->> 'person_id' LIMIT 200",
            vec![ORIGIN.into()],
        ))
        .await?;
    let mut n = 0;
    for r in &decided {
        let (Ok(org_id), Ok(Some(person)), Ok(ids)) = (
            r.try_get::<i64>("", "org_id"),
            r.try_get::<Option<String>>("", "person"),
            r.try_get::<Vec<Uuid>>("", "ids"),
        ) else {
            continue;
        };
        if r.try_get::<bool>("", "open").unwrap_or(true) {
            continue;
        }
        let Ok(person_id) = Uuid::parse_str(&person) else {
            continue;
        };
        let gone = profiles::person(db, org_id, person_id).await.is_err()
            || profiles::opposed_any(db, org_id, &[format!("person:{person_id}")]).await?;
        let accepted = r.try_get::<Option<String>>("", "accepted").ok().flatten();
        match (gone, accepted) {
            (true, _) => {}
            (false, Some(url)) => {
                let asset = db
                    .query_one_raw(stmt(
                        "SELECT id FROM media_assets WHERE org_id = $1
                           AND metadata ->> 'source_url' = $2 ORDER BY created_at DESC LIMIT 1",
                        vec![org_id.into(), url.into()],
                    ))
                    .await?
                    .and_then(|a| a.try_get::<Uuid>("", "id").ok());
                if let Some(a) = asset {
                    profiles::stamp_to_read(ctx, org_id, &person, a).await?;
                }
            }
            (false, None) => {
                // Every proposal rejected: the next query of the latest ladder,
                // in the agent of who asked it.
                let step = r.try_get::<Option<i32>>("", "step").ok().flatten().unwrap_or(0);
                let last = db
                    .query_one_raw(stmt(
                        "SELECT created_by, plan -> 'params' ->> 'subject' AS subject
                         FROM agent_runs WHERE org_id = $1
                           AND plan -> 'purpose' ->> 'pipeline' = $2
                           AND plan -> 'params' ->> 'person_id' = $3
                         ORDER BY created_at DESC LIMIT 1",
                        vec![org_id.into(), PIPELINE.into(), person.clone().into()],
                    ))
                    .await?;
                let author = last
                    .as_ref()
                    .and_then(|l| l.try_get::<Option<i64>>("", "created_by").ok().flatten());
                let subject: Option<Value> = last
                    .and_then(|l| l.try_get::<Option<String>>("", "subject").ok().flatten())
                    .and_then(|s| serde_json::from_str(&s).ok());
                let row = subject.and_then(|s| asked_row(&person, &s, step.max(0) as usize + 1));
                if let Some(row) = row {
                    if let Err(e) = store::run_asked(ctx, org_id, author, PIPELINE, vec![row]).await
                    {
                        tracing::warn!(org_id, error = ?e, "next person search not asked");
                    }
                }
            }
        }
        db.execute_raw(stmt(
            "UPDATE proposals SET evidence = evidence || '{\"followed\": true}'::jsonb
             WHERE id = ANY($1)",
            vec![ids.into()],
        ))
        .await?;
        n += 1;
    }
    Ok(n)
}

/// The user id of `manual:<id>` (who entered the object), if any.
fn author_of(source_ref: Option<&str>) -> Option<i64> {
    source_ref?.strip_prefix("manual:")?.parse().ok()
}

/// Starts the matches of the day (D788): people of the org's targets and
/// prospects, the best target score first, never someone who objected, who
/// has a LinkedIn account already, or was searched within the retry days.
/// Only in an org with a paired browser, under its daily searches.
async fn start_matches(ctx: &AppContext) -> Result<u64, ExtensionError> {
    let db = &ctx.db;
    let orgs = db
        .query_all_raw(stmt(
            "SELECT DISTINCT p.org_id FROM packs p
             WHERE p.key = 'prospecting_b2b'
               AND EXISTS (SELECT 1 FROM extension_agents a WHERE a.org_id = p.org_id)",
            vec![],
        ))
        .await?;
    let mut n = 0;
    for org_id in orgs
        .iter()
        .filter_map(|r| r.try_get::<i64>("", "org_id").ok())
    {
        let per_day = (setting(db, org_id, SET_PER_DAY).await as i64).min(pm::SEARCHES_PER_DAY_MAX);
        let left = (per_day - searches_today(db, org_id).await?).min(STARTS_PER_SWEEP);
        if left <= 0 {
            continue;
        }
        let retry = setting(db, org_id, SET_RETRY).await.max(1) as i64;
        let people = db
            .query_all_raw(stmt(
                "SELECT p.id, p.source_ref FROM objects p
                   JOIN resource_edges e ON e.org_id = p.org_id AND e.relation = 'employs'
                     AND e.target_object_id = p.id AND e.valid_to IS NULL
                   JOIN objects c ON c.id = e.source_object_id AND c.valid_to IS NULL
                 WHERE p.org_id = $1 AND p.type_key = 'person' AND p.valid_to IS NULL
                   AND p.erased_at IS NULL
                   AND c.properties ->> 'account_status' IN ('target', 'prospect')
                   AND NOT EXISTS (SELECT 1 FROM prospecting_oppositions o
                     WHERE o.org_id = p.org_id AND o.fingerprint = 'person:' || p.id::text)
                   AND NOT EXISTS (SELECT 1 FROM resource_edges u
                     JOIN objects i ON i.id = u.target_object_id
                     WHERE u.org_id = p.org_id AND u.relation = 'uses_identifier'
                       AND u.source_object_id = p.id AND u.valid_to IS NULL
                       AND i.properties ->> 'value' LIKE '%linkedin.com/in/%')
                   AND NOT EXISTS (SELECT 1 FROM agent_runs a
                     WHERE a.org_id = p.org_id AND a.plan -> 'purpose' ->> 'pipeline' = $4
                       AND a.plan -> 'params' ->> 'person_id' = p.id::text
                       AND a.created_at > now() - make_interval(days => $2::int))
                 ORDER BY (c.properties ->> 'target_score')::numeric DESC NULLS LAST, p.created_at
                 LIMIT $3",
                vec![org_id.into(), (retry as i32).into(), left.into(), PIPELINE.into()],
            ))
            .await?;
        for r in &people {
            let Ok(person) = r.try_get::<Uuid>("", "id") else {
                continue;
            };
            let author = author_of(
                r.try_get::<Option<String>>("", "source_ref")
                    .ok()
                    .flatten()
                    .as_deref(),
            );
            match request(ctx, org_id, author, person, true).await {
                Ok(_) => n += 1,
                Err(ExtensionError::MatchDailyCap { .. } | ExtensionError::RunDailyCap { .. }) => {
                    break
                }
                Err(e) => tracing::debug!(error = %e, "person match not started"),
            }
        }
    }
    Ok(n)
}

/// One pass of the sweep.
pub async fn sweep(ctx: &AppContext) -> Result<u64, ExtensionError> {
    let followed = follow_proposals(ctx).await?;
    Ok(followed + start_matches(ctx).await?)
}

/// Background sweep, started at boot — skipped in tests.
pub fn spawn_sweeper(ctx: &AppContext) {
    use loco_rs::config::WorkerMode;
    if matches!(ctx.config.workers.mode, WorkerMode::ForegroundBlocking) {
        return;
    }
    let ctx = ctx.clone();
    tokio::spawn(async move {
        let period = std::time::Duration::from_secs(SWEEP_INTERVAL_SECS);
        let mut tick = tokio::time::interval(period);
        loop {
            tick.tick().await;
            if !crate::services::singleton::my_turn(&ctx.db, "person-match", period).await {
                continue;
            }
            match sweep(&ctx).await {
                Ok(n) if n > 0 => tracing::info!(n, "person match sweep"),
                Ok(_) => {}
                Err(e) => tracing::warn!(error = %e, "person match sweep failed"),
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::author_of;

    #[test]
    fn the_author_of_an_entered_person_runs_its_searches() {
        assert_eq!(author_of(Some("manual:42")), Some(42));
        assert_eq!(author_of(Some("import:companies_fr")), None);
        assert_eq!(author_of(None), None);
    }
}
