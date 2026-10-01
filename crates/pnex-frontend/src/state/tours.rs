//! Deep-link éditeur de studio : un composant (assistant, dashboard…) pose
//! l'id du tour à ouvrir ; `pages/studio.rs` le consomme via use_effect
//! (école `state/flows.rs::OPEN_FLOW`).

use dioxus::prelude::*;

pub static OPEN_TOUR: GlobalSignal<Option<String>> = GlobalSignal::new(|| None);
