//! ETL — conversions, formules (+ data sources), fluides.
//!
//! Modèle « sans copies » (divergence assumée vs Django) : les entités
//! fournies par l'app ont `org_id` NULL et sont **référencées directement**
//! par les orgs ; une ligne `org_id` n'existe que si l'org crée ou
//! personnalise la sienne. Les tables Django FormulaImport/ConversionImport
//! (copie par user + suivi de mise à jour) sont supprimées.
//!
//! NB : les FK nullable sont posées en SQL brut — le suffixe `?` des refs
//! `create_table` de loco-rs 1.0.1 ne produit ni colonne nullable ni
//! ON DELETE SET NULL (constaté par test, voir tests/schema_invariants.rs).

use loco_rs::schema::*;
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

const NULLABLE_FKS: &[(&str, &str, &str)] = &[
    // (table, colonne, table référencée)
    ("fluid_catalogs", "org_id", "organizations"),
    ("unit_conversions", "org_id", "organizations"),
    ("formulas", "org_id", "organizations"),
    ("formulas", "fluid_property_group_id", "fluid_property_groups"),
    ("formula_data_sources", "device_registry_id", "device_registries"),
    ("formula_data_sources", "unit_conversion_id", "unit_conversions"),
];

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        create_table(
            m,
            "fluid_property_group",
            &[
                ("id", ColType::PkAuto),
                ("name", ColType::StringLenUniq(50)),
                ("display_name", ColType::StringLen(100)),
                ("group_type", ColType::Enum(
                    "fluid_group_type".to_string(),
                    vec![
                        "basic_thermo".to_string(),
                        "specific_heat".to_string(),
                        "transport".to_string(),
                        "psychrometric".to_string(),
                        "all_thermo".to_string(),
                        "custom".to_string(),
                    ],
                )),
                ("property_codes", ColType::JsonBinaryNull),
                ("description", ColType::TextNull),
                ("default_fluid", ColType::StringLenNull(100)),
                ("cache_ttl", ColType::IntegerWithDefault(60)),
                ("is_predefined", ColType::BooleanWithDefault(false)),
            ],
            &[],
        )
        .await?;

        create_table(
            m,
            "fluid_catalog",
            &[
                ("id", ColType::PkAuto),
                // NULL = fluide fourni par l'app (catalogue global).
                ("org_id", ColType::BigIntegerNull),
                ("name", ColType::StringLen(100)),
                ("coolprop_name", ColType::StringLen(200)),
                ("category", ColType::Enum(
                    "fluid_category".to_string(),
                    vec![
                        "water".to_string(),
                        "refrigerant".to_string(),
                        "air".to_string(),
                        "hydrocarbon".to_string(),
                        "cryogenic".to_string(),
                        "mixture".to_string(),
                        "other".to_string(),
                    ],
                )),
                ("is_predefined", ColType::BooleanWithDefault(false)),
                ("description", ColType::TextNull),
                ("chemical_formula", ColType::StringLenNull(50)),
                ("cas_number", ColType::StringLenNull(50)),
                ("min_temperature_k", ColType::DoubleNull),
                ("max_temperature_k", ColType::DoubleNull),
                ("min_pressure_pa", ColType::DoubleNull),
                ("max_pressure_pa", ColType::DoubleNull),
            ],
            &[],
        )
        .await?;

        create_table(
            m,
            "unit_conversion",
            &[
                ("id", ColType::PkAuto),
                // NULL = conversion fournie par l'app, partagée sans copie.
                ("org_id", ColType::BigIntegerNull),
                ("name", ColType::StringLen(255)),
                ("from_unit", ColType::StringLen(50)),
                ("to_unit", ColType::StringLen(50)),
                ("conversion_type", ColType::Enum(
                    "conversion_kind".to_string(),
                    vec![
                        "linear".to_string(),
                        "affine".to_string(),
                        "custom".to_string(),
                    ],
                )),
                ("multiplier", ColType::DoubleWithDefault(1.0)),
                ("offset", ColType::DoubleWithDefault(0.0)),
                // Expression sûre évaluée côté Rust (parité safe_eval) — pas
                // de Python arbitraire comme côté Django.
                ("expression", ColType::TextNull),
                ("description", ColType::TextNull),
                ("is_predefined", ColType::BooleanWithDefault(false)),
                ("global_id", ColType::UuidNull),
                ("version", ColType::IntegerWithDefault(1)),
                ("category", ColType::StringLenNull(50)),
                ("tags", ColType::JsonBinaryNull),
                ("import_count", ColType::IntegerWithDefault(0)),
            ],
            &[],
        )
        .await?;

        create_table(
            m,
            "formula",
            &[
                ("id", ColType::PkAuto),
                // NULL = formule fournie par l'app, partagée sans copie.
                ("org_id", ColType::BigIntegerNull),
                ("fluid_property_group_id", ColType::BigIntegerNull),
                ("name", ColType::StringLen(255)),
                ("description", ColType::TextNull),
                ("formula_type", ColType::Enum(
                    "formula_kind".to_string(),
                    vec![
                        "simple_math".to_string(),
                        "fluid_property".to_string(),
                        "power_calculation".to_string(),
                        "rate_of_change".to_string(),
                    ],
                )),
                ("expression", ColType::Text),
                ("result_unit", ColType::TextNull),
                ("fluid_config", ColType::JsonBinaryNull),
                ("is_predefined", ColType::BooleanWithDefault(false)),
                ("global_id", ColType::UuidNull),
                ("version", ColType::IntegerWithDefault(1)),
                ("category", ColType::StringLenNull(50)),
                ("tags", ColType::JsonBinaryNull),
                ("import_count", ColType::IntegerWithDefault(0)),
                ("compute_on_event", ColType::BooleanWithDefault(false)),
                ("cache_ttl", ColType::IntegerWithDefault(60)),
                ("last_computed_at", ColType::TimestampWithTimeZoneNull),
            ],
            &[],
        )
        .await?;

        create_table(
            m,
            "formula_data_source",
            &[
                ("id", ColType::PkAuto),
                ("device_registry_id", ColType::BigIntegerNull),
                ("unit_conversion_id", ColType::BigIntegerNull),
                ("source_type", ColType::Enum(
                    "data_source_kind".to_string(),
                    vec!["device".to_string(), "constant".to_string()],
                )),
                ("measurement_name", ColType::StringLenNull(255)),
                ("constant_type", ColType::EnumNull(
                    "constant_kind".to_string(),
                    vec![
                        "number".to_string(),
                        "string".to_string(),
                        "boolean".to_string(),
                    ],
                )),
                ("constant_value", ColType::TextNull),
                ("variable_name", ColType::StringLen(100)),
                ("sort_order", ColType::IntegerWithDefault(0)),
            ],
            &[("formulas", "formula_id")],
        )
        .await?;

        // Doubles unique_together Django → index uniques partiels (NULL org =
        // ligne globale). global_id des formules est unique.
        let mut sql = String::from(
            "CREATE UNIQUE INDEX uniq_fluid_catalogs_org_coolprop ON fluid_catalogs (org_id, coolprop_name) WHERE org_id IS NOT NULL;
             CREATE UNIQUE INDEX uniq_fluid_catalogs_predefined_coolprop ON fluid_catalogs (coolprop_name) WHERE is_predefined;
             CREATE UNIQUE INDEX uniq_unit_conversions_org_units ON unit_conversions (org_id, from_unit, to_unit) WHERE org_id IS NOT NULL;
             CREATE UNIQUE INDEX uniq_unit_conversions_predefined_units ON unit_conversions (from_unit, to_unit) WHERE is_predefined;
             CREATE UNIQUE INDEX uniq_formulas_global_id ON formulas (global_id) WHERE global_id IS NOT NULL;
             CREATE UNIQUE INDEX uniq_formula_data_sources_formula_var ON formula_data_sources (formula_id, variable_name);",
        );
        for (table, col, ref_table) in NULLABLE_FKS {
            sql.push_str(&format!(
                "ALTER TABLE {table} ADD CONSTRAINT fk_{table}_{col}_to_{ref_table} FOREIGN KEY ({col}) REFERENCES {ref_table} (id) ON DELETE SET NULL;"
            ));
        }
        m.get_connection().execute_unprepared(&sql).await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        drop_table(m, "formula_data_source").await?;
        drop_table(m, "formula").await?;
        drop_table(m, "unit_conversion").await?;
        drop_table(m, "fluid_catalog").await?;
        drop_table(m, "fluid_property_group").await?;
        for e in [
                "data_source_kind",
                "formula_kind",
                "conversion_kind",
                "fluid_category",
                "fluid_group_type",
                "constant_kind",
            ] {
                drop_enum_type(m, e).await?;
            }
        Ok(())
    }
}
