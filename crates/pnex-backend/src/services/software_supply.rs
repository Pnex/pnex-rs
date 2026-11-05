//! Scans into the ontology (agent-environments.md D708, D728): the image, its SBOM, its
//! components and the flaws it is exposed to become objects of the `software_supply` pack,
//! installed on first use. Safe to run again: everything is matched on its natural key, and
//! an exposure a scan no longer finds is closed, so it stays as history.

use std::collections::{BTreeMap, HashMap};

use pnex_core::environments::ScanReport;
use pnex_core::ontology::api::{LinkInput, ObjectInput};
use sea_orm::sea_query::Expr;
use sea_orm::{
    ColumnTrait, ConnectionTrait, DatabaseBackend, DatabaseConnection, EntityTrait, QueryFilter,
    Statement,
};
use serde_json::{json, Map, Value};
use uuid::Uuid;

use crate::models::_entities::sea_orm_active_enums::OrgMemberRole;
use crate::models::_entities::{objects, resource_edges};
use crate::services::ontology::{self, packs, Actor, OntologyError, OrgSchema, Result};

const PACK: &str = "software_supply";
const IMAGE: &str = "software_image";
const BOM: &str = "software_bom";
const COMPONENT: &str = "software_component";
const VULNERABILITY: &str = "software_vulnerability";
const HAS_BOM: &str = "image_has_bom";
const LISTS: &str = "bom_lists_component";
const EXPOSED: &str = "image_exposed_to";
const CHUNK: usize = 500;
const ATTRIBUTE_MAX: usize = 2000;

fn props(pairs: &[(&str, Value)]) -> Map<String, Value> {
    pairs
        .iter()
        .filter(|(_, v)| !v.is_null())
        .map(|(k, v)| (k.to_string(), v.clone()))
        .collect()
}

/// Installs the pack the first time an org needs it. A scan is no reason to refuse an org
/// whose own types hold these keys: the caller logs the conflict and goes on.
async fn schema_with_pack(db: &DatabaseConnection, org_id: i64) -> Result<OrgSchema> {
    let schema = ontology::schema(db, org_id).await?;
    if schema.object_type(IMAGE).is_some() {
        return Ok(schema);
    }
    let pack = packs::shipped()
        .into_iter()
        .find(|p| p.key == PACK)
        .ok_or(OntologyError::NotFound)?;
    packs::install(db, org_id, None, &pack).await?;
    Ok(ontology::schema(db, org_id).await?)
}

/// `natural key -> object` for the live objects of a type, by chunks.
async fn existing(
    db: &DatabaseConnection,
    org_id: i64,
    type_key: &str,
    key: &str,
    values: &[String],
) -> Result<HashMap<String, Uuid>> {
    let mut found = HashMap::new();
    for chunk in values.chunks(CHUNK) {
        let rows = objects::Entity::find()
            .filter(objects::Column::OrgId.eq(org_id))
            .filter(objects::Column::TypeKey.eq(type_key))
            .filter(objects::Column::ValidTo.is_null())
            .filter(Expr::cust_with_values(
                format!("properties ->> '{key}' = ANY($1)"),
                [chunk.to_vec()],
            ))
            .all(db)
            .await?;
        for row in rows {
            if let Some(value) = row.properties.get(key).and_then(Value::as_str) {
                found.insert(value.to_string(), row.id);
            }
        }
    }
    Ok(found)
}

/// The objects of these natural keys, created when missing. `new` gives title and properties.
async fn ensure_all<F>(
    db: &DatabaseConnection,
    schema: &OrgSchema,
    org_id: i64,
    actor: &Actor,
    (type_key, key): (&str, &str),
    values: &[String],
    new: F,
) -> Result<HashMap<String, Uuid>>
where
    F: Fn(&str) -> (String, Map<String, Value>),
{
    let mut found = existing(db, org_id, type_key, key, values).await?;
    let missing: Vec<&String> = values.iter().filter(|v| !found.contains_key(*v)).collect();
    for chunk in missing.chunks(CHUNK) {
        let inputs: Vec<ObjectInput> = chunk
            .iter()
            .map(|value| {
                let (title, properties) = new(value);
                ObjectInput {
                    type_key: type_key.to_string(),
                    title,
                    properties,
                    expected_version: None,
                }
            })
            .collect();
        let ids = ontology::objects::create_many_with(db, schema, org_id, actor, &inputs).await?;
        for (value, id) in chunk.iter().zip(ids) {
            found.insert((*value).clone(), id);
        }
    }
    Ok(found)
}

