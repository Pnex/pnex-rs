//! Service des dashboards (studio SCADA, D24/D40/D41) — point d'écriture
//! unique des documents versionnés, école `services/flow.rs`.
//!
//! Cycle de vie **D24** : save = nouvelle version = live — pas de publish
//! séparé en V1. `current_version_number` (côté parent) pointe la version
//! rendue en live ; le restore (école media) **re-positionne** ce pointeur
//! sans créer de version. Les versions sont append-only, jamais purgées.
//!
//! Aucun runtime à acquitter ici (contrairement aux flows) : « déployer »
//! un dashboard = changer la version que les viewers lisent — un simple
//! swap de pointeur en base.

use loco_rs::prelude::*;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QuerySelect, Set,
    TransactionTrait,
};

use crate::models::_entities::{dashboard_versions, dashboards};
use pnex_core::{DashboardLayout, VizViolation};

/// Erreurs d'écriture d'un dashboard — le contrôleur les mappe en HTTP
/// (400 champ/violations, 409 conflit, 500).
#[derive(Debug)]
pub enum DashboardWriteError {
    /// `validate_layout` non satisfait.
    Violations(Vec<VizViolation>),
    /// Nom vide après trim.
    NameRequired,
    /// Nom > 255 caractères (colonne varchar(255)).
    NameTooLong,
    /// Concurrence optimiste perdue.
    Conflict { expected: i64, current: i64 },
    /// Version visée par un restore inexistante.
    VersionUnknown,
    /// Échec base de données.
    Db,
}

/// Validation commune (nom + layout) avant toute écriture.
fn validate_dashboard_write(
    name: &str,
    layout: &DashboardLayout,
) -> Result<(), DashboardWriteError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(DashboardWriteError::NameRequired);
    }
    if name.chars().count() > 255 {
        return Err(DashboardWriteError::NameTooLong);
    }
    let violations = pnex_core::validate_layout(layout);
    if !violations.is_empty() {
        return Err(DashboardWriteError::Violations(violations));
    }
    Ok(())
}

/// D131: provisions the controls declared by the control widgets (inside
/// the save transaction) and returns the layout with every control widget
/// bound to its control — the version stored and returned to the editor.
async fn bind_surface_controls<C: sea_orm::ConnectionTrait>(
    db: &C,
    org_id: i64,
    dashboard_id: uuid::Uuid,
    layout: &DashboardLayout,
) -> Result<DashboardLayout, DashboardWriteError> {
    use crate::services::surface_controls::{sync_surface, DeclaredControl};
    use pnex_core::ui_control::{ControlKind, ControlRef, ORIGIN_DASHBOARD};
    let mut items: Vec<DeclaredControl> = layout
        .widgets
        .iter()
        .filter_map(|w| {
            let kind = ControlKind::ALL
                .into_iter()
                .find(|k| k.widget_type() == w.widget_type)?;
            Some(DeclaredControl {
                item_id: w.id.clone(),
                kind: Some(kind),
                label: w.title.clone(),
                current: w.options.control.as_ref().map(|c| c.control_id),
                spec: w.options.control_spec.clone(),
            })
        })
        .collect();
    // Home cards (D138): one control per active role, item `{widget}.{role}`.
    for w in &layout.widgets {
        let Some(home) = &w.options.home else {
            continue;
        };
        let title = if w.title.trim().is_empty() {
            w.id.as_str()
        } else {
            w.title.trim()
        };
        // Room = title of the mobile section holding the card.
        let room = layout
            .sections
            .iter()
            .find(|s| s.items.contains(&w.id))
            .map(|s| s.title.as_str())
            .unwrap_or_default();
        for r in home.active_roles() {
            items.push(DeclaredControl {
                item_id: pnex_core::home::role_item_id(&w.id, r.role),
                kind: Some(r.kind),
                label: pnex_core::home::role_control_label(room, title, r.role),
                current: home.control_of(r.role),
                spec: home.spec_of(r.role),
            });
        }
    }
    let outcome = sync_surface(db, org_id, ORIGIN_DASHBOARD, dashboard_id, &items, None)
        .await
        .map_err(|_| DashboardWriteError::Db)?;
    let mut bound = layout.clone();
    for w in &mut bound.widgets {
        if let Some(id) = outcome.bound.get(&w.id) {
            w.options.control = Some(ControlRef { control_id: *id });
        }
        let wid = w.id.clone();
        if let Some(home) = w.options.home.as_mut() {
            let active: Vec<&'static str> = home.active_roles().iter().map(|r| r.role).collect();
            home.controls
                .retain(|role, _| active.contains(&role.as_str()));
            for role in active {
                let item = pnex_core::home::role_item_id(&wid, role);
                if let Some(id) = outcome.bound.get(&item) {
                    home.controls
                        .insert(role.to_string(), ControlRef { control_id: *id });
                }
            }
        }
    }
    Ok(bound)
}

