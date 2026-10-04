//! Surface-declared controls (D131, `docs/architecture/surfaces-controls.md`).
//!
//! A surface declares, the engine registers: a dashboard control widget or
//! an annotation `control` item without a control gets its **own** control
//! when the surface is saved, inside the save transaction. The control
//! records its declaring item in `origin` (`{kind}:{surface}:{item}`), so
//! the flow node catalog lists it under its surface, and the next save of
//! the same item finds it again (stable id, stable key).
//!
//! Linking an item to an existing control (shared state across surfaces,
//! D125) stays possible: the item then carries that control id and nothing
//! is provisioned for it.
//!
//! Removing a declaring item releases its control: deleted when nothing
//! else references it (flows, other surfaces), otherwise kept as a
//! standalone control (`origin` cleared) so no flow loses its trigger.

use std::collections::{HashMap, HashSet};

use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter, QuerySelect, Set,
};
use uuid::Uuid;

use pnex_core::ui_control::{
    auto_control_key, check_control_label, control_origin, control_origin_prefix,
    parse_control_origin, ControlKind, ControlOrigin, ControlSpec, CONTROL_LABEL_MAX_LEN,
    ORIGIN_ANNOTATION, ORIGIN_DASHBOARD,
};

use crate::models::_entities::{
    annotation_layer_versions, annotation_layers, controls, dashboard_versions, dashboards,
    flow_versions, flows,
};

/// One surface item that drives a control.
#[derive(Debug, Clone)]
pub struct DeclaredControl {
    /// Widget id or annotation item id.
    pub item_id: String,
    /// Kind of the control to provision (widget type / item kind); `None`
    /// = the item only links an existing control, never provisions.
    pub kind: Option<ControlKind>,
    /// Display label of the item (widget title, annotation label); empty =
    /// the item id is used.
    pub label: String,
    /// Control the item is bound to, if any.
    pub current: Option<Uuid>,
}

/// Result of a sync: the control bound to each item, and the controls
/// deleted (their Valkey value is forgotten by the caller after commit).
#[derive(Debug, Default)]
pub struct SyncOutcome {
    pub bound: HashMap<String, Uuid>,
    pub deleted: Vec<Uuid>,
}

/// Why a sync failed.
#[derive(Debug)]
pub enum SyncError {
    /// An item links a control absent from the org and declares no kind to
    /// provision its own.
    UnknownControl(Uuid),
    Db,
}

impl From<sea_orm::DbErr> for SyncError {
    fn from(e: sea_orm::DbErr) -> Self {
        tracing::warn!(error = %e, "surface controls: database error");
        SyncError::Db
    }
}

/// Label of a provisioned control: the item label, else the item id, cut
/// to the column rule.
fn label_for(item: &DeclaredControl) -> String {
    let trimmed = item.label.trim();
    let base = if trimmed.is_empty() || check_control_label(trimmed).is_some() {
        item.item_id.as_str()
    } else {
        trimmed
    };
    let clean: String = base.chars().filter(|c| !c.is_control()).collect();
    let cut: String = clean.chars().take(CONTROL_LABEL_MAX_LEN).collect();
    if cut.trim().is_empty() {
        "control".to_string()
    } else {
        cut
    }
}

/// First free key of the org starting from `base` (`base`, `base-2`, …).
async fn free_key<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    base: &str,
    reserved: &HashSet<String>,
) -> Result<String, SyncError> {
    let mut stem = base.to_string();
    stem.truncate(58);
    let taken: HashSet<String> = controls::Entity::find()
        .filter(controls::Column::OrgId.eq(org_id))
        .filter(controls::Column::Key.starts_with(&stem))
        .select_only()
        .column(controls::Column::Key)
        .into_tuple::<String>()
        .all(db)
        .await?
        .into_iter()
        .collect();
    if !taken.contains(base) && !reserved.contains(base) {
        return Ok(base.to_string());
    }
    for n in 2.. {
        let candidate = format!("{stem}-{n}");
        if !taken.contains(&candidate) && !reserved.contains(&candidate) {
            return Ok(candidate);
        }
    }
    unreachable!("an unbounded counter always finds a free key")
}

