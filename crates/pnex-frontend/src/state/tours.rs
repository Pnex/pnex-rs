//! Studio editor deep-link: a component (assistant, dashboard…) sets
//! the id of the tour to open; `pages/studio.rs` consumes it via use_effect
//! (one-shot signal, consumed once).

use dioxus::prelude::*;

pub static OPEN_TOUR: GlobalSignal<Option<String>> = GlobalSignal::new(|| None);
