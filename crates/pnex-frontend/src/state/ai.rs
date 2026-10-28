//! Global state of the AI assistant: status (UI visibility) hydrated at shell
//! boot, refreshed after a change of the LLM providers.

use dioxus::prelude::*;

use crate::api;

pub static AI_STATUS: GlobalSignal<Option<pnex_core::AiStatus>> = GlobalSignal::new(|| None);

/// Queries the status and updates the signal (None on error).
pub async fn refresh_status() {
    match api::ai::status().await {
        Ok(status) => AI_STATUS.with_mut(|s| *s = Some(status)),
        Err(_) => AI_STATUS.with_mut(|s| *s = None),
    }
}

/// Server-side kill-switch active → the panel is **hidden** (A3).
/// Status not hydrated yet (boot/failure) → hidden too; the failure is
/// retried on org change (see AssistantPanel).
pub fn visible() -> bool {
    AI_STATUS.read().as_ref().is_some_and(|s| s.enabled)
}

/// A provider resolves (org default or platform default); otherwise the
/// drawer shows the "not configured" hint.
pub fn configured() -> bool {
    AI_STATUS.read().as_ref().is_some_and(|s| s.configured)
}
