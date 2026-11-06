//! A widget showing an aggregate of objects (object-aggregates.md D584). It
//! reads on its own timer, not in the `series-batch`, then hands the value
//! to the same body as a series: thresholds, formats and symbols stay theirs.

use std::time::Duration;

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::ontology::aggregate::{
    reads_object, AggregateGroup, AggregateResult, AggregateValue, ObjectsSource, SetAggregateQuery,
};
use pnex_core::{TelemetryPoint, Widget};

use crate::api;
use crate::components::app_widgets::AppRunCx;
use crate::components::ontology::{key_label, tr};
use crate::util::sleep;

const POLL_SECS: u64 = 60;

async fn read(
    o: &ObjectsSource,
    app: Option<(String, std::collections::BTreeMap<String, String>)>,
) -> Result<AggregateResult, crate::api::error::ApiError> {
    if let Some(t) = &o.transitions {
        return api::ontology::transitions(t).await;
    }
    match (&o.object_set, &o.query, app) {
        (Some(set), _, Some((id, params))) => {
            let req = SetAggregateQuery {
                params,
                group_by: o.group_by.clone(),
                metrics: o.metrics.clone(),
                limit: o.limit,
            };
            api::apps::aggregate_set(&id, set, &req).await
        }
        // A type dashboard with no object picked yet.
        (_, Some(q), _) if reads_object(q) => Ok(AggregateResult::default()),
        (_, Some(q), _) => api::ontology::aggregate(&o.request(q.clone())).await,
        // A set outside its application: nothing to read.
        _ => Ok(AggregateResult::default()),
    }
}

/// `10 EUR · 5 USD`, in a fixed order.
fn money_text(v: &AggregateValue, decimals: u8) -> String {
    let d = decimals as usize;
    v.by_currency
        .iter()
        .map(|(c, x)| format!("{x:.d$} {c}"))
        .collect::<Vec<_>>()
        .join(" · ")
}

/// The number of a value: its value, or its only currency's.
fn single(v: &AggregateValue) -> Option<f64> {
    v.value.or_else(|| match v.by_currency.len() {
        1 => v.by_currency.values().next().copied(),
        _ => None,
    })
}

/// `YYYY-MM-DD` as seconds, for a line.
fn day_secs(day: &str) -> Option<f64> {
    let d = chrono::NaiveDate::parse_from_str(day, "%Y-%m-%d").ok()?;
    Some(d.and_hms_opt(0, 0, 0)?.and_utc().timestamp() as f64)
}

/// What a group is called: the object's title, the enum's label, the date;
/// the objects without a value have their own line.
fn group_label(
    g: &AggregateGroup,
    types: &[pnex_core::ontology::api::ObjectTypeView],
    type_key: Option<&str>,
    property: Option<&str>,
) -> String {
    if let Some(l) = g.labels.first().cloned().flatten() {
        return l;
    }
    let Some(key) = g.key.first().cloned().flatten() else {
        return t!("objects-no-value").to_string();
    };
    let def = type_key
        .and_then(|t| types.iter().find(|v| v.def.key == t))
        .and_then(|v| {
            v.def
                .properties
                .iter()
                .find(|p| Some(p.key.as_str()) == property)
        });
    match def.and_then(|p| p.labels.get(&key)) {
        Some(names) => tr(&key_label(&key), names),
        None => key,
    }
}

