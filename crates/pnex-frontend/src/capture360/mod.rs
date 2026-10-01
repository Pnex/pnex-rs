//! Capture panoramique 360 guidée (Take 360) — mobile.
//!
//! Modèle V3 : protocole 3 anneaux (à plat, haut, bas), pas-à-pas STRICT
//! — N cibles par anneau calculées sur la hfov portrait réelle
//! (`guidance::ring_plan`), auto-shutter sur la cible courante après
//! dwell aligné+stable, stitching on-device via `pnex-stitcher` puis
//! upload `kind=panorama` et job serveur HD via le pipeline media.
//!
//! Écoles suivies :
//! - `capture.rs` : JNI brut, atomics purs + polling page 500 ms, libellés
//!   `t!` résolus avant détachement, stubs hors Android (garde `task check`
//!   vert sur desktop/web) ;
//! - `media_viewer.rs` : JS injecté via `document::eval` (preview
//!   getUserMedia + grab canvas → JPEG → base64 chunké).
//!
//! Sous-modules Android : `sensors` (rotation vector via ndk-sys),
//! `fov`/`wakelock` (JNI Camera2 / Window flag), `frames` (pont JS),
//! `pipeline` (decode → stitch → upload, `spawn_forever`), `overlay`
//! (composant dioxus plein écran).

// Desktop/web : les consommateurs runtime de guidance/state sont cfg
// android (overlay, sensors, pipeline) — sans allow, ces items sont
// « morts » côté desktop (école capture.rs : allow(dead_code) documenté,
// pas de code fantôme dans les cibles natives non-android).
// filelog n'a de sens que hors wasm : JNI + ndk_context (deps absentes
// de la cible web). Les consommateurs wasm (media_viewer wasm32) ne
// l'appellent pas — seul l'impl natif de media_viewer y touche.
#[cfg(not(target_arch = "wasm32"))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub mod filelog;
// guidance porte son propre #![allow(dead_code)] interne (machine
// d'alignement en réserve v2) — un outer cfg_attr serait un doublon
// (clippy 1.96 duplicated_attributes).
pub mod guidance;
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub mod state;

#[cfg(target_os = "android")]
pub mod fov;
#[cfg(target_os = "android")]
pub mod frames;
#[cfg(target_os = "android")]
pub mod overlay;
#[cfg(target_os = "android")]
pub mod pipeline;
#[cfg(target_os = "android")]
pub mod sensors;
#[cfg(target_os = "android")]
pub mod wakelock;

/// Stub hors Android : même signature que le vrai composant (la page media
/// compile sur toutes les cibles natives) — le clic du bouton est sans
/// effet côté desktop (cf. page media, école `capture.rs`).
#[cfg(not(target_os = "android"))]
pub mod overlay {
    use dioxus::prelude::*;

    #[component]
    pub fn Take360Overlay(on_close: Callback<()>, on_uploaded: Callback<()>) -> Element {
        let _ = (on_close, on_uploaded);
        rsx! {}
    }
}
