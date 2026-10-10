//! Topic taxonomies (media-ingest.md D168): CRUD of `taxonomies` and the
//! append-only `taxonomy_versions` (`flow_versions` school). Shared by the
//! HTTP controller and the assistant tools (same validations, same 409).

use pnex_core::err_codes;
use pnex_core::taxonomy::{
    check_topics, Taxonomy, TaxonomyInput, TaxonomyVersion, TaxonomyVersionInput, Topic,
    DESCRIPTION_MAX, NAME_MAX, NOTE_MAX,
};
use sea_orm::sea_query::Expr;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, Condition, ConnectionTrait, DatabaseConnection, DbErr,
    EntityTrait, QueryFilter, QueryOrder, Set, TransactionTrait,
};
use uuid::Uuid;

use crate::models::_entities::{taxonomies, taxonomy_versions};

#[derive(Debug, thiserror::Error)]
pub enum TaxonomyError {
    #[error("invalid {field}: {token}")]
    Invalid { field: String, token: String },
    #[error("taxonomy not found")]
    NotFound,
    #[error("taxonomy name taken")]
    NameTaken,
    #[error("taxonomy version conflict (current {current})")]
    VersionConflict { current: i32 },
    #[error(transparent)]
    Db(#[from] DbErr),
}

fn invalid(field: &str, token: impl Into<String>) -> TaxonomyError {
    TaxonomyError::Invalid {
        field: field.to_string(),
        token: token.into(),
    }
}

fn too_long(max: usize) -> String {
    format!("{}:{max}", err_codes::FIELD_MAX_LENGTH)
}

/// Topics of a stored version; an unreadable row reads as no topic.
pub fn topics_of(v: &taxonomy_versions::Model) -> Vec<Topic> {
    serde_json::from_value(v.topics.clone()).unwrap_or_default()
}

pub fn version_view(v: &taxonomy_versions::Model) -> TaxonomyVersion {
    TaxonomyVersion {
        version: v.version,
        topics: topics_of(v),
        note: v.note.clone().unwrap_or_default(),
        created_at: v.created_at.to_rfc3339(),
    }
}

pub fn view(t: &taxonomies::Model, current: Option<&taxonomy_versions::Model>) -> Taxonomy {
    Taxonomy {
        id: t.id.to_string(),
        name: t.name.clone(),
        description: t.description.clone().unwrap_or_default(),
        current_version: t.current_version,
        created_at: t.created_at.to_rfc3339(),
        updated_at: t.updated_at.to_rfc3339(),
        current: current.map(version_view),
    }
}

/// Taxonomies of the org (by name) with their current version.
pub async fn list<C: ConnectionTrait>(db: &C, org_id: i64) -> Result<Vec<Taxonomy>, DbErr> {
    let rows = taxonomies::Entity::find()
        .filter(taxonomies::Column::OrgId.eq(org_id))
        .order_by_asc(taxonomies::Column::Name)
        .all(db)
        .await?;
    let mut cond = Condition::any();
    for t in rows.iter().filter(|t| t.current_version > 0) {
        cond = cond.add(
            Condition::all()
                .add(taxonomy_versions::Column::TaxonomyId.eq(t.id))
                .add(taxonomy_versions::Column::Version.eq(t.current_version)),
        );
    }
    let current = if rows.iter().any(|t| t.current_version > 0) {
        taxonomy_versions::Entity::find()
            .filter(taxonomy_versions::Column::OrgId.eq(org_id))
            .filter(cond)
            .all(db)
            .await?
    } else {
        Vec::new()
    };
    Ok(rows
        .iter()
        .map(|t| view(t, current.iter().find(|v| v.taxonomy_id == t.id)))
        .collect())
}

pub async fn find<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    id: Uuid,
) -> Result<taxonomies::Model, TaxonomyError> {
    taxonomies::Entity::find_by_id(id)
        .filter(taxonomies::Column::OrgId.eq(org_id))
        .one(db)
        .await?
        .ok_or(TaxonomyError::NotFound)
}

/// One version of a taxonomy of the org.
pub async fn find_version<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    id: Uuid,
    version: i32,
) -> Result<Option<taxonomy_versions::Model>, DbErr> {
    taxonomy_versions::Entity::find()
        .filter(taxonomy_versions::Column::OrgId.eq(org_id))
        .filter(taxonomy_versions::Column::TaxonomyId.eq(id))
        .filter(taxonomy_versions::Column::Version.eq(version))
        .one(db)
        .await
}

/// Read model of one taxonomy with its current version.
pub async fn get<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    id: Uuid,
) -> Result<Taxonomy, TaxonomyError> {
    let t = find(db, org_id, id).await?;
    let current = find_version(db, org_id, id, t.current_version).await?;
    Ok(view(&t, current.as_ref()))
}

/// Versions of a taxonomy of the org, newest first.
pub async fn versions<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    id: Uuid,
) -> Result<Vec<TaxonomyVersion>, TaxonomyError> {
    find(db, org_id, id).await?;
    Ok(taxonomy_versions::Entity::find()
        .filter(taxonomy_versions::Column::OrgId.eq(org_id))
        .filter(taxonomy_versions::Column::TaxonomyId.eq(id))
        .order_by_desc(taxonomy_versions::Column::Version)
        .all(db)
        .await?
        .iter()
        .map(version_view)
        .collect())
}