#[component]
pub fn ObjectsBody(widget: Widget) -> Element {
    // A screen a pack ships names its widgets per language.
    let mut widget = widget;
    if widget.title.trim().is_empty() {
        if let Some(a) = &widget.options.app {
            widget.title = tr("", &a.titles);
        }
    }
    // A count has no decimals, whatever the widget's default.
    if widget.options.decimals.is_none() {
        let shown = widget
            .source
            .first()
            .and_then(|s| s.objects.as_ref())
            .and_then(|o| o.metrics.iter().find(|m| m.key == o.show));
        if shown.and_then(|m| m.op).is_some_and(|op| !op.numeric()) {
            widget.options.decimals = Some(0);
        }
    }
    let cx = try_use_context::<AppRunCx>();
    let mut tick = use_signal(|| 0u32);
    let mut polling = use_signal(|| false);
    // Props are not tracked by the resource: mirror them (see range_bars).
    let mut current = use_signal(|| widget.clone());
    if *current.peek() != widget {
        current.set(widget.clone());
    }
    let run_tick = cx.as_ref().map(|c| c.tick);
    let app_cx = cx;
    let data = use_resource(move || {
        let w = current();
        let _ = tick();
        let _ = run_tick.map(|t| t());
        let app = app_cx.as_ref().map(|c| {
            let vars = c.vars.read().clone();
            (c.app.read().id.clone(), vars)
        });
        async move {
            let o = w.source.first()?.objects.clone()?;
            // Only the parameters of this source, filled from the variables.
            let app = app.map(|(id, vars)| {
                let params = o
                    .params
                    .iter()
                    .filter_map(|(p, v)| {
                        let value = vars.get(v).filter(|x| !x.is_empty())?;
                        Some((p.clone(), value.clone()))
                    })
                    .collect::<std::collections::BTreeMap<_, _>>();
                (id, params)
            });
            // A parameter left empty is said below, not sent (D508).
            if app.as_ref().is_some_and(|(_, p)| p.len() < o.params.len()) {
                return None;
            }
            Some(read(&o, app).await)
        }
    });
    if !polling() {
        polling.set(true);
        spawn(async move {
            sleep(Duration::from_secs(POLL_SECS)).await;
            polling.set(false);
            tick.with_mut(|t| *t += 1);
        });
    }
    let types = use_resource(|| async { api::ontology::types().await.unwrap_or_default() });

    let Some(o) = widget.source.first().and_then(|s| s.objects.clone()) else {
        return rsx! {};
    };
    // A parameter left empty: said, not sent (D508).
    let pending = cx.as_ref().is_some_and(|c| {
        let vars = c.vars.read();
        o.params
            .values()
            .any(|v| vars.get(v).is_none_or(String::is_empty))
    });
    if pending {
        return rsx! {
            p { class: "p-3 text-xs text-gray-500", {t!("apps-filter-pending")} }
        };
    }
    let state = data.read().clone().flatten();
    let result = match state {
        None => return rsx! {},
        Some(Err(e)) => {
            return rsx! {
                p {
                    class: "p-3 text-xs text-gray-500",
                    "data-testid": "objects-error",
                    {crate::api::error_i18n::localize(&e)}
                }
            };
        }
        Some(Ok(r)) => r,
    };
    let value_of = |g: &AggregateGroup| g.values.get(&o.show).cloned().unwrap_or_default();

    if widget.widget_type == pnex_core::RANGE_BARS {
        let all_types = types.read().clone().unwrap_or_default();
        let set_type = cx.as_ref().and_then(|c| {
            let app = c.app.read();
            app.object_sets
                .get(o.object_set.as_deref()?)?
                .query
                .type_key
                .clone()
        });
        let type_key = o
            .query
            .as_ref()
            .and_then(|q| q.type_key.clone())
            .or(set_type)
            .or(o.transitions.as_ref().map(|t| t.type_key.clone()));
        let property = o
            .group_by
            .first()
            .and_then(|g| g.property.clone())
            .or(o.transitions.as_ref().map(|t| t.property.clone()));
        // A funnel bar counts what passed a value: it filters nothing (D726).
        let pick = widget
            .options
            .app
            .as_ref()
            .and_then(|a| a.variable.clone())
            .filter(|v| !v.is_empty() && o.transitions.is_none() && cx.is_some());
        return rsx! {
            ObjectsBars {
                widget: widget.clone(),
                pick,
                rows: result
                    .groups
                    .iter()
                    .chain(result.others.iter())
                    .enumerate()
                    .map(|(i, g)| {
                        let is_others = i >= result.groups.len();
                        let label = if is_others {
                            t!("objects-others").to_string()
                        } else {
                            group_label(g, &all_types, type_key.as_deref(), property.as_deref())
                        };
                        // "Others" and "no value" have no value to filter on.
                        let key = g.key.first().cloned().flatten().filter(|_| !is_others);
                        (key, label, value_of(g))
                    })
                    .collect::<Vec<_>>(),
                truncated: result.truncated,
            }
        };
    }

    if widget.widget_type == "line" {
        let points: Vec<TelemetryPoint> = result
            .groups
            .iter()
            .filter_map(|g| {
                let ts = day_secs(g.key.first()?.as_deref()?)?;
                Some(TelemetryPoint {
                    ts,
                    value: single(&value_of(g))?,
                })
            })
            .collect();
        return rsx! {
            crate::components::dashboard_widget::WidgetBody { widget: widget.clone(), points, values: None }
        };
    }

    let value = result.groups.first().map(value_of).unwrap_or_default();
    // Several currencies have no single number: they are written out.
    if value.by_currency.len() > 1 {
        let decimals = widget.options.decimals.unwrap_or(0);
        return rsx! {
            div { class: "flex h-full w-full flex-col justify-center rounded-lg px-3",
                span { class: "truncate text-xs font-medium text-gray-500", "{widget.title}" }
                span {
                    class: "text-lg font-semibold tabular-nums",
                    "data-testid": "objects-money",
                    {money_text(&value, decimals)}
                }
            }
        };
    }
    let mut shown = widget.clone();
    if let Some(c) = value.by_currency.keys().next() {
        shown.options.unit.get_or_insert_with(|| c.clone());
    }
    let points: Vec<TelemetryPoint> = single(&value)
        .map(|v| TelemetryPoint {
            ts: crate::util::now_secs(),
            value: v,
        })
        .into_iter()
        .collect();
    rsx! {
        crate::components::dashboard_widget::WidgetBody { widget: shown, points, values: None }
    }
}

