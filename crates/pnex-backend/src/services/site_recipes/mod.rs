//! Site navigation rules and their run journal (site-recipes.md D323,
//! D326). A recipe is YAML anchored on one web site (normalised host,
//! [`site_host`]); saving bumps its version. A run records the YAML it
//! played (a studio run plays an unsaved draft), its steps as they finish,
//! its extracts and the storage keys of its screenshots and PDFs. The
//! browser part lives in [`browser`].

pub mod browser;
pub mod cdp;
pub mod endpoints;

use std::collections::BTreeMap;

use pnex_core::site_recipe::{
    site_host, validate, Recipe, RunExtract, RunStatus, RunStep, SiteRecipe, SiteRecipeRun,
    SiteRecipeVersion, Violation, WebSite,
};
use sea_orm::prelude::DateTimeWithTimeZone;
use sea_orm::{
    ConnectionTrait, DatabaseBackend, DatabaseConnection, FromQueryResult, Statement,
    TransactionTrait,
};
use uuid::Uuid;

/// Largest recipe YAML, bytes.
pub const YAML_MAX: usize = 64 * 1024;

fn stmt(sql: &str, values: Vec<sea_orm::Value>) -> Statement {
    Statement::from_sql_and_values(DatabaseBackend::Postgres, sql, values)
}

#[derive(Debug)]
pub enum RecipeError {
    Invalid(Vec<Violation>),
    NotFound,
    NameTaken,
    VersionConflict,
    Db(sea_orm::DbErr),
}

impl From<sea_orm::DbErr> for RecipeError {
    fn from(e: sea_orm::DbErr) -> Self {
        RecipeError::Db(e)
    }
}

/// YAML → checked recipe (syntax errors point at `yaml`).
pub fn parse(yaml: &str) -> Result<Recipe, Vec<Violation>> {
    let bad = |token: String| {
        vec![Violation {
            path: "yaml".into(),
            token,
        }]
    };
    if yaml.len() > YAML_MAX {
        return Err(bad(format!("max_length:{YAML_MAX}")));
    }
    let doc: serde_json::Value = serde_saphyr::from_str(yaml).map_err(|_| bad("syntax".into()))?;
    validate(&doc)
}

// ───────────────────────────── recipes ─────────────────────────────

#[derive(Debug, Clone, FromQueryResult)]
struct RecipeRow {
    id: Uuid,
    site: String,
    name: String,
    yaml: String,
    version: i32,
    created_at: DateTimeWithTimeZone,
    updated_at: DateTimeWithTimeZone,
}

const RECIPE_COLUMNS: &str = "id, site, name, yaml, version, created_at, updated_at";

impl RecipeRow {
    fn view(&self) -> SiteRecipe {
        SiteRecipe {
            id: self.id.to_string(),
            site: self.site.clone(),
            name: self.name.clone(),
            yaml: self.yaml.clone(),
            version: self.version,
            recipe: parse(&self.yaml).ok(),
            created_at: self.created_at.to_rfc3339(),
            updated_at: self.updated_at.to_rfc3339(),
        }
    }
}

/// Web sites of the org (one per host carrying a recipe), most recently
/// run first.
pub async fn sites(
    db: &DatabaseConnection,
    org_id: i64,
    limit: i64,
    offset: i64,
) -> Result<(i64, Vec<WebSite>), sea_orm::DbErr> {
    #[derive(FromQueryResult)]
    struct Row {
        site: String,
        recipes: i64,
        last_run_at: Option<DateTimeWithTimeZone>,
        last_status: Option<String>,
    }
    #[derive(FromQueryResult)]
    struct Count {
        n: i64,
    }
    let count = Count::find_by_statement(stmt(
        "SELECT count(DISTINCT site)::bigint AS n FROM site_recipes WHERE org_id = $1",
        vec![org_id.into()],
    ))
    .one(db)
    .await?
    .map_or(0, |c| c.n);
    let rows = Row::find_by_statement(stmt(
        "SELECT r.site, count(*)::bigint AS recipes, l.started_at AS last_run_at, l.status AS last_status \
         FROM site_recipes r \
         LEFT JOIN LATERAL (SELECT started_at, status FROM site_recipe_runs x \
             WHERE x.org_id = r.org_id AND x.site = r.site ORDER BY started_at DESC LIMIT 1) l ON true \
         WHERE r.org_id = $1 GROUP BY r.site, l.started_at, l.status \
         ORDER BY l.started_at DESC NULLS LAST, r.site LIMIT $2 OFFSET $3",
        vec![org_id.into(), limit.into(), offset.into()],
    ))
    .all(db)
    .await?;
    Ok((
        count,
        rows.into_iter()
            .map(|r| WebSite {
                site: r.site,
                recipes: r.recipes,
                last_run_at: r.last_run_at.map(|t| t.to_rfc3339()),
                last_status: r.last_status.as_deref().map(RunStatus::parse),
            })
            .collect(),
    ))
}

