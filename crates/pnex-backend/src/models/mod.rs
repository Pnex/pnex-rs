//! Modèles SeaORM — entities générées depuis le schéma (`cargo loco db
//! entities`, à régénérer après chaque migration). La logique métier
//! (hooks, validations) vit dans des modules services dédiés, pas ici.

pub mod _entities;
pub mod annotations;
pub mod build_records;
pub mod device_capabilities;
pub mod device_registries;
pub mod device_states;
pub mod device_tokens;
pub mod device_types;
pub mod firmware_artifacts;
pub mod fluid_mixtures;
pub mod formula_data_sources;
pub mod formulas;
pub mod mcu_boards;
pub mod openobserve_orgs;
pub mod organization_members;
pub mod organizations;
pub mod predefined_device_capabilities;
pub mod predefined_devices;
pub mod saved_views;
pub mod site_diagrams;
pub mod sites;
pub mod subscription_tiers;
pub mod svg_files;
pub mod unit_conversions;
pub mod user_profiles;
pub mod users;
