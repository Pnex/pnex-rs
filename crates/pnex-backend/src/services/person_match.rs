//! A person entered by a sales rep, matched with a network profile through
//! a search bar (agent-unification.md D788). The server holds the state
//! (`person_matches`): the ladder of queries, each played as a run of the
//! default rule `person_search` in the author's browser agent (D550), the
//! result cards and opened profiles it brings back, their scores, and what
//! the pack's thresholds allow: attached to the person, proposed in "To
//! validate" (a job change always), or the next query. No site is named
//! here: searching and reading a network is the rule's job. An hourly sweep
//! starts the matches of the targets and prospects under caps, skips anyone
//! who objected, and follows the proposals a human decided.

use std::collections::BTreeMap;

use loco_rs::app::AppContext;
use pnex_core::err_codes;
use pnex_core::ontology::proposal::{ProposalInput, ProposalKind};
use pnex_core::person_match::{
    self as pm, MatchCandidate, MatchState, PersonMatch, Subject, Verdict,
};
use pnex_core::site_recipe::RunExtract;
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement};
use serde_json::Value;
use uuid::Uuid;

use crate::services::extension::ExtensionError;
use crate::services::profiles;
use crate::services::site_recipes::purpose::{self, Purpose};
use crate::services::site_recipes::StartError;

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

/// A match row, as stored.
struct Row {
    id: Uuid,
    org_id: i64,
    person_id: Uuid,
    organization_id: Option<Uuid>,
    state: MatchState,
    step: u32,
    queries: Vec<String>,
    subject: Subject,
    candidates: Vec<MatchCandidate>,
    state_code: Option<String>,
    created_by: Option<i64>,
    created_at: String,
    updated_at: String,
}

const COLS: &str = "id, org_id, person_id, organization_id, state, step, queries, subject, \
                    candidates, state_code, created_by, created_at, updated_at";

fn row_of(r: &sea_orm::QueryResult) -> Option<Row> {
    let json = |k: &str| r.try_get::<Value>("", k).ok().unwrap_or(Value::Null);
    let at = |k: &str| {
        r.try_get::<chrono::DateTime<chrono::FixedOffset>>("", k)
            .map(|t| t.to_rfc3339())
            .unwrap_or_default()
    };
    Some(Row {
        id: r.try_get("", "id").ok()?,
        org_id: r.try_get("", "org_id").ok()?,
        person_id: r.try_get("", "person_id").ok()?,
        organization_id: r.try_get("", "organization_id").ok().flatten(),
        state: MatchState::parse(&r.try_get::<String>("", "state").ok()?)?,
        step: r.try_get::<i32>("", "step").ok()?.max(0) as u32,
        queries: serde_json::from_value(json("queries")).unwrap_or_default(),
        subject: serde_json::from_value(json("subject")).unwrap_or_default(),
        candidates: serde_json::from_value(json("candidates")).unwrap_or_default(),
        state_code: r.try_get("", "state_code").ok().flatten(),
        created_by: r.try_get("", "created_by").ok().flatten(),
        created_at: at("created_at"),
        updated_at: at("updated_at"),
    })
}

impl Row {
    fn view(&self) -> PersonMatch {
        PersonMatch {
            id: self.id.to_string(),
            person_id: self.person_id.to_string(),
            organization_id: self.organization_id.map(|o| o.to_string()),
            state: self.state,
            step: self.step,
            queries: self.queries.clone(),
            candidates: self.candidates.clone(),
            state_code: self.state_code.clone(),
            created_at: self.created_at.clone(),
            updated_at: self.updated_at.clone(),
        }
    }
}

async fn load(
    db: &impl ConnectionTrait,
    org_id: i64,
    id: Uuid,
) -> Result<Option<Row>, ExtensionError> {
    Ok(db
        .query_one_raw(stmt(
            &format!("SELECT {COLS} FROM person_matches WHERE org_id = $1 AND id = $2"),
            vec![org_id.into(), id.into()],
        ))
        .await?
        .as_ref()
        .and_then(row_of))
}

async fn save(db: &impl ConnectionTrait, m: &Row) -> Result<(), ExtensionError> {
    db.execute_raw(stmt(
        "UPDATE person_matches SET state = $3, step = $4, candidates = $5, state_code = $6,
            updated_at = now()
         WHERE org_id = $1 AND id = $2",
        vec![
            m.org_id.into(),
            m.id.into(),
            m.state.as_str().into(),
            (m.step as i32).into(),
            serde_json::to_value(&m.candidates)
                .unwrap_or_default()
                .into(),
            m.state_code.clone().into(),
        ],
    ))
    .await?;
    Ok(())
}

