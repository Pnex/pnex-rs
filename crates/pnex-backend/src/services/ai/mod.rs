//! Assistant IA intégré (2026-09-06) — chat multi-tools, connecteur par
//! organisation, lecture OpenObserve, création/modification de **drafts**
//! de flows.
//!
//! Garde-fous transversaux (décisions A1-A5, 2026-09-06) :
//! - la surface exécutable par le LLM est **exactement**
//!   [`crate::services::ai::tools::execute`] — un `match` fermé sur 10
//!   outils lecture/écriture-draft ; pas de deploy, pas de suppression,
//!   pas de commande device, aucun outil « générique » (http/sql) ;
//! - la config LLM se résout **env > connecteur d'org > désactivé**
//!   (`config::resolve`), kill-switch `settings.ai.enabled` ;
//! - le chemin `flow_supervisor` / `POST /flows/{id}/deploy` n'est
//!   référencé nulle part dans ce module.

pub mod agent;
pub mod config;
pub mod context;
pub mod conversations;
pub mod error;
pub mod provider;
pub mod providers;
pub mod tools;

pub use config::{AiSettings, ResolvedAiConfig};
pub use error::AiError;
pub use provider::Provider;