/// Open links of `source` under `relation`: `target -> link id`.
async fn open_links(
    db: &DatabaseConnection,
    org_id: i64,
    source: Uuid,
    relation: &str,
) -> Result<HashMap<Uuid, i64>> {
    Ok(resource_edges::Entity::find()
        .filter(resource_edges::Column::OrgId.eq(org_id))
        .filter(resource_edges::Column::SourceObjectId.eq(source))
        .filter(resource_edges::Column::Relation.eq(relation))
        .filter(resource_edges::Column::ValidTo.is_null())
        .all(db)
        .await?
        .into_iter()
        .filter_map(|e| e.target_object_id.map(|t| (t, e.id)))
        .collect())
}

fn link(relation: &str, source: Uuid, target: Uuid, attributes: Map<String, Value>) -> LinkInput {
    LinkInput {
        link_type: relation.to_string(),
        source_id: source.to_string(),
        target_id: target.to_string(),
        attributes,
        valid_from: None,
        valid_to: None,
    }
}

fn short(text: &str) -> String {
    text.chars().take(ATTRIBUTE_MAX).collect()
}

/// What a report says about one advisory: its worst spelling of the severity is the only
/// one, as the tool gives it per advisory; packages and fixes are gathered.
#[derive(Default)]
struct Exposure {
    severity: String,
    packages: Vec<String>,
    fixed: Vec<String>,
}

fn exposures(report: &ScanReport) -> BTreeMap<String, Exposure> {
    let mut out: BTreeMap<String, Exposure> = BTreeMap::new();
    for f in report.findings.iter().filter(|f| !f.id.is_empty()) {
        let e = out.entry(f.id.clone()).or_default();
        e.severity.clone_from(&f.severity);
        if !e.packages.contains(&f.package) {
            e.packages.push(f.package.clone());
        }
        if let Some(fixed) = f.fixed.as_ref().filter(|x| !e.fixed.contains(x)) {
            e.fixed.push(fixed.clone());
        }
    }
    out
}

/// Projects the scan held by runner job `job`. One at a time per org, across pods: two scans
/// ending together would otherwise both find a link missing and both open it.
pub async fn project_scan(db: &DatabaseConnection, org_id: i64, job: Uuid) -> Result<()> {
    use crate::services::db_lock::{ns, TenantLock};
    let lock = TenantLock::acquire(
        db,
        ns::SOFTWARE_SUPPLY,
        org_id,
        std::time::Duration::from_secs(120),
    )
    .await?;
    let out = project_locked(db, org_id, job).await;
    lock.release().await;
    out
}

