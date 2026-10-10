//! Time ranges (media-ingest.md D169, D182): CRUD and idempotent upsert of
//! `time_ranges`. Shared by the HTTP API, the CSV/ICS import and the flow
//! runtime endpoint (`range_upsert` node): same validation everywhere.
//!
//! - the scope is resolved in the org (R1): a stream id of the org, or the
//!   org itself (`scope_id` = org id, set here);
//! - an upsert with an `external_id` merges onto the existing range of the
//!   same scope (absent fields keep their value): a schedule re-imported or
//!   realigned later updates instead of duplicating;
//! - `source_url` is `http(s)` only (R11), `source_ref` is built by the
//!   callers from server-side values (D184).

pub mod import;

use chrono::{DateTime, FixedOffset};
use pnex_core::err_codes;
use pnex_core::time_range::{
    is_http_url, RangeOrigin, ScopeKind, TimeRange, TimeRangeInput, ATTRS_MAX_BYTES, CATEGORY_MAX,
    EXTERNAL_ID_MAX, LABEL_MAX, SOURCE_URL_MAX,
};
use sea_orm::sea_query::Expr;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, Condition, ConnectionTrait, DbErr, EntityTrait, Order,
    QueryFilter, QueryOrder, Select, Set, SqlErr,
};
use uuid::Uuid;

use crate::models::_entities::time_ranges;

#[derive(Debug, thiserror::Error)]
pub enum RangeError {
    #[error("invalid {field}: {token}")]
    Invalid { field: String, token: String },
    #[error("time range not found")]
    NotFound,
    /// The scope is not a stream (or the org) of the caller's org.
    #[error("unknown scope")]
    ScopeUnknown,
    #[error(transparent)]
    Db(#[from] DbErr),
}

fn invalid(field: &str, token: impl Into<String>) -> RangeError {
    RangeError::Invalid {
        field: field.to_string(),
        token: token.into(),
    }
}

fn too_long(max: usize) -> String {
    format!("{}:{max}", err_codes::FIELD_MAX_LENGTH)
}

/// A scope resolved in the org.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scope {
    pub kind: ScopeKind,
    pub id: String,
}

/// Resolves a requested scope in the org (R1). `stream`: a live stream of
/// the org by id; `org`: the org itself (`id` empty or the org id).
pub async fn resolve_scope<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    kind: &str,
    id: &str,
) -> Result<Scope, RangeError> {
    let kind = ScopeKind::from_wire(kind.trim()).ok_or_else(|| invalid("scope_kind", "invalid"))?;
    let id = id.trim();
    match kind {
        ScopeKind::Stream => {
            let uuid = Uuid::parse_str(id).map_err(|_| RangeError::ScopeUnknown)?;
            match crate::services::media_ingest::streams::find(db, org_id, uuid).await {
                Ok(s) => Ok(Scope {
                    kind,
                    id: s.id.to_string(),
                }),
                Err(crate::services::media_ingest::streams::StreamError::NotFound) => {
                    Err(RangeError::ScopeUnknown)
                }
                Err(e) => Err(RangeError::Db(DbErr::Custom(e.to_string()))),
            }
        }
        // Any live object of the org (D182); the id is its identity UUID.
        ScopeKind::Object => {
            let uuid = Uuid::parse_str(id).map_err(|_| RangeError::ScopeUnknown)?;
            match crate::services::ontology::objects::find(db, org_id, uuid).await {
                Ok(o) if o.valid_to.is_none() => Ok(Scope {
                    kind,
                    id: o.id.to_string(),
                }),
                Ok(_) | Err(crate::services::ontology::OntologyError::NotFound) => {
                    Err(RangeError::ScopeUnknown)
                }
                Err(e) => Err(RangeError::Db(DbErr::Custom(e.to_string()))),
            }
        }
        ScopeKind::Org => {
            let own = org_id.to_string();
            if !id.is_empty() && id != own {
                return Err(RangeError::ScopeUnknown);
            }
            Ok(Scope { kind, id: own })
        }
    }
}