/// Crée un dashboard **et sa version 1** (une transaction). Le layout
/// peut être absent (canvas par défaut posé par le front au premier
/// save).
pub async fn create_dashboard(
    db: &DatabaseConnection,
    org_id: i64,
    name: &str,
    description: Option<String>,
    layout: Option<&DashboardLayout>,
    author: Option<String>,
) -> Result<(dashboards::Model, i64), DashboardWriteError> {
    let default_layout = DashboardLayout {
        canvas: pnex_core::CanvasSpec {
            width: 1600,
            height: 900,
            background: Some("#f8fafc".into()),
        },
        widgets: vec![],
        wires: vec![],
        ..Default::default()
    };
    let layout = layout.unwrap_or(&default_layout);
    validate_dashboard_write(name, layout)?;
    let txn = db.begin().await.map_err(|_| DashboardWriteError::Db)?;
    let dashboard = dashboards::ActiveModel {
        name: Set(name.trim().to_string()),
        description: Set(description),
        current_version_number: Set(1),
        org_id: Set(org_id),
        ..Default::default()
    }
    .insert(&txn)
    .await
    .map_err(|_| DashboardWriteError::Db)?;
    let layout = bind_surface_controls(&txn, org_id, dashboard.id, layout).await?;
    dashboard_versions::ActiveModel {
        dashboard_id: Set(dashboard.id),
        version_number: Set(1),
        layout: Set(serde_json::to_value(&layout).map_err(|_| DashboardWriteError::Db)?),
        author: Set(author),
        ..Default::default()
    }
    .insert(&txn)
    .await
    .map_err(|_| DashboardWriteError::Db)?;
    txn.commit().await.map_err(|_| DashboardWriteError::Db)?;
    tracing::info!(dashboard_id = %dashboard.id, org_id, "dashboard créé (v1)");
    Ok((dashboard, 1))
}

/// Numéro de la dernière version d'un dashboard (0 si aucune — ne doit
/// pas arriver : la création pose toujours v1).
pub async fn latest_version_number<C: sea_orm::ConnectionTrait>(
    db: &C,
    dashboard_id: uuid::Uuid,
) -> Result<i64, DashboardWriteError> {
    Ok(dashboard_versions::Entity::find()
        .filter(dashboard_versions::Column::DashboardId.eq(dashboard_id))
        .select_only()
        .column_as(dashboard_versions::Column::VersionNumber.max(), "latest")
        .into_tuple::<Option<i64>>()
        .one(db)
        .await
        .map_err(|_| DashboardWriteError::Db)?
        .flatten()
        .unwrap_or(0))
}

