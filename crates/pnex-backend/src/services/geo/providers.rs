//! Org geo providers (geo-layers.md §8, L16–L28): CRUD of
//! `/api/v1/geo/providers`, one default per capability and the basemaps of
//! the map. Like LLM providers (D119), nothing is seeded: an org without a
//! provider has no basemap, geocoding nor routing until it adds one.
//! School of `services::ai::providers`.

use std::collections::{BTreeMap, HashMap};

use pnex_core::err_codes;
use pnex_core::geo::{
    Basemap, GeoCapability, GeoProvider, GeoProviderInput, GeoProviderKind, DEFAULT_KEY_PARAM,
};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DatabaseConnection, DbErr, EntityTrait,
    QueryFilter, QueryOrder, Set, TransactionTrait,
};
use uuid::Uuid;

use crate::models::_entities::{geo_provider_defaults, geo_providers};
use crate::services::secrets::store::{self, StoreError, Writer};
use crate::services::secrets::{geo, Keyring};

const NAME_MAX: usize = 255;
const BASE_URL_MAX: usize = 2048;
const TIMEOUT_MS: std::ops::RangeInclusive<u32> = 500..=60_000;
/// Key of the dark style variant in a basemap's `params` (L24).
pub const STYLE_URL_DARK: &str = "style_url_dark";

/// Who writes: the vault writer and whether a value may be typed.
#[derive(Clone, Copy, Debug)]
pub struct Author {
    pub user_id: Option<i64>,
    /// Owner/admin of the org (D117).
    pub can_write_secrets: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    #[error("invalid {field}: {token}")]
    Invalid { field: &'static str, token: String },
    #[error("geo provider not found")]
    NotFound,
    #[error("geo provider name taken")]
    NameTaken,
    #[error(transparent)]
    Store(#[from] StoreError),
}

impl From<DbErr> for ProviderError {
    fn from(e: DbErr) -> Self {
        Self::Store(StoreError::Db(e))
    }
}

fn invalid(field: &'static str, token: impl Into<String>) -> ProviderError {
    ProviderError::Invalid {
        field,
        token: token.into(),
    }
}

fn max_len(field: &'static str, value: &str, max: usize) -> Result<(), ProviderError> {
    if value.chars().count() > max {
        return Err(invalid(
            field,
            format!("{}:{max}", err_codes::FIELD_MAX_LENGTH),
        ));
    }
    Ok(())
}

/// Absolute http(s) URL, accepted by the egress guard (R8). The guarded
/// resolver re-checks the resolved address at call time.
fn check_url(field: &'static str, raw: &str) -> Result<(), ProviderError> {
    max_len(field, raw, BASE_URL_MAX)?;
    let url = reqwest::Url::parse(raw).map_err(|_| invalid(field, err_codes::FIELD_INVALID))?;
    if !matches!(url.scheme(), "http" | "https") || url.cannot_be_a_base() {
        return Err(invalid(field, err_codes::FIELD_INVALID));
    }
    // URLs are returned to every member: credentials go through the vault
    // (R4, R16).
    if !url.username().is_empty() || url.password().is_some() {
        return Err(invalid(field, err_codes::FIELD_INVALID));
    }
    if let Some(code) = pnex_core::egress::check_url(&url) {
        return Err(invalid(field, code));
    }
    Ok(())
}

/// Checks an input; returns the trimmed name.
fn validate(input: &GeoProviderInput) -> Result<String, ProviderError> {
    let name = input.name.trim();
    if name.is_empty() {
        return Err(invalid("name", err_codes::FIELD_REQUIRED));
    }
    max_len("name", name, NAME_MAX)?;
    if input.capabilities.is_empty() {
        return Err(invalid("capabilities", err_codes::FIELD_REQUIRED));
    }
    let supported = input.kind.supported();
    if input.capabilities.iter().any(|c| !supported.contains(c)) {
        return Err(invalid("capabilities", err_codes::FIELD_INVALID));
    }
    if input
        .default_for
        .iter()
        .any(|c| !input.capabilities.contains(c))
    {
        return Err(invalid("default_for", err_codes::FIELD_INVALID));
    }
    check_url("base_url", input.base_url.trim())?;
    if let Some(k) = &input.key_param {
        let k = k.trim();
        if k.is_empty()
            || k.len() > 64
            || !k
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        {
            return Err(invalid("key_param", err_codes::FIELD_INVALID));
        }
    }
    if let Some(dark) = input.params.get(STYLE_URL_DARK) {
        check_url("params", dark)?;
    }
    if input.params.keys().any(|k| k.trim().is_empty()) {
        return Err(invalid("params", err_codes::FIELD_INVALID));
    }
    if !TIMEOUT_MS.contains(&input.timeout_ms) {
        return Err(invalid("timeout_ms", err_codes::FIELD_INVALID));
    }
    if input
        .rate_limit_per_s
        .is_some_and(|r| !(r.is_finite() && r > 0.0))
    {
        return Err(invalid("rate_limit_per_s", err_codes::FIELD_INVALID));
    }
    Ok(name.to_string())
}

pub async fn list<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
) -> Result<Vec<geo_providers::Model>, DbErr> {
    geo_providers::Entity::find()
        .filter(geo_providers::Column::OrgId.eq(org_id))
        .order_by_asc(geo_providers::Column::Name)
        .all(db)
        .await
}

pub async fn find<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    id: Uuid,
) -> Result<geo_providers::Model, ProviderError> {
    geo_providers::Entity::find_by_id(id)
        .filter(geo_providers::Column::OrgId.eq(org_id))
        .one(db)
        .await?
        .ok_or(ProviderError::NotFound)
}