pub fn view(r: &time_ranges::Model) -> TimeRange {
    let ts = |t: &Option<DateTime<FixedOffset>>| t.map(|t| t.to_rfc3339());
    TimeRange {
        id: r.id.to_string(),
        scope_kind: ScopeKind::from_wire(&r.scope_kind).unwrap_or(ScopeKind::Org),
        scope_id: r.scope_id.clone(),
        label: r.label.clone(),
        external_id: r.external_id.clone(),
        category: r.category.clone(),
        planned_start: ts(&r.planned_start),
        planned_end: ts(&r.planned_end),
        actual_start: ts(&r.actual_start),
        actual_end: ts(&r.actual_end),
        origin: RangeOrigin::from_wire(&r.origin).unwrap_or(RangeOrigin::Manual),
        confidence: r.confidence,
        // Re-checked at read too (R11): a stored non-http value never leaves.
        source_url: r.source_url.clone().filter(|u| is_http_url(u)),
        source_ref: r.source_ref.clone(),
        attrs: r.attrs.clone(),
        created_at: r.created_at.to_rfc3339(),
        updated_at: r.updated_at.to_rfc3339(),
    }
}

/// Validated fields of a range, after merging an input onto a base.
#[derive(Debug, Clone, PartialEq)]
pub struct Fields {
    pub label: String,
    pub external_id: Option<String>,
    pub category: Option<String>,
    pub planned_start: Option<DateTime<FixedOffset>>,
    pub planned_end: Option<DateTime<FixedOffset>>,
    pub actual_start: Option<DateTime<FixedOffset>>,
    pub actual_end: Option<DateTime<FixedOffset>>,
    pub confidence: Option<f32>,
    pub source_url: Option<String>,
    pub attrs: serde_json::Value,
}

impl Fields {
    fn of(r: &time_ranges::Model) -> Self {
        Self {
            label: r.label.clone(),
            external_id: r.external_id.clone(),
            category: r.category.clone(),
            planned_start: r.planned_start,
            planned_end: r.planned_end,
            actual_start: r.actual_start,
            actual_end: r.actual_end,
            confidence: r.confidence,
            source_url: r.source_url.clone(),
            attrs: r.attrs.clone(),
        }
    }
}

/// Optional trimmed text: `None` keeps `base`, empty clears.
fn text(
    field: &str,
    raw: &Option<String>,
    base: Option<String>,
    max: usize,
) -> Result<Option<String>, RangeError> {
    let Some(raw) = raw else {
        return Ok(base);
    };
    let v = raw.trim();
    if v.chars().count() > max {
        return Err(invalid(field, too_long(max)));
    }
    Ok((!v.is_empty()).then(|| v.to_string()))
}

fn time(
    field: &str,
    raw: &Option<String>,
    base: Option<DateTime<FixedOffset>>,
) -> Result<Option<DateTime<FixedOffset>>, RangeError> {
    match raw.as_deref().map(str::trim) {
        None => Ok(base),
        Some("") => Ok(None),
        Some(s) => DateTime::parse_from_rfc3339(s)
            .map(Some)
            .map_err(|_| invalid(field, err_codes::FIELD_INVALID)),
    }
}

