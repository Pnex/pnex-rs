//! Dashboard tools of the assistant (D144, ai-assistant.md §9.3).
//!
//! Writes go through `services::dashboards` — the service of the UI's HTTP
//! controller (same validations, same control provisioning, same 409). A
//! dashboard save is live, so the model always passes the version it read.
//!
//! Coupled widgets: a widget whose control feeds a `control-source` node of
//! a **deployed** flow defines what that flow receives and what the human
//! believes they operate. While such a flow runs, the assistant may only
//! move or resize the widget; removing, re-binding, retyping, re-specifying
//! or renaming it is refused with `ai-flow-running` and the list of flows.
//! Detection is server-side, from the deployed versions — never from the
//! model's word. The assistant never writes a control value (D143).

use std::collections::{BTreeMap, HashMap};

use pnex_core::{DashboardLayout, Widget};
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder};
use serde_json::{json, Value};
use uuid::Uuid;

use super::tools::{ToolDeps, ToolError, ToolOutcome};
use crate::models::_entities::{dashboard_versions, dashboards};
use crate::services::dashboards::DashboardWriteError;

/// Listing cap.
const DASHBOARDS_CAP: u64 = 50;

/// Deployed flows `(id, name)` listening to each control of the org.
async fn coupled_controls(
    db: &DatabaseConnection,
    org_id: i64,
) -> Result<HashMap<Uuid, Vec<(i64, String)>>, String> {
    let deployed = crate::controllers::flows::deployed_flows_with_versions(db, org_id)
        .await
        .map_err(|_| "reading deployed flows failed".to_string())?;
    let mut out: HashMap<Uuid, Vec<(i64, String)>> = HashMap::new();
    for (flow, version) in deployed {
        let Ok(graph) = serde_json::from_value::<pnex_core::FlowGraph>(version.graph) else {
            continue;
        };
        for id in pnex_core::control_refs_of(&graph) {
            out.entry(id)
                .or_default()
                .push((flow.id, flow.name.clone()));
        }
    }
    Ok(out)
}

/// Deployed flows a widget is coupled to (deduplicated, by flow id).
fn flows_of(widget: &Widget, coupled: &HashMap<Uuid, Vec<(i64, String)>>) -> Vec<(i64, String)> {
    let mut flows: BTreeMap<i64, String> = BTreeMap::new();
    for id in widget.control_ids() {
        for (fid, name) in coupled.get(&id).into_iter().flatten() {
            flows.insert(*fid, name.clone());
        }
    }
    flows.into_iter().collect()
}

/// A widget with its free parts (geometry, display sources) neutralized:
/// two widgets equal under this view differ only by what the assistant may
/// change on a coupled widget.
fn frozen_view(w: &Widget) -> Widget {
    let mut v = w.clone();
    v.x = 0;
    v.y = 0;
    v.w = 0;
    v.h = 0;
    v.source.clear();
    v
}

/// One refused change on a coupled widget.
#[derive(Debug, Clone, PartialEq)]
pub struct CouplingConflict {
    pub widget_id: String,
    /// `removed` | `changed`.
    pub change: &'static str,
    pub flows: Vec<(i64, String)>,
}

/// Changes of `new` against `old` that touch coupled widgets.
pub fn coupling_conflicts(
    old: &DashboardLayout,
    new: &DashboardLayout,
    coupled: &HashMap<Uuid, Vec<(i64, String)>>,
) -> Vec<CouplingConflict> {
    old.widgets
        .iter()
        .filter_map(|ow| {
            let flows = flows_of(ow, coupled);
            if flows.is_empty() {
                return None;
            }
            let change = match new.widgets.iter().find(|nw| nw.id == ow.id) {
                None => "removed",
                Some(nw) if frozen_view(nw) != frozen_view(ow) => "changed",
                Some(_) => return None,
            };
            Some(CouplingConflict {
                widget_id: ow.id.clone(),
                change,
                flows,
            })
        })
        .collect()
}

