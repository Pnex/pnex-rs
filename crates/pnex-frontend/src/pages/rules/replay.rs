//! Replay of a rule over a window back from now: when it would have fired, sending
//! nothing (D830).

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::rules::{Rule, RuleReplay};

use crate::api;
use crate::api::error_i18n::localize;
use crate::app::Route;
use crate::components::modal::Modal;
use crate::components::ontology::short_time;
use crate::components::ui::{BTN_PRIMARY, FIELD_ERROR, FIELD_HELP, FIELD_LABEL, INPUT};

const WINDOWS: [(&str, &str); 5] = [
    ("1h", "rules-window-1h"),
    ("6h", "rules-window-6h"),
    ("24h", "rules-window-24h"),
    ("7d", "rules-window-7d"),
    ("30d", "rules-window-30d"),
];

#[component]
pub(super) fn ReplayDialog(rule: Rule, on_close: EventHandler<()>) -> Element {
    let mut window = use_signal(|| "24h".to_string());
    let mut result = use_signal(|| None::<RuleReplay>);
    let mut busy = use_signal(|| false);
    let mut error = use_signal(|| None::<String>);
    let id = rule.id.clone();
    let run = move |_| {
        let id = id.clone();
        let w = window();
        busy.set(true);
        error.set(None);
        spawn(async move {
            match api::rules::replay(&id, &w).await {
                Ok(r) => result.set(Some(r)),
                Err(e) => error.set(Some(localize(&e))),
            }
            busy.set(false);
        });
    };
    let chosen = window();
    rsx! {
        Modal {
            title: t!("rules-replay-title", name : rule.name.clone()).to_string(),
            max_width: "max-w-lg".to_string(),
            on_close: move |_| on_close.call(()),
            div { class: "space-y-4",
                div { class: "flex flex-wrap items-end gap-3",
                    div { class: "min-w-0 flex-1",
                        label { class: FIELD_LABEL, r#for: "replay-window",
                            {t!("rules-replay-window")}
                        }
                        select {
                            id: "replay-window",
                            class: INPUT,
                            onchange: move |e| window.set(e.value()),
                            for (key, label) in WINDOWS {
                                option {
                                    key: "{key}",
                                    value: "{key}",
                                    selected: chosen == key,
                                    {t!(label)}
                                }
                            }
                        }
                    }
                    button {
                        r#type: "button",
                        class: BTN_PRIMARY,
                        disabled: busy(),
                        onclick: run,
                        {t!("rules-replay")}
                    }
                }
                if let Some(e) = error() {
                    p { class: FIELD_ERROR, role: "alert", "{e}" }
                }
                if let Some(r) = result() {
                    ReplayEvents { replay: r }
                }
            }
        }
    }
}

#[component]
fn ReplayEvents(replay: RuleReplay) -> Element {
    let n = replay.events.len();
    rsx! {
        div { class: "space-y-2", "data-testid": "replay-result",
            p { class: "text-sm text-ink",
                if n == 0 {
                    {t!("rules-replay-none")}
                } else {
                    {t!("rules-replay-count", count : n)}
                }
            }
            if !replay.complete {
                p { class: FIELD_HELP, role: "status", {t!("rules-replay-incomplete")} }
            }
            if n > 0 {
                ol { class: "max-h-72 divide-y divide-line overflow-y-auto rounded-md border border-line",
                    for (i, ev) in replay.events.iter().enumerate() {
                        li {
                            key: "{i}",
                            class: "flex items-center justify-between gap-3 px-3 py-2 text-sm",
                            span { class: "tabular-nums text-slate whitespace-nowrap",
                                "{short_time(&ev.at)}"
                            }
                            if let Some(id) = ev.object_id.clone() {
                                Link {
                                    to: Route::OntologyObject {
                                        id: id.clone(),
                                    },
                                    class: "min-w-0 truncate text-signal-700 underline-offset-2 hover:underline",
                                    "{id.get(..8).unwrap_or(&id)}"
                                }
                            } else if let Some(v) = ev.value {
                                span { class: "tabular-nums text-ink", "{v}" }
                            }
                        }
                    }
                }
            }
        }
    }
}
