//! Metro-line stepper: numbered stations joined by a line, the stations
//! passed filled, the current one ringed. Each station can be a button
//! (`on_pick`), so the line doubles as navigation between the steps of a
//! page (Vision Lab) or of a wizard.

use dioxus::prelude::*;

use crate::components::icons;

/// One station of the line.
#[derive(Clone, PartialEq)]
pub struct Station {
    pub label: String,
    /// Finished: the dot shows a tick.
    pub done: bool,
    /// Optional `data-testid` of the station button.
    pub testid: Option<&'static str>,
}

#[component]
pub fn Stepper(
    /// Accessible name of the line.
    label: String,
    stations: Vec<Station>,
    current: usize,
    on_pick: EventHandler<usize>,
) -> Element {
    let last = stations.len().saturating_sub(1);
    rsx! {
        ol { class: "flex items-center", "aria-label": "{label}",
            for (i, s) in stations.into_iter().enumerate() {
                li { key: "{i}", class: "flex items-center",
                    button {
                        class: "group flex items-center gap-2 rounded-full py-1 pr-2 focus-visible:outline-2 focus-visible:outline-signal",
                        r#type: "button",
                        "aria-current": if i == current { "step" } else { "false" },
                        "data-testid": s.testid.unwrap_or("stepper-station"),
                        onclick: move |_| on_pick.call(i),
                        span { class: dot_class(i, current, s.done),
                            if s.done && i != current {
                                icons::Check { class: "h-4 w-4" }
                            } else {
                                "{i + 1}"
                            }
                        }
                        span { class: label_class(i == current), "{s.label}" }
                    }
                    if i < last {
                        span {
                            class: if s.done { "mx-1 h-0.5 w-6 rounded-full bg-signal sm:w-10" } else { "mx-1 h-0.5 w-6 rounded-full bg-line-strong sm:w-10" },
                            "aria-hidden": "true",
                        }
                    }
                }
            }
        }
    }
}

/// Dot of a station (full literals for the Tailwind scan).
fn dot_class(i: usize, current: usize, done: bool) -> &'static str {
    if i == current {
        "flex h-8 w-8 items-center justify-center rounded-full bg-signal text-sm font-semibold text-white ring-4 ring-signal-200"
    } else if done {
        "flex h-8 w-8 items-center justify-center rounded-full bg-signal text-sm font-semibold text-white group-hover:opacity-90"
    } else {
        "flex h-8 w-8 items-center justify-center rounded-full border-2 border-line-strong bg-surface text-sm font-semibold text-slate group-hover:border-signal group-hover:text-signal"
    }
}

/// Label of a station: only the current one stays on a phone.
fn label_class(current: bool) -> &'static str {
    if current {
        "text-sm font-semibold text-ink"
    } else {
        "hidden text-sm text-slate group-hover:text-ink sm:inline"
    }
}
