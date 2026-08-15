//! Seed idempotent du catalogue global — réutilise les fixtures YAML Django
//! (bootstrap_db/data), copiées telles quelles dans `fixtures/`.
//!
//! Usage : `cargo loco task seed` (depuis crates/pnex-backend).

use loco_rs::prelude::*;
use sea_orm::{ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, Set};

use crate::models::_entities::{
    device_capabilities, device_types, fluid_catalogs, formulas, mcu_boards,
    predefined_device_capabilities, predefined_devices, subscription_tiers,
    unit_conversions,
};
use crate::models::_entities::sea_orm_active_enums::{
    CapabilityMode, ConversionKind, FluidCategory, FormulaKind,
};

pub struct Seed;

#[async_trait]
impl Task for Seed {
    fn task(&self) -> TaskInfo {
        TaskInfo {
            name: "seed".to_string(),
            detail: "Seed idempotent du catalogue global (fixtures YAML Django réutilisées) : device types, capabilities, MCU boards, predefined devices, tiers, conversions globales, formules globales, fluides.\nUsage :\ncargo loco task seed".to_string(),
        }
    }

    async fn run(&self, ctx: &AppContext, _vars: &task::Vars) -> Result<()> {
        let db = &ctx.db;
        let base = std::path::Path::new("fixtures");

        let n = seed_device_types(db, &base.join("devices/device_type.yaml")).await?;
        println!("  device types : {n}");
        let n = seed_simple_capabilities(db, &base.join("devices/device_cap.yaml")).await?;
        println!("  capabilities : {n}");
        let n = seed_mcu_boards(db, &base.join("devices/mcu.yaml")).await?;
        println!("  mcu boards : {n}");
        let n = seed_predefined_devices(db, &base.join("devices/predefined_device.yaml")).await?;
        println!("  predefined devices : {n}");
        let n = seed_subscription_tiers(db, &base.join("subscriptions/subscription.yaml")).await?;
        println!("  tiers : {n}");

        let mut n = 0;
        for file in yaml_files(&base.join("conversions/global"))? {
            n += seed_global_conversions(db, &file).await?;
        }
        println!("  conversions globales : {n}");
        let mut n = 0;
        for file in yaml_files(&base.join("formulas/global"))? {
            n += seed_global_formulas(db, &file).await?;
        }
        println!("  formules globales : {n}");
        let n = seed_fluids(db, &base.join("fluids/common_fluids.yaml")).await?;
        println!("  fluides : {n}");

        println!("✅ Seed terminé (idempotent : relançable sans effet de bord)");
        Ok(())
    }
}

// ---------- helpers ----------

type Db = DatabaseConnection;

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


/// Parité Django bootstrap_db : parse l'UUID sinon dérive un uuid5 DNS
/// déterministe (stable entre les runs → update_or_create idempotent).
fn parse_global_id(raw: &str) -> Result<uuid::Uuid> {
    uuid::Uuid::parse_str(raw).or_else(|_| {
        Ok(uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_DNS, raw.as_bytes()))
    })
}

/// « 15 minutes » / « 1 day » / « 6 months » → secondes (mois = 30 j,
/// an = 365 j — approximation des DurationField Django).
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
        other => return Err(Error::string(&format!("unité de durée inconnue : {other:?}"))),
    };
    Ok(amount * mult)
}

// ---------- device catalogue ----------

async fn seed_device_types(db: &Db, path: &std::path::Path) -> Result<usize> {
    #[derive(serde::Deserialize)]
    struct Row {
        name: String,
    }
    let rows: Vec<Row> = read_yaml(path)?;
    for r in &rows {
        let existing = device_types::Entity::find()
            .filter(device_types::Column::Name.eq(&r.name))
            .one(db)
            .await?;
        let mut am = existing.map_or_else(<device_types::ActiveModel as Default>::default, |m| {
            m.into_active_model()
        });
        am.name = Set(r.name.clone());
        am.save(db).await?;
    }
    Ok(rows.len())
}

