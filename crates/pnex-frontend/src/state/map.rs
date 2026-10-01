//! Deep-link carte : global search (D69) poses the POI id to open;
//! `pages/map.rs` consumes it via use_effect to select the pin (opens the
//! POI drawer; the drawer fetches its detail from the id alone).

use dioxus::prelude::*;

pub static OPEN_POI: GlobalSignal<Option<String>> = GlobalSignal::new(|| None);
