//! The session view (markets-research.md D738, D742): the abnormal moves of
//! the watched series around a date, and the world events of the same
//! window. Being in the same window is shown as such, never as a cause.

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::markets::moves::{MarketSession, SessionMove};

use crate::api;
use crate::app::Route;

const SECTION: &str = "bg-white rounded-lg border border-gray-200 p-4 space-y-3";

/// The change as people read it: `-5.5 %` for a return, `+0.25` otherwise.
fn shown(m: &SessionMove) -> String {
    if m.mode == "log_return" {
        format!("{:+.1} %", (m.measure.exp() - 1.0) * 100.0)
    } else {
        format!("{:+.2}", m.measure)
    }
}

#[component]
pub fn SessionPanel(date: String) -> Element {
    let session = use_resource(move || {
        let date = date.clone();
        async move { api::markets::session(&date).await }
    });
    let read = session.read();
    rsx! {
        div { class: "space-y-4", "data-testid": "onto-session",
            p { class: "text-xs text-gray-500", {t!("onto-figures-disclaimer")} }
            match &*read {
                Some(Err(e)) => rsx! {
                    p { class: "text-sm text-red-700", {crate::api::error_i18n::localize(e)} }
                },
                Some(Ok(s)) => rsx! {
                    SessionBody { session: s.clone() }
                },
                None => rsx! {},
            }
        }
    }
}

#[component]
fn SessionBody(session: MarketSession) -> Element {
    let s = session;
    rsx! {
        div { class: SECTION,
            h3 { class: "text-sm font-medium text-gray-900",
                {t!("onto-session-moves", date : s.date.clone())}
            }
            if s.moves.is_empty() {
                p { class: "text-sm text-gray-500", {t!("onto-session-no-move")} }
            } else {
                table {
                    class: "w-full text-sm",
                    "data-testid": "onto-session-moves",
                    thead {
                        tr { class: "text-left text-xs text-gray-500",
                            th { class: "py-1 pr-4", {t!("onto-session-col-series")} }
                            th { class: "py-1 pr-4", {t!("onto-session-col-day")} }
                            th { class: "py-1 pr-4 text-right", {t!("onto-session-col-change")} }
                            th { class: "py-1 text-right", {t!("onto-session-col-sigmas")} }
                        }
                    }
                    tbody { class: "divide-y divide-gray-100",
                        for m in s.moves.iter() {
                            tr { key: "{m.id}",
                                td { class: "py-1 pr-4",
                                    Link {
                                        class: "text-signal-600 hover:underline",
                                        to: Route::OntologyObject {
                                            id: m.id.clone(),
                                        },
                                        "{m.series_code}"
                                    }
                                    if m.backfilled {
                                        span { class: "ml-2 text-xs text-gray-400",
                                            {t!("onto-session-backfilled")}
                                        }
                                    }
                                }
                                td { class: "py-1 pr-4 font-mono text-xs", "{m.event_at}" }
                                td { class: "py-1 pr-4 text-right font-mono", {shown(m)} }
                                td { class: "py-1 text-right font-mono", {format!("{:.1}", m.score)} }
                            }
                        }
                    }
                }
            }
        }
        div { class: SECTION,
            h3 { class: "text-sm font-medium text-gray-900",
                {
                    t!(
                        "onto-session-same-window", from : s.window_from.clone(), to : s.window_to
                        .clone()
                    )
                }
            }
            p { class: "text-xs text-amber-700", {t!("onto-session-not-cause")} }
            if s.world_events.is_empty() {
                p { class: "text-sm text-gray-500", {t!("onto-session-no-world-event")} }
            } else {
                ul {
                    class: "divide-y divide-gray-100",
                    "data-testid": "onto-session-world",
                    for w in s.world_events.iter() {
                        li {
                            key: "{w.id}",
                            class: "flex flex-wrap items-center gap-2 py-2 text-sm",
                            Link {
                                class: "text-signal-600 hover:underline",
                                to: Route::OntologyObject {
                                    id: w.id.clone(),
                                },
                                "{w.title}"
                            }
                            span { class: "text-xs text-gray-500 font-mono", "{w.event_at}" }
                            if w.status == "confirmed" {
                                span { class: "text-xs text-green-700", {t!("onto-session-confirmed")} }
                            } else {
                                span { class: "text-xs text-amber-700",
                                    {t!("onto-session-unconfirmed")}
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