async fn seed_simple_capabilities(db: &Db, path: &std::path::Path) -> Result<usize> {
    #[derive(serde::Deserialize)]
    struct Row {
        name: String,
        mode: Option<String>,
    }
    let rows: Vec<Row> = read_yaml(path)?;
    for r in &rows {
        let mode = match r.mode.as_deref() {
            None | Some("input") => CapabilityMode::Input,
            Some("output") => CapabilityMode::Output,
            Some("input_output") => CapabilityMode::InputOutput,
            Some(other) => {
                return Err(Error::string(&format!("mode capability inconnu : {other:?}")))
            }
        };
        let existing = device_capabilities::Entity::find()
            .filter(device_capabilities::Column::Name.eq(&r.name))
            .one(db)
            .await?;
        let mut am = existing.map_or_else(<device_capabilities::ActiveModel as Default>::default, |m| {
            m.into_active_model()
        });
        am.name = Set(r.name.clone());
        am.mode = Set(mode);
        am.save(db).await?;
    }
    Ok(rows.len())
}

async fn seed_mcu_boards(db: &Db, path: &std::path::Path) -> Result<usize> {
    #[derive(serde::Deserialize)]
    struct Row {
        name: String,
        soc: Option<String>,
    }
    let rows: Vec<Row> = read_yaml(path)?;
    for r in &rows {
        let existing = mcu_boards::Entity::find()
            .filter(mcu_boards::Column::Name.eq(&r.name))
            .one(db)
            .await?;
        let mut am =
            existing.map_or_else(<mcu_boards::ActiveModel as Default>::default, |m| m.into_active_model());
        am.name = Set(r.name.clone());
        am.soc = Set(r.soc.clone().unwrap_or_else(|| "esp32".to_string()));
        am.save(db).await?;
    }
    Ok(rows.len())
}

