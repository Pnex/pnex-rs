//! Live blocks of a page (analysis.md D831): computed here, with the rights of
//! the person reading, and drawn by doc.js. The settings dialog writes the
//! block's attributes into the page like any other edit.

use std::time::Duration;

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::analysis::{AnalysisState, REUSE_MAX_SECS};
use pnex_core::ontology::aggregate::{
    AggregateGroup, AggregateQuery, DateBucket, GroupKey, Metric, MetricOp,
};
use pnex_core::ontology::api::{FilterOp, ObjectTypeView, OntologyQuery, PropertyFilter};
use pnex_core::ontology::PropertyKind;
use pnex_core::pages::{LiveBlock, LIVE_TITLE_MAX};

use crate::api;
use crate::api::error_i18n::{localize, localize_code};
use crate::components::crud::form::FormDialog;
use crate::components::objects_widget::{group_label, money_text};
use crate::components::ontology::filter_row::FilterRow;
use crate::components::ontology::property_input::prop_label;
use crate::components::ontology::type_label;
use crate::components::ui::{BTN_SECONDARY_SM, FIELD_LABEL, INPUT};
use crate::doc_editor::{self, LiveResult};

/// Rows drawn in the page; the others are counted.
const ROWS_SHOWN: usize = 50;
/// Key of the one metric a block computes.
const METRIC: &str = "value";
/// Groups of a calculation shown, the rest summed under "Others".
const GROUPS: u32 = 20;
/// Cells of a crosstab read (row × column pairs), and its columns shown (AN4).
const PIVOT_CELLS: u32 = 1000;
const PIVOT_COLUMNS: usize = 12;
const OPS: [MetricOp; 5] = [
    MetricOp::Count,
    MetricOp::Sum,
    MetricOp::Avg,
    MetricOp::Min,
    MetricOp::Max,
];
const RADIO_ROW: &str = "flex items-start gap-3 rounded-md border border-line px-3 py-2 cursor-pointer has-[:checked]:border-signal has-[:checked]:bg-signal-50";

fn op_label(op: MetricOp) -> String {
    match op {
        MetricOp::Count | MetricOp::CountDistinct => t!("live-op-count"),
        MetricOp::Sum => t!("live-op-sum"),
        MetricOp::Avg => t!("live-op-avg"),
        MetricOp::Min => t!("live-op-min"),
        MetricOp::Max => t!("live-op-max"),
    }
    .to_string()
}

fn op_wire(op: MetricOp) -> &'static str {
    match op {
        MetricOp::Count | MetricOp::CountDistinct => "count",
        MetricOp::Sum => "sum",
        MetricOp::Avg => "avg",
        MetricOp::Min => "min",
        MetricOp::Max => "max",
    }
}

fn number(x: f64) -> String {
    if x.fract() == 0.0 && x.abs() < 1e15 {
        format!("{x:.0}")
    } else {
        format!("{x:.2}")
    }
}

fn error(note: String) -> LiveResult {
    LiveResult {
        status: "error",
        note,
        ..Default::default()
    }
}

pub fn loading() -> LiveResult {
    LiveResult {
        status: "loading",
        note: t!("live-computing").to_string(),
        ..Default::default()
    }
}

fn done(columns: Vec<String>, mut rows: Vec<Vec<String>>) -> LiveResult {
    let total = rows.len();
    rows.truncate(ROWS_SHOWN);
    LiveResult {
        status: "done",
        note: t!("live-as-of", time: crate::util::now_label()).to_string(),
        columns,
        rows,
        more: if total > ROWS_SHOWN {
            t!("live-more-rows", count: total - ROWS_SHOWN).to_string()
        } else {
            String::new()
        },
    }
}

/// The block computed now, with the reader's rights. `force` skips the
/// reuse of a SQL run of the last five minutes.
pub async fn compute(b: &LiveBlock, force: bool, types: &[ObjectTypeView]) -> LiveResult {
    match b.source.as_str() {
        "objects" => match b.aggregate() {
            Some(q) if q.query.type_key.is_some() => objects(&q, types).await,
            _ => error(t!("live-unset").to_string()),
        },
        _ if b.sql.trim().is_empty() => error(t!("live-unset").to_string()),
        _ => sql(&b.sql, force).await,
    }
}

