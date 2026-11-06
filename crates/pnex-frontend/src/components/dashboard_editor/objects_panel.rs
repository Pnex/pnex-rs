//! Inspector section: a widget shows numbers over objects instead of a series
//! (object-aggregates.md D583). The form covers one calculation and one split;
//! a richer source (a pack's ratio, several metrics, two hops) stays as it is
//! and is only summed up here.

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::ontology::aggregate::{
    reads_object, DateBucket, GroupKey, Metric, MetricOp, ObjectsSource, VALUE_OBJECT,
};
use pnex_core::ontology::api::{Direction, FilterOp, Hop, OntologyQuery, PropertyFilter};
use pnex_core::ontology::{PropertyKind, TypeSet};
use pnex_core::{SourceRef, Widget};

use super::EditorCx;
use crate::api;
use crate::components::ontology::filter_row::FilterRow;
use crate::components::ontology::property_input::prop_label;
use crate::components::ontology::{key_label, type_label};
use crate::components::ui::{FIELD_LABEL, INPUT};

const SHOWN: &str = "value";

fn op_key(op: MetricOp) -> &'static str {
    match op {
        MetricOp::Count => "count",
        MetricOp::CountDistinct => "count_distinct",
        MetricOp::Sum => "sum",
        MetricOp::Avg => "avg",
        MetricOp::Min => "min",
        MetricOp::Max => "max",
    }
}

fn op_of(key: &str) -> MetricOp {
    match key {
        "sum" => MetricOp::Sum,
        "avg" => MetricOp::Avg,
        "min" => MetricOp::Min,
        "max" => MetricOp::Max,
        _ => MetricOp::Count,
    }
}

fn op_label(op: MetricOp) -> String {
    match op {
        MetricOp::Sum => t!("objects-op-sum").to_string(),
        MetricOp::Avg => t!("objects-op-avg").to_string(),
        MetricOp::Min => t!("objects-op-min").to_string(),
        MetricOp::Max => t!("objects-op-max").to_string(),
        _ => t!("objects-op-count").to_string(),
    }
}

fn metric(op: MetricOp, property: Option<String>) -> Metric {
    Metric {
        key: SHOWN.into(),
        op: Some(op),
        property,
        weight: None,
        filter: None,
        ratio: vec![],
        percent: false,
    }
}

/// The split as a select value: `p:<key>`, `d:<key>`, `l:<link>:out|in`.
fn split_value(g: Option<&GroupKey>) -> String {
    match g {
        Some(GroupKey {
            property: Some(k),
            bucket: Some(_),
            ..
        }) => format!("d:{k}"),
        Some(GroupKey {
            property: Some(k), ..
        }) => format!("p:{k}"),
        Some(g) => g
            .hops
            .first()
            .map(|h| {
                let dir = if h.direction == Direction::In {
                    "in"
                } else {
                    "out"
                };
                format!("l:{}:{dir}", h.link_type)
            })
            .unwrap_or_default(),
        None => String::new(),
    }
}

fn split_of(value: &str, bucket: DateBucket) -> Vec<GroupKey> {
    let mut parts = value.splitn(3, ':');
    let key = match (parts.next(), parts.next(), parts.next()) {
        (Some("p"), Some(k), _) => GroupKey {
            property: Some(k.into()),
            ..Default::default()
        },
        (Some("d"), Some(k), _) => GroupKey {
            property: Some(k.into()),
            bucket: Some(bucket),
            ..Default::default()
        },
        (Some("l"), Some(l), Some(d)) => GroupKey {
            hops: vec![Hop {
                link_type: l.into(),
                direction: if d == "in" {
                    Direction::In
                } else {
                    Direction::Out
                },
            }],
            ..Default::default()
        },
        _ => return Vec::new(),
    };
    vec![key]
}

fn bucket_of(key: &str) -> DateBucket {
    match key {
        "day" => DateBucket::Day,
        "week" => DateBucket::Week,
        "quarter" => DateBucket::Quarter,
        "year" => DateBucket::Year,
        _ => DateBucket::Month,
    }
}

/// The filters on the objects the widget counts: those a link reaches, when
/// the query walks one.
fn filter_list(q: &OntologyQuery) -> &Vec<PropertyFilter> {
    if q.traverse.is_empty() {
        &q.filters
    } else {
        &q.reached
    }
}