/// Merges `input` onto `base` (absent = keep, empty = clear) and checks
/// the result. `Err` carries the first faulty field and its token.
pub fn clean(
    input: &TimeRangeInput,
    base: Option<&time_ranges::Model>,
) -> Result<Fields, RangeError> {
    let base = base.map(Fields::of);
    let b = base.as_ref();
    let label = text("label", &input.label, b.map(|b| b.label.clone()), LABEL_MAX)?
        .ok_or_else(|| invalid("label", err_codes::FIELD_REQUIRED))?;
    let external_id = text(
        "external_id",
        &input.external_id,
        b.and_then(|b| b.external_id.clone()),
        EXTERNAL_ID_MAX,
    )?;
    let category = text(
        "category",
        &input.category,
        b.and_then(|b| b.category.clone()),
        CATEGORY_MAX,
    )?;
    let planned_start = time(
        "planned_start",
        &input.planned_start,
        b.and_then(|b| b.planned_start),
    )?;
    let planned_end = time(
        "planned_end",
        &input.planned_end,
        b.and_then(|b| b.planned_end),
    )?;
    let actual_start = time(
        "actual_start",
        &input.actual_start,
        b.and_then(|b| b.actual_start),
    )?;
    let actual_end = time(
        "actual_end",
        &input.actual_end,
        b.and_then(|b| b.actual_end),
    )?;
    for (start_name, start, end_name, end) in [
        ("planned_start", planned_start, "planned_end", planned_end),
        ("actual_start", actual_start, "actual_end", actual_end),
    ] {
        match (start, end) {
            (Some(s), Some(e)) if e <= s => {
                return Err(invalid(end_name, err_codes::FIELD_INVALID))
            }
            (Some(_), None) => return Err(invalid(end_name, err_codes::FIELD_REQUIRED)),
            (None, Some(_)) => return Err(invalid(start_name, err_codes::FIELD_REQUIRED)),
            _ => {}
        }
    }
    if planned_start.is_none() && actual_start.is_none() {
        return Err(invalid("planned_start", err_codes::FIELD_REQUIRED));
    }
    let confidence = match input.confidence {
        None => b.and_then(|b| b.confidence),
        Some(c) if c.is_finite() && (0.0..=1.0).contains(&c) => Some(c as f32),
        Some(_) => return Err(invalid("confidence", err_codes::FIELD_INVALID)),
    };
    let source_url = text(
        "source_url",
        &input.source_url,
        b.and_then(|b| b.source_url.clone()),
        SOURCE_URL_MAX,
    )?;
    if source_url.as_deref().is_some_and(|u| !is_http_url(u)) {
        return Err(invalid("source_url", err_codes::FIELD_INVALID));
    }
    let attrs = match &input.attrs {
        None => b.map_or_else(|| serde_json::json!({}), |b| b.attrs.clone()),
        Some(serde_json::Value::Null) => serde_json::json!({}),
        Some(v @ serde_json::Value::Object(_)) => {
            if serde_json::to_vec(v).map_or(usize::MAX, |b| b.len()) > ATTRS_MAX_BYTES {
                return Err(invalid("attrs", too_long(ATTRS_MAX_BYTES)));
            }
            v.clone()
        }
        Some(_) => return Err(invalid("attrs", err_codes::FIELD_INVALID)),
    };
    Ok(Fields {
        label,
        external_id,
        category,
        planned_start,
        planned_end,
        actual_start,
        actual_end,
        confidence,
        source_url,
        attrs,
    })
}

/// Who writes: origin + provenance (D184), both from server-side values.
#[derive(Debug, Clone)]
pub struct Writer {
    pub origin: RangeOrigin,
    pub source_ref: Option<String>,
}

pub async fn find<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    id: Uuid,
) -> Result<time_ranges::Model, RangeError> {
    time_ranges::Entity::find_by_id(id)
        .filter(time_ranges::Column::OrgId.eq(org_id))
        .one(db)
        .await?
        .ok_or(RangeError::NotFound)
}

async fn by_external_id<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    scope: &Scope,
    external_id: &str,
) -> Result<Option<time_ranges::Model>, DbErr> {
    time_ranges::Entity::find()
        .filter(time_ranges::Column::OrgId.eq(org_id))
        .filter(time_ranges::Column::ScopeKind.eq(scope.kind.wire()))
        .filter(time_ranges::Column::ScopeId.eq(scope.id.as_str()))
        .filter(time_ranges::Column::ExternalId.eq(external_id))
        .one(db)
        .await
}