fn property_name(types: &[ObjectTypeView], type_key: Option<&str>, key: &str) -> String {
    types
        .iter()
        .find(|v| Some(v.def.key.as_str()) == type_key)
        .and_then(|v| v.def.properties.iter().find(|p| p.key == key))
        .map(prop_label)
        .unwrap_or_else(|| key.to_string())
}

/// Header of the value column: "Count", or "Sum · Amount".
fn metric_header(m: Option<&Metric>, types: &[ObjectTypeView], type_key: Option<&str>) -> String {
    let Some(m) = m else {
        return String::new();
    };
    let op = op_label(m.op.unwrap_or(MetricOp::Count));
    match &m.property {
        Some(p) => format!("{op} · {}", property_name(types, type_key, p)),
        None => op,
    }
}

async fn objects(q: &AggregateQuery, types: &[ObjectTypeView]) -> LiveResult {
    let res = match api::ontology::aggregate(q).await {
        Ok(r) => r,
        Err(e) => return error(localize(&e)),
    };
    let type_key = q.query.type_key.as_deref();
    let by = q.group_by.first().and_then(|g| g.property.as_deref());
    let grouped = !q.group_by.is_empty();
    let metric = q.metrics.first();
    let key = metric.map(|m| m.key.as_str()).unwrap_or(METRIC);
    if let Some(across) = q.group_by.get(1).and_then(|g| g.property.as_deref()) {
        let label = |g: &AggregateGroup, i: usize, p: Option<&str>| {
            let one = AggregateGroup {
                key: vec![g.key.get(i).cloned().flatten()],
                labels: vec![g.labels.get(i).cloned().flatten()],
                ..Default::default()
            };
            group_label(&one, types, type_key, p)
        };
        let cells: Vec<(String, String, String)> = res
            .groups
            .iter()
            .map(|g| {
                let v = g.values.get(key).map(value_text).unwrap_or_default();
                (label(g, 0, by), label(g, 1, Some(across)), v)
            })
            .collect();
        let corner = by
            .map(|p| property_name(types, type_key, p))
            .unwrap_or_default();
        let (columns, rows, cut) = pivot(corner, &cells, PIVOT_COLUMNS);
        let mut out = done(columns, rows);
        if cut || res.others.is_some() {
            out.more = [out.more, t!("live-pivot-cut").to_string()]
                .join(" ")
                .trim()
                .to_string();
        }
        return out;
    }
    let mut columns = Vec::new();
    if grouped {
        columns.push(
            by.map(|p| property_name(types, type_key, p))
                .unwrap_or_default(),
        );
    }
    columns.push(metric_header(metric, types, type_key));
    let cell = |g: &AggregateGroup| g.values.get(key).map(value_text).unwrap_or_default();
    let mut rows: Vec<Vec<String>> = res
        .groups
        .iter()
        .map(|g| {
            let mut r = Vec::new();
            if grouped {
                r.push(group_label(g, types, type_key, by));
            }
            r.push(cell(g));
            r
        })
        .collect();
    if let Some(o) = &res.others {
        let mut r = vec![t!("live-others").to_string()];
        r.push(cell(o));
        rows.push(r);
    }
    done(columns, rows)
}

fn value_text(v: &pnex_core::ontology::aggregate::AggregateValue) -> String {
    match v.value {
        Some(x) => number(x),
        None => money_text(v, 2),
    }
}

