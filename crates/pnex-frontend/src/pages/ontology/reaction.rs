//! How an instrument reacted (markets-research.md D532, D535): one date, or a
//! kind of filing over the followed instruments against drawn dates. Aids to
//! analysis: the number of events is always in view.

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::market_study::{Distribution, RuleResult};

use crate::api;
use crate::components::ui::INPUT;

const SECTION: &str = "bg-white rounded-lg border border-gray-200 p-4 space-y-3";
/// Filing kinds worth a first look: (form, item, label key).
const RULES: [(&str, &str, &str); 7] = [
    ("8-K", "2.02", "onto-reaction-rule-results"),
    ("8-K", "5.02", "onto-reaction-rule-management"),
    ("8-K", "1.01", "onto-reaction-rule-agreement"),
    ("8-K", "2.01", "onto-reaction-rule-acquisition"),
    ("10-Q", "", "onto-reaction-rule-quarterly"),
    ("10-K", "", "onto-reaction-rule-annual"),
    ("8-K", "", "onto-reaction-rule-all"),
];

fn percent(v: f64) -> String {
    format!("{:+.2} %", v * 100.0)
}

fn window_label(from: i32, to: i32) -> String {
    format!("[{from:+}, {to:+}]")
}

#[component]
pub fn ReactionPanel(id: String) -> Element {
    rsx! {
        div { class: "space-y-4", "data-testid": "onto-reaction",
            p { class: "text-xs text-gray-500", {t!("onto-figures-disclaimer")} }
            OneDate { id }
            RuleTest {}
        }
    }
}

#[component]
fn OneDate(id: String) -> Element {
    let mut date = use_signal(String::new);
    let study = use_resource(move || {
        let (id, date) = (id.clone(), date());
        async move {
            if date.is_empty() {
                return Ok(None);
            }
            api::markets::event_study(&id, &date).await
        }
    });
    let read = study.read();
    rsx! {
        div { class: SECTION,
            label { class: "text-sm text-gray-700 space-y-1 block",
                span { class: "block font-medium", {t!("onto-reaction-date")} }
                input {
                    class: INPUT,
                    r#type: "date",
                    "data-testid": "onto-reaction-date",
                    value: "{date}",
                    onchange: move |e| date.set(e.value()),
                }
            }
            match &*read {
                Some(Err(e)) => rsx! {
                    p { class: "text-sm text-red-700", {crate::api::error_i18n::localize(e)} }
                },
                Some(Ok(Some(s))) => rsx! {
                    p { class: "text-sm text-gray-700",
                        {
                            t!(
                                "onto-reaction-summary", day : s.day_zero.clone(), beta : format!("{:.2}", s
                                .beta)
                            )
                        }
                    }
                    if let Some(v) = s.relative_volume {
                        p { class: "text-sm text-gray-700", {t!("onto-reaction-volume", ratio : format!("{v:.1}"))} }
                    }
                    table { class: "w-full text-sm",
                        thead {
                            tr { class: "text-left text-xs text-gray-500",
                                th { class: "py-1 pr-4", {t!("onto-reaction-window")} }
                                th { class: "py-1 pr-4 text-right", {t!("onto-reaction-raw")} }
                                th { class: "py-1 text-right", {t!("onto-reaction-abnormal")} }
                            }
                        }
                        tbody { class: "divide-y divide-gray-100",
                            for w in s.windows.iter() {
                                tr { key: "{w.from}{w.to}",
                                    td { class: "py-1 pr-4 font-mono", {window_label(w.from, w.to)} }
                                    td { class: "py-1 pr-4 text-right font-mono", {percent(w.raw)} }
                                    td { class: "py-1 text-right font-mono", {percent(w.abnormal)} }
                                }
                            }
                        }
                    }
                },
                Some(Ok(None)) if !date().is_empty() => rsx! {
                    p { class: "text-sm text-gray-500", {t!("onto-reaction-none")} }
                },
                _ => rsx! {},
            }
        }
    }
}

