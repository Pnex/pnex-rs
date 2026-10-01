//! Deep-link fonctions : global search (D69) poses the function id to open;
//! `pages/functions.rs` consumes it via use_effect to open the editor.

use dioxus::prelude::*;

pub static OPEN_FUNCTION: GlobalSignal<Option<i64>> = GlobalSignal::new(|| None);
