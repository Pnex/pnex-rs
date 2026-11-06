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
pub fn ReactionPanel(id: String, instrument: bool, series: String) -> Element {
    rsx! {
        div { class: "space-y-4", "data-testid": "onto-reaction",
            p { class: "text-xs text-gray-500", {t!("onto-figures-disclaimer")} }
            if instrument {
                OneDate { id }
                RuleTest {}
            }
            if !series.is_empty() {
                EventRuleTest { series }
            }
        }
    }
}

/// World event kinds and move filters a rule on events offers (D743).
const GEO_KINDS: [&str; 6] = [
    "armed_conflict",
    "attack",
    "sanctions",
    "blockade",
    "ceasefire",
    "natural_disaster",
];
const ASSET_CLASSES: [&str; 6] = [
    "equity_index",
    "commodity",
    "rates",
    "volatility",
    "fx",
    "crypto",
];

/// How this series moved after world events or abnormal sessions (D743).
#[component]
fn EventRuleTest(series: String) -> Element {
    let mut dates = use_signal(|| "geo_events".to_string());
    let mut kind = use_signal(String::new);
    let mut country = use_signal(String::new);
    let mut unconfirmed = use_signal(|| false);
    let mut asset_class = use_signal(String::new);
    let mut direction = use_signal(String::new);
    let result = use_resource(move || {
        let series = series.clone();
        let filters = if dates() == "geo_events" {
            vec![
                ("kind", kind()),
                ("country", country()),
                ("include_unconfirmed", unconfirmed().to_string()),
            ]
        } else {
            vec![("asset_class", asset_class()), ("direction", direction())]
        };
        let d = dates();
        async move { api::markets::rule_test_on_events(&d, &series, &filters).await }
    });
    let read = result.read();
    rsx! {
        div { class: SECTION, "data-testid": "onto-event-rule",
            label { class: "text-sm text-gray-700 space-y-1 block",
                span { class: "block font-medium", {t!("onto-event-rule")} }
                select {
                    class: INPUT,
                    "data-testid": "onto-event-rule-dates",
                    onchange: move |e| dates.set(e.value()),
                    option {
                        value: "geo_events",
                        selected: dates() == "geo_events",
                        {t!("onto-event-rule-world")}
                    }
                    option {
                        value: "market_moves",
                        selected: dates() == "market_moves",
                        {t!("onto-event-rule-moves")}
                    }
                }
            }
            if dates() == "geo_events" {
                div { class: "flex flex-wrap gap-3 items-end",
                    select { class: INPUT, onchange: move |e| kind.set(e.value()),
                        option { value: "", selected: kind().is_empty(),
                            {t!("onto-event-rule-any-kind")}
                        }
                        for k in GEO_KINDS {
                            option {
                                key: "{k}",
                                value: "{k}",
                                selected: kind() == k,
                                {rule_label(&format!("onto-geo-kind-{}", k.replace('_', "-")))}
                            }
                        }
                    }
                    input {
                        class: INPUT,
                        placeholder: t!("onto-event-rule-country"),
                        value: "{country}",
                        onchange: move |e| country.set(e.value().trim().to_uppercase()),
                    }
                    label { class: "text-sm text-gray-700 flex items-center gap-2",
                        input {
                            r#type: "checkbox",
                            checked: unconfirmed(),
                            onchange: move |e| unconfirmed.set(e.checked()),
                        }
                        {t!("onto-event-rule-unconfirmed")}
                    }
                }
            } else {
                div { class: "flex flex-wrap gap-3",
                    select {
                        class: INPUT,
                        onchange: move |e| asset_class.set(e.value()),
                        option { value: "", selected: asset_class().is_empty(),
                            {t!("onto-event-rule-any-class")}
                        }
                        for c in ASSET_CLASSES {
                            option {
                                key: "{c}",
                                value: "{c}",
                                selected: asset_class() == c,
                                {rule_label(&format!("onto-asset-class-{}", c.replace('_', "-")))}
                            }
                        }
                    }
                    select {
                        class: INPUT,
                        onchange: move |e| direction.set(e.value()),
                        option { value: "", selected: direction().is_empty(),
                            {t!("onto-event-rule-any-direction")}
                        }
                        option { value: "up", selected: direction() == "up",
                            {t!("onto-event-rule-up")}
                        }
                        option { value: "down", selected: direction() == "down",
                            {t!("onto-event-rule-down")}
                        }
                    }
                }
            }
            p { class: "text-xs text-gray-500", {t!("onto-event-rule-known-at")} }
            match &*read {
                Some(Err(e)) => rsx! {
                    p { class: "text-sm text-red-700", {crate::api::error_i18n::localize(e)} }
                },
                Some(Ok(r)) => rsx! {
                    p { class: "text-sm text-gray-700",
                        {t!("onto-event-rule-counts", events : r.filings, merged : r.merged)}
                    }
                    if let Some(share) = r.unconfirmed_share {
                        p { class: "text-sm text-amber-700",
                            {t!("onto-event-rule-unconfirmed-share", share : format!("{:.0}", share * 100.0))}
                        }
                    }
                    RuleTable { result: r.clone(), on_events: true }
                },
                None => rsx! {},
            }
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
                    RuleTable { result: r.clone(), on_events: false }
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
fn RuleTable(result: RuleResult, on_events: bool) -> Element {
    if result.benchmark_missing {
        return rsx! {
            p { class: "text-sm text-gray-500", {t!("onto-reaction-no-benchmark")} }
        };
    }
    rsx! {
        if !on_events {
            p { class: "text-sm text-gray-700",
                {
                    t!(
                        "onto-reaction-rule-counts", filings : result.filings, instruments : result
                        .instruments
                    )
                }
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