/// Nothing to do without a report.
async fn project_locked(db: &DatabaseConnection, org_id: i64, job: Uuid) -> Result<()> {
    let Some(row) = db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT s.report, s.sbom_sha256, b.id AS build_id, b.repository, b.digest, b.number, \
               e.id AS environment_id, e.name \
             FROM environment_scans s \
             JOIN environment_builds b ON b.id = s.build_id \
             JOIN environments e ON e.id = b.environment_id \
             WHERE s.org_id = $1 AND s.job_id = $2",
            vec![org_id.into(), job.into()],
        ))
        .await?
    else {
        return Ok(());
    };
    let Some(report) = row
        .try_get::<Option<Value>>("", "report")?
        .and_then(|v| serde_json::from_value::<ScanReport>(v).ok())
    else {
        return Ok(());
    };
    let repository: String = row.try_get("", "repository")?;
    let digest: Option<String> = row.try_get("", "digest")?;
    let Some(digest) = digest else {
        return Ok(());
    };
    let reference = format!("{repository}@{digest}");
    let number: i32 = row.try_get("", "number")?;
    let environment: String = row.try_get("", "name")?;
    let environment_id: Uuid = row.try_get("", "environment_id")?;
    let build_id: Uuid = row.try_get("", "build_id")?;
    let sha256: Option<String> = row.try_get("", "sbom_sha256")?;

    let schema = schema_with_pack(db, org_id).await?;
    let actor = Actor {
        user_id: None,
        role: OrgMemberRole::Admin,
        source_ref: format!("environment-scan:{job}"),
    };
    let abbreviated = digest.get(..19).unwrap_or(&digest).to_string();
    let image_title = format!("{repository}@{abbreviated}");
    let image = ensure_all(
        db,
        &schema,
        org_id,
        &actor,
        (IMAGE, "reference"),
        std::slice::from_ref(&reference),
        |_| {
            (
                image_title.clone(),
                props(&[
                    ("reference", json!(reference)),
                    ("repository", json!(repository)),
                    ("digest", json!(digest)),
                    ("source_environment", json!(environment)),
                    ("source_version", json!(number)),
                ]),
            )
        },
    )
    .await?[&reference];
    let bom = ensure_all(
        db,
        &schema,
        org_id,
        &actor,
        (BOM, "image_reference"),
        std::slice::from_ref(&reference),
        |_| {
            (
                format!("SBOM {image_title}"),
                props(&[
                    ("image_reference", json!(reference)),
                    ("format", json!("CycloneDX")),
                    ("tool", json!("Trivy")),
                    ("component_count", json!(report.components.len())),
                    ("sha256", sha256.as_ref().map_or(Value::Null, |s| json!(s))),
                    (
                        "document",
                        json!(format!(
                            "/api/v1/environments/{environment_id}/builds/{build_id}/sbom"
                        )),
                    ),
                ]),
            )
        },
    )
    .await?[&reference];
    if !open_links(db, org_id, image, HAS_BOM)
        .await?
        .contains_key(&bom)
    {
        ontology::links::create_with(
            db,
            &schema,
            org_id,
            &actor,
            &link(HAS_BOM, image, bom, Map::new()),
        )
        .await?;
    }

    // Components: an image never changes, so what is already listed stays.
    let by_purl: BTreeMap<&str, _> = report
        .components
        .iter()
        .filter(|c| !c.purl.is_empty())
        .map(|c| (c.purl.as_str(), c))
        .collect();
    let purls: Vec<String> = by_purl.keys().map(|p| p.to_string()).collect();
    let components = ensure_all(
        db,
        &schema,
        org_id,
        &actor,
        (COMPONENT, "purl"),
        &purls,
        |purl| {
            let c = by_purl[purl];
            (
                format!("{} {}", c.name, c.version),
                props(&[
                    ("purl", json!(purl)),
                    ("package_name", json!(c.name)),
                    ("package_version", json!(c.version)),
                ]),
            )
        },
    )
    .await?;
    let listed = open_links(db, org_id, bom, LISTS).await?;
    let new_links: Vec<LinkInput> = components
        .values()
        .filter(|c| !listed.contains_key(*c))
        .map(|c| link(LISTS, bom, *c, Map::new()))
        .collect();
    for chunk in new_links.chunks(CHUNK) {
        ontology::links::create_many_with(db, &schema, org_id, &actor, chunk).await?;
    }

    // Exposures follow the latest scan: new ones open, gone ones close.
    let found = exposures(&report);
    let advisories: Vec<String> = found.keys().cloned().collect();
    let vulnerabilities = ensure_all(
        db,
        &schema,
        org_id,
        &actor,
        (VULNERABILITY, "advisory"),
        &advisories,
        |advisory| {
            (
                advisory.to_string(),
                props(&[
                    ("advisory", json!(advisory)),
                    ("severity", json!(found[advisory].severity)),
                ]),
            )
        },
    )
    .await?;
    let mut open = open_links(db, org_id, image, EXPOSED).await?;
    for (advisory, exposure) in &found {
        let vulnerability = vulnerabilities[advisory];
        if open.remove(&vulnerability).is_none() {
            let attributes = props(&[
                ("packages", json!(short(&exposure.packages.join(", ")))),
                ("fixed_in", json!(short(&exposure.fixed.join(", ")))),
            ]);
            ontology::links::create_with(
                db,
                &schema,
                org_id,
                &actor,
                &link(EXPOSED, image, vulnerability, attributes),
            )
            .await?;
        }
    }
    for gone in open.into_values() {
        ontology::links::close(db, org_id, &actor, gone, None).await?;
    }
    Ok(())
}

/// In the background: a big image means thousands of objects, and the runner should not
/// wait for them. A failure is logged and the next scan tries again.
pub fn spawn_projection(db: DatabaseConnection, org_id: i64, job: Uuid) {
    tokio::spawn(async move {
        if let Err(e) = project_scan(&db, org_id, job).await {
            tracing::warn!(org_id, %job, error = ?e, "scan not projected to the ontology");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use pnex_core::environments::ScanFinding;

    #[test]
    fn an_advisory_gathers_its_packages_and_fixes() {
        let finding = |id: &str, package: &str, fixed: Option<&str>| ScanFinding {
            id: id.into(),
            purl: None,
            package: package.into(),
            installed: "1".into(),
            fixed: fixed.map(str::to_string),
            severity: "high".into(),
        };
        let report = ScanReport {
            components: vec![],
            findings: vec![
                finding("CVE-1", "a", Some("2")),
                finding("CVE-1", "b", Some("2")),
                finding("CVE-2", "a", None),
                finding("", "a", None),
            ],
        };
        let out = exposures(&report);
        assert_eq!(out.len(), 2);
        assert_eq!(out["CVE-1"].packages, ["a", "b"]);
        assert_eq!(out["CVE-1"].fixed, ["2"]);
        assert!(out["CVE-2"].fixed.is_empty());
    }
}
