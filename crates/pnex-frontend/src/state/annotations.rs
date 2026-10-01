//! Deep-link annotations : global search (D69) poses the layer id to open;
//! `pages/annotations.rs` consumes it via use_effect to switch to the
//! layer viewer (`View::Editor`).

use dioxus::prelude::*;

pub static OPEN_LAYER: GlobalSignal<Option<String>> = GlobalSignal::new(|| None);
