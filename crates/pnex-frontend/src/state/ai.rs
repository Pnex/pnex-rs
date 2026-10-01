//! État global de l'assistant IA : statut (visibilité UI) hydraté au boot
//! du shell, refreshed after a change of the LLM providers.

use dioxus::prelude::*;

use crate::api;

pub static AI_STATUS: GlobalSignal<Option<pnex_core::AiStatus>> = GlobalSignal::new(|| None);

/// Interroge le statut et met à jour le signal (None en cas d'erreur).
pub async fn refresh_status() {
    match api::ai::status().await {
        Ok(status) => AI_STATUS.with_mut(|s| *s = Some(status)),
        Err(_) => AI_STATUS.with_mut(|s| *s = None),
    }
}

/// Kill-switch actif côté serveur → le panneau est **caché** (A3).
/// Statut pas encore hydraté (boot/échec) → caché aussi ; l'échec est
/// réessayé au changement d'org (cf. AssistantPanel).
pub fn visible() -> bool {
    AI_STATUS.read().as_ref().is_some_and(|s| s.enabled)
}

/// A provider resolves (org default or platform default); otherwise the
/// drawer shows the "not configured" hint.
pub fn configured() -> bool {
    AI_STATUS.read().as_ref().is_some_and(|s| s.configured)
}