/// Recipes of the org, of one site when given (any spelling of its host).
pub async fn list(
    db: &DatabaseConnection,
    org_id: i64,
    site: Option<&str>,
) -> Result<Vec<SiteRecipe>, sea_orm::DbErr> {
    let site = site.map(|s| site_host(s).unwrap_or_else(|| s.trim().to_string()));
    let rows = RecipeRow::find_by_statement(stmt(
        &format!(
            "SELECT {RECIPE_COLUMNS} FROM site_recipes WHERE org_id = $1 \
             AND ($2::text IS NULL OR site = $2) ORDER BY site, name LIMIT 500"
        ),
        vec![org_id.into(), site.into()],
    ))
    .all(db)
    .await?;
    Ok(rows.iter().map(RecipeRow::view).collect())
}

pub async fn get(
    db: &DatabaseConnection,
    org_id: i64,
    id: Uuid,
) -> Result<SiteRecipe, RecipeError> {
    RecipeRow::find_by_statement(stmt(
        &format!("SELECT {RECIPE_COLUMNS} FROM site_recipes WHERE org_id = $1 AND id = $2"),
        vec![org_id.into(), id.into()],
    ))
    .one(db)
    .await?
    .map(|r| r.view())
    .ok_or(RecipeError::NotFound)
}

fn unique_violation(e: &sea_orm::DbErr) -> bool {
    e.to_string().contains("site_recipes_org_id_site_name_key")
}

pub async fn create(
    db: &DatabaseConnection,
    org_id: i64,
    user_id: i64,
    yaml: &str,
) -> Result<SiteRecipe, RecipeError> {
    let recipe = parse(yaml).map_err(RecipeError::Invalid)?;
    let site = site_host(&recipe.site).expect("validated site");
    let txn = db.begin().await?;
    let row = RecipeRow::find_by_statement(stmt(
        &format!(
            "INSERT INTO site_recipes (id, org_id, site, name, yaml, created_by, updated_by) \
             VALUES ($1, $2, $3, $4, $5, $6, $6) RETURNING {RECIPE_COLUMNS}"
        ),
        vec![
            Uuid::new_v4().into(),
            org_id.into(),
            site.into(),
            recipe.name.clone().into(),
            yaml.to_string().into(),
            user_id.into(),
        ],
    ))
    .one(&txn)
    .await
    .map_err(|e| {
        if unique_violation(&e) {
            RecipeError::NameTaken
        } else {
            RecipeError::Db(e)
        }
    })?
    .expect("RETURNING");
    keep_version(&txn, &row, user_id).await?;
    txn.commit().await?;
    Ok(row.view())
}

/// Files the row's YAML under its version (D221).
async fn keep_version<C: ConnectionTrait>(
    db: &C,
    row: &RecipeRow,
    user_id: i64,
) -> Result<(), sea_orm::DbErr> {
    db.execute_raw(stmt(
        "INSERT INTO site_recipe_versions (recipe_id, version, yaml, created_by) VALUES ($1, $2, $3, $4) \
         ON CONFLICT (recipe_id, version) DO NOTHING",
        vec![
            row.id.into(),
            row.version.into(),
            row.yaml.clone().into(),
            user_id.into(),
        ],
    ))
    .await?;
    Ok(())
}

