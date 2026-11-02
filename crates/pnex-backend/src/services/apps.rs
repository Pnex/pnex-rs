//! Applications declared in an org (external-apps.md D433). An
//! application dresses a service account of the org with the scope it
//! reads; the change feed keeps to that scope when the application calls.

use std::collections::{BTreeMap, HashMap};

use chrono::{DateTime, FixedOffset};
use pnex_core::apps::{
    bind, check, check_build, check_object_sets, params_of, AppBuildInput, AppInput, AppScope,
    AppScreen, AppScreenView, AppVariable, AppView, AppWidgetOptions, ObjectSetDef, VariableKind,
};
use pnex_core::err_codes::{
    FIELD_INVALID_UUID, FIELD_NOT_FOUND, FIELD_OUT_OF_SCOPE, FIELD_REQUIRED, FIELD_UNIQUE,
};
use pnex_core::ontology::api::{Direction, QueryResult};
use pnex_core::ontology::TypeSet;
use pnex_core::{DashboardLayout, VizViolation};
use sea_orm::{ConnectionTrait, DatabaseConnection, DbErr, FromQueryResult, Statement};
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    /// `{field: token}` of the input.
    #[error("invalid fields")]
    Fields(BTreeMap<String, String>),
    #[error("not found")]
    NotFound,
    #[error("version conflict (current {current})")]
    VersionConflict { current: i32 },
    /// A screen names what its application does not have (D508).
    #[error("invalid screen")]
    Violations(Vec<VizViolation>),
    /// Outside the scope or the action list of the application (D511).
    #[error("out of scope")]
    OutOfScope,
    /// The query of an object set refused or failed.
    #[error(transparent)]
    Query(#[from] crate::services::ontology::OntologyError),
    #[error(transparent)]
    Db(#[from] DbErr),
}

pub type Result<T> = std::result::Result<T, AppError>;

fn field(name: &str, token: &str) -> AppError {
    AppError::Fields(BTreeMap::from([(name.to_string(), token.to_string())]))
}

fn stmt(sql: &str, values: Vec<sea_orm::Value>) -> Statement {
    Statement::from_sql_and_values(sea_orm::DatabaseBackend::Postgres, sql, values)
}

const SELECT: &str =
    "SELECT id, key, name, names, description, service_account_id, scope, object_sets, \
                      screens, home, variables, actions, version, created_at, updated_at FROM apps";

#[derive(FromQueryResult)]
struct Row {
    id: Uuid,
    key: String,
    name: String,
    names: serde_json::Value,
    description: String,
    service_account_id: Option<Uuid>,
    scope: serde_json::Value,
    object_sets: serde_json::Value,
    screens: serde_json::Value,
    home: Option<String>,
    variables: serde_json::Value,
    actions: serde_json::Value,
    version: i32,
    created_at: DateTime<FixedOffset>,
    updated_at: DateTime<FixedOffset>,
}

impl From<Row> for AppView {
    fn from(r: Row) -> Self {
        AppView {
            id: r.id.to_string(),
            key: r.key,
            name: r.name,
            names: serde_json::from_value(r.names).unwrap_or_default(),
            description: r.description,
            service_account_id: r.service_account_id.map(|i| i.to_string()),
            scope: serde_json::from_value(r.scope).unwrap_or_default(),
            object_sets: serde_json::from_value(r.object_sets).unwrap_or_default(),
            screens: serde_json::from_value(r.screens).unwrap_or_default(),
            home: r.home,
            variables: serde_json::from_value(r.variables).unwrap_or_default(),
            actions: serde_json::from_value(r.actions).unwrap_or_default(),
            version: r.version,
            created_at: r.created_at.to_rfc3339(),
            updated_at: r.updated_at.to_rfc3339(),
        }
    }
}

pub async fn list(db: &DatabaseConnection, org_id: i64) -> Result<Vec<AppView>> {
    Ok(Row::find_by_statement(stmt(
        &format!("{SELECT} WHERE org_id = $1 ORDER BY name, key"),
        vec![org_id.into()],
    ))
    .all(db)
    .await?
    .into_iter()
    .map(Into::into)
    .collect())
}

pub async fn get(db: &impl ConnectionTrait, org_id: i64, id: Uuid) -> Result<AppView> {
    Row::find_by_statement(stmt(
        &format!("{SELECT} WHERE org_id = $1 AND id = $2"),
        vec![org_id.into(), id.into()],
    ))
    .one(db)
    .await?
    .map(Into::into)
    .ok_or(AppError::NotFound)
}

/// The application a service account signs in for, if any.
pub async fn of_service_account(
    db: &impl ConnectionTrait,
    org_id: i64,
    service_account_id: Uuid,
) -> Result<Option<AppView>> {
    Ok(Row::find_by_statement(stmt(
        &format!("{SELECT} WHERE org_id = $1 AND service_account_id = $2"),
        vec![org_id.into(), service_account_id.into()],
    ))
    .one(db)
    .await?
    .map(Into::into))
}

/// Field checks that need the org: the scope names existing types, and the
/// service account is the org's and no other application's.
async fn check_in_org(
    db: &DatabaseConnection,
    org_id: i64,
    input: &AppInput,
    except: Option<Uuid>,
) -> Result<(Option<Uuid>, AppScope)> {
    let errors = check(input);
    if !errors.is_empty() {
        return Err(AppError::Fields(errors));
    }
    let s = crate::services::ontology::schema(db, org_id).await?;
    if input.scope.types.iter().any(|k| s.object_type(k).is_none()) {
        return Err(field("scope.types", FIELD_NOT_FOUND));
    }
    if input
        .scope
        .link_types
        .iter()
        .any(|k| s.link_type(k).is_none())
    {
        return Err(field("scope.link_types", FIELD_NOT_FOUND));
    }
    let account = match input
        .service_account_id
        .as_deref()
        .filter(|s| !s.is_empty())
    {
        None => None,
        Some(raw) => {
            let id = Uuid::parse_str(raw)
                .map_err(|_| field("service_account_id", FIELD_INVALID_UUID))?;
            if crate::services::iam::service::get_account(db, org_id, id)
                .await
                .is_err()
            {
                return Err(field("service_account_id", FIELD_NOT_FOUND));
            }
            if of_service_account(db, org_id, id)
                .await?
                .is_some_and(|a| Some(a.id.as_str()) != except.map(|e| e.to_string()).as_deref())
            {
                return Err(field("service_account_id", FIELD_UNIQUE));
            }
            Some(id)
        }
    };
    for (name, set) in &input.object_sets {
        check_set(&s, &input.scope, name, set)?;
    }
    for a in &input.actions {
        let known = a.split_once('.').is_some_and(|(t, k)| {
            input.scope.types.iter().any(|x| x == t)
                && s.object_type(t).is_some_and(|d| d.action(k).is_some())
        });
        if !known {
            return Err(field("actions", FIELD_OUT_OF_SCOPE));
        }
    }
    let mut scope = input.scope.clone();
    for list in [
        &mut scope.types,
        &mut scope.link_types,
        &mut scope.media_kinds,
    ] {
        list.sort();
        list.dedup();
    }
    Ok((account, scope))
}

/// An object set keeps to the scope: its type, every link it follows and
/// the types those links reach (D434).
fn check_set(
    s: &crate::services::ontology::OrgSchema,
    scope: &AppScope,
    name: &str,
    set: &ObjectSetDef,
) -> Result<()> {
    let in_scope = |t: &String| scope.types.contains(t);
    if !set.query.type_key.as_ref().is_some_and(in_scope) {
        return Err(field(
            &format!("object_sets.{name}.type_key"),
            FIELD_OUT_OF_SCOPE,
        ));
    }
    for hop in &set.query.traverse {
        let reached = s.link_type(&hop.link_type).map(|l| match hop.direction {
            Direction::Out => &l.to_types,
            Direction::In => &l.from_types,
        });
        let ok = scope.link_types.contains(&hop.link_type)
            && matches!(reached, Some(TypeSet::Only(types)) if types.iter().all(in_scope));
        if !ok {
            return Err(field(
                &format!("object_sets.{name}.traverse"),
                FIELD_OUT_OF_SCOPE,
            ));
        }
    }
    Ok(())
}

/// Runs an object set of an application with its parameters (text), paged.
pub async fn run_set(
    db: &DatabaseConnection,
    client: Option<&crate::services::openobserve::client::Client>,
    org_id: i64,
    app: &AppView,
    name: &str,
    given: &HashMap<String, String>,
    limit: Option<u64>,
    offset: Option<u64>,
    sort: Option<(String, bool)>,
    hide: crate::services::ontology::redact::Redaction,
) -> Result<QueryResult> {
    let set = app.object_sets.get(name).ok_or(AppError::NotFound)?;
    let mut q = bind(&set.query, given).map_err(|p| field(&p, FIELD_REQUIRED))?;
    q.limit = limit.or(q.limit);
    q.offset = offset.or(q.offset);
    if let Some((key, descending)) = sort {
        q.order_by = Some(key);
        q.descending = descending;
    }
    Ok(crate::services::ontology::query::run(db, client, org_id, &q, hide).await?)
}

async fn key_taken(
    db: &DatabaseConnection,
    org_id: i64,
    key: &str,
    except: Option<Uuid>,
) -> Result<bool> {
    Ok(db
        .query_one_raw(stmt(
            "SELECT 1 AS one FROM apps WHERE org_id = $1 AND key = $2 AND id IS DISTINCT FROM $3",
            vec![org_id.into(), key.into(), except.into()],
        ))
        .await?
        .is_some())
}

pub async fn create(
    db: &DatabaseConnection,
    org_id: i64,
    user_id: i64,
    input: &AppInput,
) -> Result<AppView> {
    let (account, scope) = check_in_org(db, org_id, input, None).await?;
    if key_taken(db, org_id, &input.key, None).await? {
        return Err(field("key", FIELD_UNIQUE));
    }
    let id = Uuid::new_v4();
    db.execute_raw(stmt(
        "INSERT INTO apps (id, org_id, key, name, description, service_account_id, scope, \
           object_sets, created_by, actions) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
        vec![
            id.into(),
            org_id.into(),
            input.key.clone().into(),
            input.name.trim().to_string().into(),
            input.description.clone().into(),
            account.into(),
            serde_json::to_value(&scope).unwrap_or_default().into(),
            serde_json::to_value(&input.object_sets)
                .unwrap_or_default()
                .into(),
            user_id.into(),
            serde_json::to_value(&input.actions)
                .unwrap_or_default()
                .into(),
        ],
    ))
    .await?;
    get(db, org_id, id).await
}

/// Replaces an application on the version it was read on.
pub async fn update(
    db: &DatabaseConnection,
    org_id: i64,
    id: Uuid,
    input: &AppInput,
) -> Result<AppView> {
    let current = get(db, org_id, id).await?;
    let Some(expected) = input.expected_version else {
        return Err(field(
            "expected_version",
            pnex_core::err_codes::FIELD_REQUIRED,
        ));
    };
    let (account, scope) = check_in_org(db, org_id, input, Some(id)).await?;
    if key_taken(db, org_id, &input.key, Some(id)).await? {
        return Err(field("key", FIELD_UNIQUE));
    }
    let done = db
        .execute_raw(stmt(
            "UPDATE apps SET key = $3, name = $4, description = $5, service_account_id = $6, \
               scope = $7, object_sets = $9, actions = $10, version = version + 1, updated_at = now() \
             WHERE org_id = $1 AND id = $2 AND version = $8",
            vec![
                org_id.into(),
                id.into(),
                input.key.clone().into(),
                input.name.trim().to_string().into(),
                input.description.clone().into(),
                account.into(),
                serde_json::to_value(&scope).unwrap_or_default().into(),
                expected.into(),
                serde_json::to_value(&input.object_sets)
                    .unwrap_or_default()
                    .into(),
                serde_json::to_value(&input.actions)
                    .unwrap_or_default()
                    .into(),
            ],
        ))
        .await?;
    if done.rows_affected() != 1 {
        return Err(AppError::VersionConflict {
            current: current.version,
        });
    }
    get(db, org_id, id).await
}

/// Deleting an application takes its screens with it.
pub async fn delete(ctx: &loco_rs::app::AppContext, org_id: i64, id: Uuid) -> Result<()> {
    let db = &ctx.db;
    for s in get(db, org_id, id).await?.screens {
        purge_screen(ctx, org_id, &s).await?;
    }
    let done = db
        .execute_raw(stmt(
            "DELETE FROM apps WHERE org_id = $1 AND id = $2",
            vec![org_id.into(), id.into()],
        ))
        .await?;
    if done.rows_affected() == 0 {
        return Err(AppError::NotFound);
    }
    Ok(())
}

async fn purge_screen(
    ctx: &loco_rs::app::AppContext,
    org_id: i64,
    screen: &AppScreen,
) -> Result<()> {
    if let Ok(id) = Uuid::parse_str(&screen.dashboard_id) {
        crate::services::dashboards::purge(ctx, org_id, id).await?;
    }
    Ok(())
}

/// Type and property of a variable exist in the application's scope.
fn check_variable(
    s: &crate::services::ontology::OrgSchema,
    scope: &AppScope,
    i: usize,
    v: &AppVariable,
) -> Result<()> {
    if let Some(t) = &v.type_key {
        if !scope.types.contains(t) {
            return Err(field(
                &format!("variables.{i}.type_key"),
                FIELD_OUT_OF_SCOPE,
            ));
        }
        if let Some(p) = &v.property {
            let known = s
                .object_type(t)
                .is_some_and(|d| d.properties.iter().any(|x| &x.key == p));
            if !known {
                return Err(field(&format!("variables.{i}.property"), FIELD_NOT_FOUND));
            }
        }
    }
    Ok(())
}

/// Replaces what a builder changes (D507, Q-B1) on the version it was read
/// on: a new screen key creates its dashboard, a missing key removes it.
pub async fn build(
    ctx: &loco_rs::app::AppContext,
    org_id: i64,
    id: Uuid,
    input: &AppBuildInput,
    author: &str,
) -> Result<AppView> {
    let db = &ctx.db;
    let current = get(db, org_id, id).await?;
    let mut errors = check_build(input);
    check_object_sets(&input.object_sets, &mut errors);
    if !errors.is_empty() {
        return Err(AppError::Fields(errors));
    }
    let s = crate::services::ontology::schema(db, org_id).await?;
    for (name, set) in &input.object_sets {
        check_set(&s, &current.scope, name, set)?;
    }
    for (i, v) in input.variables.iter().enumerate() {
        check_variable(&s, &current.scope, i, v)?;
    }
    if input.expected_version != current.version {
        return Err(AppError::VersionConflict {
            current: current.version,
        });
    }
    let mut screens = Vec::with_capacity(input.screens.len());
    let mut created = Vec::new();
    for i in &input.screens {
        let name = i.name.trim().to_string();
        let kept = current.screens.iter().find(|s| s.key == i.key);
        let (dashboard_id, published_version) = match kept {
            Some(k) => (k.dashboard_id.clone(), k.published_version),
            None => {
                let (d, _) = crate::services::dashboards::create_dashboard(
                    db,
                    org_id,
                    &name,
                    None,
                    None,
                    Some(author.to_string()),
                )
                .await
                .map_err(|_| DbErr::Custom("screen not created".into()))?;
                db.execute_raw(stmt(
                    "UPDATE dashboards SET app_id = $1 WHERE id = $2",
                    vec![id.into(), d.id.into()],
                ))
                .await?;
                created.push(d.id);
                (d.id.to_string(), None)
            }
        };
        screens.push(AppScreen {
            key: i.key.clone(),
            name,
            names: kept.map(|k| k.names.clone()).unwrap_or_default(),
            icon: i.icon.clone(),
            dashboard_id,
            published_version,
        });
    }
    let done = db
        .execute_raw(stmt(
            "UPDATE apps SET screens = $3, home = $4, variables = $5, object_sets = $6, \
               version = version + 1, updated_at = now() \
             WHERE org_id = $1 AND id = $2 AND version = $7",
            vec![
                org_id.into(),
                id.into(),
                serde_json::to_value(&screens).unwrap_or_default().into(),
                input.home.clone().into(),
                serde_json::to_value(&input.variables)
                    .unwrap_or_default()
                    .into(),
                serde_json::to_value(&input.object_sets)
                    .unwrap_or_default()
                    .into(),
                input.expected_version.into(),
            ],
        ))
        .await?;
    if done.rows_affected() != 1 {
        for d in created {
            crate::services::dashboards::purge(ctx, org_id, d).await?;
        }
        return Err(AppError::VersionConflict {
            current: get(db, org_id, id).await?.version,
        });
    }
    for gone in current
        .screens
        .iter()
        .filter(|c| !screens.iter().any(|s| s.key == c.key))
    {
        purge_screen(ctx, org_id, gone).await?;
    }
    for s in &screens {
        if let Ok(d) = Uuid::parse_str(&s.dashboard_id) {
            db.execute_raw(stmt(
                "UPDATE dashboards SET name = $1 WHERE id = $2 AND name <> $1",
                vec![s.name.clone().into(), d.into()],
            ))
            .await?;
        }
    }
    get(db, org_id, id).await
}

/// The layout of one version of a screen's dashboard.
async fn layout_at(
    db: &impl ConnectionTrait,
    dashboard_id: Uuid,
    version: i64,
) -> Result<Option<DashboardLayout>> {
    Ok(db
        .query_one_raw(stmt(
            "SELECT layout FROM dashboard_versions WHERE dashboard_id = $1 AND version_number = $2",
            vec![dashboard_id.into(), version.into()],
        ))
        .await?
        .and_then(|r| r.try_get::<serde_json::Value>("", "layout").ok())
        .and_then(|v| serde_json::from_value(v).ok()))
}

/// Readers now see the current version of every screen (Q-B2), each checked
/// against the application as it is now.
pub async fn publish(
    db: &DatabaseConnection,
    org_id: i64,
    id: Uuid,
    expected_version: i32,
) -> Result<AppView> {
    let app = get(db, org_id, id).await?;
    if app.version != expected_version {
        return Err(AppError::VersionConflict {
            current: app.version,
        });
    }
    let s = crate::services::ontology::schema(db, org_id).await?;
    let mut screens = app.screens.clone();
    let mut violations = Vec::new();
    for screen in &mut screens {
        let dashboard = Uuid::parse_str(&screen.dashboard_id).map_err(|_| AppError::NotFound)?;
        let current: i64 = db
            .query_one_raw(stmt(
                "SELECT current_version_number AS v FROM dashboards WHERE id = $1 AND org_id = $2",
                vec![dashboard.into(), org_id.into()],
            ))
            .await?
            .and_then(|r| r.try_get("", "v").ok())
            .ok_or(AppError::NotFound)?;
        let layout = layout_at(db, dashboard, current)
            .await?
            .ok_or(AppError::NotFound)?;
        violations.extend(check_screen(&s, &app, &layout));
        screen.published_version = Some(current);
    }
    if !violations.is_empty() {
        return Err(AppError::Violations(violations));
    }
    let done = db
        .execute_raw(stmt(
            "UPDATE apps SET screens = $3, version = version + 1, updated_at = now() \
             WHERE org_id = $1 AND id = $2 AND version = $4",
            vec![
                org_id.into(),
                id.into(),
                serde_json::to_value(&screens).unwrap_or_default().into(),
                expected_version.into(),
            ],
        ))
        .await?;
    if done.rows_affected() != 1 {
        return Err(AppError::VersionConflict {
            current: get(db, org_id, id).await?.version,
        });
    }
    get(db, org_id, id).await
}

/// A screen as its readers see it: the published version, 404 before any.
pub async fn screen(db: &DatabaseConnection, app: &AppView, key: &str) -> Result<AppScreenView> {
    let screen = app
        .screens
        .iter()
        .find(|s| s.key == key)
        .ok_or(AppError::NotFound)?;
    let version = screen.published_version.ok_or(AppError::NotFound)?;
    let dashboard = Uuid::parse_str(&screen.dashboard_id).map_err(|_| AppError::NotFound)?;
    Ok(AppScreenView {
        key: screen.key.clone(),
        name: screen.name.clone(),
        version,
        layout: layout_at(db, dashboard, version)
            .await?
            .ok_or(AppError::NotFound)?,
    })
}

/// What the widgets of a screen name exists in its application (D508).
pub fn check_screen(
    s: &crate::services::ontology::OrgSchema,
    app: &AppView,
    layout: &DashboardLayout,
) -> Vec<VizViolation> {
    let mut out = Vec::new();
    for w in &layout.widgets {
        if let Some(o) = &w.options.app {
            check_widget(s, app, &w.widget_type, o, &mut |code: &str, msg: String| {
                out.push(VizViolation::new(Some(&w.id), code, msg));
            });
        }
    }
    out
}

fn check_widget(
    s: &crate::services::ontology::OrgSchema,
    app: &AppView,
    widget_type: &str,
    o: &AppWidgetOptions,
    push: &mut impl FnMut(&str, String),
) {
    let variable = |name: &str| app.variables.iter().find(|v| v.name == name);
    let columns_on = |type_key: Option<&str>, push: &mut dyn FnMut(&str, String)| {
        let def = type_key.and_then(|t| s.object_type(t));
        for c in o.columns.iter().filter(|c| c.as_str() != "title") {
            if !def.is_some_and(|d| d.properties.iter().any(|p| &p.key == c)) {
                push("app_column_unknown", format!("unknown property \"{c}\""));
            }
        }
    };
    // The variable a widget reads or writes, of the kind it needs.
    let wants_object = matches!(
        widget_type,
        "object_table" | "object_fields" | "action_button"
    );
    let required = matches!(widget_type, "object_fields" | "action_button" | "input");
    match o.variable.as_deref().filter(|v| !v.is_empty()) {
        None if required => push(
            "app_variable_missing",
            "this widget needs a variable".into(),
        ),
        None => {}
        Some(name) => match variable(name) {
            None => push(
                "app_variable_unknown",
                format!("unknown variable \"{name}\""),
            ),
            Some(v) => {
                let is_object = v.kind == VariableKind::Object;
                if (wants_object && !is_object) || (widget_type == "input" && is_object) {
                    push(
                        "app_variable_kind",
                        format!("variable \"{name}\" has the wrong kind"),
                    );
                }
            }
        },
    }
    match widget_type {
        "object_table" | "object_count" => {
            let Some(set) = o.object_set.as_deref().and_then(|n| app.object_sets.get(n)) else {
                push("app_set_unknown", "unknown object set".into());
                return;
            };
            for p in params_of(&set.query) {
                match o.params.get(&p) {
                    None => push(
                        "app_param_unset",
                        format!("parameter \"{p}\" has no variable"),
                    ),
                    Some(v) if variable(v).is_none() => {
                        push("app_variable_unknown", format!("unknown variable \"{v}\""));
                    }
                    Some(_) => {}
                }
            }
            columns_on(set.query.type_key.as_deref(), push);
            if let Some(k) = o.screen.as_deref().filter(|k| !k.is_empty()) {
                if !app.screens.iter().any(|s| s.key == k) {
                    push("app_screen_unknown", format!("unknown screen \"{k}\""));
                }
            }
        }
        "object_fields" => {
            let of_variable = o
                .variable
                .as_deref()
                .and_then(variable)
                .and_then(|v| v.type_key.as_deref());
            columns_on(of_variable, push);
        }
        "object_form" => {
            if !o
                .object_type
                .as_ref()
                .is_some_and(|t| app.scope.types.contains(t))
            {
                push(
                    "app_type_out_of_scope",
                    "type out of the application's scope".into(),
                );
            }
        }
        "action_button" if !o.action.as_ref().is_some_and(|a| app.actions.contains(a)) => {
            push(
                "app_action_not_permitted",
                "action not in the application's list".into(),
            );
        }
        _ => {}
    }
}

/// Checks a screen before a save, with its application read here.
pub async fn check_screen_of(
    db: &impl ConnectionTrait,
    org_id: i64,
    app_id: Uuid,
    layout: &DashboardLayout,
) -> Result<Vec<VizViolation>> {
    let app = get(db, org_id, app_id).await?;
    let s = crate::services::ontology::schema(db, org_id).await?;
    Ok(check_screen(&s, &app, layout))
}

/// A write through a screen keeps to its application's scope (D511).
pub async fn guard_type(
    db: &impl ConnectionTrait,
    org_id: i64,
    app_id: Uuid,
    type_key: &str,
) -> Result<()> {
    if get(db, org_id, app_id)
        .await?
        .scope
        .types
        .iter()
        .any(|t| t == type_key)
    {
        Ok(())
    } else {
        Err(AppError::OutOfScope)
    }
}

/// An action through a screen is in its application's list (D440, D511).
pub async fn guard_action(
    db: &impl ConnectionTrait,
    org_id: i64,
    app_id: Uuid,
    type_key: &str,
    action: &str,
) -> Result<()> {
    let wanted = format!("{type_key}.{action}");
    if get(db, org_id, app_id).await?.actions.contains(&wanted) {
        Ok(())
    } else {
        Err(AppError::OutOfScope)
    }
}

/// Creates what a pack ships (D513), screens published: an application the
/// org already has of that key is its own and stays as it is. `Ok(false)`
/// = kept.
pub async fn install_shipped(
    db: &DatabaseConnection,
    org_id: i64,
    user_id: Option<i64>,
    shipped: &pnex_core::apps::PackApp,
) -> Result<bool> {
    if key_taken(db, org_id, &shipped.key, None).await? {
        return Ok(false);
    }
    let Some(user) = user_id else {
        // A pack installed by the platform: nobody to own the declaration.
        return Ok(false);
    };
    let app = create(
        db,
        org_id,
        user,
        &AppInput {
            key: shipped.key.clone(),
            name: shipped.name.clone(),
            description: shipped.description.clone(),
            scope: shipped.scope.clone(),
            object_sets: shipped.object_sets.clone(),
            actions: shipped.actions.clone(),
            ..Default::default()
        },
    )
    .await?;
    let id = Uuid::parse_str(&app.id).map_err(|_| AppError::NotFound)?;
    let mut screens = Vec::with_capacity(shipped.screens.len());
    let mut planned = app.clone();
    planned.variables = shipped.variables.clone();
    planned.screens = shipped
        .screens
        .iter()
        .map(|s| AppScreen {
            key: s.key.clone(),
            ..Default::default()
        })
        .collect();
    let schema = crate::services::ontology::schema(db, org_id).await?;
    for s in &shipped.screens {
        let violations = check_screen(&schema, &planned, &s.layout);
        if !violations.is_empty() {
            return Err(AppError::Violations(violations));
        }
        let (d, version) =
            crate::services::dashboards::create_screen(db, org_id, id, &s.name, &s.layout)
                .await
                .map_err(|_| DbErr::Custom("screen not created".into()))?;
        screens.push(AppScreen {
            key: s.key.clone(),
            name: s.name.clone(),
            names: s.names.clone(),
            icon: None,
            dashboard_id: d.id.to_string(),
            published_version: Some(version),
        });
    }
    db.execute_raw(stmt(
        "UPDATE apps SET names = $3, screens = $4, home = $5, variables = $6 \
         WHERE org_id = $1 AND id = $2",
        vec![
            org_id.into(),
            id.into(),
            serde_json::to_value(&shipped.names)
                .unwrap_or_default()
                .into(),
            serde_json::to_value(&screens).unwrap_or_default().into(),
            shipped.home.clone().into(),
            serde_json::to_value(&shipped.variables)
                .unwrap_or_default()
                .into(),
        ],
    ))
    .await?;
    Ok(true)
}