/// Enregistre une **nouvelle version** (append-only) avec concurrence
/// optimiste : `expected_version_number` doit viser la version **courante**
/// (le pointeur live — un restore le déplace et doit donc créer un
/// conflit). Le nouveau numéro = **dernier + 1** (l'index unique
/// `(dashboard_id, version_number)` l'exige) puis le pointeur suit
/// (**save = live**, D24) : les viewers la lisent au prochain polling.
pub async fn append_version(
    db: &DatabaseConnection,
    dashboard: &dashboards::Model,
    expected_version_number: i64,
    layout: &DashboardLayout,
    new_name: Option<String>,
    author: Option<String>,
) -> Result<(dashboards::Model, i64), DashboardWriteError> {
    let name_for_validation = new_name.as_deref().unwrap_or(&dashboard.name);
    validate_dashboard_write(name_for_validation, layout)?;
    if expected_version_number != dashboard.current_version_number {
        return Err(DashboardWriteError::Conflict {
            expected: expected_version_number,
            current: dashboard.current_version_number,
        });
    }
    // D123: the format is chosen at creation and never changes (duplicate
    // the dashboard to switch).
    let current_format = dashboard_versions::Entity::find()
        .filter(dashboard_versions::Column::DashboardId.eq(dashboard.id))
        .filter(dashboard_versions::Column::VersionNumber.eq(dashboard.current_version_number))
        .one(db)
        .await
        .map_err(|_| DashboardWriteError::Db)?
        .and_then(|v| v.layout.get("format").cloned())
        .and_then(|f| serde_json::from_value::<pnex_core::DashboardFormat>(f).ok())
        .unwrap_or_default();
    if current_format != layout.format {
        return Err(DashboardWriteError::Violations(vec![
            pnex_core::VizViolation::new(
                None,
                "format_immutable",
                "the dashboard format is chosen at creation and never changes",
            ),
        ]));
    }
    let txn = db.begin().await.map_err(|_| DashboardWriteError::Db)?;
    // The in-memory check above is only a fast path: a concurrent save or
    // restore (another pod) may have moved the pointer since the handler
    // read the row. The conditional UPDATE re-checks the LIVE pointer and
    // takes the row lock (Postgres) until commit, so the latest version
    // read below is the committed one and a restore is never overwritten.
    let now: sea_orm::prelude::DateTimeWithTimeZone = chrono::Utc::now().into();
    let claimed = dashboards::Entity::update_many()
        .col_expr(
            dashboards::Column::UpdatedAt,
            sea_orm::sea_query::Expr::value(now),
        )
        .filter(dashboards::Column::Id.eq(dashboard.id))
        .filter(dashboards::Column::CurrentVersionNumber.eq(expected_version_number))
        .exec(&txn)
        .await
        .map_err(|_| DashboardWriteError::Db)?;
    if claimed.rows_affected == 0 {
        let current = dashboards::Entity::find_by_id(dashboard.id)
            .one(&txn)
            .await
            .map_err(|_| DashboardWriteError::Db)?
            .map(|d| d.current_version_number)
            .unwrap_or(0);
        return Err(DashboardWriteError::Conflict {
            expected: expected_version_number,
            current,
        });
    }
    let latest = latest_version_number(&txn, dashboard.id).await?;
    let layout = bind_surface_controls(&txn, dashboard.org_id, dashboard.id, layout).await?;
    let mut active: dashboards::ActiveModel = dashboard.clone().into();
    if let Some(name) = new_name.as_deref().map(str::trim).filter(|n| !n.is_empty()) {
        active.name = Set(name.to_string());
    }
    let dashboard = active
        .update(&txn)
        .await
        .map_err(|_| DashboardWriteError::Db)?;
    let new_version = dashboard_versions::ActiveModel {
        dashboard_id: Set(dashboard.id),
        version_number: Set(latest + 1),
        layout: Set(serde_json::to_value(&layout).map_err(|_| DashboardWriteError::Db)?),
        author: Set(author),
        ..Default::default()
    }
    .insert(&txn)
    .await
    .map_err(|e| {
        if crate::services::db_lock::is_unique_violation(&e) {
            DashboardWriteError::Conflict {
                expected: expected_version_number,
                current: latest + 1,
            }
        } else {
            DashboardWriteError::Db
        }
    })?;
    let mut live: dashboards::ActiveModel = dashboard.clone().into();
    live.current_version_number = Set(new_version.version_number);
    let dashboard = live
        .update(&txn)
        .await
        .map_err(|_| DashboardWriteError::Db)?;
    txn.commit().await.map_err(|_| DashboardWriteError::Db)?;
    tracing::info!(
        dashboard_id = %dashboard.id,
        version = new_version.version_number,
        "dashboard enregistré (nouvelle version live)"
    );
    Ok((dashboard, new_version.version_number))
}

/// Restore (école media) : **re-positionne** `current_version_number`
/// sur une version existante — n'insère rien. La version visée doit
/// exister (404 sinon).
pub async fn restore_version(
    db: &DatabaseConnection,
    dashboard: &dashboards::Model,
    version_number: i64,
) -> Result<dashboards::Model, DashboardWriteError> {
    let exists = dashboard_versions::Entity::find()
        .filter(dashboard_versions::Column::DashboardId.eq(dashboard.id))
        .filter(dashboard_versions::Column::VersionNumber.eq(version_number))
        .one(db)
        .await
        .map_err(|_| DashboardWriteError::Db)?
        .is_some();
    if !exists {
        return Err(DashboardWriteError::VersionUnknown);
    }
    let mut active: dashboards::ActiveModel = dashboard.clone().into();
    active.current_version_number = Set(version_number);
    let dashboard = active
        .update(db)
        .await
        .map_err(|_| DashboardWriteError::Db)?;
    tracing::info!(
        dashboard_id = %dashboard.id,
        version = version_number,
        "dashboard restauré (pointeur re-positionné)"
    );
    Ok(dashboard)
}
