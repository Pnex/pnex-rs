//! Deep-link devices : global search (D69) poses the device id to open;
//! `pages/devices.rs` consumes it via use_effect to select the device.

use dioxus::prelude::*;

pub static OPEN_DEVICE: GlobalSignal<Option<i64>> = GlobalSignal::new(|| None);