fn apply(am: &mut time_ranges::ActiveModel, f: Fields) {
    am.label = Set(f.label);
    am.external_id = Set(f.external_id);
    am.category = Set(f.category);
    am.planned_start = Set(f.planned_start);
    am.planned_end = Set(f.planned_end);
    am.actual_start = Set(f.actual_start);
    am.actual_end = Set(f.actual_end);
    am.confidence = Set(f.confidence);
    am.source_url = Set(f.source_url);
    am.attrs = Set(f.attrs);
    am.updated_at = Set(chrono::Utc::now().into());
}

/// Creates a range, or — when `input.external_id` names an existing range
/// of the same scope — merges `input` onto it. `bool` = created. The scope
/// must come from [`resolve_scope`].
pub async fn upsert<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    scope: &Scope,
    input: &TimeRangeInput,
    writer: &Writer,
) -> Result<(time_ranges::Model, bool), RangeError> {
    let external_id = input
        .external_id
        .as_deref()
        .map(str::trim)
        .filter(|e| !e.is_empty());
    // Two attempts: a concurrent insert of the same key turns into an update.
    for _ in 0..2 {
        let existing = match external_id {
            Some(e) => by_external_id(db, org_id, scope, e).await?,
            None => None,
        };
        let fields = clean(input, existing.as_ref())?;
        if let Some(row) = existing {
            let mut am: time_ranges::ActiveModel = row.into();
            apply(&mut am, fields);
            am.origin = Set(writer.origin.wire().to_string());
            am.source_ref = Set(writer.source_ref.clone());
            return Ok((am.update(db).await?, false));
        }
        let now: sea_orm::prelude::DateTimeWithTimeZone = chrono::Utc::now().into();
        let mut am = time_ranges::ActiveModel {
            id: Set(Uuid::new_v4()),
            org_id: Set(org_id),
            scope_kind: Set(scope.kind.wire().to_string()),
            scope_id: Set(scope.id.clone()),
            origin: Set(writer.origin.wire().to_string()),
            source_ref: Set(writer.source_ref.clone()),
            created_at: Set(now),
            ..Default::default()
        };
        apply(&mut am, fields);
        match am.insert(db).await {
            Ok(row) => return Ok((row, true)),
            Err(e) if matches!(e.sql_err(), Some(SqlErr::UniqueConstraintViolation(_))) => continue,
            Err(e) => return Err(e.into()),
        }
    }
    Err(RangeError::Db(DbErr::Custom(
        "time range upsert raced twice".into(),
    )))
}

/// Updates a range of the org (scope, origin and provenance unchanged).
pub async fn update<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    id: Uuid,
    input: &TimeRangeInput,
) -> Result<time_ranges::Model, RangeError> {
    let row = find(db, org_id, id).await?;
    let fields = clean(input, Some(&row))?;
    if let Some(e) = fields.external_id.as_deref() {
        let scope = Scope {
            kind: ScopeKind::from_wire(&row.scope_kind).unwrap_or(ScopeKind::Org),
            id: row.scope_id.clone(),
        };
        if by_external_id(db, org_id, &scope, e)
            .await?
            .is_some_and(|other| other.id != row.id)
        {
            return Err(invalid("external_id", "unique"));
        }
    }
    let mut am: time_ranges::ActiveModel = row.into();
    apply(&mut am, fields);
    Ok(am.update(db).await?)
}

pub async fn delete<C: ConnectionTrait>(db: &C, org_id: i64, id: Uuid) -> Result<(), RangeError> {
    let row = find(db, org_id, id).await?;
    time_ranges::Entity::delete_by_id(row.id).exec(db).await?;
    Ok(())
}

/// List filter: an optional scope (already resolved) and time window.
#[derive(Debug, Clone, Default)]
pub struct ListFilter {
    pub scope: Option<Scope>,
    pub from: Option<DateTime<FixedOffset>>,
    pub to: Option<DateTime<FixedOffset>>,
}