fn conflicts_error(conflicts: &[CouplingConflict]) -> ToolError {
    let mut flows: BTreeMap<i64, String> = BTreeMap::new();
    for c in conflicts {
        for (id, name) in &c.flows {
            flows.insert(*id, name.clone());
        }
    }
    let widgets: Vec<String> = conflicts
        .iter()
        .map(|c| format!("{} ({})", c.widget_id, c.change))
        .collect();
    let mut err = super::tools::flow_running_error(&flows.into_iter().collect::<Vec<_>>());
    err.message = format!(
        "{} Coupled widgets touched: {}. Moving or resizing them is allowed.",
        err.message,
        widgets.join(", ")
    );
    err
}

/// Widgets the assistant adds always declare a fresh control: a binding
/// the model would set on a new widget is dropped, so it can never attach
/// itself to an existing control (and through it, to a running flow).
fn unbind_new_widgets(layout: &mut DashboardLayout, keep: &[String]) {
    for w in &mut layout.widgets {
        if keep.contains(&w.id) {
            continue;
        }
        w.options.control = None;
        if let Some(home) = w.options.home.as_mut() {
            home.controls.clear();
        }
    }
}

fn write_error(e: DashboardWriteError) -> ToolError {
    let message = match e {
        DashboardWriteError::Violations(v) => format!(
            "invalid layout: {}",
            serde_json::to_string(&v).unwrap_or_default()
        ),
        DashboardWriteError::NameRequired => "dashboard name required".into(),
        DashboardWriteError::NameTooLong => "dashboard name too long (> 255 characters)".into(),
        DashboardWriteError::Conflict { current, .. } => format!(
            "version conflict: the dashboard is now at version {current} — reload it with get_dashboard and redo the change"
        ),
        DashboardWriteError::VersionUnknown => "unknown version".into(),
        DashboardWriteError::Db => "database error".into(),
    };
    message.into()
}

fn require_write(deps: &ToolDeps<'_>) -> Result<(), ToolError> {
    if deps.can_write {
        Ok(())
    } else {
        Err(
            "owner, admin or member role required — the assistant cannot change dashboards for you"
                .to_string()
                .into(),
        )
    }
}

async fn find(deps: &ToolDeps<'_>, id: Uuid) -> Result<dashboards::Model, ToolError> {
    dashboards::Entity::find_by_id(id)
        .filter(dashboards::Column::OrgId.eq(deps.org_id))
        .one(deps.db)
        .await
        .map_err(|e| format!("reading dashboard: {e}"))?
        .ok_or_else(|| format!("dashboard {id} not found in this organization").into())
}

async fn current_layout(
    db: &DatabaseConnection,
    d: &dashboards::Model,
) -> Result<DashboardLayout, ToolError> {
    dashboard_versions::Entity::find()
        .filter(dashboard_versions::Column::DashboardId.eq(d.id))
        .filter(dashboard_versions::Column::VersionNumber.eq(d.current_version_number))
        .one(db)
        .await
        .map_err(|e| format!("reading layout: {e}"))?
        .and_then(|v| serde_json::from_value(v.layout).ok())
        .ok_or_else(|| "current layout unreadable".to_string().into())
}

fn uuid_arg(args: &Value, key: &str) -> Result<Uuid, ToolError> {
    args.get(key)
        .and_then(Value::as_str)
        .and_then(|s| Uuid::parse_str(s).ok())
        .ok_or_else(|| format!("argument '{key}' missing or not a UUID").into())
}

fn layout_arg(args: &Value) -> Result<DashboardLayout, ToolError> {
    let v = args
        .get("layout")
        .ok_or_else(|| "argument 'layout' missing".to_string())?;
    serde_json::from_value(v.clone()).map_err(|e| format!("unreadable layout: {e}").into())
}

