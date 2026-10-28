//! Editing annotations on media (D59) — shared sub-components.
//! Slice 3: the live popover [`popover`] (read-only, viewer + media page);
//! the full editing panel (inspector, versions) comes with the
//! studio integration (slice 4).

/// Inspecteur d'item (cible, label, couleur, suppression).
pub mod inspector;
/// Editing panel (layer + items + placement + save + conflict).
pub mod panel;
pub mod popover;
/// Editor state (AnnotationEditorCx) + tested pure reducers.
pub mod state;
/// Control / reading target editors (D128/D129).
pub mod surface_targets;
/// Versions drawer + publishing (tour_editor/versions.rs pattern).
pub mod versions;
