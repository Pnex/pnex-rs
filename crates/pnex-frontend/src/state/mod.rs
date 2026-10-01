//! État global de l'app (signals Dioxus) : session, org courante.

pub mod ai;
pub mod annotations;
pub mod compat;
pub mod devices;
pub mod flows;
pub mod functions;
pub mod map;
pub mod media;
pub mod org;
pub mod session;
pub mod toasts;
pub mod tours;
/// Préférences UI (rail de la sidebar) — préférences d'affichage globales.
pub mod ui;
/// Vision model names cache (flow canvas subtitles).
pub mod vision;
pub mod viz;