/// Replaces the YAML; `expected_version` set and stale → conflict.
pub async fn update(
    db: &DatabaseConnection,
    org_id: i64,
    user_id: i64,
    id: Uuid,
    yaml: &str,
    expected_version: Option<i32>,
) -> Result<SiteRecipe, RecipeError> {
    let recipe = parse(yaml).map_err(RecipeError::Invalid)?;
    let current = get(db, org_id, id).await?;
    if expected_version.is_some_and(|v| v != current.version) {
        return Err(RecipeError::VersionConflict);
    }
    let site = site_host(&recipe.site).expect("validated site");
    let txn = db.begin().await?;
    let row = RecipeRow::find_by_statement(stmt(
        &format!(
            "UPDATE site_recipes SET site = $3, name = $4, yaml = $5, version = version + 1, \
             updated_by = $6, updated_at = now() \
             WHERE org_id = $1 AND id = $2 AND version = $7 RETURNING {RECIPE_COLUMNS}"
        ),
        vec![
            org_id.into(),
            id.into(),
            site.into(),
            recipe.name.clone().into(),
            yaml.to_string().into(),
            user_id.into(),
            current.version.into(),
        ],
    ))
    .one(&txn)
    .await
    .map_err(|e| {
        if unique_violation(&e) {
            RecipeError::NameTaken
        } else {
            RecipeError::Db(e)
        }
    })?
    .ok_or(RecipeError::VersionConflict)?;
    keep_version(&txn, &row, user_id).await?;
    txn.commit().await?;
    Ok(row.view())
}

/// Saved versions of a recipe, newest first.
pub async fn versions(
    db: &DatabaseConnection,
    org_id: i64,
    id: Uuid,
) -> Result<Vec<SiteRecipeVersion>, RecipeError> {
    get(db, org_id, id).await?;
    #[derive(FromQueryResult)]
    struct Row {
        version: i32,
        yaml: String,
        author: Option<String>,
        created_at: DateTimeWithTimeZone,
    }
    let rows = Row::find_by_statement(stmt(
        "SELECT v.version, v.yaml, u.email AS author, v.created_at FROM site_recipe_versions v \
         LEFT JOIN users u ON u.id = v.created_by WHERE v.recipe_id = $1 ORDER BY v.version DESC LIMIT 500",
        vec![id.into()],
    ))
    .all(db)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| SiteRecipeVersion {
            version: r.version,
            yaml: r.yaml,
            author: r.author,
            created_at: r.created_at.to_rfc3339(),
        })
        .collect())
}

/// Makes an old version the active one: saved again as a new version.
pub async fn restore(
    db: &DatabaseConnection,
    org_id: i64,
    user_id: i64,
    id: Uuid,
    version: i32,
) -> Result<SiteRecipe, RecipeError> {
    let old = versions(db, org_id, id)
        .await?
        .into_iter()
        .find(|v| v.version == version)
        .ok_or(RecipeError::NotFound)?;
    update(db, org_id, user_id, id, &old.yaml, None).await
}

pub async fn delete(db: &DatabaseConnection, org_id: i64, id: Uuid) -> Result<(), RecipeError> {
    let res = db
        .execute_raw(stmt(
            "DELETE FROM site_recipes WHERE org_id = $1 AND id = $2",
            vec![org_id.into(), id.into()],
        ))
        .await?;
    if res.rows_affected() == 0 {
        return Err(RecipeError::NotFound);
    }
    Ok(())
}

// ───────────────────────────── runs ─────────────────────────────

#[derive(Debug, Clone, FromQueryResult)]
struct RunRow {
    id: Uuid,
    recipe_id: Option<Uuid>,
    recipe_version: Option<i32>,
    site: String,
    name: String,
    trigger: String,
    status: String,
    failed_step: Option<i32>,
    reason: Option<String>,
    steps: serde_json::Value,
    extracts: serde_json::Value,
    keys: serde_json::Value,
    started_at: DateTimeWithTimeZone,
    finished_at: Option<DateTimeWithTimeZone>,
}

const RUN_COLUMNS: &str = "id, recipe_id, recipe_version, site, name, trigger, status, \
     failed_step, reason, steps, extracts, keys, started_at, finished_at";

impl RunRow {
    fn view(&self) -> SiteRecipeRun {
        let stale = self.status == "running"
            && self.started_at < chrono::Utc::now() - chrono::Duration::minutes(STALE_MINUTES);
        let (status, reason) = if stale {
            (
                RunStatus::Failed,
                Some(pnex_core::site_recipe::reason::INTERRUPTED.to_string()),
            )
        } else {
            (RunStatus::parse(&self.status), self.reason.clone())
        };
        SiteRecipeRun {
            id: self.id.to_string(),
            recipe_id: self.recipe_id.map(|u| u.to_string()),
            recipe_version: self.recipe_version,
            site: self.site.clone(),
            name: self.name.clone(),
            trigger: self.trigger.clone(),
            status,
            failed_step: self.failed_step.map(|i| i.max(0) as u32),
            reason,
            steps: serde_json::from_value(self.steps.clone()).unwrap_or_default(),
            extracts: serde_json::from_value(self.extracts.clone()).unwrap_or_default(),
            changes: BTreeMap::new(),
            started_at: self.started_at.to_rfc3339(),
            finished_at: self.finished_at.map(|t| t.to_rfc3339()),
        }
    }