/// Reconciles the controls declared by one surface with its items (see the
/// module doc). Runs on the caller's connection — the save transaction.
pub async fn sync_surface<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    surface_kind: &str,
    surface_id: Uuid,
    items: &[DeclaredControl],
    author: Option<i64>,
) -> Result<SyncOutcome, SyncError> {
    let prefix = control_origin_prefix(surface_kind, surface_id);
    let owned: Vec<controls::Model> = controls::Entity::find()
        .filter(controls::Column::OrgId.eq(org_id))
        .filter(controls::Column::Origin.starts_with(&prefix))
        .all(db)
        .await?;
    let owned_by_origin: HashMap<String, controls::Model> = owned
        .iter()
        .filter_map(|m| m.origin.clone().map(|o| (o, m.clone())))
        .collect();

    // Ids bound by the items: only controls of this org count (an id from
    // elsewhere, or deleted meanwhile, is replaced by the item's own one).
    let wanted: Vec<Uuid> = items
        .iter()
        .filter_map(|i| i.current)
        .filter(|id| !id.is_nil())
        .collect();
    let known: HashSet<Uuid> = if wanted.is_empty() {
        HashSet::new()
    } else {
        controls::Entity::find()
            .filter(controls::Column::OrgId.eq(org_id))
            .filter(controls::Column::Id.is_in(wanted))
            .select_only()
            .column(controls::Column::Id)
            .into_tuple::<Uuid>()
            .all(db)
            .await?
            .into_iter()
            .collect()
    };

    let mut out = SyncOutcome::default();
    let mut reserved_keys: HashSet<String> = HashSet::new();
    for item in items {
        let origin = control_origin(surface_kind, surface_id, &item.item_id);
        let own = owned_by_origin.get(&origin);
        let linked = item.current.filter(|id| known.contains(id));
        let bound = linked.or(own.map(|m| m.id));
        if let Some(id) = bound {
            // Own control: its label follows the item label.
            if let Some(m) = own.filter(|m| m.id == id) {
                let label = label_for(item);
                if !item.label.trim().is_empty() && m.label != label {
                    let mut active: controls::ActiveModel = m.clone().into();
                    active.label = Set(label);
                    active.update(db).await?;
                }
            }
            out.bound.insert(item.item_id.clone(), id);
            continue;
        }
        let Some(kind) = item.kind else {
            return Err(SyncError::UnknownControl(item.current.unwrap_or_default()));
        };
        let key = free_key(
            db,
            org_id,
            &auto_control_key(surface_kind, surface_id, &item.item_id),
            &reserved_keys,
        )
        .await?;
        reserved_keys.insert(key.clone());
        let spec = ControlSpec::new(kind);
        let created = controls::ActiveModel {
            id: Set(Uuid::new_v4()),
            org_id: Set(org_id),
            key: Set(key),
            label: Set(label_for(item)),
            kind: Set(kind.as_str().to_string()),
            spec: Set(serde_json::to_value(&spec).map_err(|_| SyncError::Db)?),
            created_by: Set(author),
            origin: Set(Some(origin)),
            ..Default::default()
        }
        .insert(db)
        .await?;
        tracing::info!(
            control_id = %created.id,
            org_id,
            surface = %prefix,
            "surface control provisioned"
        );
        out.bound.insert(item.item_id.clone(), created.id);
    }

    // Own controls no item binds anymore: released.
    let bound: HashSet<Uuid> = out.bound.values().copied().collect();
    let orphans: Vec<controls::Model> = owned
        .into_iter()
        .filter(|m| !bound.contains(&m.id))
        .collect();
    if orphans.is_empty() {
        return Ok(out);
    }
    let elsewhere = refs_outside(db, org_id, surface_kind, surface_id).await?;
    for m in orphans {
        let id = m.id;
        if elsewhere.contains(&id) {
            let mut active: controls::ActiveModel = m.into();
            active.origin = Set(None);
            active.update(db).await?;
        } else {
            controls::Entity::delete_by_id(id).exec(db).await?;
            out.deleted.push(id);
        }
    }
    Ok(out)
}

/// Releases every control declared by a surface being deleted.
pub async fn release_surface<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    surface_kind: &str,
    surface_id: Uuid,
) -> Result<Vec<Uuid>, SyncError> {
    Ok(
        sync_surface(db, org_id, surface_kind, surface_id, &[], None)
            .await?
            .deleted,
    )
}

/// Forgets the stored values of deleted controls (best effort, after
/// commit).
pub async fn forget_deleted(ctx: &loco_rs::app::AppContext, org_id: i64, deleted: &[Uuid]) {
    if deleted.is_empty() {
        return;
    }
    let conn = crate::services::shared_valkey::conn(&ctx.config).await;
    for id in deleted {
        crate::services::controls::forget_value(conn.clone(), org_id, *id).await;
    }
}

