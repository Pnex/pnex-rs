//! Hypotheses board of a case (cases.md K8): each hypothesis member with the
//! claims for and against it, then the member claims that need attention
//! (no source, contradicted). No score: the human concludes.

use std::collections::HashMap;

use pnex_core::ontology::api::{BoardClaim, BoardHypothesis, CaseBoard, ObjectRef, ObjectView};
use sea_orm::DatabaseConnection;
use serde_json::Value;
use uuid::Uuid;

use super::super::{links, objects, schema, OntologyError, OrgSchema, Result};
use super::membership;

const T_HYPOTHESIS: &str = "hypothesis";
const T_CLAIM: &str = "claim";
const L_SUPPORTS: &str = "supports";
const L_CONTRADICTS: &str = "contradicts";
const L_SOURCED_BY: &str = "sourced_by";

fn status_of(o: &ObjectView) -> Option<String> {
    o.properties
        .get("status")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// A claim with its status and open sources, read once per board.
async fn claim_row(
    db: &DatabaseConnection,
    org_id: i64,
    s: &OrgSchema,
    claim: &ObjectRef,
    seen: &mut HashMap<String, BoardClaim>,
) -> Result<BoardClaim> {
    if let Some(c) = seen.get(&claim.id) {
        return Ok(c.clone());
    }
    let id = Uuid::parse_str(&claim.id).map_err(|_| OntologyError::NotFound)?;
    let o = objects::view(&objects::find(db, org_id, id).await?, s);
    let sources = links::for_object(db, org_id, id, None, false)
        .await?
        .iter()
        .filter(|l| l.link_type == L_SOURCED_BY && l.source.id == claim.id)
        .count();
    let row = BoardClaim {
        claim: claim.clone(),
        status: status_of(&o),
        sources,
        justification: objects::claim_note(&o.properties),
    };
    seen.insert(claim.id.clone(), row.clone());
    Ok(row)
}

/// The hypotheses board of a case, now.
pub async fn board(db: &DatabaseConnection, org_id: i64, case_id: Uuid) -> Result<CaseBoard> {
    let ids = membership(db, org_id, case_id).await?.member_ids();
    let objs = links::refs(db, org_id, ids.iter().copied()).await?;
    let s = schema(db, org_id).await?;
    let mut seen = HashMap::new();
    let mut board = CaseBoard::default();
    for id in ids {
        let Some(o) = objs.get(&id) else { continue };
        let r = links::object_ref(o);
        if o.type_key == T_HYPOTHESIS {
            let mut h = BoardHypothesis {
                hypothesis: r.clone(),
                status: status_of(&objects::view(o, &s)),
                supports: Vec::new(),
                contradicts: Vec::new(),
            };
            for l in links::for_object(db, org_id, id, None, false).await? {
                if l.target.id != r.id || l.source.type_key != T_CLAIM {
                    continue;
                }
                let side = match l.link_type.as_str() {
                    L_SUPPORTS => &mut h.supports,
                    L_CONTRADICTS => &mut h.contradicts,
                    _ => continue,
                };
                side.push(claim_row(db, org_id, &s, &l.source, &mut seen).await?);
            }
            board.hypotheses.push(h);
        } else if o.type_key == T_CLAIM {
            let row = claim_row(db, org_id, &s, &r, &mut seen).await?;
            if row.sources == 0 {
                board.unsourced.push(row.clone());
            }
            let contradicted = links::for_object(db, org_id, id, None, false)
                .await?
                .iter()
                .any(|l| l.link_type == L_CONTRADICTS && l.target.id == r.id);
            if contradicted {
                board.contradicted.push(row);
            }
        }
    }
    Ok(board)
}
