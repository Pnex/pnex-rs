//! Global app state (Dioxus signals): session, current org.

/// Rights of the user in the current org (iam.md §9).
pub mod access;
pub mod ai;
pub mod annotations;
pub mod compat;
pub mod devices;
pub mod field_notes;
pub mod functions;
pub mod map;
pub mod media;
pub mod org;
/// Roles of the org's types and link types (D441).
pub mod roles;
pub mod session;
pub mod theme;
pub mod toasts;
pub mod tours;
/// UI preferences (sidebar rail) — global display preferences.
pub mod ui;
/// Vision model names cache (flow canvas subtitles).
pub mod vision;
pub mod viz;