/// One line per group: the label, a bar proportional to the value, the value.
/// With `pick`, a click on a line writes its value into that variable of the
/// application, and a second click clears it (D726).
#[component]
fn ObjectsBars(
    widget: Widget,
    rows: Vec<(Option<String>, String, AggregateValue)>,
    truncated: bool,
    #[props(default)] pick: Option<String>,
) -> Element {
    let cx = try_use_context::<AppRunCx>();
    let picked = match (&cx, &pick) {
        (Some(c), Some(v)) => c.vars.read().get(v).cloned().filter(|x| !x.is_empty()),
        _ => None,
    };
    let decimals = widget.options.decimals.unwrap_or(0);
    let d = decimals as usize;
    let unit = widget.options.unit.clone().unwrap_or_default();
    let max = rows
        .iter()
        .filter_map(|(_, _, v)| single(v))
        .fold(0.0_f64, f64::max);
    rsx! {
        div {
            class: "flex h-full w-full flex-col overflow-auto rounded-lg p-3",
            "data-testid": "objects-bars",
            if !widget.title.trim().is_empty() {
                span { class: "mb-2 truncate text-xs font-medium text-gray-500", "{widget.title}" }
            }
            if rows.is_empty() {
                p { class: "text-xs text-gray-500", {t!("objects-empty")} }
            }
            for (key, label, v) in rows.iter() {
                {
                    let pct = crate::components::range_bars::bar_pct(single(v), max);
                    let text = match single(v) {
                        Some(x) if v.by_currency.is_empty() => format!("{x:.d$} {unit}"),
                        _ if !v.by_currency.is_empty() => money_text(v, decimals),
                        _ => "—".to_string(),
                    };
                    let on = key.is_some() && *key == picked;
                    let target = match (&cx, &pick, key) {
                        (Some(c), Some(var), Some(k)) => Some((c.vars, var.clone(), k.clone())),
                        _ => None,
                    };
                    let class = if on {
                        "mb-0.5 block w-full rounded px-1 py-1 text-left text-xs bg-signal-50 font-medium"
                    } else {
                        "mb-0.5 block w-full rounded px-1 py-1 text-left text-xs hover:bg-gray-50"
                    };
                    rsx! {
                        if let Some((mut vars, var, k)) = target {
                            button {
                                r#type: "button",
                                class,
                                "aria-pressed": "{on}",
                                "data-testid": "objects-bar",
                                onclick: move |_| {
                                    let (var, k) = (var.clone(), k.clone());
                                    vars.with_mut(|m| {
                                        if on {
                                            m.remove(&var);
                                        } else {
                                            m.insert(var, k);
                                        }
                                    });
                                },
                                BarLine { label: label.clone(), text: text.clone(), pct }
                            }
                        } else {
                            div { class: "mb-0.5 px-1 py-1 text-xs", "data-testid": "objects-bar",
                                BarLine { label: label.clone(), text, pct }
                            }
                        }
                    }
                }
            }
            if truncated {
                p { class: "mt-1 text-xs text-gray-500", {t!("objects-truncated")} }
            }
        }
    }
}

#[component]
fn BarLine(label: String, text: String, pct: f64) -> Element {
    rsx! {
        div { class: "flex justify-between gap-2",
            span { class: "truncate", "{label}" }
            span { class: "shrink-0 tabular-nums", "{text}" }
        }
        div { class: "mt-0.5 h-1.5 rounded bg-gray-200",
            div {
                class: "h-full rounded bg-signal-500",
                style: "width: {pct}%;",
            }
        }
    }
}