pub async fn list_dashboards(deps: &ToolDeps<'_>) -> Result<ToolOutcome, ToolError> {
    use sea_orm::QuerySelect;
    let rows = dashboards::Entity::find()
        .filter(dashboards::Column::OrgId.eq(deps.org_id))
        .order_by_asc(dashboards::Column::Name)
        .limit(DASHBOARDS_CAP)
        .all(deps.db)
        .await
        .map_err(|e| format!("reading dashboards: {e}"))?;
    let list: Vec<Value> = rows
        .iter()
        .map(|d| json!({ "id": d.id, "name": d.name, "version": d.current_version_number }))
        .collect();
    Ok(ToolOutcome {
        value: json!({ "dashboards": list }),
        flow_id: None,
    })
}

pub async fn get_dashboard(deps: &ToolDeps<'_>, args: &Value) -> Result<ToolOutcome, ToolError> {
    let d = find(deps, uuid_arg(args, "dashboard_id")?).await?;
    let layout = current_layout(deps.db, &d).await?;
    let coupled = coupled_controls(deps.db, deps.org_id).await?;
    let coupled_widgets: serde_json::Map<String, Value> = layout
        .widgets
        .iter()
        .filter_map(|w| {
            let flows = flows_of(w, &coupled);
            (!flows.is_empty()).then(|| {
                let list: Vec<Value> = flows
                    .iter()
                    .map(|(id, name)| json!({ "id": id, "name": name }))
                    .collect();
                (w.id.clone(), Value::Array(list))
            })
        })
        .collect();
    Ok(ToolOutcome {
        value: json!({
            "dashboard": { "id": d.id, "name": d.name, "version": d.current_version_number },
            "layout": layout,
            "coupled_flows": coupled_widgets,
        }),
        flow_id: None,
    })
}

pub async fn validate_dashboard_layout(
    deps: &ToolDeps<'_>,
    args: &Value,
) -> Result<ToolOutcome, ToolError> {
    let layout = layout_arg(args)?;
    let violations = pnex_core::validate_layout(&layout);
    let mut conflicts: Vec<Value> = Vec::new();
    if args.get("dashboard_id").is_some_and(|v| !v.is_null()) {
        let d = find(deps, uuid_arg(args, "dashboard_id")?).await?;
        let old = current_layout(deps.db, &d).await?;
        let coupled = coupled_controls(deps.db, deps.org_id).await?;
        conflicts = coupling_conflicts(&old, &layout, &coupled)
            .into_iter()
            .map(|c| {
                json!({
                    "widget_id": c.widget_id,
                    "change": c.change,
                    "flows": c.flows.iter().map(|(id, n)| json!({"id": id, "name": n})).collect::<Vec<_>>(),
                })
            })
            .collect();
    }
    Ok(ToolOutcome {
        value: json!({
            "valid": violations.is_empty() && conflicts.is_empty(),
            "violations": violations,
            "coupled_conflicts": conflicts,
        }),
        flow_id: None,
    })
}

pub async fn create_dashboard(deps: &ToolDeps<'_>, args: &Value) -> Result<ToolOutcome, ToolError> {
    require_write(deps)?;
    let name = args
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| "argument 'name' missing".to_string())?;
    let mut layout = match args.get("layout").filter(|v| !v.is_null()) {
        Some(_) => Some(layout_arg(args)?),
        None => None,
    };
    if let Some(l) = layout.as_mut() {
        unbind_new_widgets(l, &[]);
    }
    let (d, version) = crate::services::dashboards::create_dashboard(
        deps.db,
        deps.org_id,
        name,
        args.get("description")
            .and_then(Value::as_str)
            .map(str::to_string),
        layout.as_ref(),
        deps.author.clone(),
    )
    .await
    .map_err(write_error)?;
    Ok(ToolOutcome {
        value: json!({ "dashboard_id": d.id, "name": d.name, "version": version }),
        flow_id: None,
    })
}