fn filter_list_mut(q: &mut OntologyQuery) -> &mut Vec<PropertyFilter> {
    if q.traverse.is_empty() {
        &mut q.filters
    } else {
        &mut q.reached
    }
}

fn first_type(set: &TypeSet) -> Option<String> {
    match set {
        TypeSet::Only(keys) => keys.first().cloned(),
        TypeSet::Any => None,
    }
}

fn def_props(
    types: &[pnex_core::ontology::api::ObjectTypeView],
    key: Option<&str>,
) -> Vec<pnex_core::ontology::PropertyDef> {
    key.and_then(|k| types.iter().find(|v| v.def.key == k))
        .map(|v| v.def.properties.clone())
        .unwrap_or_default()
}

/// A source named by its first field set, the rest empty.
fn blank(metric_op: MetricOp) -> ObjectsSource {
    ObjectsSource {
        metrics: vec![metric(metric_op, None)],
        show: SHOWN.into(),
        ..Default::default()
    }
}

#[component]
pub fn ObjectsSourcePanel(cx: EditorCx, widget: Widget, can_write: bool) -> Element {
    let mut layout = cx.layout;
    let types = use_resource(|| async { api::ontology::types().await.unwrap_or_default() });
    let links = use_resource(|| async { api::ontology::link_types().await.unwrap_or_default() });
    let source = widget.source.first().and_then(|s| s.objects.clone());
    let app = cx.app.read().clone();
    // Set on a type dashboard (D187): an aggregate can follow the shown object.
    let shown_type = layout.read().object_type.clone();
    // A signal keeps the closures below `Copy`, one per handler.
    let mut id = use_signal(String::new);
    if *id.peek() != widget.id {
        id.set(widget.id.clone());
    }
    let mut edit = move |f: &mut dyn FnMut(&mut Vec<SourceRef>)| {
        let id = id.peek().clone();
        layout.with_mut(|l| {
            if let Some(w) = l.widgets.iter_mut().find(|w| w.id == id) {
                f(&mut w.source);
            }
        });
    };
    let mut change = move |f: &mut dyn FnMut(&mut ObjectsSource)| {
        edit(&mut |sources| {
            if let Some(o) = sources.first_mut().and_then(|s| s.objects.as_mut()) {
                f(o);
            }
        });
    };

    let Some(o) = source else {
        return rsx! {
            div { class: "mb-3",
                button {
                    class: "w-full rounded border border-gray-300 px-2 py-1.5 text-sm",
                    "data-testid": "objects-source-on",
                    disabled: !can_write,
                    onclick: move |_| {
                        edit(
                            &mut |sources| {
                                sources.clear();
                                sources
                                    .push(SourceRef {
                                        role: "primary".into(),
                                        metric: String::new(),
                                        device_id: String::new(),
                                        window: "1h".into(),
                                        memory: None,
                                        labels: Default::default(),
                                        object_property: None,
                                        objects: Some(blank(MetricOp::Count)),
                                    });
                            },
                        );
                    },
                    {t!("objects-source-on")}
                }
            }
        };
    };

    let all_types = types.read().clone().unwrap_or_default();
    let all_links = links.read().clone().unwrap_or_default();
    let type_key = match (&o.object_set, &o.query, &app) {
        (Some(set), _, Some(a)) => a
            .object_sets
            .get(set)
            .and_then(|s| s.query.type_key.clone()),
        // What a link reaches, when the query walks one (several end types:
        // the first; the save refuses a property the others lack).
        (_, Some(q), _) => match q.traverse.last() {
            Some(hop) => all_links
                .iter()
                .find(|l| l.def.key == hop.link_type)
                .and_then(|l| match hop.direction {
                    Direction::Out => first_type(&l.def.to_types),
                    Direction::In => first_type(&l.def.from_types),
                }),
            None => q.type_key.clone(),
        },
        _ => None,
    };
    let filters = o
        .query
        .as_ref()
        .map(|q| filter_list(q).clone())
        .unwrap_or_default();
    let follows_object = o.query.as_ref().is_some_and(reads_object);
    let scope = o
        .query
        .as_ref()
        .filter(|_| follows_object)
        .and_then(|q| q.traverse.first())
        .map(|h| {
            split_value(Some(&GroupKey {
                hops: vec![h.clone()],
                ..Default::default()
            }))
        })
        .unwrap_or_default();
    let scopes: Vec<(String, String)> = shown_type
        .iter()
        .flat_map(|t| {
            all_links.iter().flat_map(move |l| {
                let name = |n, names| crate::components::ontology::tr(n, names);
                let mut out = Vec::new();
                if l.def.from_types.contains(t) {
                    out.push((
                        format!("l:{}:out", l.def.key),
                        name(&l.def.name, &l.def.names),
                    ));
                }
                if l.def.to_types.contains(t) {
                    out.push((
                        format!("l:{}:in", l.def.key),
                        name(&l.def.inverse_name, &l.def.inverse_names),
                    ));
                }
                out
            })
        })
        .collect();
    let filter_props: Vec<(String, String)> = def_props(&all_types, type_key.as_deref())
        .iter()
        .filter(|p| !p.kind.is_temporal())
        .map(|p| (p.key.clone(), prop_label(p)))
        .collect();
    let first_prop = filter_props.first().map(|(k, _)| k.clone());
    let def = type_key
        .as_ref()
        .and_then(|t| all_types.iter().find(|v| &v.def.key == t))
        .map(|v| v.def.clone());
    let props = def
        .as_ref()
        .map(|d| d.properties.clone())
        .unwrap_or_default();
    let numeric: Vec<_> = props
        .iter()
        .filter(|p| matches!(p.kind, PropertyKind::Number { .. } | PropertyKind::Money))
        .cloned()
        .collect();
    let splits: Vec<(String, String)> = props
        .iter()
        .filter_map(|p| match p.kind {
            PropertyKind::Date | PropertyKind::DateTime => Some((
                format!("d:{}", p.key),
                t!("objects-split-date", name: prop_label(p)).to_string(),
            )),
            PropertyKind::Enum { .. } | PropertyKind::Text { .. } | PropertyKind::Bool => {
                Some((format!("p:{}", p.key), prop_label(p)))
            }
            _ => None,
        })
        .chain(all_links.iter().flat_map(|l| {
            let k = type_key.clone().unwrap_or_default();
            let mut out = Vec::new();
            if l.def.from_types.contains(&k) {
                out.push((
                    format!("l:{}:out", l.def.key),
                    crate::components::ontology::tr(&l.def.name, &l.def.names),
                ));
            }
            if l.def.to_types.contains(&k) {
                out.push((
                    format!("l:{}:in", l.def.key),
                    crate::components::ontology::tr(&l.def.inverse_name, &l.def.inverse_names),
                ));
            }
            out
        }))
        .collect();
    let simple = o.metrics.len() == 1 && o.group_by.len() <= 1 && o.metrics[0].filter.is_none();
    let current = o
        .metrics
        .first()
        .cloned()
        .unwrap_or_else(|| metric(MetricOp::Count, None));
    let current_op = current.op.unwrap_or(MetricOp::Count);
    let split = split_value(o.group_by.first());
    // In an application, a bar click can write the enum variable of the
    // property the bars split by (sales-dashboards.md D726).
    let bar_var = widget.options.app.as_ref().and_then(|a| a.variable.clone());
    let bar_vars: Vec<String> = match (&app, split.strip_prefix("p:")) {
        (Some(a), Some(prop)) if widget.widget_type == pnex_core::RANGE_BARS => a
            .variables
            .iter()
            .filter(|v| {
                v.kind == pnex_core::apps::VariableKind::Enum
                    && v.property.as_deref() == Some(prop)
                    && v.type_key == type_key
            })
            .map(|v| v.name.clone())
            .collect(),
        _ => Vec::new(),
    };
    let bucket = o
        .group_by
        .first()
        .and_then(|g| g.bucket)
        .unwrap_or(DateBucket::Month);
    let needs_property = current_op.numeric();

    rsx! {
        div {
            class: "mb-3 flex flex-col gap-2",
            "data-testid": "objects-source",
            label { class: FIELD_LABEL, {t!("objects-source-title")} }
            if let Some(a) = &app {
                select {
                    class: INPUT,
                    "data-testid": "objects-set",
                    disabled: !can_write,
                    onchange: move |e| {
                        let v = e.value();
                        change(
                            &mut |s| {
                                s.query = None;
                                s.object_set = (!v.is_empty()).then(|| v.clone());
                            },
                        );
                    },
                    option { value: "", selected: o.object_set.is_none(), {t!("objects-pick-set")} }
                    for name in a.object_sets.keys() {
                        option {
                            key: "{name}",
                            value: "{name}",
                            selected: o.object_set.as_deref() == Some(name.as_str()),
                            "{name}"
                        }
                    }
                }
            } else {
                if !scopes.is_empty() {
                    select {
                        class: INPUT,
                        "data-testid": "objects-scope",
                        disabled: !can_write,
                        onchange: move |e| {
                            let hops = split_of(&e.value(), DateBucket::Month)
                                .pop()
                                .map(|g| g.hops)
                                .unwrap_or_default();
                            let shown = shown_type.clone();
                            change(
                                &mut |s| {
                                    s.object_set = None;
                                    s.query = (!hops.is_empty())
                                        .then(|| OntologyQuery {
                                            type_key: shown.clone(),
                                            ids: vec![VALUE_OBJECT.into()],
                                            traverse: hops.clone(),
                                            ..Default::default()
                                        });
                                    s.group_by.clear();
                                    s.metrics = vec![metric(MetricOp::Count, None)];
                                    s.show = SHOWN.into();
                                },
                            );
                        },
                        option { value: "", selected: scope.is_empty(), {t!("objects-scope-all")} }
                        for (value, label) in scopes.iter() {
                            option {
                                key: "{value}",
                                value: "{value}",
                                selected: &scope == value,
                                {t!("objects-scope-link", name : label.clone())}
                            }
                        }
                    }
                }
            }
            if app.is_none() && !follows_object {
                select {
                    class: INPUT,
                    "data-testid": "objects-type",
                    disabled: !can_write,
                    onchange: move |e| {
                        let v = e.value();
                        change(
                            &mut |s| {
                                s.object_set = None;
                                s.query = (!v.is_empty()) // What named the old type no longer means anything.
                                    .then(|| OntologyQuery {
                                        type_key: Some(v.clone()),
                                        ..Default::default()
                                    });
                                s.group_by.clear();
                                s.metrics = vec![metric(MetricOp::Count, None)];
                                s.show = SHOWN.into();
                            },
                        );
                    },
                    option { value: "", selected: type_key.is_none(), {t!("objects-pick-type")} }
                    for v in all_types.iter() {
                        option {
                            key: "{v.def.key}",
                            value: "{v.def.key}",
                            selected: type_key.as_deref() == Some(v.def.key.as_str()),
                            {type_label(&v.def)}
                        }
                    }
                }
            }
            if o.query.is_some() {
                label { class: FIELD_LABEL, {t!("objects-filters")} }
                for (i, f) in filters.iter().cloned().enumerate() {
                    FilterRow {
                        key: "{i}",
                        i,
                        filter: f,
                        props: filter_props.clone(),
                        placeholder: t!("objects-filter-value"),
                        disabled: !can_write,
                        on_change: move |next: PropertyFilter| {
                            change(
                                &mut |s| {
                                    if let Some(x) = s
                                        .query
                                        .as_mut()
                                        .and_then(|q| filter_list_mut(q).get_mut(i))
                                    {
                                        *x = next.clone();
                                    }
                                },
                            );
                        },
                        on_remove: move |_| {
                            change(
                                &mut |s| {
                                    if let Some(q) = s.query.as_mut() {
                                        filter_list_mut(q).remove(i);
                                    }
                                },
                            );
                        },
                    }
                }
                button {
                    class: "text-left text-xs text-gray-500 underline",
                    "data-testid": "objects-filter-add",
                    disabled: !can_write || first_prop.is_none(),
                    onclick: move |_| {
                        let key = first_prop.clone().unwrap_or_default();
                        change(
                            &mut |s| {
                                if let Some(q) = s.query.as_mut() {
                                    filter_list_mut(q)
                                        .push(PropertyFilter {
                                            key: key.clone(),
                                            op: FilterOp::Eq,
                                            value: "".into(),
                                        });
                                }
                            },
                        );
                    },
                    {t!("objects-filter-add")}
                }
            }
            if simple {
                select {
                    class: INPUT,
                    "data-testid": "objects-op",
                    disabled: !can_write,
                    onchange: move |e| {
                        let op = op_of(&e.value());
                        change(
                            &mut |s| {
                                let keep = s.metrics.first().and_then(|m| m.property.clone());
                                s.metrics = vec![
                                    metric(op, if op.numeric() { keep.clone() } else { None }),
                                ];
                            },
                        );
                    },
                    for op in [MetricOp::Count, MetricOp::Sum, MetricOp::Avg, MetricOp::Min, MetricOp::Max] {
                        option {
                            key: "{op_key(op)}",
                            value: op_key(op),
                            selected: current_op == op,
                            {op_label(op)}
                        }
                    }
                }
                if needs_property {
                    select {
                        class: INPUT,
                        "data-testid": "objects-property",
                        disabled: !can_write,
                        onchange: move |e| {
                            let v = e.value();
                            change(
                                &mut |s| {
                                    if let Some(m) = s.metrics.first_mut() {
                                        m.property = (!v.is_empty()).then(|| v.clone());
                                    }
                                },
                            );
                        },
                        option { value: "", selected: current.property.is_none(),
                            {t!("objects-pick-property")}
                        }
                        for p in numeric.iter() {
                            option {
                                key: "{p.key}",
                                value: "{p.key}",
                                selected: current.property.as_deref() == Some(p.key.as_str()),
                                {prop_label(p)}
                            }
                        }
                    }
                }
                select {
                    class: INPUT,
                    "data-testid": "objects-split",
                    disabled: !can_write,
                    onchange: move |e| {
                        let v = e.value();
                        change(&mut |s| s.group_by = split_of(&v, bucket));
                        // The bar variable belongs to the old split.
                        let id = id.peek().clone();
                        layout
                            .with_mut(|l| {
                                if let Some(a) = l
                                    .widgets
                                    .iter_mut()
                                    .find(|w| w.id == id)
                                    .and_then(|w| w.options.app.as_mut())
                                {
                                    a.variable = None;
                                }
                            });
                    },
                    option { value: "", selected: split.is_empty(), {t!("objects-no-split")} }
                    for (value, label) in splits.iter() {
                        option {
                            key: "{value}",
                            value: "{value}",
                            selected: &split == value,
                            "{label}"
                        }
                    }
                }
                if split.starts_with("d:") {
                    select {
                        class: INPUT,
                        "data-testid": "objects-bucket",
                        disabled: !can_write,
                        onchange: move |e| {
                            let b = bucket_of(&e.value());
                            change(
                                &mut |s| {
                                    if let Some(g) = s.group_by.first_mut() {
                                        g.bucket = Some(b);
                                    }
                                },
                            );
                        },
                        for (k, label) in [
                            ("day", t!("objects-bucket-day")),
                            ("week", t!("objects-bucket-week")),
                            ("month", t!("objects-bucket-month")),
                            ("quarter", t!("objects-bucket-quarter")),
                            ("year", t!("objects-bucket-year")),
                        ]
                        {
                            option {
                                key: "{k}",
                                value: k,
                                selected: bucket.sql() == k,
                                "{label}"
                            }
                        }
                    }
                }
                if !bar_vars.is_empty() {
                    label { class: "text-xs text-gray-500", {t!("insp-app-bar-variable")} }
                    select {
                        class: INPUT,
                        "data-testid": "objects-bar-variable",
                        disabled: !can_write,
                        onchange: move |e| {
                            let v = e.value();
                            let id = id.peek().clone();
                            layout
                                .with_mut(|l| {
                                    if let Some(w) = l.widgets.iter_mut().find(|w| w.id == id) {
                                        w.options.app.get_or_insert_with(Default::default).variable = (!v
                                            .is_empty())
                                            .then(|| v.clone());
                                    }
                                });
                        },
                        option { value: "", selected: bar_var.is_none(), {t!("insp-app-none")} }
                        for v in bar_vars.iter() {
                            option {
                                key: "{v}",
                                value: "{v}",
                                selected: bar_var.as_deref() == Some(v.as_str()),
                                "{v}"
                            }
                        }
                    }
                }
            } else {
                p {
                    class: "text-xs text-gray-500",
                    "data-testid": "objects-advanced",
                    {
                        t!(
                            "objects-advanced", metrics : o.metrics.iter().map(| m | key_label(& m.key))
                            .collect::< Vec < _ >> ().join(", ")
                        )
                    }
                }
            }
            button {
                class: "text-left text-xs text-gray-500 underline",
                "data-testid": "objects-source-off",
                disabled: !can_write,
                onclick: move |_| edit(&mut |sources| sources.clear()),
                {t!("objects-source-off")}
            }
        }
    }
}
