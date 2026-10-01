//! Deep-link éditeur de flows : le panneau assistant (ou tout autre
//! composant) pose l'id du flow à ouvrir ; `pages/flows.rs` le consomme
//! via use_effect pour sélectionner le flow dans la liste/éditeur.

use dioxus::prelude::*;

pub static OPEN_FLOW: GlobalSignal<Option<i64>> = GlobalSignal::new(|| None);