fn dist_cell(d: &Option<Distribution>) -> String {
    match d {
        Some(d) => format!(
            "{} ({} ; {}) · n = {} · {:.0} % > 0",
            percent(d.mean),
            percent(d.mean_low),
            percent(d.mean_high),
            d.count,
            d.positive * 100.0
        ),
        None => "—".to_string(),
    }
}

#[component]
fn RuleTest() -> Element {
    let mut chosen = use_signal(|| 0usize);
    let result = use_resource(move || {
        let (form, item, _) = RULES[chosen()];
        async move { api::markets::rule_test(form, item).await }
    });
    let read = result.read();
    rsx! {
        div { class: SECTION,
            label { class: "text-sm text-gray-700 space-y-1 block",
                span { class: "block font-medium", {t!("onto-reaction-rule")} }
                select {
                    class: INPUT,
                    "data-testid": "onto-reaction-rule",
                    onchange: move |e| chosen.set(e.value().parse().unwrap_or(0)),
                    for (i, (_, _, key)) in RULES.iter().enumerate() {
                        option {
                            key: "{i}",
                            value: "{i}",
                            selected: i == chosen(),
                            {rule_label(key)}
                        }
                    }
                }
            }
            match &*read {
                Some(Err(e)) => rsx! {
                    p { class: "text-sm text-red-700", {crate::api::error_i18n::localize(e)} }
                },
                Some(Ok(r)) => rsx! {
                    RuleTable { result: r.clone() }
                },
                None => rsx! {},
            }
        }
    }
}

fn rule_label(key: &str) -> String {
    dioxus_i18n::prelude::i18n().translate(key)
}

#[component]
fn RuleTable(result: RuleResult) -> Element {
    if result.benchmark_missing {
        return rsx! {
            p { class: "text-sm text-gray-500", {t!("onto-reaction-no-benchmark")} }
        };
    }
    rsx! {
        p { class: "text-sm text-gray-700",
            {
                t!(
                    "onto-reaction-rule-counts", filings : result.filings, instruments : result
                    .instruments
                )
            }
        }
        div { class: "overflow-x-auto",
            table { class: "w-full text-sm",
                thead {
                    tr { class: "text-left text-xs text-gray-500",
                        th { class: "py-1 pr-4", {t!("onto-reaction-window")} }
                        th { class: "py-1 pr-4", {t!("onto-reaction-events")} }
                        th { class: "py-1", {t!("onto-reaction-baseline")} }
                    }
                }
                tbody { class: "divide-y divide-gray-100",
                    for w in result.windows.iter() {
                        tr { key: "{w.from}{w.to}",
                            td { class: "py-1 pr-4 font-mono", {window_label(w.from, w.to)} }
                            td { class: "py-1 pr-4 font-mono text-xs", {dist_cell(&w.events)} }
                            td { class: "py-1 font-mono text-xs", {dist_cell(&w.baseline)} }
                        }
                    }
                }
            }
        }
        p { class: "text-xs text-gray-500", {t!("onto-reaction-survivors")} }
    }
}

/// A fictitious position's result (D534): nothing here reaches a broker.
#[component]
pub fn PositionPanel(id: String) -> Element {
    let result = use_resource(move || {
        let id = id.clone();
        async move { api::markets::position_result(&id).await }
    });
    let read = result.read();
    rsx! {
        div { class: SECTION, "data-testid": "onto-position-result",
            p { class: "text-xs text-gray-500", {t!("onto-position-fictitious")} }
            match &*read {
                Some(Err(e)) => rsx! {
                    p { class: "text-sm text-red-700", {crate::api::error_i18n::localize(e)} }
                },
                Some(Ok(Some(r))) => rsx! {
                    p { class: "text-sm text-gray-700",
                        {
                            t!(
                                "onto-position-span", entry : r.entry_on.clone(), entry_close : r.entry_close
                                .to_string(), last : r.last_on.clone(), last_close : r.last_close.to_string()
                            )
                        }
                    }
                    p { class: "text-lg font-medium text-gray-900",
                        {format!("{} ({:+.2})", percent(r.total_return), r.result)}
                    }
                },
                Some(Ok(None)) => rsx! {
                    p { class: "text-sm text-gray-500", {t!("onto-position-none")} }
                },
                None => rsx! {},
            }
        }
    }
}