pub async fn update_dashboard(deps: &ToolDeps<'_>, args: &Value) -> Result<ToolOutcome, ToolError> {
    require_write(deps)?;
    let d = find(deps, uuid_arg(args, "dashboard_id")?).await?;
    let expected = args
        .get("expected_version")
        .and_then(Value::as_i64)
        .ok_or_else(|| "argument 'expected_version' missing".to_string())?;
    let mut layout = layout_arg(args)?;
    let old = current_layout(deps.db, &d).await?;
    let coupled = coupled_controls(deps.db, deps.org_id).await?;
    let conflicts = coupling_conflicts(&old, &layout, &coupled);
    if !conflicts.is_empty() {
        return Err(conflicts_error(&conflicts));
    }
    let existing: Vec<String> = old.widgets.iter().map(|w| w.id.clone()).collect();
    unbind_new_widgets(&mut layout, &existing);
    let name = args.get("name").and_then(Value::as_str).map(str::to_string);
    let (d, version) = crate::services::dashboards::append_version(
        deps.db,
        &d,
        expected,
        &layout,
        name,
        deps.author.clone(),
    )
    .await
    .map_err(write_error)?;
    let added: Vec<&str> = layout
        .widgets
        .iter()
        .filter(|w| !existing.contains(&w.id))
        .map(|w| w.id.as_str())
        .collect();
    let removed: Vec<&str> = old
        .widgets
        .iter()
        .filter(|w| !layout.widgets.iter().any(|n| n.id == w.id))
        .map(|w| w.id.as_str())
        .collect();
    let changed: Vec<&str> = old
        .widgets
        .iter()
        .filter(|w| layout.widgets.iter().any(|n| n.id == w.id && n != *w))
        .map(|w| w.id.as_str())
        .collect();
    Ok(ToolOutcome {
        value: json!({
            "dashboard_id": d.id,
            "version": version,
            "live": true,
            "widgets_added": added,
            "widgets_removed": removed,
            "widgets_changed": changed,
        }),
        flow_id: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn widget(id: &str, control: Option<Uuid>) -> Widget {
        let mut w: Widget = serde_json::from_value(json!({
            "id": id, "type": "switch", "title": "Pump", "x": 0, "y": 0, "w": 2, "h": 2
        }))
        .expect("widget");
        w.options.control =
            control.map(|control_id| pnex_core::ui_control::ControlRef { control_id });
        w
    }

    fn layout(widgets: Vec<Widget>) -> DashboardLayout {
        DashboardLayout {
            widgets,
            ..Default::default()
        }
    }

    #[test]
    fn coupled_widgets_may_only_move() {
        let c = Uuid::new_v4();
        let coupled = HashMap::from([(c, vec![(7, "pump loop".to_string())])]);
        let old = layout(vec![widget("w1", Some(c)), widget("w2", None)]);

        // Move + resize the coupled one, rename the free one: allowed.
        let mut moved = old.clone();
        moved.widgets[0].x = 10;
        moved.widgets[0].w = 4;
        moved.widgets[1].title = "Fan".into();
        assert!(coupling_conflicts(&old, &moved, &coupled).is_empty());

        // Rename the coupled one: refused with its flow.
        let mut renamed = old.clone();
        renamed.widgets[0].title = "Valve".into();
        let c1 = coupling_conflicts(&old, &renamed, &coupled);
        assert_eq!(c1.len(), 1);
        assert_eq!(c1[0].change, "changed");
        assert_eq!(c1[0].flows, vec![(7, "pump loop".to_string())]);

        // Re-bind or remove: refused.
        let mut rebound = old.clone();
        rebound.widgets[0].options.control = Some(pnex_core::ui_control::ControlRef {
            control_id: Uuid::new_v4(),
        });
        assert_eq!(coupling_conflicts(&old, &rebound, &coupled).len(), 1);
        let removed = layout(vec![widget("w2", None)]);
        assert_eq!(
            coupling_conflicts(&old, &removed, &coupled)[0].change,
            "removed"
        );
    }

    #[test]
    fn new_widgets_never_keep_a_binding() {
        let c = Uuid::new_v4();
        let mut l = layout(vec![widget("w1", Some(c)), widget("w9", Some(c))]);
        unbind_new_widgets(&mut l, &["w1".to_string()]);
        assert!(l.widgets[0].options.control.is_some());
        assert!(l.widgets[1].options.control.is_none());
    }
}