    /// Storage keys of the run's files (name → key).
    fn keys(&self) -> BTreeMap<String, String> {
        serde_json::from_value(self.keys.clone()).unwrap_or_default()
    }
}

/// What a new run plays.
pub struct NewRun<'a> {
    pub recipe_id: Option<Uuid>,
    pub recipe_version: Option<i32>,
    pub recipe: &'a Recipe,
    pub yaml: &'a str,
    pub params: &'a BTreeMap<String, String>,
    pub trigger: &'a str,
    pub user_id: Option<i64>,
}

pub async fn insert_run(
    db: &DatabaseConnection,
    org_id: i64,
    run: &NewRun<'_>,
) -> Result<SiteRecipeRun, sea_orm::DbErr> {
    let row = RunRow::find_by_statement(stmt(
        &format!(
            "INSERT INTO site_recipe_runs (id, org_id, recipe_id, recipe_version, site, name, yaml, \
             params, trigger, status, keys, created_by) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, 'running', '{{}}', $10) RETURNING {RUN_COLUMNS}"
        ),
        vec![
            Uuid::new_v4().into(),
            org_id.into(),
            run.recipe_id.into(),
            run.recipe_version.into(),
            site_host(&run.recipe.site).unwrap_or_default().into(),
            run.recipe.name.clone().into(),
            run.yaml.to_string().into(),
            serde_json::to_value(run.params).unwrap_or_default().into(),
            run.trigger.to_string().into(),
            run.user_id.into(),
        ],
    ))
    .one(db)
    .await?;
    Ok(row.expect("RETURNING").view())
}

/// Saves the steps played so far (the studio polls them).
pub async fn save_progress(
    db: &DatabaseConnection,
    run_id: Uuid,
    steps: &[RunStep],
    extracts: &[RunExtract],
    keys: &BTreeMap<String, String>,
) -> Result<(), sea_orm::DbErr> {
    db.execute_raw(stmt(
        "UPDATE site_recipe_runs SET steps = $2, extracts = $3, keys = $4 WHERE id = $1",
        vec![
            run_id.into(),
            serde_json::to_value(steps).unwrap_or_default().into(),
            serde_json::to_value(extracts).unwrap_or_default().into(),
            serde_json::to_value(keys).unwrap_or_default().into(),
        ],
    ))
    .await?;
    Ok(())
}

pub async fn finish(
    db: &DatabaseConnection,
    run_id: Uuid,
    status: RunStatus,
    failed_step: Option<u32>,
    reason: Option<&str>,
) -> Result<(), sea_orm::DbErr> {
    db.execute_raw(stmt(
        "UPDATE site_recipe_runs SET status = $2, failed_step = $3, reason = $4, finished_at = now() \
         WHERE id = $1",
        vec![
            run_id.into(),
            status.as_str().into(),
            failed_step.map(|i| i as i32).into(),
            reason.map(str::to_string).into(),
        ],
    ))
    .await?;
    Ok(())
}

/// A run still `running` after this was cut by a restart (longer than
/// the run timeout of [`browser`]).
const STALE_MINUTES: i64 = 11;

/// Runs left `running` by a stopped server never finish: failed at boot
/// (only stale ones: another pod may be playing the recent ones).
pub async fn recover(db: &DatabaseConnection) -> Result<u64, sea_orm::DbErr> {
    let res = db
        .execute_raw(stmt(
            &format!(
                "UPDATE site_recipe_runs SET status = 'failed', reason = $1, finished_at = now() \
                 WHERE status = 'running' AND started_at < now() - interval '{STALE_MINUTES} minutes'"
            ),
            vec![pnex_core::site_recipe::reason::INTERRUPTED.into()],
        ))
        .await?;
    Ok(res.rows_affected())
}