/// A crosstab of `(row, column, value)` cells: rows and columns in the order they
/// first come, `max_columns` columns at most; `true` when columns were left out.
fn pivot(
    corner: String,
    cells: &[(String, String, String)],
    max_columns: usize,
) -> (Vec<String>, Vec<Vec<String>>, bool) {
    let mut columns: Vec<&str> = Vec::new();
    let mut rows: Vec<(&str, Vec<String>)> = Vec::new();
    let mut cut = false;
    for (r, c, v) in cells {
        let j = match columns.iter().position(|x| x == c) {
            Some(j) => j,
            None if columns.len() < max_columns => {
                columns.push(c);
                columns.len() - 1
            }
            None => {
                cut = true;
                continue;
            }
        };
        let i = match rows.iter().position(|(x, _)| x == r) {
            Some(i) => i,
            None => {
                rows.push((r, Vec::new()));
                rows.len() - 1
            }
        };
        let row = &mut rows[i].1;
        if row.len() <= j {
            row.resize(j + 1, String::new());
        }
        row[j] = v.clone();
    }
    let width = columns.len();
    let header = std::iter::once(corner)
        .chain(columns.iter().map(|c| c.to_string()))
        .collect();
    let body = rows
        .into_iter()
        .map(|(r, mut vs)| {
            vs.resize(width, String::new());
            std::iter::once(r.to_string()).chain(vs).collect()
        })
        .collect();
    (header, body, cut)
}

async fn sql(text: &str, force: bool) -> LiveResult {
    let age = (!force).then_some(REUSE_MAX_SECS);
    // The reader's other query (another block, the SQL tab) runs first.
    let mut tries = 0;
    let mut q = loop {
        match api::analysis::submit(text, age).await {
            Ok(q) => break q,
            Err(e) if e.code.as_deref() == Some("analysis-busy") && tries < 150 => {
                tries += 1;
                crate::util::sleep(Duration::from_millis(500)).await;
            }
            Err(e) => return error(localize(&e)),
        }
    };
    loop {
        match q.state {
            AnalysisState::Done => break,
            AnalysisState::Failed => {
                let code = q.code.clone().unwrap_or_default();
                return error(localize_code(&code, Some(&q.args), &code));
            }
            _ => {}
        }
        crate::util::sleep(Duration::from_millis(700)).await;
        q = match api::analysis::get(&q.id).await {
            Ok(q) => q,
            Err(e) => return error(localize(&e)),
        };
    }
    let rows = q
        .rows
        .into_iter()
        .map(|row| row.into_iter().map(Option::unwrap_or_default).collect())
        .collect();
    done(q.columns, rows)
}

/// Settings of a block as the dialog edits them.
#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct Draft {
    title: String,
    source: String,
    sql: String,
    type_key: String,
    op: String,
    property: String,
    /// Rows: the property the objects are split by.
    group: String,
    /// Columns of a crosstab (AN4), only with `group`.
    column: String,
    filters: Vec<PropertyFilter>,
}

impl Draft {
    fn of(b: &LiveBlock) -> Self {
        let q = b.aggregate().unwrap_or_default();
        let m = q.metrics.first();
        Self {
            title: b.title.clone(),
            source: if b.source == "objects" {
                "objects"
            } else {
                "sql"
            }
            .into(),
            sql: b.sql.clone(),
            type_key: q.query.type_key.clone().unwrap_or_default(),
            op: op_wire(m.and_then(|m| m.op).unwrap_or(MetricOp::Count)).into(),
            property: m.and_then(|m| m.property.clone()).unwrap_or_default(),
            group: q
                .group_by
                .first()
                .and_then(|g| g.property.clone())
                .unwrap_or_default(),
            column: q
                .group_by
                .get(1)
                .and_then(|g| g.property.clone())
                .unwrap_or_default(),
            filters: q.query.filters,
        }
    }

    /// A calculation on objects, as the crosstab tab starts it.
    pub(crate) fn objects() -> Self {
        Self {
            source: "objects".into(),
            op: "count".into(),
            ..Default::default()
        }
    }

    fn op(&self) -> MetricOp {
        OPS.into_iter()
            .find(|o| op_wire(*o) == self.op)
            .unwrap_or(MetricOp::Count)
    }