fn clean_name(raw: &str) -> Result<String, TaxonomyError> {
    let name = raw.trim();
    if name.is_empty() {
        return Err(invalid("name", err_codes::FIELD_REQUIRED));
    }
    if name.chars().count() > NAME_MAX {
        return Err(invalid("name", too_long(NAME_MAX)));
    }
    Ok(name.to_string())
}

fn clean_description(raw: &str) -> Result<Option<String>, TaxonomyError> {
    let d = raw.trim();
    if d.chars().count() > DESCRIPTION_MAX {
        return Err(invalid("description", too_long(DESCRIPTION_MAX)));
    }
    Ok((!d.is_empty()).then(|| d.to_string()))
}

async fn name_taken<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    name: &str,
    except: Option<Uuid>,
) -> Result<bool, DbErr> {
    let mut q = taxonomies::Entity::find()
        .filter(taxonomies::Column::OrgId.eq(org_id))
        .filter(taxonomies::Column::Name.eq(name));
    if let Some(id) = except {
        q = q.filter(taxonomies::Column::Id.ne(id));
    }
    Ok(q.one(db).await?.is_some())
}

/// Creates an empty taxonomy (version 0): its topics come with the first
/// version.
pub async fn create(
    db: &DatabaseConnection,
    org_id: i64,
    user_id: Option<i64>,
    input: &TaxonomyInput,
) -> Result<taxonomies::Model, TaxonomyError> {
    let name = clean_name(input.name.as_deref().unwrap_or_default())?;
    let description = clean_description(input.description.as_deref().unwrap_or_default())?;
    let txn = db.begin().await?;
    if name_taken(&txn, org_id, &name, None).await? {
        return Err(TaxonomyError::NameTaken);
    }
    let now: sea_orm::prelude::DateTimeWithTimeZone = chrono::Utc::now().into();
    let row = taxonomies::ActiveModel {
        id: Set(Uuid::new_v4()),
        org_id: Set(org_id),
        name: Set(name),
        description: Set(description),
        current_version: Set(0),
        created_by: Set(user_id),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(&txn)
    .await?;
    txn.commit().await?;
    Ok(row)
}

/// Renames / redescribes a taxonomy. Deployed flows keep the label they
/// were stamped with until their next projection.
pub async fn update(
    db: &DatabaseConnection,
    org_id: i64,
    id: Uuid,
    input: &TaxonomyInput,
) -> Result<taxonomies::Model, TaxonomyError> {
    let txn = db.begin().await?;
    let row = find(&txn, org_id, id).await?;
    let mut am: taxonomies::ActiveModel = row.into();
    if let Some(name) = &input.name {
        let name = clean_name(name)?;
        if name_taken(&txn, org_id, &name, Some(id)).await? {
            return Err(TaxonomyError::NameTaken);
        }
        am.name = Set(name);
    }
    if let Some(d) = &input.description {
        am.description = Set(clean_description(d)?);
    }
    am.updated_at = Set(chrono::Utc::now().into());
    let row = am.update(&txn).await?;
    txn.commit().await?;
    Ok(row)
}

/// Deletes a taxonomy and its versions. A deployed `topic_classify` on it
/// matches nothing after its next projection and refuses its next deploy.
pub async fn delete(db: &DatabaseConnection, org_id: i64, id: Uuid) -> Result<(), TaxonomyError> {
    let row = find(db, org_id, id).await?;
    taxonomies::Entity::delete_by_id(row.id).exec(db).await?;
    Ok(())
}

/// Appends version `current_version + 1` when `expected_version` is the
/// current one (optimistic concurrency, D144), else `VersionConflict`.
pub async fn add_version(
    db: &DatabaseConnection,
    org_id: i64,
    id: Uuid,
    user_id: Option<i64>,
    input: &TaxonomyVersionInput,
) -> Result<taxonomy_versions::Model, TaxonomyError> {
    let topics = check_topics(&input.topics).map_err(|(f, t)| invalid(&f, t))?;
    let note = input.note.trim();
    if note.chars().count() > NOTE_MAX {
        return Err(invalid("note", too_long(NOTE_MAX)));
    }
    let txn = db.begin().await?;
    let row = find(&txn, org_id, id).await?;
    let next = input.expected_version + 1;
    // Conditional bump: two writers on the same version, one wins.
    let bumped = taxonomies::Entity::update_many()
        .col_expr(taxonomies::Column::CurrentVersion, Expr::value(next))
        .col_expr(
            taxonomies::Column::UpdatedAt,
            Expr::value(sea_orm::prelude::DateTimeWithTimeZone::from(
                chrono::Utc::now(),
            )),
        )
        .filter(taxonomies::Column::Id.eq(row.id))
        .filter(taxonomies::Column::CurrentVersion.eq(input.expected_version))
        .exec(&txn)
        .await?;
    if bumped.rows_affected != 1 {
        return Err(TaxonomyError::VersionConflict {
            current: row.current_version,
        });
    }
    let version = taxonomy_versions::ActiveModel {
        id: Set(Uuid::new_v4()),
        org_id: Set(org_id),
        taxonomy_id: Set(row.id),
        version: Set(next),
        topics: Set(serde_json::to_value(&topics).unwrap_or_default()),
        note: Set((!note.is_empty()).then(|| note.to_string())),
        created_by: Set(user_id),
        created_at: Set(chrono::Utc::now().into()),
    }
    .insert(&txn)
    .await?;
    txn.commit().await?;
    Ok(version)
}
