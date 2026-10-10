//! Idempotent seed of the global catalog: the device catalog comes from the
//! typed registry `pnex_core::catalog` (D121); tiers, conversions and
//! formulas from the YAML fixtures under `fixtures/`.
//!
//! Usage : `cargo loco task seed` (depuis crates/pnex-backend).

use loco_rs::prelude::*;
use pnex_core::catalog;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DatabaseBackend, DatabaseConnection,
    DatabaseTransaction, EntityTrait, QueryFilter, Set, Statement, TransactionTrait,
};

use crate::models::_entities::sea_orm_active_enums::{CapabilityMode, ConversionKind, FormulaKind};
use crate::models::_entities::{
    device_capabilities, device_types, formulas, mcu_boards, organizations,
    predefined_device_capabilities, predefined_devices, subscription_tiers, unit_conversions,
};

pub struct Seed;

#[async_trait]
impl Task for Seed {
    fn task(&self) -> TaskInfo {
        TaskInfo {
            name: "seed".to_string(),
            detail: "Idempotent seed of the global catalog (device catalog from code, the rest from the YAML fixtures): device types, capabilities, MCU boards, predefined devices, tiers, global conversions, global formulas.\nUsage:\ncargo loco task seed".to_string(),
        }
    }

    async fn run(&self, ctx: &AppContext, _vars: &task::Vars) -> Result<()> {
        // The dev task always seeds tiers (SaaS-like local stack).
        seed_catalog(&ctx.db, std::path::Path::new("fixtures"), true).await?;
        println!("✅ Seed done (idempotent: re-runnable without side effects)");
        Ok(())
    }
}

/// Idempotent upsert of the global catalog: the device catalog from
/// `pnex_core::catalog`, the rest from the YAML fixtures under `base`. `with_tiers = false` skips subscription tiers: in self-hosted
/// mode no tier exists, so new orgs get none and no quota applies.
///
/// Runs in a single transaction guarded by a Postgres transaction-scoped
/// advisory lock: with several API replicas seeding at boot, the others wait
/// for the first commit, then find the rows already there (no duplicates).
/// The lock is released on commit/rollback, even if the process dies.
pub async fn seed_catalog(
    db: &DatabaseConnection,
    base: &std::path::Path,
    with_tiers: bool,
) -> Result<()> {
    let txn = db.begin().await?;
    txn.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "SELECT pg_advisory_xact_lock($1)",
        [SEED_LOCK_KEY.into()],
    ))
    .await?;
    seed_catalog_in(&txn, base, with_tiers).await?;
    txn.commit().await?;
    Ok(())
}

/// Advisory lock key of the catalog seed (arbitrary, stable i64).
const SEED_LOCK_KEY: i64 = 0x504e_4558_5345_4544; // "PNEXSEED"

async fn seed_catalog_in(db: &Db, base: &std::path::Path, with_tiers: bool) -> Result<()> {
    let n = seed_device_types(db).await?;
    tracing::info!(n, "seed: device types");
    let n = seed_capabilities(db).await?;
    tracing::info!(n, "seed: capabilities");
    let n = seed_mcu_boards(db).await?;
    tracing::info!(n, "seed: mcu boards");
    let n = seed_predefined_devices(db).await?;
    tracing::info!(n, "seed: predefined devices");
    if with_tiers {
        let n = seed_subscription_tiers(db, &base.join("subscriptions/subscription.yaml")).await?;
        tracing::info!(n, "seed: subscription tiers");
        let n = backfill_org_tiers(db).await?;
        tracing::info!(n, "seed: orgs without tier given the default tier");
    }

    let mut n = 0;
    for file in yaml_files(&base.join("conversions/global"))? {
        n += seed_global_conversions(db, &file).await?;
    }
    tracing::info!(n, "seed: global conversions");
    let mut n = 0;
    for file in yaml_files(&base.join("formulas/global"))? {
        n += seed_global_formulas(db, &file).await?;
    }
    tracing::info!(n, "seed: global formulas");

    // No fluid seed: the external CoolProp/RefProp service is the source
    // of truth; the database only keeps org-created custom mixtures.
    Ok(())
}

// ---------- helpers ----------

type Db = DatabaseTransaction;

fn read_yaml<T: for<'de> serde::Deserialize<'de>>(path: &std::path::Path) -> Result<Vec<T>> {
    let f = std::fs::File::open(path)
        .map_err(|e| Error::string(&format!("fixture {} illisible : {e}", path.display())))?;
    serde_yaml::from_reader(f)
        .map_err(|e| Error::string(&format!("fixture {} invalide : {e}", path.display())))
}