/// Control ids referenced outside the given surface: latest and deployed
/// versions of every flow, current versions of the other dashboards,
/// latest and published versions of the other annotation layers.
async fn refs_outside<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    surface_kind: &str,
    surface_id: Uuid,
) -> Result<HashSet<Uuid>, SyncError> {
    let mut refs = HashSet::new();

    // Flows: latest saved version + deployed version.
    let org_flows: Vec<(i64, Option<i64>)> = flows::Entity::find()
        .filter(flows::Column::OrgId.eq(org_id))
        .select_only()
        .column(flows::Column::Id)
        .column(flows::Column::DeployedVersionId)
        .into_tuple()
        .all(db)
        .await?;
    if !org_flows.is_empty() {
        let ids: Vec<i64> = org_flows.iter().map(|f| f.0).collect();
        let latest: Vec<(i64, Option<i64>)> = flow_versions::Entity::find()
            .filter(flow_versions::Column::FlowId.is_in(ids))
            .select_only()
            .column(flow_versions::Column::FlowId)
            .column_as(flow_versions::Column::VersionNumber.max(), "latest")
            .group_by(flow_versions::Column::FlowId)
            .into_tuple()
            .all(db)
            .await?;
        let mut cond = sea_orm::Condition::any();
        for (flow_id, n) in &latest {
            if let Some(n) = n {
                cond = cond.add(
                    sea_orm::Condition::all()
                        .add(flow_versions::Column::FlowId.eq(*flow_id))
                        .add(flow_versions::Column::VersionNumber.eq(*n)),
                );
            }
        }
        let deployed: Vec<i64> = org_flows.iter().filter_map(|f| f.1).collect();
        if !deployed.is_empty() {
            cond = cond.add(flow_versions::Column::Id.is_in(deployed));
        }
        let graphs: Vec<serde_json::Value> = flow_versions::Entity::find()
            .filter(cond)
            .select_only()
            .column(flow_versions::Column::Graph)
            .into_tuple()
            .all(db)
            .await?;
        for g in graphs {
            if let Ok(graph) = serde_json::from_value::<pnex_core::FlowGraph>(g) {
                refs.extend(pnex_core::control_refs_of(&graph));
            }
        }
    }

    // Dashboards: current version of each other dashboard.
    let mut boards = dashboards::Entity::find().filter(dashboards::Column::OrgId.eq(org_id));
    if surface_kind == ORIGIN_DASHBOARD {
        boards = boards.filter(dashboards::Column::Id.ne(surface_id));
    }
    let boards: Vec<(Uuid, i64)> = boards
        .select_only()
        .column(dashboards::Column::Id)
        .column(dashboards::Column::CurrentVersionNumber)
        .into_tuple()
        .all(db)
        .await?;
    if !boards.is_empty() {
        let mut cond = sea_orm::Condition::any();
        for (id, n) in &boards {
            cond = cond.add(
                sea_orm::Condition::all()
                    .add(dashboard_versions::Column::DashboardId.eq(*id))
                    .add(dashboard_versions::Column::VersionNumber.eq(*n)),
            );
        }
        let layouts: Vec<serde_json::Value> = dashboard_versions::Entity::find()
            .filter(cond)
            .select_only()
            .column(dashboard_versions::Column::Layout)
            .into_tuple()
            .all(db)
            .await?;
        for l in layouts {
            if let Ok(layout) = serde_json::from_value::<pnex_core::DashboardLayout>(l) {
                refs.extend(
                    layout
                        .widgets
                        .iter()
                        .filter_map(|w| w.options.control.as_ref().map(|c| c.control_id)),
                );
            }
        }
    }

    // Annotation layers: latest and published version of each other layer.
    let mut layers =
        annotation_layers::Entity::find().filter(annotation_layers::Column::OrgId.eq(org_id));
    if surface_kind == ORIGIN_ANNOTATION {
        layers = layers.filter(annotation_layers::Column::Id.ne(surface_id));
    }
    let layers: Vec<(Uuid, Option<Uuid>)> = layers
        .select_only()
        .column(annotation_layers::Column::Id)
        .column(annotation_layers::Column::PublishedVersionId)
        .into_tuple()
        .all(db)
        .await?;
    if !layers.is_empty() {
        let ids: Vec<Uuid> = layers.iter().map(|l| l.0).collect();
        let latest: Vec<(Uuid, Option<i64>)> = annotation_layer_versions::Entity::find()
            .filter(annotation_layer_versions::Column::LayerId.is_in(ids))
            .select_only()
            .column(annotation_layer_versions::Column::LayerId)
            .column_as(
                annotation_layer_versions::Column::VersionNumber.max(),
                "latest",
            )
            .group_by(annotation_layer_versions::Column::LayerId)
            .into_tuple()
            .all(db)
            .await?;
        let mut cond = sea_orm::Condition::any();
        for (layer_id, n) in &latest {
            if let Some(n) = n {
                cond = cond.add(
                    sea_orm::Condition::all()
                        .add(annotation_layer_versions::Column::LayerId.eq(*layer_id))
                        .add(annotation_layer_versions::Column::VersionNumber.eq(*n)),
                );
            }
        }
        let published: Vec<Uuid> = layers.iter().filter_map(|l| l.1).collect();
        if !published.is_empty() {
            cond = cond.add(annotation_layer_versions::Column::Id.is_in(published));
        }
        let docs: Vec<serde_json::Value> = annotation_layer_versions::Entity::find()
            .filter(cond)
            .select_only()
            .column(annotation_layer_versions::Column::Doc)
            .into_tuple()
            .all(db)
            .await?;
        for d in docs {
            if let Ok(doc) = serde_json::from_value::<pnex_core::AnnotationDoc>(d) {
                refs.extend(doc.items.iter().filter_map(|it| match &it.target {
                    pnex_core::AnnotationTarget::Control { control_id, .. }
                        if !control_id.is_nil() =>
                    {
                        Some(*control_id)
                    }
                    _ => None,
                }));
            }
        }
    }
    Ok(refs)
}