/// `capability → provider` defaults of `org_id`.
async fn defaults<C: ConnectionTrait>(db: &C, org_id: i64) -> Result<HashMap<String, Uuid>, DbErr> {
    Ok(geo_provider_defaults::Entity::find()
        .filter(geo_provider_defaults::Column::OrgId.eq(org_id))
        .all(db)
        .await?
        .into_iter()
        .map(|d| (d.capability, d.provider_id))
        .collect())
}

/// Default provider of `org_id` for `cap`, if any.
pub async fn default_for<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    cap: GeoCapability,
) -> Result<Option<geo_providers::Model>, DbErr> {
    let Some(d) = geo_provider_defaults::Entity::find_by_id((org_id, cap.as_str().to_string()))
        .one(db)
        .await?
    else {
        return Ok(None);
    };
    geo_providers::Entity::find_by_id(d.provider_id)
        .one(db)
        .await
}

/// Points the defaults of `caps` at `id`; the capabilities of `id` that
/// have no default yet also get it (first provider of a capability).
/// Defaults on `id` for capabilities it lost are removed.
async fn write_defaults<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    id: Uuid,
    caps: &[GeoCapability],
    wanted: &[GeoCapability],
) -> Result<(), DbErr> {
    let current = defaults(db, org_id).await?;
    for cap in GeoCapability::ALL {
        let key = cap.as_str().to_string();
        let holder = current.get(&key).copied();
        let set = wanted.contains(&cap) || (caps.contains(&cap) && holder.is_none());
        if set && holder != Some(id) {
            geo_provider_defaults::Entity::delete_by_id((org_id, key.clone()))
                .exec(db)
                .await?;
            geo_provider_defaults::ActiveModel {
                org_id: Set(org_id),
                capability: Set(key),
                provider_id: Set(id),
            }
            .insert(db)
            .await?;
        } else if !caps.contains(&cap) && holder == Some(id) {
            geo_provider_defaults::Entity::delete_by_id((org_id, key))
                .exec(db)
                .await?;
        }
    }
    Ok(())
}

async fn name_taken<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    name: &str,
    except: Option<Uuid>,
) -> Result<bool, DbErr> {
    let mut q = geo_providers::Entity::find()
        .filter(geo_providers::Column::OrgId.eq(org_id))
        .filter(geo_providers::Column::Name.eq(name));
    if let Some(id) = except {
        q = q.filter(geo_providers::Column::Id.ne(id));
    }
    Ok(q.one(db).await?.is_some())
}

fn writer(org_id: i64, by: Author) -> Writer {
    Writer {
        org_id: Some(org_id),
        user_id: by.user_id,
    }
}

fn caps_json(caps: &[GeoCapability]) -> serde_json::Value {
    serde_json::to_value(caps).unwrap_or_default()
}

fn map_json(m: &BTreeMap<String, String>) -> serde_json::Value {
    serde_json::to_value(m).unwrap_or_default()
}

fn key_param(input: &GeoProviderInput) -> String {
    input
        .key_param
        .as_deref()
        .map(str::trim)
        .unwrap_or(DEFAULT_KEY_PARAM)
        .to_string()
}