/// Liste triée des fichiers YAML d'un dossier de fixtures.
fn yaml_files(dir: &std::path::Path) -> Result<Vec<std::path::PathBuf>> {
    let mut files: Vec<_> = std::fs::read_dir(dir)
        .map_err(|e| Error::string(&format!("dossier {} illisible : {e}", dir.display())))?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "yaml"))
        .collect();
    files.sort();
    Ok(files)
}

/// Parses the UUID, else derives a
/// deterministic DNS uuid5 (stable across runs → idempotent update_or_create).
fn parse_global_id(raw: &str) -> Result<uuid::Uuid> {
    uuid::Uuid::parse_str(raw).or_else(|_| {
        Ok(uuid::Uuid::new_v5(
            &uuid::Uuid::NAMESPACE_DNS,
            raw.as_bytes(),
        ))
    })
}

/// "15 minutes" / "1 day" / "6 months" → seconds (month = 30 d,
/// year = 365 d).
fn parse_duration_secs(s: &str) -> Result<i64> {
    let s = s.trim().to_lowercase();
    let mut parts = s.split_whitespace();
    let amount: i64 = parts
        .next()
        .and_then(|n| n.parse().ok())
        .ok_or_else(|| Error::string(&format!("durée invalide : {s:?}")))?;
    let unit = parts.next().unwrap_or("seconds");
    let mult = match unit.trim_end_matches('s') {
        "second" => 1,
        "minute" => 60,
        "hour" => 3600,
        "day" => 86_400,
        "week" => 604_800,
        "month" => 2_592_000,
        "year" => 31_536_000,
        other => {
            return Err(Error::string(&format!(
                "unité de durée inconnue : {other:?}"
            )))
        }
    };
    Ok(amount * mult)
}

// ---------- device catalogue ----------
//
// Source of truth = the typed registry `pnex_core::catalog` (D121). Upsert
// by name, never prune: a row removed from the registry stays in the base
// (registered devices may still reference it).

async fn seed_device_types(db: &Db) -> Result<usize> {
    for t in catalog::DeviceType::ALL {
        let existing = device_types::Entity::find()
            .filter(device_types::Column::Name.eq(t.name()))
            .one(db)
            .await?;
        let mut am = existing.map_or_else(<device_types::ActiveModel as Default>::default, |m| {
            m.into_active_model()
        });
        am.name = Set(t.name().to_string());
        am.save(db).await?;
    }
    Ok(catalog::DeviceType::ALL.len())
}

async fn seed_capabilities(db: &Db) -> Result<usize> {
    for c in catalog::Capability::ALL {
        let mode = match c.mode() {
            catalog::CapabilityMode::Input => CapabilityMode::Input,
            catalog::CapabilityMode::Output => CapabilityMode::Output,
            catalog::CapabilityMode::InputOutput => CapabilityMode::InputOutput,
        };
        let existing = device_capabilities::Entity::find()
            .filter(device_capabilities::Column::Name.eq(c.name()))
            .one(db)
            .await?;
        let mut am = existing.map_or_else(
            <device_capabilities::ActiveModel as Default>::default,
            |m| m.into_active_model(),
        );
        am.name = Set(c.name().to_string());
        am.mode = Set(mode);
        am.save(db).await?;
    }
    Ok(catalog::Capability::ALL.len())
}

/// Board variants with their v2 profile (`mcu_boards.details`).
async fn seed_mcu_boards(db: &Db) -> Result<usize> {
    for b in catalog::boards() {
        let existing = mcu_boards::Entity::find()
            .filter(mcu_boards::Column::Name.eq(b.name))
            .one(db)
            .await?;
        let mut am = existing.map_or_else(<mcu_boards::ActiveModel as Default>::default, |m| {
            m.into_active_model()
        });
        am.name = Set(b.name.to_string());
        am.soc = Set(b.soc_str().to_string());
        am.pretty_name = Set(b.pretty_name.map(str::to_string));
        am.pio_board = Set(b.pio_board.map(str::to_string));
        if let Some(profile) = b.profile {
            am.details = Set(Some(serde_json::to_value(profile())?));
        }
        am.save(db).await?;
    }
    Ok(catalog::boards().len())
}