async fn seed_predefined_devices(db: &Db, path: &std::path::Path) -> Result<usize> {
    #[derive(serde::Deserialize)]
    struct Row {
        name: String,
        pretty_name: Option<String>,
        revision: Option<String>,
        device_type_name: String,
        capabilities_names: Vec<String>,
        board_name: String,
        device_doc_url: Option<String>,
        prestashop_product_id: Option<serde_json::Value>,
        prestashop_buy_url: Option<String>,
        byod_doc_url: Option<String>,
        image_source_url: Option<String>,
        stl_files_url: Option<String>,
        description: Option<String>,
    }
    let rows: Vec<Row> = read_yaml(path)?;
    for r in &rows {
        let device_type = device_types::Entity::find()
            .filter(device_types::Column::Name.eq(&r.device_type_name))
            .one(db)
            .await?
            .ok_or_else(|| {
                Error::string(&format!("device_type {} absent du seed", r.device_type_name))
            })?;

        // Quirk Django conservée : le board « generic » n'est pas dans mcu.yaml,
        // il est créé à la volée par get_or_create.
        let board = match mcu_boards::Entity::find()
            .filter(mcu_boards::Column::Name.eq(&r.board_name))
            .one(db)
            .await?
        {
            Some(b) => b,
            None => {
                mcu_boards::ActiveModel {
                    name: Set(r.board_name.clone()),
                    soc: Set("generic".to_string()),
                    ..Default::default()
                }
                .insert(db)
                .await?
            }
        };

        let existing = predefined_devices::Entity::find()
            .filter(predefined_devices::Column::Name.eq(&r.name))
            .one(db)
            .await?;
        let is_update = existing.is_some();
        let mut am = existing.map_or_else(
            <predefined_devices::ActiveModel as Default>::default,
            |m| m.into_active_model(),
        );
        am.name = Set(r.name.clone());
        am.pretty_name = Set(r.pretty_name.clone());
        am.revision = Set(r.revision.clone().unwrap_or_default());
        am.device_type_id = Set(device_type.id);
        am.board_id = Set(board.id);
        am.device_doc_url = Set(r.device_doc_url.clone());
        am.prestashop_product_id = Set(
            r.prestashop_product_id
                .as_ref()
                .map(|v| match v {
                    serde_json::Value::String(s) => s.clone(),
                    other => other.to_string(),
                }),
        );
        am.prestashop_buy_url = Set(r.prestashop_buy_url.clone());
        am.byod_doc_url = Set(r.byod_doc_url.clone());
        am.image_source_url = Set(r.image_source_url.clone());
        am.stl_files_url = Set(r.stl_files_url.clone());
        am.description = Set(r.description.clone());
        let device = if is_update {
            am.update(db).await?
        } else {
            am.insert(db).await?
        };

        // M2M : remplacement atomique des liens (idempotent).
        predefined_device_capabilities::Entity::delete_many()
            .filter(
                predefined_device_capabilities::Column::PredefinedDeviceId.eq(device.id),
            )
            .exec(db)
            .await?;
        for cap_name in &r.capabilities_names {
            let cap = device_capabilities::Entity::find()
                .filter(device_capabilities::Column::Name.eq(cap_name))
                .one(db)
                .await?
                .ok_or_else(|| {
                    Error::string(&format!("capability {cap_name} absente du seed"))
                })?;
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
    Ok(rows.len())
}

// ---------- subscription tiers ----------

async fn seed_subscription_tiers(db: &Db, path: &std::path::Path) -> Result<usize> {
    #[derive(serde::Deserialize)]
    struct Row {
        name: String,
        max_sensor_devices: i32,
        max_actuator_devices: i32,
        max_mixed_devices: i32,
        min_build_interval: String,
        data_retention: Option<String>,
    }
    let rows: Vec<Row> = read_yaml(path)?;
    for r in &rows {
        let existing = subscription_tiers::Entity::find()
            .filter(subscription_tiers::Column::Name.eq(&r.name))
            .one(db)
            .await?;
        let mut am = existing.map_or_else(<subscription_tiers::ActiveModel as Default>::default, |m| {
            m.into_active_model()
        });
        am.name = Set(r.name.clone());
        am.max_sensor_devices = Set(r.max_sensor_devices);
        am.max_actuator_devices = Set(r.max_actuator_devices);
        am.max_mixed_devices = Set(r.max_mixed_devices);
        am.min_build_interval_secs = Set(parse_duration_secs(&r.min_build_interval)?);
        am.data_retention_secs = Set(
            r.data_retention
                .as_deref()
                .map(parse_duration_secs)
                .transpose()?,
        );
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
                return Err(Error::string(&format!("conversion_type inconnu : {other:?}")))
            }
        };
        let global_id = parse_global_id(&r.global_id)?;
        let existing = unit_conversions::Entity::find()
            .filter(unit_conversions::Column::GlobalId.eq(global_id))
            .one(db)
            .await?;
        let mut am = existing.map_or_else(<unit_conversions::ActiveModel as Default>::default, |m| {
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
            other => {
                return Err(Error::string(&format!("formula_type inconnu : {other:?}")))
            }
        };
        let global_id = parse_global_id(&r.global_id)?;
        let existing = formulas::Entity::find()
            .filter(formulas::Column::GlobalId.eq(global_id))
            .one(db)
            .await?;
        let mut am =
            existing.map_or_else(<formulas::ActiveModel as Default>::default, |m| m.into_active_model());
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

async fn seed_fluids(db: &Db, path: &std::path::Path) -> Result<usize> {
    #[derive(serde::Deserialize)]
    struct Row {
        name: String,
        coolprop_name: String,
        category: String,
        description: Option<String>,
        chemical_formula: Option<String>,
        cas_number: Option<String>,
        min_temperature_k: Option<f64>,
        max_temperature_k: Option<f64>,
        min_pressure_pa: Option<f64>,
        max_pressure_pa: Option<f64>,
    }
    let rows: Vec<Row> = read_yaml(path)?;
    for r in &rows {
        let category = match r.category.as_str() {
            "water" => FluidCategory::Water,
            "refrigerant" => FluidCategory::Refrigerant,
            "air" => FluidCategory::Air,
            "hydrocarbon" => FluidCategory::Hydrocarbon,
            "cryogenic" => FluidCategory::Cryogenic,
            "mixture" => FluidCategory::Mixture,
            "other" => FluidCategory::Other,
            other => {
                return Err(Error::string(&format!("catégorie fluide inconnue : {other:?}")))
            }
        };
        let existing = fluid_catalogs::Entity::find()
            .filter(fluid_catalogs::Column::CoolpropName.eq(&r.coolprop_name))
            .filter(fluid_catalogs::Column::IsPredefined.eq(true))
            .one(db)
            .await?;
        let mut am = existing.map_or_else(<fluid_catalogs::ActiveModel as Default>::default, |m| {
            m.into_active_model()
        });
        am.org_id = Set(None);
        am.name = Set(r.name.clone());
        am.coolprop_name = Set(r.coolprop_name.clone());
        am.category = Set(category);
        am.is_predefined = Set(true);
        am.description = Set(r.description.clone());
        am.chemical_formula = Set(r.chemical_formula.clone());
        am.cas_number = Set(r.cas_number.clone());
        am.min_temperature_k = Set(r.min_temperature_k);
        am.max_temperature_k = Set(r.max_temperature_k);
        am.min_pressure_pa = Set(r.min_pressure_pa);
        am.max_pressure_pa = Set(r.max_pressure_pa);
        am.save(db).await?;
    }
    Ok(rows.len())
}