    pub(crate) fn block(&self, bid: &str, types: &[ObjectTypeView]) -> LiveBlock {
        let query = if self.source == "objects" && !self.type_key.is_empty() {
            let op = self.op();
            let def = types.iter().find(|v| v.def.key == self.type_key);
            let is_date = |k: &str| {
                def.and_then(|v| v.def.properties.iter().find(|p| p.key == k))
                    .is_some_and(|p| matches!(p.kind, PropertyKind::Date | PropertyKind::DateTime))
            };
            let key = |p: &String| GroupKey {
                property: Some(p.clone()),
                bucket: is_date(p).then_some(DateBucket::Month),
                hops: Vec::new(),
            };
            let pivot = !self.group.is_empty() && !self.column.is_empty();
            let q = AggregateQuery {
                query: OntologyQuery {
                    type_key: Some(self.type_key.clone()),
                    filters: self.filters.clone(),
                    ..Default::default()
                },
                group_by: [&self.group, &self.column]
                    .into_iter()
                    .take(if pivot { 2 } else { 1 })
                    .filter(|p| !p.is_empty())
                    .map(key)
                    .collect(),
                metrics: vec![Metric {
                    key: METRIC.into(),
                    op: Some(op),
                    property: (op != MetricOp::Count && !self.property.is_empty())
                        .then(|| self.property.clone()),
                    weight: None,
                    filter: None,
                    ratio: Vec::new(),
                    percent: false,
                }],
                limit: Some(if pivot { PIVOT_CELLS } else { GROUPS }),
            };
            serde_json::to_string(&q).unwrap_or_default()
        } else {
            String::new()
        };
        LiveBlock {
            bid: bid.to_string(),
            source: self.source.clone(),
            sql: if self.source == "sql" {
                self.sql.trim().to_string()
            } else {
                String::new()
            },
            query,
            title: self.title.trim().to_string(),
        }
    }

    /// Ready to save: a query, or a type (and a number for a non-count).
    pub(crate) fn complete(&self) -> bool {
        self.title.chars().count() <= LIVE_TITLE_MAX
            && match self.source.as_str() {
                "objects" => {
                    !self.type_key.is_empty()
                        && (self.op() == MetricOp::Count || !self.property.is_empty())
                }
                _ => !self.sql.trim().is_empty(),
            }
    }
}

