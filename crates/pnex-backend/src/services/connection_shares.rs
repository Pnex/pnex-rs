//! A member's own network connections shared with a team org (crm.md
//! §3.6, D999).
//!
//! The connections stay in the org where the member reads them (their
//! personal org) unless the member turns sharing on in a team org. Turned
//! on, the team org gets a person that stands for the member (found by the
//! member's e-mail, else created), and each connection the member's run
//! reads lands there too, keyed like in the run, with a `knows` link from the
//! member's person: the network asserts that link (D998). Turned off, those
//! links close (history kept) and every person nobody else knows, which
//! carries nothing else, gets an erasure proposal to validate.

use pnex_core::browser_agent::ConnectionShare;
use pnex_core::ontology::roles;
use pnex_core::pipeline::doc::ObjectsTarget;
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement};
use serde_json::{json, Map, Value};
use uuid::Uuid;

use crate::services::ontology::redact::Redaction;
use crate::services::ontology::{self, Actor, OntologyError};

/// The networks a member may share; a pipeline names one in `share`.
pub const NETWORKS: &[&str] = &["linkedin"];
/// Column and alias of the member's person in the mapping of a share.
const MEMBER: &str = "share_member";

fn stmt(sql: &str, values: Vec<sea_orm::Value>) -> Statement {
    Statement::from_sql_and_values(DatabaseBackend::Postgres, sql, values)
}

/// `source_ref` of everything a share writes, all members alike.
fn source_ref(network: &str) -> String {
    format!("share:{network}")
}

fn check_network(network: &str) -> ontology::Result<()> {
    if NETWORKS.contains(&network) {
        Ok(())
    } else {
        Err(ontology::invalid(
            "network",
            pnex_core::err_codes::FIELD_INVALID,
        ))
    }
}

/// Whether the member shares the network with the org.
pub async fn get(
    db: &DatabaseConnection,
    org_id: i64,
    user_id: i64,
    network: &str,
) -> ontology::Result<ConnectionShare> {
    check_network(network)?;
    let row = db
        .query_one_raw(stmt(
            "SELECT shared, person_id FROM connection_shares
             WHERE org_id = $1 AND user_id = $2 AND network = $3",
            vec![org_id.into(), user_id.into(), network.into()],
        ))
        .await?;
    Ok(row
        .map(|r| ConnectionShare {
            shared: r.try_get("", "shared").unwrap_or(false),
            person_id: r
                .try_get::<Option<Uuid>>("", "person_id")
                .ok()
                .flatten()
                .map(|u| u.to_string()),
        })
        .unwrap_or_default())
}

/// Turns sharing on or off for the caller (`actor`), whose e-mail and name
/// find or make their person. The caller's right to write objects in the
/// org is checked by the route.
#[allow(clippy::too_many_arguments)]
pub async fn set(
    db: &DatabaseConnection,
    org_id: i64,
    actor: &Actor,
    user: &crate::models::_entities::users::Model,
    hide: &Redaction,
    network: &str,
    shared: bool,
) -> ontology::Result<ConnectionShare> {
    check_network(network)?;
    let before = get(db, org_id, user.id, network).await?;
    let person = match before.person_id.as_deref().map(Uuid::parse_str) {
        Some(Ok(id)) if live(db, org_id, id).await => Some(id),
        _ if shared => Some(member_person(db, org_id, actor, user, hide).await?),
        _ => None,
    };
    db.execute_raw(stmt(
        "INSERT INTO connection_shares (org_id, user_id, network, shared, person_id)
         VALUES ($1, $2, $3, $4, $5)
         ON CONFLICT (org_id, user_id, network)
         DO UPDATE SET shared = $4, person_id = $5, updated_at = now()",
        vec![
            org_id.into(),
            user.id.into(),
            network.into(),
            shared.into(),
            person.into(),
        ],
    ))
    .await?;
    if !shared {
        if let Some(person) = person {
            stop(db, org_id, actor, network, person).await?;
        }
    }
    get(db, org_id, user.id, network).await
}