/// Creates a provider; the API key is optional (public engines need none).
pub async fn create(
    db: &DatabaseConnection,
    ring: &Keyring,
    org_id: i64,
    by: Author,
    input: &GeoProviderInput,
) -> Result<geo_providers::Model, ProviderError> {
    let name = validate(input)?;
    let txn = db.begin().await?;
    if name_taken(&txn, org_id, &name, None).await? {
        return Err(ProviderError::NameTaken);
    }
    let id = Uuid::new_v4();
    let secret = match &input.api_key {
        None => None,
        Some(key) => {
            geo::save(
                &txn,
                ring,
                writer(org_id, by),
                by.can_write_secrets,
                id,
                &name,
                None,
                None,
                Some(key),
            )
            .await?
        }
    };
    let now: sea_orm::prelude::DateTimeWithTimeZone = chrono::Utc::now().into();
    let row = geo_providers::ActiveModel {
        id: Set(id),
        org_id: Set(org_id),
        name: Set(name),
        kind: Set(input.kind.as_str().to_string()),
        capabilities: Set(caps_json(&input.capabilities)),
        base_url: Set(input.base_url.trim().to_string()),
        secret_id: Set(secret),
        key_param: Set(key_param(input)),
        params: Set(map_json(&input.params)),
        rate_limit_per_s: Set(input.rate_limit_per_s),
        timeout_ms: Set(input.timeout_ms as i32),
        store_allowed: Set(input.store_allowed),
        created_by: Set(by.user_id),
        updated_by: Set(by.user_id),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(&txn)
    .await?;
    write_defaults(&txn, org_id, id, &input.capabilities, &input.default_for).await?;
    txn.commit().await?;
    Ok(row)
}

/// Updates a provider; `api_key: None` keeps the current key.
pub async fn update(
    db: &DatabaseConnection,
    ring: &Keyring,
    org_id: i64,
    by: Author,
    id: Uuid,
    input: &GeoProviderInput,
) -> Result<geo_providers::Model, ProviderError> {
    let name = validate(input)?;
    let txn = db.begin().await?;
    let row = find(&txn, org_id, id).await?;
    if name_taken(&txn, org_id, &name, Some(id)).await? {
        return Err(ProviderError::NameTaken);
    }
    let secret = if input.api_key.is_some() || row.secret_id.is_some() {
        geo::save(
            &txn,
            ring,
            writer(org_id, by),
            by.can_write_secrets,
            id,
            &name,
            Some(&row.name),
            row.secret_id,
            input.api_key.as_ref(),
        )
        .await?
    } else {
        None
    };
    let mut am: geo_providers::ActiveModel = row.into();
    am.name = Set(name);
    am.kind = Set(input.kind.as_str().to_string());
    am.capabilities = Set(caps_json(&input.capabilities));
    am.base_url = Set(input.base_url.trim().to_string());
    am.secret_id = Set(secret);
    am.key_param = Set(key_param(input));
    am.params = Set(map_json(&input.params));
    am.rate_limit_per_s = Set(input.rate_limit_per_s);
    am.timeout_ms = Set(input.timeout_ms as i32);
    am.store_allowed = Set(input.store_allowed);
    am.updated_by = Set(by.user_id);
    let row = am.update(&txn).await?;
    write_defaults(&txn, org_id, id, &input.capabilities, &input.default_for).await?;
    txn.commit().await?;
    Ok(row)
}

/// Deletes a provider, its defaults (FK cascade) and its dedicated secret.
pub async fn delete(db: &DatabaseConnection, org_id: i64, id: Uuid) -> Result<(), ProviderError> {
    let txn = db.begin().await?;
    let row = find(&txn, org_id, id).await?;
    if row.secret_id.is_some() {
        geo::release(&txn, Some(org_id), id, &row.name).await?;
    }
    geo_providers::Entity::delete_by_id(id).exec(&txn).await?;
    txn.commit().await?;
    Ok(())
}

/// Parsed capabilities of a row (unknown values dropped).
pub fn caps_of(r: &geo_providers::Model) -> Vec<GeoCapability> {
    r.capabilities
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|v| v.as_str().and_then(GeoCapability::parse))
        .collect()
}

pub fn strings_of(v: &serde_json::Value) -> BTreeMap<String, String> {
    serde_json::from_value(v.clone()).unwrap_or_default()
}