async fn seed_predefined_devices(db: &Db) -> Result<usize> {
    for p in catalog::products() {
        let device_type = device_types::Entity::find()
            .filter(device_types::Column::Name.eq(p.device_type.name()))
            .one(db)
            .await?
            .ok_or_else(|| {
                Error::string(&format!("device_type {} not seeded", p.device_type.name()))
            })?;
        let board = mcu_boards::Entity::find()
            .filter(mcu_boards::Column::Name.eq(p.board.name))
            .one(db)
            .await?
            .ok_or_else(|| Error::string(&format!("board {} not seeded", p.board.name)))?;

        let existing = predefined_devices::Entity::find()
            .filter(predefined_devices::Column::Name.eq(p.name))
            .one(db)
            .await?;
        let is_update = existing.is_some();
        let mut am = existing
            .map_or_else(<predefined_devices::ActiveModel as Default>::default, |m| {
                m.into_active_model()
            });
        let shop = p.shop;
        am.name = Set(p.name.to_string());
        am.pretty_name = Set(Some(p.pretty_name.to_string()));
        am.revision = Set(p.revision.to_string());
        am.device_type_id = Set(device_type.id);
        am.board_id = Set(board.id);
        am.device_doc_url = Set(shop.map(|s| s.device_doc_url.to_string()));
        am.prestashop_product_id = Set(shop.map(|s| s.prestashop_product_id.to_string()));
        am.prestashop_buy_url = Set(shop.map(|s| s.prestashop_buy_url.to_string()));
        am.byod_doc_url = Set(shop.map(|s| s.byod_doc_url.to_string()));
        am.image_source_url = Set(shop.map(|s| s.image_source_url.to_string()));
        am.stl_files_url = Set(shop.map(|s| s.stl_files_url.to_string()));
        am.description = Set(Some(p.description.to_string()));
        am.description_i18n = Set(Some(p.description_i18n.to_string()));
        am.peripherals = Set(None);
        let device = if is_update {
            am.update(db).await?
        } else {
            am.insert(db).await?
        };

        // M2M: replace the capability links (idempotent).
        predefined_device_capabilities::Entity::delete_many()
            .filter(predefined_device_capabilities::Column::PredefinedDeviceId.eq(device.id))
            .exec(db)
            .await?;
        for c in p.capabilities {
            let cap = device_capabilities::Entity::find()
                .filter(device_capabilities::Column::Name.eq(c.name()))
                .one(db)
                .await?
                .ok_or_else(|| Error::string(&format!("capability {} not seeded", c.name())))?;
            predefined_device_capabilities::ActiveModel {
                predefined_device_id: Set(device.id),
                device_capability_id: Set(cap.id),
                created_at: sea_orm::ActiveValue::NotSet,
                updated_at: sea_orm::ActiveValue::NotSet,
            }
            .insert(db)
            .await?;
        }
    }
    Ok(catalog::products().len())
}

// ---------- subscription tiers ----------

/// Gives the default tier to organizations that have none (created before
/// the tiers existed, e.g. an instance switched from self-hosted to SaaS):
/// in SaaS an org without tier has no limits at all. Idempotent.
async fn backfill_org_tiers(db: &Db) -> Result<u64> {
    let Some(tier_id) = crate::controllers::orgs::default_org_tier(db).await else {
        return Ok(0);
    };
    let res = organizations::Entity::update_many()
        .col_expr(
            organizations::Column::SubscriptionTierId,
            sea_orm::sea_query::Expr::value(tier_id),
        )
        .filter(organizations::Column::SubscriptionTierId.is_null())
        .exec(db)
        .await?;
    Ok(res.rows_affected)
}

async fn seed_subscription_tiers(db: &Db, path: &std::path::Path) -> Result<usize> {
    #[derive(serde::Deserialize)]
    struct Row {
        name: String,
        max_sensor_devices: i32,
        max_actuator_devices: i32,
        max_mixed_devices: i32,
        min_build_interval: String,
        data_retention: Option<String>,
        /// Telemetry storage quota (MB); absent = unlimited.
        #[serde(default)]
        max_telemetry_mb: Option<i64>,
    }
    let rows: Vec<Row> = read_yaml(path)?;
    for r in &rows {
        let existing = subscription_tiers::Entity::find()
            .filter(subscription_tiers::Column::Name.eq(&r.name))
            .one(db)
            .await?;
        let mut am = existing
            .map_or_else(<subscription_tiers::ActiveModel as Default>::default, |m| {
                m.into_active_model()
            });
        am.name = Set(r.name.clone());
        am.max_sensor_devices = Set(r.max_sensor_devices);
        am.max_actuator_devices = Set(r.max_actuator_devices);
        am.max_mixed_devices = Set(r.max_mixed_devices);
        am.min_build_interval_secs = Set(parse_duration_secs(&r.min_build_interval)?);
        am.data_retention_secs = Set(r
            .data_retention
            .as_deref()
            .map(parse_duration_secs)
            .transpose()?);
        am.max_telemetry_mb = Set(r.max_telemetry_mb);
        am.save(db).await?;
    }
    Ok(rows.len())
}