async fn live(db: &DatabaseConnection, org_id: i64, id: Uuid) -> bool {
    ontology::objects::find(db, org_id, id)
        .await
        .is_ok_and(|o| o.valid_to.is_none() && o.erased_at.is_none())
}

/// The person of the org using the member's e-mail as an identifier, else
/// a new person titled with their name.
async fn member_person(
    db: &DatabaseConnection,
    org_id: i64,
    actor: &Actor,
    user: &crate::models::_entities::users::Model,
    hide: &Redaction,
) -> ontology::Result<Uuid> {
    let found = crate::services::mail::invitations::people(db, org_id, &user.email, hide).await;
    if let Some(id) = found.iter().find_map(|id| Uuid::parse_str(id).ok()) {
        return Ok(id);
    }
    let s = ontology::schema(db, org_id).await?;
    let person_type = s.type_key(roles::PERSON).ok_or(OntologyError::NotFound)?;
    let title = user
        .full_name
        .clone()
        .filter(|n| !n.trim().is_empty())
        .unwrap_or_else(|| user.email.clone());
    let input = pnex_core::ontology::api::ObjectInput {
        type_key: person_type.to_string(),
        title,
        ..Default::default()
    };
    let v = ontology::objects::create_with(db, &s, org_id, actor, &input).await?;
    Uuid::parse_str(&v.id).map_err(|_| OntologyError::NotFound)
}

/// Closes the member's shared `knows` links, then proposes to erase each
/// person they reached when the share brought it and nothing else holds it:
/// no open link of any kind, no closed link but a share's, never edited
/// since its creation (version 1).
async fn stop(
    db: &DatabaseConnection,
    org_id: i64,
    actor: &Actor,
    network: &str,
    person: Uuid,
) -> ontology::Result<()> {
    let s = ontology::schema(db, org_id).await?;
    let Some(knows) = s.link_key(roles::KNOWS) else {
        return Ok(());
    };
    let from = source_ref(network);
    let open = db
        .query_all_raw(stmt(
            "SELECT id, target_object_id FROM resource_edges
             WHERE org_id = $1 AND relation = $2 AND source_object_id = $3
               AND source_ref = $4 AND valid_to IS NULL",
            vec![
                org_id.into(),
                knows.into(),
                person.into(),
                from.clone().into(),
            ],
        ))
        .await?;
    let mut reached = Vec::new();
    for row in open {
        let id: i64 = row.try_get("", "id")?;
        ontology::links::close(db, org_id, actor, id, None).await?;
        reached.extend(row.try_get::<Option<Uuid>>("", "target_object_id")?);
    }
    if reached.is_empty() {
        return Ok(());
    }
    let alone = db
        .query_all_raw(stmt(
            "SELECT o.id, o.type_key FROM objects o
             WHERE o.org_id = $1 AND o.id = ANY($2) AND o.valid_to IS NULL
               AND o.erased_at IS NULL AND o.source_ref = $3 AND o.version = 1
               AND NOT EXISTS (SELECT 1 FROM resource_edges l
                 WHERE l.org_id = o.org_id
                   AND (l.source_object_id = o.id OR l.target_object_id = o.id)
                   AND (l.valid_to IS NULL OR l.source_ref IS DISTINCT FROM $3))",
            vec![org_id.into(), reached.into(), from.clone().into()],
        ))
        .await?;
    for row in alone {
        let id: Uuid = row.try_get("", "id")?;
        let p = pnex_core::ontology::proposal::ProposalInput {
            kind: Some(pnex_core::ontology::proposal::ProposalKind::ObjectErase),
            type_key: row.try_get("", "type_key")?,
            target_id: Some(id.to_string()),
            evidence: json!({ "share": network, "reason": "share_stopped" }),
            origin: from.clone(),
            ..Default::default()
        };
        if let Err(e) = ontology::proposals::propose(db, org_id, &p).await {
            tracing::warn!(org_id, %id, error = %e, "erasure not proposed");
        }
    }
    Ok(())
}