/// Journal of a site or a recipe, newest first.
pub async fn runs(
    db: &DatabaseConnection,
    org_id: i64,
    site: Option<&str>,
    recipe_id: Option<Uuid>,
    limit: i64,
    offset: i64,
) -> Result<(i64, Vec<SiteRecipeRun>), sea_orm::DbErr> {
    #[derive(FromQueryResult)]
    struct Count {
        n: i64,
    }
    let site = site.map(|s| site_host(s).unwrap_or_else(|| s.trim().to_string()));
    let filter =
        "org_id = $1 AND ($2::text IS NULL OR site = $2) AND ($3::uuid IS NULL OR recipe_id = $3)";
    let count = Count::find_by_statement(stmt(
        &format!("SELECT count(*)::bigint AS n FROM site_recipe_runs WHERE {filter}"),
        vec![org_id.into(), site.clone().into(), recipe_id.into()],
    ))
    .one(db)
    .await?
    .map_or(0, |c| c.n);
    let rows = RunRow::find_by_statement(stmt(
        &format!(
            "SELECT {RUN_COLUMNS} FROM site_recipe_runs WHERE {filter} \
             ORDER BY started_at DESC LIMIT $4 OFFSET $5"
        ),
        vec![
            org_id.into(),
            site.into(),
            recipe_id.into(),
            limit.into(),
            offset.into(),
        ],
    ))
    .all(db)
    .await?;
    Ok((count, rows.iter().map(RunRow::view).collect()))
}

async fn run_row(db: &DatabaseConnection, org_id: i64, id: Uuid) -> Result<RunRow, RecipeError> {
    RunRow::find_by_statement(stmt(
        &format!("SELECT {RUN_COLUMNS} FROM site_recipe_runs WHERE org_id = $1 AND id = $2"),
        vec![org_id.into(), id.into()],
    ))
    .one(db)
    .await?
    .ok_or(RecipeError::NotFound)
}

/// One run, with what changed in each extract since the last earlier run
/// of the same rule (same site and name) that brought it back.
pub async fn run(
    db: &DatabaseConnection,
    org_id: i64,
    id: Uuid,
) -> Result<SiteRecipeRun, RecipeError> {
    let row = run_row(db, org_id, id).await?;
    let mut out = row.view();
    // shortcut: looks 20 runs back for each extract; enough for a rule
    // that fails now and then, a dedicated extract index if not.
    let earlier = RunRow::find_by_statement(stmt(
        &format!(
            "SELECT {RUN_COLUMNS} FROM site_recipe_runs WHERE org_id = $1 AND site = $2 AND name = $3 \
             AND id <> $4 AND finished_at IS NOT NULL AND started_at < $5 \
             ORDER BY started_at DESC LIMIT 20"
        ),
        vec![
            org_id.into(),
            row.site.clone().into(),
            row.name.clone().into(),
            id.into(),
            row.started_at.into(),
        ],
    ))
    .all(db)
    .await?;
    let earlier: Vec<SiteRecipeRun> = earlier.iter().map(RunRow::view).collect();
    for e in &out.extracts {
        let old = earlier
            .iter()
            .find_map(|r| r.extracts.iter().find(|o| o.name == e.name));
        match old {
            Some(old) if old.sha256 == e.sha256 => {}
            Some(old) => {
                out.changes.insert(
                    e.name.clone(),
                    pnex_core::text_diff::diff(&old.text, &e.text),
                );
            }
            None => {
                out.changes
                    .insert(e.name.clone(), pnex_core::text_diff::diff("", &e.text));
            }
        }
    }
    Ok(out)
}

/// Storage key of a run's file (`step-3.jpg`, `pdf-5.pdf`), if it has one.
pub async fn file_key(
    db: &DatabaseConnection,
    org_id: i64,
    id: Uuid,
    file: &str,
) -> Result<Option<String>, RecipeError> {
    Ok(run_row(db, org_id, id).await?.keys().remove(file))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn yaml_written_by_the_visual_editor_parses_back() {
        let yaml = "name: programme\nsite: of-exemple.example\nparams: [code]\nallow_hosts: [cdn.of-exemple.example]\n\
                    steps:\n  - open: \"/formations/\"\n  - click: {text: \"Tout accepter\", optional: true}\n\
                    \x20 - click: {selector: \"a[href*='seo-geo']\"}\n  - wait: {text: \"Programme\", timeout_ms: 5000}\n\
                    \x20 - follow_pdf: {href_contains: \".pdf\", as: \"pdf\", optional: true}\n\
                    \x20 - extract: {between: [\"Programme\", \"Modalités\"], as: \"programme\"}\n\
                    \x20 - assert: {count_min: {selector: \"a\", min: 3}}\n  - assert: {not_empty: \"programme\"}\n";
        let recipe = parse(yaml).expect("hand-written YAML");
        let written = pnex_core::site_recipe::to_yaml(&recipe);
        assert_eq!(parse(&written).expect("written YAML"), recipe, "{written}");
    }
}
