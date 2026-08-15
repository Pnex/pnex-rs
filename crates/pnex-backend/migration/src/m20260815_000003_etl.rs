//! ETL — conversions, formules (+ data sources), mélanges de fluides custom.
//!
//! Modèle « sans copies » (divergence assumée vs Django) : les entités
//! fournies par l'app ont `org_id` NULL et sont **référencées directement**
//! par les orgs ; une ligne `org_id` n'existe que si l'org crée ou
//! personnalise la sienne. Les tables Django FormulaImport/ConversionImport
//! (copie par user + suivi de mise à jour) sont supprimées.
//!
//! **Pas de catalogue de fluides en base** (directive) : les propriétés de
//! fluides passent par le service externe FastAPI (CoolProp/RefProp), qui est
//! la source de vérité — ses messages d'erreur sont renvoyés tels quels au
//! client. Seules les orgs définissent des **mélanges custom** (table
//! `fluid_mixtures`). Les FluidCatalog/FluidPropertyGroup Django (miroir
//! statique de FluidsList / config app) disparaissent.
//!
//! Références nullable = suffixe `?` sur le nom de la table référencée
//! (colonne nullable + ON DELETE SET NULL) ; ne pas redéclarer la colonne
//! dans `cols`, sinon la déclaration écrase celle générée par le ref.

use loco_rs::schema::*;
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        create_table(
            m,
            "fluid_mixture",
            &[
                ("id", ColType::PkAuto),
                ("name", ColType::StringLen(100)),
                ("description", ColType::TextNull),
                // Composition structurée du mélange, ex.
                // [{"fluid": "R32", "fraction": 0.5, "basis": "mole"}, …].
                // Le rendu vers la syntaxe du service fluids se fait côté
                // Rust, pas en base.
                ("composition", ColType::JsonBinary),
            ],
            // Mélange custom = toujours propriété d'une org (scoping D2).
            &[("organizations", "org_id")],
        )
        .await?;

        create_table(
            m,
            "unit_conversion",
            &[
                ("id", ColType::PkAuto),
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
            // NULL = conversion fournie par l'app, partagée sans copie.
            &[("organizations?", "org_id")],
        )
        .await?;

        create_table(
            m,
            "formula",
            &[
                ("id", ColType::PkAuto),
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
                // Le fluide est nommé dans l'expression même
                // (ex. PropsSI('H','T',t,'P',p,'Water')) — résolu à runtime
                // par le service fluids, sans référence catalogue en base.
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
            // NULL = formule fournie par l'app, partagée sans copie.
            &[("organizations?", "org_id")],
        )
        .await?;

        create_table(
            m,
            "formula_data_source",
            &[
                ("id", ColType::PkAuto),
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
            &[
                ("formulas", "formula_id"),
                ("device_registries?", "device_registry_id"),
                ("unit_conversions?", "unit_conversion_id"),
            ],
        )
        .await?;

        // Doubles unique_together Django → index uniques partiels (NULL org =
        // ligne globale). global_id des formules est unique.
        let sql = String::from(
            "CREATE UNIQUE INDEX uniq_fluid_mixtures_org_name ON fluid_mixtures (org_id, name);
             CREATE UNIQUE INDEX uniq_unit_conversions_org_units ON unit_conversions (org_id, from_unit, to_unit) WHERE org_id IS NOT NULL;
             CREATE UNIQUE INDEX uniq_unit_conversions_predefined_units ON unit_conversions (from_unit, to_unit) WHERE is_predefined;
             CREATE UNIQUE INDEX uniq_formulas_global_id ON formulas (global_id) WHERE global_id IS NOT NULL;
             CREATE UNIQUE INDEX uniq_formula_data_sources_formula_var ON formula_data_sources (formula_id, variable_name);",
        );
        m.get_connection().execute_unprepared(&sql).await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        drop_table(m, "formula_data_source").await?;
        drop_table(m, "formula").await?;
        drop_table(m, "unit_conversion").await?;
        drop_table(m, "fluid_mixture").await?;
        for e in [
                "data_source_kind",
                "formula_kind",
                "conversion_kind",
                "constant_kind",
            ] {
                drop_enum_type(m, e).await?;
            }
        Ok(())
    }
}
