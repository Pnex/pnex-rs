//! Built-in AI assistant (2026-09-06) — multi-tool chat, per-organization
//! connector, OpenObserve reads, creation/editing of flow **drafts**.
//!
//! Cross-cutting safeguards (decisions A1-A5, 2026-09-06):
//! - the surface executable by the LLM is **exactly**
//!   [`crate::services::ai::tools::execute`] — a closed `match` over 10
//!   read/draft-write tools; no deploy, no deletion,
//!   no device command, no "generic" tool (http/sql);
//! - the LLM config resolves as **env > org connector > disabled**
//!   (`config::resolve`), kill-switch `settings.ai.enabled`;
//! - the `flow_supervisor` / `POST /flows/{id}/deploy` path is
//!   referenced nowhere in this module.

pub mod agent;
pub mod calendar_tools;
pub mod config;
pub mod context;
pub mod conversations;
pub mod dashboard_tools;
pub mod diagnose;
pub mod doc_tools;
pub mod error;
pub mod knowledge;
pub mod mail_tools;
pub mod more_tools;
pub mod ontology_tools;
pub mod provider;
pub mod providers;
pub mod site_tools;
pub mod tools;

pub use config::{AiSettings, ResolvedAiConfig};
pub use error::AiError;
pub use provider::Provider;
