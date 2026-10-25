//! Evidence of an object (external-apps.md D437): the media reached from
//! it through link types declared `evidence`, two hops at most (a finding
//! → the page it was seen in → the parts of the page's proof), with their
//! current file and, for a web proof part, the proof's seal.

use std::collections::HashMap;

use pnex_core::ontology::api::{EvidenceItem, ProofSeal};
use pnex_core::resources::KIND_MEDIA_ASSET;
use sea_orm::{DatabaseConnection, FromQueryResult, Statement};
use uuid::Uuid;

use super::{links::object_ref, schema, Result};
use crate::models::_entities::objects;

/// Hops followed from the object.
const DEPTH: usize = 2;

fn stmt(sql: &str, values: Vec<sea_orm::Value>) -> Statement {
    Statement::from_sql_and_values(sea_orm::DatabaseBackend::Postgres, sql, values)
}

#[derive(FromQueryResult)]
struct Edge {
    source: Uuid,
    target: Uuid,
    relation: String,
}

#[derive(FromQueryResult)]
struct Media {
    asset_id: Uuid,
    kind: String,
    metadata: Option<serde_json::Value>,
    version_number: Option<i64>,
    sha256: Option<String>,
    size_bytes: Option<i64>,
    content_type: Option<String>,
    proof_id: Option<Uuid>,
    seq: Option<i64>,
    captured_at: Option<chrono::DateTime<chrono::FixedOffset>>,
    manifest_sha256: Option<String>,
    prev_sha256: Option<String>,
}

pub async fn of(db: &DatabaseConnection, org_id: i64, id: Uuid) -> Result<Vec<EvidenceItem>> {
    super::objects::find(db, org_id, id).await?;
    let s = schema(db, org_id).await?;
    let evidence: Vec<String> = s
        .links
        .iter()
        .filter(|(l, _, _)| l.evidence)
        .map(|(l, _, _)| l.key.clone())
        .collect();
    if evidence.is_empty() {
        return Ok(Vec::new());
    }
    // Breadth first, the path of each reached object kept.
    let mut paths: HashMap<Uuid, Vec<String>> = HashMap::from([(id, Vec::new())]);
    let mut frontier = vec![id];
    let mut reached: Vec<Uuid> = Vec::new();
    for _ in 0..DEPTH {
        if frontier.is_empty() {
            break;
        }
        let edges = Edge::find_by_statement(stmt(
            "SELECT source_object_id AS source, target_object_id AS target, relation \
             FROM resource_edges WHERE org_id = $1 AND source_object_id = ANY($2) \
               AND relation = ANY($3) AND valid_to IS NULL AND target_object_id IS NOT NULL \
             ORDER BY id",
            vec![
                org_id.into(),
                frontier.clone().into(),
                evidence.clone().into(),
            ],
        ))
        .all(db)
        .await?;
        let mut next = Vec::new();
        for e in edges {
            if paths.contains_key(&e.target) {
                continue;
            }
            let mut path = paths.get(&e.source).cloned().unwrap_or_default();
            path.push(e.relation);
            paths.insert(e.target, path);
            next.push(e.target);
            reached.push(e.target);
        }
        frontier = next;
    }
    let found = super::links::refs(db, org_id, reached.iter().copied()).await?;
    let media: Vec<&objects::Model> = reached
        .iter()
        .filter_map(|r| found.get(r))
        .filter(|o| o.type_key == KIND_MEDIA_ASSET && o.valid_to.is_none())
        .collect();
    let assets: Vec<Uuid> = media
        .iter()
        .filter_map(|o| Uuid::parse_str(&o.native_id).ok())
        .collect();
    let rows = Media::find_by_statement(stmt(
        "SELECT a.id AS asset_id, a.kind, a.metadata, v.version_number, v.sha256, v.size_bytes, \
           v.content_type, p.id AS proof_id, p.seq, p.captured_at, p.manifest_sha256, p.prev_sha256 \
         FROM media_assets a \
         LEFT JOIN media_versions v ON v.id = a.current_version_id \
         LEFT JOIN web_proofs p ON p.org_id = a.org_id AND p.state = 'sealed' \
           AND p.id::text = a.metadata ->> 'proof_id' \
         WHERE a.org_id = $1 AND a.id = ANY($2)",
        vec![org_id.into(), assets.into()],
    ))
    .all(db)
    .await?;
    let by_asset: HashMap<String, Media> = rows
        .into_iter()
        .map(|m| (m.asset_id.to_string(), m))
        .collect();
    Ok(media
        .into_iter()
        .filter_map(|o| {
            let m = by_asset.get(&o.native_id)?;
            let meta = |k: &str| {
                m.metadata
                    .as_ref()
                    .and_then(|v| v.get(k))
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
            };
            Some(EvidenceItem {
                media: object_ref(o),
                asset_id: m.asset_id.to_string(),
                kind: m.kind.clone(),
                part: meta("proof_part"),
                source_url: meta("source_url"),
                version: m.version_number,
                sha256: m.sha256.clone(),
                size_bytes: m.size_bytes,
                content_type: m.content_type.clone(),
                via: paths.get(&o.id).cloned().unwrap_or_default(),
                proof: m.proof_id.map(|p| ProofSeal {
                    id: p.to_string(),
                    seq: m.seq.unwrap_or_default(),
                    captured_at: m.captured_at.map(|t| t.to_rfc3339()),
                    manifest_sha256: m.manifest_sha256.clone(),
                    prev_sha256: m.prev_sha256.clone(),
                }),
            })
        })
        .collect())
}