/// The latest matches of the person, newest first.
pub async fn list(
    db: &DatabaseConnection,
    org_id: i64,
    person_id: Uuid,
) -> Result<Vec<PersonMatch>, ExtensionError> {
    profiles::person(db, org_id, person_id).await?;
    let rows = db
        .query_all_raw(stmt(
            &format!(
                "SELECT {COLS} FROM person_matches WHERE org_id = $1 AND person_id = $2
                 ORDER BY created_at DESC LIMIT 20"
            ),
            vec![org_id.into(), person_id.into()],
        ))
        .await?;
    Ok(rows.iter().filter_map(row_of).map(|r| r.view()).collect())
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

async fn searches_today(db: &DatabaseConnection, org_id: i64) -> Result<i64, ExtensionError> {
    Ok(db
        .query_one_raw(stmt(
            "SELECT count(*) AS n FROM agent_runs WHERE org_id = $1
               AND plan -> 'purpose' ->> 'kind' = 'person_match'
               AND created_at > now() - interval '24 hours'",
            vec![org_id.into()],
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
    if let Some(open) = db
        .query_one_raw(stmt(
            &format!(
                "SELECT {COLS} FROM person_matches WHERE org_id = $1 AND person_id = $2
                   AND state IN ('searching', 'proposed')
                 ORDER BY created_at DESC LIMIT 1"
            ),
            vec![org_id.into(), person_id.into()],
        ))
        .await?
        .as_ref()
        .and_then(row_of)
    {
        return Ok(open.view());
    }
    let Some((subject, company)) = subject(db, org_id, person_id).await? else {
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
    let id = Uuid::new_v4();
    db.execute_raw(stmt(
        "INSERT INTO person_matches (id, org_id, person_id, organization_id, state, queries,
            subject, created_by)
         VALUES ($1, $2, $3, $4, 'searching', $5, $6, $7)",
        vec![
            id.into(),
            org_id.into(),
            person_id.into(),
            company.into(),
            serde_json::to_value(&queries).unwrap_or_default().into(),
            serde_json::to_value(&subject).unwrap_or_default().into(),
            created_by.into(),
        ],
    ))
    .await?;
    let mut m = load(db, org_id, id)
        .await?
        .ok_or(ExtensionError::MissionNotFound)?;
    if let Err(e) = spawn_search(ctx, &mut m).await {
        // Nothing was queued: the match does not stay open behind the refusal.
        db.execute_raw(stmt(
            "DELETE FROM person_matches WHERE id = $1",
            vec![id.into()],
        ))
        .await?;
        return Err(e);
    }
    save(db, &m).await?;
    Ok(m.view())
}

/// Plays the match's current query: a run of the default search rule in
/// the author's browser agent; past the last query, the match is not found.
async fn spawn_search(ctx: &AppContext, m: &mut Row) -> Result<(), ExtensionError> {
    let Some(query) = m.queries.get(m.step as usize).cloned() else {
        m.state = MatchState::NotFound;
        return Ok(());
    };
    let purpose = Purpose::PersonMatch {
        match_id: m.id.to_string(),
        step: m.step,
    };
    let params = BTreeMap::from([("query".to_string(), query)]);
    let rule = pnex_core::default_rules::person_search();
    match purpose::start(ctx, m.org_id, m.created_by, &rule, params, &purpose).await {
        Ok(_) => {
            m.state = MatchState::Searching;
            m.state_code = None;
            Ok(())
        }
        Err(StartError::NoAgent) => Err(ExtensionError::NoAgent),
        Err(StartError::DailyCap { max }) => Err(ExtensionError::RunDailyCap { max }),
        Err(e) => Err(ExtensionError::Db(sea_orm::DbErr::Custom(format!(
            "person search: {e:?}"
        )))),
    }
}

/// Stops a match whose person objected or is gone; `true` when stopped.
async fn stopped(db: &DatabaseConnection, m: &mut Row) -> Result<bool, ExtensionError> {
    let gone = profiles::person(db, m.org_id, m.person_id).await.is_err();
    if gone || profiles::opposed_any(db, m.org_id, &[format!("person:{}", m.person_id)]).await? {
        m.state = MatchState::Stopped;
        save(db, m).await?;
        return Ok(true);
    }
    Ok(false)
}

/// A search run of the match ended (D788): its result cards are scored on
/// the profiles it opened (on their card alone for the others), then the
/// best is attached, the plausible ones proposed, or the next query played.
#[allow(clippy::too_many_arguments)]
pub async fn search_finished(
    ctx: &AppContext,
    org_id: i64,
    run_id: Uuid,
    match_id: &str,
    step: u32,
    ok: bool,
    extracts: &[RunExtract],
) -> Result<(), ExtensionError> {
    let db = &ctx.db;
    let Ok(id) = Uuid::parse_str(match_id) else {
        return Ok(());
    };
    let Some(mut m) = load(db, org_id, id).await? else {
        return Ok(());
    };
    if m.state != MatchState::Searching || m.step != step || stopped(db, &mut m).await? {
        return Ok(());
    }
    if !ok {
        // The browser could not search (no session, a check, no permission):
        // the match waits for a human, with the reason, instead of retrying.
        let reason = db
            .query_one_raw(stmt(
                "SELECT reason FROM agent_runs WHERE id = $1",
                vec![run_id.into()],
            ))
            .await?
            .and_then(|r| r.try_get::<Option<String>>("", "reason").ok().flatten());
        m.state = MatchState::Stopped;
        m.state_code = reason;
        return save(db, &m).await;
    }
    let cards = extracts
        .iter()
        .find(|e| e.name == "results")
        .map(|e| e.cards.clone())
        .unwrap_or_default();
    let (write_at, propose_at) = (
        setting(db, org_id, SET_WRITE).await,
        setting(db, org_id, SET_PROPOSE).await,
    );
    let person = m.person_id.to_string();
    let source = format!("person_match:{}", m.id);
    for (i, card) in pm::candidates(&cards, pm::CANDIDATES_PER_SEARCH)
        .into_iter()
        .enumerate()
    {
        if m.candidates.iter().any(|c| c.url == card.url)
            || profiles::opposed_any(db, org_id, std::slice::from_ref(&card.url)).await?
        {
            continue;
        }
        // The rule opens the first profiles in card order: `profile.<n>`.
        let profile = extracts
            .iter()
            .find(|e| e.name == format!("profile.{}", i + 1))
            .map(|e| e.text.clone())
            .filter(|t| !t.trim().is_empty());
        let text = match &profile {
            Some(p) => format!("{}\n{p}", card.text),
            None => card.text.clone(),
        };
        let score = pm::score(&m.subject, &text);
        let verdict = pm::verdict(&score, write_at, propose_at);
        // Kept 30 days with the person's captured texts, only when it may
        // be theirs: a stranger's profile is not stored.
        let asset_id = match (verdict, &profile) {
            (Verdict::Write | Verdict::Propose, Some(p)) => Some(
                profiles::store_profile_text(ctx, org_id, &person, &card.url, p)
                    .await?
                    .to_string(),
            ),
            _ => None,
        };
        m.candidates.push(MatchCandidate {
            url: card.url,
            query: step,
            score: Some(score.total),
            detail: score.detail,
            left_company: score.left_company,
            verdict: Some(verdict),
            asset_id,
            proposal_id: None,
        });
    }
    let best = m
        .candidates
        .iter()
        .filter(|c| c.verdict == Some(Verdict::Write))
        .max_by_key(|c| c.score.unwrap_or(0))
        .cloned();
    if let Some(best) = best {
        let asset = best
            .asset_id
            .as_deref()
            .and_then(|a| Uuid::parse_str(a).ok());
        match asset {
            Some(a) => {
                profiles::attach_profile(ctx, org_id, &source, &person, &best.url, a).await?
            }
            None => profiles::attach_identifier(ctx, org_id, &source, &person, &best.url).await,
        }
        m.state = MatchState::Matched;
        return save(db, &m).await;
    }
    let mut proposed = false;
    for c in m
        .candidates
        .iter_mut()
        .filter(|c| c.verdict == Some(Verdict::Propose) && c.proposal_id.is_none())
    {
        let Some(identifier) = profiles::identifier_of(db, org_id, &source, &c.url).await else {
            continue;
        };
        let input = ProposalInput {
            kind: Some(ProposalKind::LinkCreate),
            type_key: "uses_identifier".into(),
            target_id: None,
            source_id: Some(person.clone()),
            link_target_id: Some(identifier),
            title: Some(c.url.clone()),
            values: serde_json::Map::new(),
            base_version: None,
            evidence: serde_json::json!({
                "url": c.url,
                "score": c.score,
                "detail": c.detail,
                "left_company": c.left_company,
                "asset_id": c.asset_id,
                "match_id": m.id.to_string(),
            }),
            origin: "person_match".into(),
            confidence: c.score.map(f64::from),
        };
        match crate::services::ontology::proposals::propose(db, org_id, &input).await {
            Ok(Some(pid)) => {
                c.proposal_id = Some(pid.to_string());
                proposed = true;
            }
            // Proposed before (same candidate, same person): that decision stands.
            Ok(None) => {}
            Err(e) => tracing::warn!(error = %e, "person match proposal not written"),
        }
    }
    if proposed {
        m.state = MatchState::Proposed;
        return save(db, &m).await;
    }
    m.step += 1;
    if let Err(e) = spawn_search(ctx, &mut m).await {
        m.state = MatchState::Stopped;
        m.state_code = Some(match e {
            ExtensionError::NoAgent => err_codes::SITE_RECIPE_NO_AGENT.into(),
            ExtensionError::RunDailyCap { .. } => err_codes::SITE_RECIPE_DAILY_CAP.into(),
            _ => pnex_core::site_recipe::reason::BROWSER.into(),
        });
    }
    save(db, &m).await
}

/// Follows the proposals a human decided: an accepted candidate is attached
/// (its text read onto the person); when every one is rejected, the next
/// query runs.
async fn follow_proposals(ctx: &AppContext) -> Result<u64, ExtensionError> {
    let db = &ctx.db;
    let rows = db
        .query_all_raw(stmt(
            &format!("SELECT {COLS} FROM person_matches WHERE state = 'proposed' LIMIT 200"),
            vec![],
        ))
        .await?;
    let mut n = 0;
    for mut m in rows.iter().filter_map(row_of) {
        if stopped(db, &mut m).await? {
            continue;
        }
        let ids: Vec<String> = m
            .candidates
            .iter()
            .filter_map(|c| c.proposal_id.clone())
            .collect();
        let statuses = db
            .query_all_raw(stmt(
                "SELECT id, status FROM proposals WHERE org_id = $1 AND id::text = ANY($2)",
                vec![m.org_id.into(), ids.into()],
            ))
            .await?;
        let status = |id: &str| {
            statuses
                .iter()
                .find(|r| {
                    r.try_get::<Uuid>("", "id")
                        .ok()
                        .map(|u| u.to_string())
                        .as_deref()
                        == Some(id)
                })
                .and_then(|r| r.try_get::<String>("", "status").ok())
                .unwrap_or_default()
        };
        let accepted = m
            .candidates
            .iter()
            .find(|c| {
                c.proposal_id
                    .as_deref()
                    .is_some_and(|p| status(p) == "accepted")
            })
            .cloned();
        if let Some(c) = accepted {
            let source = format!("person_match:{}", m.id);
            let person = m.person_id.to_string();
            match c.asset_id.as_deref().and_then(|a| Uuid::parse_str(a).ok()) {
                Some(a) => {
                    profiles::attach_profile(ctx, m.org_id, &source, &person, &c.url, a).await?
                }
                // The accepted link names the identifier already.
                None => {}
            }
            m.state = MatchState::Matched;
            save(db, &m).await?;
            n += 1;
            continue;
        }
        let open = m.candidates.iter().any(|c| {
            c.proposal_id
                .as_deref()
                .is_some_and(|p| status(p) == "proposed")
        });
        if !open {
            m.step += 1;
            if spawn_search(ctx, &mut m).await.is_err() {
                m.state = MatchState::Stopped;
            }
            save(db, &m).await?;
            n += 1;
        }
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
                   AND NOT EXISTS (SELECT 1 FROM person_matches m
                     WHERE m.org_id = p.org_id AND m.person_id = p.id
                       AND (m.state IN ('searching', 'proposed')
                            OR m.created_at > now() - make_interval(days => $2::int)))
                 ORDER BY (c.properties ->> 'target_score')::numeric DESC NULLS LAST, p.created_at
                 LIMIT $3",
                vec![org_id.into(), (retry as i32).into(), left.into()],
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