/// Ranges of the org whose planned or actual interval meets `[from, to)`,
/// by start (actual when known, else planned).
pub fn select(org_id: i64, f: &ListFilter) -> Select<time_ranges::Entity> {
    use time_ranges::Column as C;
    let mut q = time_ranges::Entity::find().filter(C::OrgId.eq(org_id));
    if let Some(s) = &f.scope {
        q = q
            .filter(C::ScopeKind.eq(s.kind.wire()))
            .filter(C::ScopeId.eq(s.id.as_str()));
    }
    let pair = |start: C, end: C| {
        let mut c = Condition::all();
        if let Some(to) = f.to {
            c = c.add(start.lt(to));
        }
        if let Some(from) = f.from {
            c = c.add(end.gt(from));
        }
        c
    };
    if f.from.is_some() || f.to.is_some() {
        q = q.filter(
            Condition::any()
                .add(pair(C::PlannedStart, C::PlannedEnd))
                .add(pair(C::ActualStart, C::ActualEnd)),
        );
    }
    q.order_by(
        Expr::cust("COALESCE(actual_start, planned_start)"),
        Order::Asc,
    )
    .order_by_asc(C::Id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(ps: &str, pe: &str) -> TimeRangeInput {
        TimeRangeInput {
            label: Some(" Le 7/9 ".into()),
            planned_start: Some(ps.into()),
            planned_end: Some(pe.into()),
            ..Default::default()
        }
    }

    fn token(r: Result<Fields, RangeError>) -> (String, String) {
        match r {
            Err(RangeError::Invalid { field, token }) => (field, token),
            other => panic!("expected a field error, got {other:?}"),
        }
    }

    #[test]
    fn clean_checks_pairs_urls_and_attrs() {
        let ok = clean(
            &input("2026-10-12T07:00:00+02:00", "2026-10-12T09:00:00+02:00"),
            None,
        )
        .expect("valid");
        assert_eq!(ok.label, "Le 7/9");
        assert_eq!(ok.attrs, serde_json::json!({}));
        assert_eq!(
            token(clean(
                &input("2026-10-12T09:00:00Z", "2026-10-12T09:00:00Z"),
                None
            )),
            ("planned_end".into(), "invalid".into())
        );
        assert_eq!(
            token(clean(&input("2026-10-12T09:00:00Z", ""), None)),
            ("planned_end".into(), "required".into())
        );
        assert_eq!(
            token(clean(&input("", ""), None)),
            ("planned_start".into(), "required".into())
        );
        assert_eq!(
            token(clean(&input("demain", "2026-10-12T09:00:00Z"), None)),
            ("planned_start".into(), "invalid".into())
        );
        let mut bad_url = input("2026-10-12T07:00:00Z", "2026-10-12T09:00:00Z");
        bad_url.source_url = Some("javascript:alert(1)".into());
        assert_eq!(
            token(clean(&bad_url, None)),
            ("source_url".into(), "invalid".into())
        );
        let mut big = input("2026-10-12T07:00:00Z", "2026-10-12T09:00:00Z");
        big.attrs = Some(serde_json::json!({ "x": "a".repeat(ATTRS_MAX_BYTES) }));
        assert_eq!(
            token(clean(&big, None)),
            ("attrs".into(), "max_length:8192".into())
        );
        let mut not_obj = input("2026-10-12T07:00:00Z", "2026-10-12T09:00:00Z");
        not_obj.attrs = Some(serde_json::json!([1]));
        assert_eq!(
            token(clean(&not_obj, None)),
            ("attrs".into(), "invalid".into())
        );
        let mut conf = input("2026-10-12T07:00:00Z", "2026-10-12T09:00:00Z");
        conf.confidence = Some(1.5);
        assert_eq!(
            token(clean(&conf, None)),
            ("confidence".into(), "invalid".into())
        );
        // Only the actual pair is enough.
        let actual = TimeRangeInput {
            label: Some("x".into()),
            actual_start: Some("2026-10-12T07:01:00Z".into()),
            actual_end: Some("2026-10-12T08:59:00Z".into()),
            ..Default::default()
        };
        assert!(clean(&actual, None).is_ok());
    }
}