/// Settings of the live block `block`, written into the page on save.
#[component]
pub(super) fn LiveBlockDialog(host: String, block: LiveBlock, on_close: Callback<()>) -> Element {
    let mut draft = use_signal(|| Draft::of(&block));
    let types = use_resource(|| async { api::ontology::types().await.unwrap_or_default() });
    let mut failed = use_signal(|| false);
    let all_types = types.read().clone().unwrap_or_default();
    let d = draft();
    let bid = block.bid.clone();
    let submit = move |_| {
        let host = host.clone();
        let next = draft().block(&bid, &types.read().clone().unwrap_or_default());
        spawn(async move {
            if doc_editor::set_live(&host, &next).await {
                on_close.call(());
            } else {
                failed.set(true);
            }
        });
    };
    let source = d.source.clone();
    rsx! {
        FormDialog {
            title: t!("live-settings").to_string(),
            submit_label: t!("pipe-save").to_string(),
            on_close: move |_| on_close.call(()),
            on_submit: submit,
            busy: false,
            valid: d.complete(),
            max_width: "max-w-xl".to_string(),
            div { class: "space-y-4",
                div {
                    label { class: FIELD_LABEL, r#for: "live-title", {t!("live-title")} }
                    input {
                        id: "live-title",
                        class: INPUT,
                        maxlength: LIVE_TITLE_MAX as i64,
                        value: "{d.title}",
                        oninput: move |e| draft.with_mut(|d| d.title = e.value()),
                    }
                }
                fieldset { class: "space-y-2",
                    legend { class: FIELD_LABEL, {t!("live-source")} }
                    for (value, label, help) in [
                        ("objects", t!("live-source-objects"), t!("live-source-objects-help")),
                        ("sql", t!("live-source-sql"), t!("live-source-sql-help")),
                    ]
                    {
                        label { key: "{value}", class: RADIO_ROW,
                            input {
                                r#type: "radio",
                                name: "live-source",
                                class: "mt-1",
                                checked: source == value,
                                onchange: move |_| draft.with_mut(|d| d.source = value.into()),
                            }
                            span {
                                span { class: "block text-sm font-medium text-ink", "{label}" }
                                span { class: "block text-sm text-gray-600", "{help}" }
                            }
                        }
                    }
                }
                if source == "sql" {
                    div {
                        label { class: FIELD_LABEL, r#for: "live-sql", {t!("pipe-query-label")} }
                        textarea {
                            id: "live-sql",
                            class: "{INPUT} font-mono text-sm min-h-40",
                            spellcheck: false,
                            value: "{d.sql}",
                            oninput: move |e| draft.with_mut(|d| d.sql = e.value()),
                        }
                    }
                } else {
                    ObjectsFields { draft, types: all_types.clone() }
                }
                p { class: "text-sm text-gray-600", {t!("live-rights-help")} }
                if failed() {
                    p { class: "text-sm text-red-600", role: "alert", {t!("live-save-failed")} }
                }
            }
        }
    }
}

/// The calculation on objects: type, operation, rows, columns of a crosstab
/// (AN4) and filters. Used by a live block and by the crosstab tab.
#[component]
pub(crate) fn ObjectsFields(draft: Signal<Draft>, types: Vec<ObjectTypeView>) -> Element {
    let all_types = types;
    let d = draft();
    let op = d.op();
    let props: Vec<_> = all_types
        .iter()
        .find(|v| v.def.key == d.type_key)
        .map(|v| v.def.properties.clone())
        .unwrap_or_default();
    let numbers: Vec<(String, String)> = props
        .iter()
        .filter(|p| matches!(p.kind, PropertyKind::Number { .. } | PropertyKind::Money))
        .map(|p| (p.key.clone(), prop_label(p)))
        .collect();
    let groups: Vec<(String, String)> = props
        .iter()
        .filter(|p| {
            matches!(
                p.kind,
                PropertyKind::Enum { .. }
                    | PropertyKind::Text { .. }
                    | PropertyKind::Bool
                    | PropertyKind::Date
                    | PropertyKind::DateTime
                    | PropertyKind::Ref { .. }
            )
        })
        .map(|p| (p.key.clone(), prop_label(p)))
        .collect();
    let type_choices: Vec<(String, String)> = all_types
        .iter()
        .map(|v| (v.def.key.clone(), type_label(&v.def)))
        .collect();
    let columns: Vec<(String, String)> = groups
        .iter()
        .filter(|(k, _)| *k != d.group)
        .cloned()
        .collect();
    let filter_props: Vec<(String, String)> = props
        .iter()
        .map(|p| (p.key.clone(), prop_label(p)))
        .collect();
    rsx! {
        div {
            label { class: FIELD_LABEL, r#for: "live-type", {t!("rules-type")} }
            select {
                id: "live-type",
                class: INPUT,
                onchange: move |e| {
                    draft
                        .with_mut(|d| {
                            d.type_key = e.value();
                            d.property.clear();
                            d.group.clear();
                        })
                },
                option { value: "", selected: d.type_key.is_empty(), {t!("gov-choose")} }
                for (key, label) in type_choices {
                    option {
                        key: "{key}",
                        value: "{key}",
                        selected: d.type_key == key,
                        "{label}"
                    }
                }
            }
        }
        div {
            label { class: FIELD_LABEL, r#for: "live-op", {t!("live-op")} }
            select {
                id: "live-op",
                class: INPUT,
                onchange: move |e| draft.with_mut(|d| d.op = e.value()),
                for o in OPS {
                    option {
                        key: "{op_wire(o)}",
                        value: "{op_wire(o)}",
                        selected: op == o,
                        {op_label(o)}
                    }
                }
            }
        }
        if op != MetricOp::Count {
            div {
                label { class: FIELD_LABEL, r#for: "live-prop", {t!("live-property")} }
                select {
                    id: "live-prop",
                    class: INPUT,
                    onchange: move |e| draft.with_mut(|d| d.property = e.value()),
                    option { value: "", selected: d.property.is_empty(), {t!("gov-choose")} }
                    for (key, label) in numbers {
                        option {
                            key: "{key}",
                            value: "{key}",
                            selected: d.property == key,
                            "{label}"
                        }
                    }
                }
            }
        }
        div {
            label { class: FIELD_LABEL, r#for: "live-group", {t!("live-group")} }
            select {
                id: "live-group",
                class: INPUT,
                onchange: move |e| {
                    draft
                        .with_mut(|d| {
                            d.group = e.value();
                            d.column.clear();
                        })
                },
                option { value: "", selected: d.group.is_empty(), {t!("live-group-none")} }
                for (key, label) in groups {
                    option {
                        key: "{key}",
                        value: "{key}",
                        selected: d.group == key,
                        "{label}"
                    }
                }
            }
        }
        if !d.group.is_empty() {
            div {
                label { class: FIELD_LABEL, r#for: "live-column", {t!("live-column")} }
                select {
                    id: "live-column",
                    class: INPUT,
                    onchange: move |e| draft.with_mut(|d| d.column = e.value()),
                    option { value: "", selected: d.column.is_empty(), {t!("live-column-none")} }
                    for (key, label) in columns {
                        option {
                            key: "{key}",
                            value: "{key}",
                            selected: d.column == key,
                            "{label}"
                        }
                    }
                }
            }
        }
        if !d.type_key.is_empty() {
            div { class: "space-y-2",
                p { class: FIELD_LABEL, {t!("rules-filters")} }
                for (i, f) in d.filters.clone().into_iter().enumerate() {
                    FilterRow {
                        key: "{i}",
                        i,
                        filter: f,
                        props: filter_props.clone(),
                        placeholder: String::new(),
                        on_change: move |next: PropertyFilter| draft.with_mut(|d| d.filters[i] = next),
                        on_remove: move |_| {
                            draft
                                .with_mut(|d| {
                                    d.filters.remove(i);
                                })
                        },
                    }
                }
                button {
                    r#type: "button",
                    class: BTN_SECONDARY_SM,
                    disabled: filter_props.is_empty(),
                    onclick: move |_| {
                        let first = filter_props.first().map(|p| p.0.clone()).unwrap_or_default();
                        draft
                            .with_mut(|d| {
                                d.filters
                                    .push(PropertyFilter {
                                        key: first,
                                        op: FilterOp::Eq,
                                        value: serde_json::Value::Null,
                                    })
                            });
                    },
                    {t!("rules-add-filter")}
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_crosstab_puts_each_value_in_its_cell() {
        let c = |r: &str, col: &str, v: &str| (r.to_string(), col.to_string(), v.to_string());
        let cells = [
            c("won", "Ann", "3"),
            c("lost", "Bob", "1"),
            c("won", "Bob", "2"),
            c("open", "Cid", "7"),
        ];
        let (head, rows, cut) = pivot("Stage".into(), &cells, 2);
        assert_eq!(head, ["Stage", "Ann", "Bob"]);
        assert_eq!(rows, [vec!["won", "3", "2"], vec!["lost", "", "1"]]);
        assert!(cut, "Cid is past the column limit");
        let (_, rows, cut) = pivot(String::new(), &cells, 12);
        assert_eq!((rows.len(), cut), (3, false));
    }

    #[test]
    fn a_draft_round_trips_through_the_block() {
        let d = Draft {
            title: "Open deals".into(),
            source: "objects".into(),
            type_key: "deal".into(),
            op: "sum".into(),
            property: "amount".into(),
            group: "stage".into(),
            column: "owner".into(),
            filters: vec![PropertyFilter {
                key: "amount".into(),
                op: FilterOp::Gt,
                value: serde_json::json!(10),
            }],
            ..Default::default()
        };
        let b = d.block("b1", &[]);
        assert!(
            pnex_core::pages::live_attr_ok("query", &b.query),
            "{}",
            b.query
        );
        assert_eq!(Draft::of(&b), d);
        let sql = Draft {
            source: "sql".into(),
            sql: " SELECT 1 ".into(),
            op: "count".into(),
            ..Default::default()
        };
        let b = sql.block("b2", &[]);
        assert_eq!((b.sql.as_str(), b.query.as_str()), ("SELECT 1", ""));
        assert!(sql.complete());
        assert!(!Draft {
            source: "objects".into(),
            op: "sum".into(),
            type_key: "deal".into(),
            ..Default::default()
        }
        .complete());
    }
}