/// Read models of `org_id`'s providers (secret = reference, never a value).
pub async fn views<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    rows: &[geo_providers::Model],
) -> Result<Vec<GeoProvider>, StoreError> {
    let ids: Vec<Uuid> = rows.iter().filter_map(|r| r.secret_id).collect();
    let names = store::names_of(db, Some(org_id), &ids).await?;
    let defaults = defaults(db, org_id).await?;
    Ok(rows
        .iter()
        .filter_map(|r| {
            let api_key = r.secret_id.and_then(|id| {
                Some(pnex_core::SecretFieldView {
                    secret_id: id,
                    name: names.get(&id)?.clone(),
                })
            });
            Some(GeoProvider {
                id: r.id,
                name: r.name.clone(),
                kind: GeoProviderKind::parse(&r.kind)?,
                capabilities: caps_of(r),
                base_url: r.base_url.clone(),
                api_key,
                key_param: r.key_param.clone(),
                params: strings_of(&r.params),
                rate_limit_per_s: r.rate_limit_per_s,
                timeout_ms: r.timeout_ms.max(0) as u32,
                store_allowed: r.store_allowed,
                default_for: GeoCapability::ALL
                    .into_iter()
                    .filter(|c| defaults.get(c.as_str()) == Some(&r.id))
                    .collect(),
                updated_at: r.updated_at.to_rfc3339(),
            })
        })
        .collect())
}

/// Appends the API key to `url` as `param` (no-op without a key).
pub fn with_key(url: &str, param: &str, key: Option<&str>) -> String {
    match (key, reqwest::Url::parse(url)) {
        (Some(key), Ok(mut u)) => {
            u.query_pairs_mut().append_pair(param, key);
            u.to_string()
        }
        _ => url.to_string(),
    }
}

/// Basemaps of `org_id` for the map. A basemap key is a browser key (like
/// a Google Maps key, restricted by referrer at the provider): it is
/// appended to the style URLs the browser loads.
pub async fn basemaps<C: ConnectionTrait>(
    db: &C,
    ring: &Keyring,
    org_id: i64,
) -> Result<Vec<Basemap>, StoreError> {
    let default = defaults(db, org_id)
        .await?
        .get(GeoCapability::Basemap.as_str())
        .copied();
    let mut out = Vec::new();
    for r in list(db, org_id).await? {
        if !caps_of(&r).contains(&GeoCapability::Basemap) {
            continue;
        }
        let key = match r.secret_id {
            Some(id) => Some(store::reveal(db, ring, Some(org_id), id).await?),
            None => None,
        };
        let key = key.as_deref();
        out.push(Basemap {
            id: r.id,
            style_url_dark: strings_of(&r.params)
                .remove(STYLE_URL_DARK)
                .map(|u| with_key(&u, &r.key_param, key)),
            is_default: default == Some(r.id),
            style_url: with_key(&r.base_url, &r.key_param, key),
            name: r.name,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(kind: GeoProviderKind, caps: &[GeoCapability]) -> GeoProviderInput {
        GeoProviderInput {
            name: "main".into(),
            kind,
            capabilities: caps.to_vec(),
            base_url: "https://geo.example.org".into(),
            api_key: None,
            key_param: None,
            params: BTreeMap::new(),
            rate_limit_per_s: None,
            timeout_ms: 5_000,
            store_allowed: false,
            default_for: vec![],
        }
    }

    #[test]
    fn validation_rejects_invalid_fields() {
        use GeoCapability::{Geocode, Reverse, Route};
        use GeoProviderKind::{Nominatim, Valhalla};
        assert!(validate(&input(Nominatim, &[Geocode, Reverse])).is_ok());
        assert!(
            validate(&input(Nominatim, &[Route])).is_err(),
            "unsupported cap"
        );
        assert!(validate(&input(Nominatim, &[])).is_err(), "no cap");
        let mut i = input(Nominatim, &[Geocode]);
        i.default_for = vec![Reverse];
        assert!(validate(&i).is_err(), "default outside capabilities");
        let mut i = input(Valhalla, &[Route]);
        i.base_url = "ftp://x".into();
        assert!(validate(&i).is_err(), "scheme");
        let mut i = input(Valhalla, &[Route]);
        i.base_url = "https://user:pw@geo.example.org".into();
        assert!(validate(&i).is_err(), "credentials in the URL");
        let mut i = input(Valhalla, &[Route]);
        i.key_param = Some("a b".into());
        assert!(validate(&i).is_err(), "key parameter name");
        let mut i = input(Valhalla, &[Route]);
        i.timeout_ms = 10;
        assert!(validate(&i).is_err(), "timeout");
    }

    #[test]
    fn key_is_appended_as_query_parameter() {
        assert_eq!(
            with_key("https://t.example/style.json?lang=fr", "key", Some("k&1")),
            "https://t.example/style.json?lang=fr&key=k%261"
        );
        assert_eq!(
            with_key("https://t.example/s", "key", None),
            "https://t.example/s"
        );
    }
}
