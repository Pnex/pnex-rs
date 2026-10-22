//! Export of a case (cases-workspace.md K33, schema `pnex.case/1`): its
//! events with their attachments, its undated pieces, the hypotheses board
//! and the pieces manifest, as they are now. Frozen states come from
//! `case_snapshots`, never from replaying the change journal.

use pnex_core::err_codes::FIELD_INVALID;
use pnex_core::ontology::api::{
    CaseAttachInput, CaseEventInput, CaseExport, CaseImported, ObjectInput, ObjectRef, PieceEntry,
};
use pnex_core::ontology::{
    CASE_CONCLUSION_PAGE, CASE_STATUS, EVENT_OCCURRED, EVENT_ORIGIN, KIND_CASE, KIND_EVENT,
};
use sea_orm::{ConnectionTrait, DatabaseConnection, TransactionTrait};
use serde_json::Value;
use uuid::Uuid;

use super::super::{invalid, links, objects, schema, ts, Actor, OntologyError, Result};
use super::{attach_in, board, create_event_in, membership, workspace, T_PERSON};

pub const SCHEMA: &str = "pnex.case/1";
const T_PIECE: &str = "piece";

pub async fn export(db: &DatabaseConnection, org_id: i64, case_id: Uuid) -> Result<CaseExport> {
    let workspace = workspace(db, org_id, case_id).await?;
    let ids = membership(db, org_id, case_id).await?.member_ids();
    let objs = links::refs(db, org_id, ids.iter().copied()).await?;
    let s = schema(db, org_id).await?;
    let text = |p: &serde_json::Map<String, Value>, k: &str| {
        p.get(k).and_then(Value::as_str).map(str::to_string)
    };
    let mut pieces = Vec::new();
    let mut persons = 0;
    for id in &ids {
        let Some(o) = objs.get(id) else { continue };
        if o.type_key == T_PERSON {
            persons += 1;
        }
        if o.type_key == T_PIECE {
            let v = objects::view(o, &s);
            pieces.push(PieceEntry {
                id: v.id,
                title: v.title,
                source_url: text(&v.properties, "source_url"),
                sha256: text(&v.properties, "sha256"),
                captured_at: text(&v.properties, "captured_at"),
            });
        }
    }
    Ok(CaseExport {
        schema: SCHEMA.into(),
        exported_at: ts(&chrono::Utc::now().into()),
        workspace,
        board: board(db, org_id, case_id).await?,
        pieces,
        persons,
    })
}

/// A live object of the org that an imported case may point at again.
async fn live<C: ConnectionTrait>(db: &C, org_id: i64, r: &ObjectRef) -> Result<bool> {
    let Ok(id) = Uuid::parse_str(&r.id) else {
        return Ok(false);
    };
    match objects::find(db, org_id, id).await {
        Ok(o) => Ok(o.valid_to.is_none() && o.type_key != KIND_EVENT && o.type_key != KIND_CASE),
        Err(OntologyError::NotFound) => Ok(false),
        Err(e) => Err(e),
    }
}

/// Re-creates an exported case, open, in one transaction: its frame, its
/// events (new objects) and their attachments to the objects still there.
pub async fn import(
    db: &DatabaseConnection,
    org_id: i64,
    actor: &Actor,
    doc: &CaseExport,
) -> Result<CaseImported> {
    crate::services::ontology::changes::deferred(async {
        if doc.schema != SCHEMA {
            return Err(invalid("schema", FIELD_INVALID));
        }
        let src = &doc.workspace.case;
        let mut props = src.properties.clone();
        props.remove(CASE_CONCLUSION_PAGE);
        props.insert(CASE_STATUS.into(), "open".into());
        let txn = db.begin().await?;
        let case = objects::create(
            &txn,
            org_id,
            actor,
            &ObjectInput {
                type_key: KIND_CASE.into(),
                title: src.title.clone(),
                properties: props,
                expected_version: None,
            },
        )
        .await?;
        let case_id = Uuid::parse_str(&case.id).map_err(|_| OntologyError::NotFound)?;
        let mut missing: Vec<ObjectRef> = Vec::new();
        let mut miss = |r: &ObjectRef| {
            if !missing.iter().any(|m| m.id == r.id) {
                missing.push(r.clone());
            }
        };
        for e in &doc.workspace.events {
            let mut attach = Vec::new();
            for a in &e.attachments {
                if live(&txn, org_id, &a.object).await? {
                    attach.push(CaseAttachInput {
                        event_id: None,
                        object_id: a.object.id.clone(),
                        role: Some(a.role.clone()),
                        detail: a.detail.clone(),
                    });
                } else {
                    miss(&a.object);
                }
            }
            let p = &e.event.properties;
            let input = CaseEventInput {
                title: e.event.title.clone(),
                occurred: p.get(EVENT_OCCURRED).cloned().unwrap_or(Value::Null),
                origin: p
                    .get(EVENT_ORIGIN)
                    .and_then(Value::as_str)
                    .map(str::to_string),
                attach,
            };
            let mut extra = p.clone();
            extra.remove(EVENT_OCCURRED);
            extra.remove(EVENT_ORIGIN);
            create_event_in(&txn, org_id, actor, case_id, &input, extra).await?;
        }
        for u in &doc.workspace.undated {
            if live(&txn, org_id, u).await? {
                let a = CaseAttachInput {
                    object_id: u.id.clone(),
                    ..Default::default()
                };
                attach_in(&txn, org_id, actor, case_id, &a).await?;
            } else {
                miss(u);
            }
        }
        txn.commit().await?;
        crate::services::ontology::changes::flush();
        Ok(CaseImported {
            case: ObjectRef {
                id: case.id,
                type_key: case.type_key,
                title: case.title,
                native_id: case.native_id,
            },
            events: doc.workspace.events.len(),
            missing,
        })
    })
    .await
}
