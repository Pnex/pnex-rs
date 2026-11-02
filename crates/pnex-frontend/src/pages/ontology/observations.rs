//! The dated values of an object (markets-research.md D541): one indicator
//! at a time, as known at a chosen day, so a past decision can be reread with
//! what was public then.

use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::api;
use crate::components::ui::INPUT;

/// `1234567.5` → `1 234 567.5`; the source's value, never rounded.
fn spaced(value: f64) -> String {
    let raw = value.to_string();
    let (sign, rest) = raw
        .strip_prefix('-')
        .map_or(("", raw.as_str()), |r| ("-", r));
    let (int, frac) = rest
        .split_once('.')
        .map_or((rest, None), |(i, f)| (i, Some(f)));
    let mut out = String::new();
    for (i, c) in int.chars().enumerate() {
        if i > 0 && (int.len() - i) % 3 == 0 {
            out.push('\u{202f}');
        }
        out.push(c);
    }
    match frac {
        Some(f) => format!("{sign}{out}.{f}"),
        None => format!("{sign}{out}"),
    }
}

#[component]
pub fn ObservationsPanel(id: String) -> Element {
    let mut day = use_signal(String::new);
    let mut chosen = use_signal(String::new);
    let values = use_resource(move || {
        let id = id.clone();
        let day = day();
        async move {
            // The end of the chosen day, as a value published that day counts.
            let as_of = (!day.is_empty()).then(|| format!("{day}T23:59:59Z"));
            api::ontology::observations(&id, as_of.as_deref()).await
        }
    });
    let read = values.read();
    let rows = match &*read {
        Some(Ok(r)) => r.clone(),
        Some(Err(e)) => {
            return rsx! {
                p { class: "text-sm text-red-700", {crate::api::error_i18n::localize(e)} }
            };
        }
        None => return rsx! {},
    };
    let mut metrics: Vec<String> = rows.iter().map(|r| r.metric.clone()).collect();
    metrics.sort();
    metrics.dedup();
    let current = if metrics.contains(&chosen()) {
        chosen()
    } else {
        metrics.first().cloned().unwrap_or_default()
    };
    let shown: Vec<_> = rows.into_iter().filter(|r| r.metric == current).collect();
    rsx! {
        div {
            class: "bg-white rounded-lg border border-gray-200 p-4 space-y-3",
            "data-testid": "onto-figures",
            p { class: "text-xs text-gray-500", {t!("onto-figures-disclaimer")} }
            div { class: "flex flex-wrap gap-3",
                label { class: "text-sm text-gray-700 space-y-1",
                    span { class: "block", {t!("onto-figures-metric")} }
                    select {
                        class: INPUT,
                        "data-testid": "onto-figures-metric",
                        value: "{current}",
                        onchange: move |e| chosen.set(e.value()),
                        for m in metrics {
                            option {
                                key: "{m}",
                                value: "{m}",
                                selected: m == current,
                                "{m}"
                            }
                        }
                    }
                }
                label { class: "text-sm text-gray-700 space-y-1",
                    span { class: "block", {t!("onto-figures-as-of")} }
                    input {
                        class: INPUT,
                        r#type: "date",
                        "data-testid": "onto-figures-as-of",
                        value: "{day}",
                        onchange: move |e| day.set(e.value()),
                    }
                }
            }
            if shown.is_empty() {
                p { class: "text-sm text-gray-500", {t!("onto-figures-empty")} }
            } else {
                div { class: "overflow-x-auto",
                    table { class: "w-full text-sm",
                        thead {
                            tr { class: "text-left text-xs text-gray-500",
                                th { class: "py-1 pr-4", {t!("onto-figures-period")} }
                                th { class: "py-1 pr-4 text-right", {t!("onto-figures-value")} }
                                th { class: "py-1 pr-4", {t!("onto-figures-known")} }
                                th { class: "py-1", {t!("onto-figures-source")} }
                            }
                        }
                        tbody { class: "divide-y divide-gray-100",
                            for (i, r) in shown.into_iter().enumerate() {
                                tr { key: "{i}",
                                    td { class: "py-1 pr-4 whitespace-nowrap",
                                        if r.period_start == r.period_end {
                                            "{r.period_end}"
                                        } else {
                                            "{r.period_start} → {r.period_end}"
                                        }
                                    }
                                    td { class: "py-1 pr-4 text-right font-mono whitespace-nowrap",
                                        {format!("{} {}", spaced(r.value), r.unit)}
                                    }
                                    td { class: "py-1 pr-4 whitespace-nowrap",
                                        if r.backfilled {
                                            {t!("onto-figures-backfilled")}
                                        } else {
                                            {r.known_at.chars().take(10).collect::<String>()}
                                        }
                                    }
                                    td { class: "py-1 font-mono text-xs text-gray-500",
                                        "{r.source_ref}"
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::spaced;

    #[test]
    fn large_values_are_readable_and_exact() {
        assert_eq!(spaced(96221000000.0), "96\u{202f}221\u{202f}000\u{202f}000");
        assert_eq!(spaced(-1234.5), "-1\u{202f}234.5");
        assert_eq!(spaced(4.19), "4.19");
    }
}
