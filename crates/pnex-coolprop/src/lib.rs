//! Wrapper safe autour de l'API C de CoolProp (v8.0.0), liée statiquement
//! dans le process par [`pnex_coolprop_sys`](pnex_coolprop_sys).
//!
//! Toutes les fonctions sont réexportées à la racine du crate
//! (`use pnex_coolprop::props_si;`). L'implémentation vit dans [`safe`].
//!
//! Origine : adapté du projet coolprop-rs (MIT, même auteur) — le serveur
//! HTTP (`coolprop-server`) est remplacé ici par un appel direct in-process.
//!
//! # Contraintes d'usage
//!
//! * CoolProp garde un état **process-global** (config, états de référence,
//!   registre des handles AbstractState, chaîne d'erreur) : toutes les
//!   fonctions sont sérialisées derrière un mutex global, et les handles
//!   `AbstractState` sont perdus au redémarrage du process.
//! * La configuration / les états de référence sont globaux : ne pas les
//!   muter depuis un serveur multi-tenant sans verrouiller l'usage.

pub mod plot;
pub mod safe;

pub use plot::*;
pub use safe::*;
