//! Édition des annotations sur médias (D59) — sous-composants partagés.
//! Tranche 3 : la popover live [`popover`] (lecture, viewer + page média) ;
//! le panneau d'édition complet (inspector, versions) arrive avec
//! l'intégration studio (tranche 4).

/// Inspecteur d'item (cible, label, couleur, suppression).
pub mod inspector;
/// Panneau d'édition (couche + items + pose + save + conflit).
pub mod panel;
pub mod popover;
/// État éditeur (AnnotationEditorCx) + réducteurs purs testés.
pub mod state;
/// Control / reading target editors (D128/D129).
pub mod surface_targets;
/// Drawer versions + publication (école tour_editor/versions.rs).
pub mod versions;