// ---------- ETL global ----------

async fn seed_global_conversions(db: &Db, path: &std::path::Path) -> Result<usize> {
    #[derive(serde::Deserialize)]
    struct Row {
        global_id: String,
        name: String,
        version: Option<i32>,
        category: Option<String>,
        tags: Option<Vec<String>>,
        description: Option<String>,
        from_unit: String,
        to_unit: String,
        conversion_type: String,
        multiplier: Option<f64>,
        offset: Option<f64>,
        expression: Option<String>,
    }
    let rows: Vec<Row> = read_yaml(path)?;
    for r in &rows {
        let kind = match r.conversion_type.as_str() {
            "linear" => ConversionKind::Linear,
            "affine" => ConversionKind::Affine,
            "custom" => ConversionKind::Custom,
            other => {
                return Err(Error::string(&format!(
                    "conversion_type inconnu : {other:?}"
                )))
            }
        };
        let global_id = parse_global_id(&r.global_id)?;
        let existing = unit_conversions::Entity::find()
            .filter(unit_conversions::Column::GlobalId.eq(global_id))
            .one(db)
            .await?;
        let mut am = existing
            .map_or_else(<unit_conversions::ActiveModel as Default>::default, |m| {
                m.into_active_model()
            });
        am.org_id = Set(None);
        am.name = Set(r.name.clone());
        am.from_unit = Set(r.from_unit.clone());
        am.to_unit = Set(r.to_unit.clone());
        am.conversion_type = Set(kind);
        am.multiplier = Set(r.multiplier.unwrap_or(1.0));
        am.offset = Set(r.offset.unwrap_or(0.0));
        am.expression = Set(r.expression.clone());
        am.description = Set(r.description.clone());
        am.is_predefined = Set(true);
        am.global_id = Set(Some(global_id));
        am.version = Set(r.version.unwrap_or(1));
        am.category = Set(r.category.clone());
        am.tags = Set(r.tags.clone().map(serde_json::to_value).transpose()?);
        am.save(db).await?;
    }
    Ok(rows.len())
}

async fn seed_global_formulas(db: &Db, path: &std::path::Path) -> Result<usize> {
    #[derive(serde::Deserialize)]
    struct Row {
        global_id: String,
        name: String,
        version: Option<i32>,
        category: Option<String>,
        tags: Option<Vec<String>>,
        description: Option<String>,
        expression: String,
        result_unit: Option<String>,
        formula_type: String,
        #[serde(default)]
        fluid_config: Option<serde_json::Value>,
        #[serde(default)]
        compute_on_event: bool,
        #[serde(default = "default_cache_ttl")]
        cache_ttl: i32,
    }
    fn default_cache_ttl() -> i32 {
        60
    }
    let rows: Vec<Row> = read_yaml(path)?;
    for r in &rows {
        let kind = match r.formula_type.as_str() {
            "simple_math" => FormulaKind::SimpleMath,
            "fluid_property" => FormulaKind::FluidProperty,
            "power_calculation" => FormulaKind::PowerCalculation,
            "rate_of_change" => FormulaKind::RateOfChange,
            other => return Err(Error::string(&format!("formula_type inconnu : {other:?}"))),
        };
        let global_id = parse_global_id(&r.global_id)?;
        let existing = formulas::Entity::find()
            .filter(formulas::Column::GlobalId.eq(global_id))
            .one(db)
            .await?;
        let mut am = existing.map_or_else(<formulas::ActiveModel as Default>::default, |m| {
            m.into_active_model()
        });
        am.org_id = Set(None);
        am.name = Set(r.name.clone());
        am.description = Set(r.description.clone());
        am.formula_type = Set(kind);
        am.expression = Set(r.expression.clone());
        am.result_unit = Set(r.result_unit.clone());
        am.fluid_config = Set(r.fluid_config.clone());
        am.is_predefined = Set(true);
        am.global_id = Set(Some(global_id));
        am.version = Set(r.version.unwrap_or(1));
        am.category = Set(r.category.clone());
        am.tags = Set(r.tags.clone().map(serde_json::to_value).transpose()?);
        am.compute_on_event = Set(r.compute_on_event);
        am.cache_ttl = Set(r.cache_ttl);
        am.save(db).await?;
    }
    Ok(rows.len())
}
