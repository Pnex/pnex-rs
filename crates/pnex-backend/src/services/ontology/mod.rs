//! Ontology core (docs/architecture/ontology.md D176–D191): org object and
//! link types, objects, temporal links, queries, graph, series, packs.
//!
//! Shared by the HTTP controller (`controllers/ontology.rs`) and the
//! assistant tools: same validations, same 409. The org always comes from
//! the principal (R1); write roles are checked here (R2, D188).
//!
//! Identity rows of system objects are written by database triggers
//! (migration `m20261011_000002_ontology`); this module never inserts them.

pub mod changes;
pub mod graph;
pub mod link_types;
pub mod links;
pub mod objects;
pub mod packs;
pub mod query;
pub mod series;
pub mod types;

use pnex_core::ontology::{
    system_link_types, system_object_types, LinkTypeDef, ObjectTypeDef, WriteRole,
};
use sea_orm::{ConnectionTrait, DbErr, EntityTrait, QueryFilter};

use crate::models::_entities::sea_orm_active_enums::OrgMemberRole;
use crate::models::_entities::{link_types as lt, object_type_versions, object_types};

#[derive(Debug, thiserror::Error)]
pub enum OntologyError {
    #[error("invalid {field}: {token}")]
    Invalid { field: String, token: String },
    #[error("not found")]
    NotFound,
    #[error("key taken")]
    KeyTaken,
    #[error("version conflict (current {current})")]
    VersionConflict { current: i32 },
    /// A type still has live objects (or a link type open links).
    #[error("type in use")]
    TypeInUse,
    /// The caller's role is below the type's write role (D188).
    #[error("write forbidden")]
    WriteForbidden,
    /// System types, system link types and native titles are read-only here.
    #[error("system read only")]
    SystemReadOnly,
    /// The link type does not admit these ends.
    #[error("link not allowed")]
    LinkNotAllowed,
    /// One-to-one / one-to-many violated by an open link.
    #[error("link cardinality")]
    LinkCardinality,
    #[error(transparent)]
    Db(#[from] DbErr),
}

pub type Result<T> = std::result::Result<T, OntologyError>;

pub fn invalid(field: impl Into<String>, token: impl Into<String>) -> OntologyError {
    OntologyError::Invalid {
        field: field.into(),
        token: token.into(),
    }
}

impl From<(String, String)> for OntologyError {
    fn from((field, token): (String, String)) -> Self {
        Self::Invalid { field, token }
    }
}

/// Every type and link type visible to an org: system ones then the org's.
#[derive(Debug, Clone)]
pub struct OrgSchema {
    pub types: Vec<(ObjectTypeDef, i32, Option<String>)>,
    pub links: Vec<(LinkTypeDef, i32, Option<String>)>,
}

impl OrgSchema {
    pub fn object_type(&self, key: &str) -> Option<&ObjectTypeDef> {
        self.types.iter().map(|(t, _, _)| t).find(|t| t.key == key)
    }

    pub fn type_version(&self, key: &str) -> Option<i32> {
        self.types
            .iter()
            .find(|(t, _, _)| t.key == key)
            .map(|(_, v, _)| *v)
    }

    pub fn link_type(&self, key: &str) -> Option<&LinkTypeDef> {
        self.links.iter().map(|(l, _, _)| l).find(|l| l.key == key)
    }

    pub fn knows(&self, key: &str) -> bool {
        self.object_type(key).is_some()
    }

    pub fn org_types(&self) -> Vec<ObjectTypeDef> {
        self.types
            .iter()
            .filter(|(t, _, _)| !t.system)
            .map(|(t, _, _)| t.clone())
            .collect()
    }

    pub fn org_links(&self) -> Vec<LinkTypeDef> {
        self.links
            .iter()
            .filter(|(l, _, _)| !l.system)
            .map(|(l, _, _)| l.clone())
            .collect()
    }
}

/// Loads the schema of an org (a handful of rows: no cache).
pub async fn schema<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
) -> std::result::Result<OrgSchema, DbErr> {
    use sea_orm::ColumnTrait;
    let mut types: Vec<(ObjectTypeDef, i32, Option<String>)> = system_object_types()
        .into_iter()
        .map(|t| (t, 0, None))
        .collect();
    let mut org = object_types::Entity::find()
        .filter(object_types::Column::OrgId.eq(org_id))
        .all(db)
        .await?;
    org.sort_by(|a, b| a.key.cmp(&b.key));
    for t in org {
        let v = object_type_versions::Entity::find()
            .filter(object_type_versions::Column::ObjectTypeId.eq(t.id))
            .filter(object_type_versions::Column::Version.eq(t.current_version))
            .one(db)
            .await?;
        let Some(def) = v.and_then(|v| serde_json::from_value::<ObjectTypeDef>(v.definition).ok())
        else {
            continue;
        };
        // An org row under a system key extends the system type (D176).
        if let Some(sys) = types
            .iter_mut()
            .find(|(s, _, _)| s.system && s.key == def.key)
        {
            sys.0.properties = def.properties;
            sys.1 = t.current_version;
            continue;
        }
        types.push((def, t.current_version, t.pack_key));
    }
    let mut links: Vec<(LinkTypeDef, i32, Option<String>)> = system_link_types()
        .into_iter()
        .map(|l| (l, 0, None))
        .collect();
    let mut org_links = lt::Entity::find()
        .filter(lt::Column::OrgId.eq(org_id))
        .all(db)
        .await?;
    org_links.sort_by(|a, b| a.key.cmp(&b.key));
    for l in org_links {
        if let Ok(def) = serde_json::from_value::<LinkTypeDef>(l.definition) {
            links.push((def, l.version, l.pack_key));
        }
    }
    Ok(OrgSchema { types, links })
}

/// D188: may this role write objects of a type requiring `need`?
pub fn role_allows(role: &OrgMemberRole, need: WriteRole) -> bool {
    let rank = |r: &OrgMemberRole| match r {
        OrgMemberRole::Owner => 3,
        OrgMemberRole::Admin => 2,
        OrgMemberRole::Member => 1,
        _ => 0,
    };
    let need = match need {
        WriteRole::Member => 1,
        WriteRole::Admin => 2,
        WriteRole::Owner => 3,
    };
    rank(role) >= need
}

/// Who writes, for provenance (D184) and the D188 lock.
#[derive(Debug, Clone)]
pub struct Actor {
    pub user_id: Option<i64>,
    pub role: OrgMemberRole,
    /// D184 `source_ref`: `manual:<user>`, `import:<pack>`, `api:…`.
    pub source_ref: String,
}

impl Actor {
    pub fn manual(user_id: i64, role: OrgMemberRole) -> Self {
        Self {
            user_id: Some(user_id),
            role,
            source_ref: format!("manual:{user_id}"),
        }
    }
}

pub fn ts(t: &sea_orm::prelude::DateTimeWithTimeZone) -> String {
    t.to_rfc3339()
}

/// Parses an optional RFC 3339 instant; `field` names the error.
pub fn parse_instant(
    field: &str,
    raw: Option<&str>,
) -> Result<Option<sea_orm::prelude::DateTimeWithTimeZone>> {
    match raw.map(str::trim).filter(|s| !s.is_empty()) {
        None => Ok(None),
        Some(s) => chrono::DateTime::parse_from_rfc3339(s)
            .map(Some)
            .map_err(|_| invalid(field, pnex_core::err_codes::FIELD_INVALID)),
    }
}