/// Writes the rows of the member's own run (`user`, in `source_org`) into
/// every other org where the member shares `network`, while they may still
/// write objects there. A failure is logged: the member's own run stands.
pub async fn fan_out(
    db: &DatabaseConnection,
    source_org: i64,
    user: i64,
    network: &str,
    target: &ObjectsTarget,
    rows: &[Map<String, Value>],
) {
    let shares = match db
        .query_all_raw(stmt(
            "SELECT org_id, person_id FROM connection_shares
             WHERE user_id = $1 AND network = $2 AND shared
               AND org_id <> $3 AND person_id IS NOT NULL",
            vec![user.into(), network.into(), source_org.into()],
        ))
        .await
    {
        Ok(rows) => rows,
        Err(e) => {
            tracing::warn!(user, network, error = %e, "connection shares not read");
            return;
        }
    };
    for share in shares {
        let (Ok(org_id), Ok(person)) = (
            share.try_get::<i64>("", "org_id"),
            share.try_get::<Uuid>("", "person_id"),
        ) else {
            continue;
        };
        // R2: membership and right re-read at each write, never cached.
        let allowed = crate::services::iam::store::member_grants(db, org_id, user)
            .await
            .ok()
            .flatten()
            .is_some_and(|g| g.can("ontology:write"));
        if !allowed {
            tracing::info!(
                org_id,
                user,
                network,
                "share skipped: no right to write objects"
            );
            continue;
        }
        if let Err(e) = write_share(db, org_id, person, network, target, rows).await {
            tracing::info!(org_id, user, network, error = %e, "shared connections not written");
        }
    }
}

async fn write_share(
    db: &DatabaseConnection,
    org_id: i64,
    person: Uuid,
    network: &str,
    target: &ObjectsTarget,
    rows: &[Map<String, Value>],
) -> Result<usize, String> {
    if !live(db, org_id, person).await {
        return Err("the member's person is gone".into());
    }
    let s = ontology::schema(db, org_id)
        .await
        .map_err(|e| e.to_string())?;
    let (Some(person_type), Some(knows)) = (s.type_key(roles::PERSON), s.link_key(roles::KNOWS))
    else {
        return Err("no person type or knows link".into());
    };
    let Some(connection) = target.objects.iter().find(|o| o.type_key == person_type) else {
        return Err("the rows hold no person".into());
    };
    // Existing objects of the team org keep their values: a share only adds.
    let mut objects: Vec<Value> = target
        .objects
        .iter()
        .filter(|o| s.object_type(&o.type_key).is_some())
        .filter_map(|o| serde_json::to_value(o).ok())
        .map(|mut v| {
            v["update"] = json!(false);
            v
        })
        .collect();
    objects.push(json!({
        "as": MEMBER, "type": person_type, "key": "@id", "create": false,
        "properties": { "@id": MEMBER },
    }));
    let kept = |a: &String| {
        target
            .objects
            .iter()
            .any(|o| &o.alias == a && s.object_type(&o.type_key).is_some())
    };
    let mut links: Vec<Value> = target
        .links
        .iter()
        .filter(|l| s.link_type(&l.link_type).is_some() && kept(&l.from) && kept(&l.to))
        .filter_map(|l| serde_json::to_value(l).ok())
        .collect();
    links.push(json!({ "type": knows, "from": MEMBER, "to": connection.alias }));
    let mapping: pnex_core::ontology::import::Mapping = serde_json::from_value(json!({
        "key": format!("share_{network}"),
        "name": source_ref(network),
        "objects": objects,
        "links": links,
    }))
    .map_err(|e| e.to_string())?;
    let rows: Vec<Value> = rows
        .iter()
        .map(|r| {
            let mut r = r.clone();
            r.insert(MEMBER.into(), person.to_string().into());
            Value::Object(r)
        })
        .collect();
    let source = source_ref(network);
    let writer = ontology::imports::RowsWriter {
        org_id,
        pipeline: &source,
        step: "share",
        source_ref: &source,
        author: None,
        snapshot: false,
    };
    ontology::imports::write_rows(db, &writer, &mapping, &rows)
        .await
        .map_err(|e| e.to_string())
}
