//! Saved arrangement of the case graph (cases-workspace.md K31): when the
//! team moves nodes for a clearer picture, the positions are kept for
//! everyone who opens the case; "Re-layout" clears them and the automatic
//! layout takes over again. Layout coordinates, never screen pixels.

use pnex_core::err_codes::FIELD_INVALID;
use pnex_core::ontology::api::CaseLayout;
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement};
use uuid::Uuid;

use super::super::{invalid, Actor, Result};
use super::case_of;

/// Positions kept at most (the graph draws 300 nodes, expansions included).
const MAX_NODES: usize = 1_000;
/// Far beyond any layout: a position past it is a client bug, not a layout.
const MAX_COORD: f64 = 1.0e6;

pub async fn layout(db: &DatabaseConnection, org_id: i64, case_id: Uuid) -> Result<CaseLayout> {
    case_of(db, org_id, case_id).await?;
    let row = db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT positions FROM case_layouts WHERE org_id = $1 AND case_id = $2",
            [org_id.into(), case_id.into()],
        ))
        .await?;
    let positions = row
        .and_then(|r| r.try_get::<serde_json::Value>("", "positions").ok())
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default();
    Ok(CaseLayout { positions })
}

/// Replaces the saved positions; none = back to the automatic layout.
pub async fn save_layout(
    db: &DatabaseConnection,
    org_id: i64,
    actor: &Actor,
    case_id: Uuid,
    input: &CaseLayout,
) -> Result<CaseLayout> {
    case_of(db, org_id, case_id).await?;
    if input.positions.len() > MAX_NODES {
        return Err(invalid("positions", format!("max_items:{MAX_NODES}")));
    }
    let sane = |v: f64| v.is_finite() && v.abs() <= MAX_COORD;
    if input
        .positions
        .iter()
        .any(|(k, (x, y))| k.len() > 64 || !sane(*x) || !sane(*y))
    {
        return Err(invalid("positions", FIELD_INVALID));
    }
    if input.positions.is_empty() {
        db.execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "DELETE FROM case_layouts WHERE org_id = $1 AND case_id = $2",
            [org_id.into(), case_id.into()],
        ))
        .await?;
        return Ok(CaseLayout::default());
    }
    let json = serde_json::to_value(&input.positions).unwrap_or_default();
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "INSERT INTO case_layouts (case_id, org_id, positions, updated_by) VALUES ($1, $2, $3, $4) \
         ON CONFLICT (case_id) DO UPDATE SET positions = $3, updated_by = $4, updated_at = now()",
        [case_id.into(), org_id.into(), json.into(), actor.user_id.into()],
    ))
    .await?;
    Ok(input.clone())
}
