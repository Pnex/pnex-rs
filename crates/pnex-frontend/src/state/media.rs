//! Deep-link media: global search (D69) poses the asset id to open;
//! `pages/media.rs` consumes it via use_effect to open the viewer modal.

use dioxus::prelude::*;

pub static OPEN_MEDIA: GlobalSignal<Option<String>> = GlobalSignal::new(|| None);