/// Resolves the origin of each listed control for display (one query per
/// surface kind). Surfaces deleted meanwhile keep `surface_name: None`.
pub async fn resolve_origins<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    rows: &[controls::Model],
) -> Result<HashMap<Uuid, ControlOrigin>, SyncError> {
    let mut parsed: Vec<(Uuid, String, Uuid, String)> = Vec::new();
    for m in rows {
        if let Some((kind, surface, item)) = m.origin.as_deref().and_then(parse_control_origin) {
            parsed.push((m.id, kind.to_string(), surface, item.to_string()));
        }
    }
    if parsed.is_empty() {
        return Ok(HashMap::new());
    }
    let ids_of =
        |kind: &str| -> Vec<Uuid> { parsed.iter().filter(|p| p.1 == kind).map(|p| p.2).collect() };
    let mut names: HashMap<Uuid, String> = HashMap::new();
    let dash_ids = ids_of(ORIGIN_DASHBOARD);
    if !dash_ids.is_empty() {
        let found: Vec<(Uuid, String)> = dashboards::Entity::find()
            .filter(dashboards::Column::OrgId.eq(org_id))
            .filter(dashboards::Column::Id.is_in(dash_ids))
            .select_only()
            .column(dashboards::Column::Id)
            .column(dashboards::Column::Name)
            .into_tuple()
            .all(db)
            .await?;
        names.extend(found);
    }
    let layer_ids = ids_of(ORIGIN_ANNOTATION);
    if !layer_ids.is_empty() {
        let found: Vec<(Uuid, String)> = annotation_layers::Entity::find()
            .filter(annotation_layers::Column::OrgId.eq(org_id))
            .filter(annotation_layers::Column::Id.is_in(layer_ids))
            .select_only()
            .column(annotation_layers::Column::Id)
            .column(annotation_layers::Column::Name)
            .into_tuple()
            .all(db)
            .await?;
        names.extend(found);
    }
    Ok(parsed
        .into_iter()
        .map(|(control_id, surface, surface_id, item_id)| {
            let surface_name = names.get(&surface_id).cloned();
            (
                control_id,
                ControlOrigin {
                    surface,
                    surface_id,
                    surface_name,
                    item_id,
                },
            )
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(label: &str) -> DeclaredControl {
        DeclaredControl {
            item_id: "w-0001".into(),
            kind: Some(ControlKind::Switch),
            label: label.into(),
            current: None,
        }
    }

    #[test]
    fn label_falls_back_to_the_item_id() {
        assert_eq!(label_for(&item("Pump")), "Pump");
        assert_eq!(label_for(&item("  ")), "w-0001");
        assert_eq!(label_for(&item(&"x".repeat(500))), "w-0001");
    }
}
