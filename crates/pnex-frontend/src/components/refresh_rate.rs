//! Auto-refresh driver for live pages: a user-selectable cadence
//! (1 s … 60 s) plus a "refresh now" button, both bumping the page's
//! `reload` counter that its resources subscribe to.
//!
//! One 1 s ticker per page (dropped with the component): a cadence change
//! applies at the next tick instead of waiting out the previous sleep, and
//! "refresh now" restarts the countdown.

use std::time::Duration;

use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::components::crud::filters::RefreshButton;
use crate::util::sleep;

/// Selectable cadences, in seconds.
pub const RATES_SECS: [u64; 7] = [1, 5, 10, 15, 30, 45, 60];

/// Handle returned by [`use_auto_refresh`]; `Copy` (signals only).
#[derive(Clone, Copy, PartialEq)]
pub struct AutoRefresh {
    reload: Signal<u32>,
    secs: Signal<u64>,
    elapsed: Signal<u64>,
}

impl AutoRefresh {
    /// Reloads immediately and restarts the countdown.
    pub fn refresh_now(mut self) {
        self.elapsed.set(0);
        self.reload.with_mut(|r| *r += 1);
    }
}

/// Starts the ticker bumping `reload` every `secs` (initially
/// `default_secs`) while the calling component is mounted.
pub fn use_auto_refresh(reload: Signal<u32>, default_secs: u64) -> AutoRefresh {
    let secs = use_signal(|| default_secs);
    let elapsed = use_signal(|| 0u64);
    let auto = AutoRefresh {
        reload,
        secs,
        elapsed,
    };
    use_future(move || async move {
        let mut auto = auto;
        loop {
            sleep(Duration::from_secs(1)).await;
            // `peek`: the ticker must not subscribe to its own state.
            let next = *auto.elapsed.peek() + 1;
            if next >= *auto.secs.peek() {
                auto.refresh_now();
            } else {
                auto.elapsed.set(next);
            }
        }
    });
    auto
}

/// Cadence picker + "refresh now" button. `on_refresh` runs in addition to
/// the reload (e.g. re-fetching data outside the page's resources).
#[component]
pub fn RefreshRateControl(auto: AutoRefresh, on_refresh: Option<Callback<()>>) -> Element {
    let mut secs = auto.secs;
    let current = secs();
    rsx! {
        div { class: "flex items-center gap-2",
            label { class: "flex items-center gap-2 text-xs text-gray-500",
                // Phone: the select alone (its aria-label names it).
                span { class: "hidden sm:inline", {t!("refresh-rate-label")} }
                select {
                    class: "px-2 py-1.5 text-sm border border-gray-300 rounded-lg bg-white",
                    "aria-label": t!("refresh-rate-label"),
                    value: "{current}",
                    onchange: move |e| {
                        if let Ok(v) = e.value().parse::<u64>() {
                            secs.set(v);
                        }
                    },
                    for r in RATES_SECS {
                        option {
                            key: "{r}",
                            value: "{r}",
                            selected: r == current,
                            {t!("refresh-rate-option", secs : r)}
                        }
                    }
                }
            }
            RefreshButton {
                on_click: move |_| {
                    auto.refresh_now();
                    if let Some(cb) = on_refresh {
                        cb.call(());
                    }
                },
            }
        }
    }
}
