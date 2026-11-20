//! Parts of an installed equipment (parts-docs.md PD1, Q-PD4): the parts
//! list of its model kept to the lines of its serial number, build date and
//! variant; a part goes to the equipment's open work order (made when there
//! is none), whose parts export as CSV. Reads and writes go through the
//! generic ontology API, under the caller's rights.

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::ontology::api::{LinkInput, LinkView, ObjectInput, ObjectView};
use serde_json::{json, Map, Value};

use crate::api;
use crate::components::ui::{BTN_SECONDARY_SM, FIELD_HELP};

/// Work order states still open to new parts.
const OPEN_STATES: [&str; 4] = ["requested", "planned", "in_progress", "on_hold"];

/// Compares two serial numbers as a person reads them: runs of digits by
/// value (`S900` < `S1000`), the rest as text, case aside.
pub fn serial_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    fn parts(s: &str) -> Vec<(bool, String)> {
        let mut out: Vec<(bool, String)> = Vec::new();
        for c in s.trim().to_uppercase().chars() {
            let digit = c.is_ascii_digit();
            match out.last_mut() {
                Some((d, run)) if *d == digit => run.push(c),
                _ => out.push((digit, c.to_string())),
            }
        }
        out
    }
    let (pa, pb) = (parts(a), parts(b));
    for (x, y) in pa.iter().zip(&pb) {
        let o = match (x.0, y.0) {
            (true, true) => {
                let (tx, ty) = (x.1.trim_start_matches('0'), y.1.trim_start_matches('0'));
                tx.len().cmp(&ty.len()).then_with(|| tx.cmp(ty))
            }
            _ => x.1.cmp(&y.1),
        };
        if o.is_ne() {
            return o;
        }
    }
    pa.len().cmp(&pb.len())
}

/// What is known of the unit, to keep the lines of the parts list.
#[derive(Clone, Default, PartialEq)]
pub struct Unit {
    pub serial: Option<String>,
    /// `YYYY-MM-DD`.
    pub built_on: Option<String>,
    pub variant: Option<String>,
}

/// Whether a parts list line (its link attributes) applies to the unit. An
/// unknown serial, date or variant of the unit keeps the line: nothing says
/// it does not apply.
pub fn applies(line: &Map<String, Value>, unit: &Unit) -> bool {
    let text = |k: &str| {
        line.get(k)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
    };
    if let Some(s) = unit.serial.as_deref().filter(|s| !s.trim().is_empty()) {
        if text("serial_from").is_some_and(|f| serial_cmp(s, f).is_lt())
            || text("serial_to").is_some_and(|t| serial_cmp(s, t).is_gt())
        {
            return false;
        }
    }
    if let Some(d) = unit.built_on.as_deref() {
        if text("date_from").is_some_and(|f| d < f) || text("date_to").is_some_and(|t| d > t) {
            return false;
        }
    }
    match (
        text("variant"),
        unit.variant
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty()),
    ) {
        (Some(want), Some(have)) => want.eq_ignore_ascii_case(have),
        _ => true,
    }
}

/// The parts of a work order as CSV: part number, title, quantity.
pub fn basket_csv(lines: &[(String, String, f64)]) -> String {
    let cell = |s: &str| {
        if s.contains([',', '"', '\n']) {
            format!("\"{}\"", s.replace('"', "\"\""))
        } else {
            s.to_string()
        }
    };
    let mut out = String::from("part_number,part,quantity\n");
    for (number, title, qty) in lines {
        out.push_str(&format!("{},{},{qty}\n", cell(number), cell(title)));
    }
    out
}

fn text_prop(o: &ObjectView, k: &str) -> Option<String> {
    o.properties
        .get(k)
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// A line of the parts list as shown.
#[derive(Clone, PartialEq)]
struct Line {
    part_id: String,
    part: String,
    attrs: Map<String, Value>,
    applies: bool,
}

/// The parts list of the equipment's model, its lines marked for the unit.
async fn lines_of(
    object: &ObjectView,
) -> Result<Option<(String, Vec<Line>)>, crate::api::error::ApiError> {
    let links = api::ontology::object_links(&object.id, None, None, false).await?;
    let Some(model) = links
        .iter()
        .find(|l| l.link_type == "instance_of" && l.source.id == object.id)
        .map(|l| l.target.clone())
    else {
        return Ok(None);
    };
    let unit = Unit {
        serial: text_prop(object, "serial"),
        built_on: text_prop(object, "manufactured_on"),
        variant: text_prop(object, "variant"),
    };
    let model_links = api::ontology::object_links(&model.id, None, None, false).await?;
    let mut lines: Vec<Line> = model_links
        .into_iter()
        .filter(|l| l.link_type == "bom_line" && l.source.id == model.id)
        .map(|l: LinkView| Line {
            applies: applies(&l.attributes, &unit),
            part_id: l.target.id,
            part: l.target.title,
            attrs: l.attributes,
        })
        .collect();
    let pos = |l: &Line| {
        l.attrs
            .get("position")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    };
    lines.sort_by(|a, b| serial_cmp(&pos(a), &pos(b)));
    Ok(Some((model.title, lines)))
}

/// The equipment's open work order, made when it has none.
async fn open_work_order(object: &ObjectView) -> Result<String, crate::api::error::ApiError> {
    let links = api::ontology::object_links(&object.id, None, None, false).await?;
    for l in links
        .iter()
        .filter(|l| l.link_type == "work_order_on" && l.target.id == object.id)
    {
        let wo = api::ontology::object(&l.source.id).await?;
        if text_prop(&wo, "state").is_some_and(|s| OPEN_STATES.contains(&s.as_str())) {
            return Ok(wo.id);
        }
    }
    let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
    let props = json!({ "state": "requested", "kind": "corrective", "requested_on": today });
    let wo = api::ontology::create_object(&ObjectInput {
        type_key: "work_order".into(),
        title: format!("{} · {}", t!("eq-parts-wo-title"), object.title),
        properties: props.as_object().cloned().unwrap_or_default(),
        expected_version: None,
    })
    .await?;
    api::ontology::create_link(&LinkInput {
        link_type: "work_order_on".into(),
        source_id: wo.id.clone(),
        target_id: object.id.clone(),
        ..Default::default()
    })
    .await?;
    Ok(wo.id)
}

/// Adds the part to the equipment's open work order.
async fn add_to_basket(
    object: &ObjectView,
    line: &Line,
) -> Result<(), crate::api::error::ApiError> {
    let wo = open_work_order(object).await?;
    let qty = line
        .attrs
        .get("quantity")
        .and_then(Value::as_f64)
        .unwrap_or(1.0);
    api::ontology::create_link(&LinkInput {
        link_type: "uses_part".into(),
        source_id: wo,
        target_id: line.part_id.clone(),
        attributes: json!({ "quantity": qty })
            .as_object()
            .cloned()
            .unwrap_or_default(),
        ..Default::default()
    })
    .await?;
    Ok(())
}

/// On an equipment's page: its parts list for its serial, date and variant.
#[component]
pub fn EquipmentParts(object: ObjectView) -> Element {
    let obj = use_signal(|| object.clone());
    let mut all = use_signal(|| false);
    let mut added = use_signal(Vec::<String>::new);
    let list = use_resource(move || async move { lines_of(&obj()).await });
    let can_write = crate::state::access::can("ontology:write");
    let Some(Ok(Some((model, lines)))) = list.read().clone() else {
        return rsx! {};
    };
    let hidden = lines.iter().filter(|l| !l.applies).count();
    rsx! {
        section { class: "space-y-2", "data-testid": "equipment-parts",
            div { class: "flex flex-wrap items-baseline justify-between gap-2",
                h2 { class: "text-base font-semibold text-ink",
                    {t!("eq-parts-title", model : model.clone())}
                }
                if hidden > 0 {
                    button {
                        r#type: "button",
                        class: "text-sm text-signal hover:underline",
                        "data-testid": "equipment-parts-all",
                        onclick: move |_| all.toggle(),
                        if all() {
                            {t!("eq-parts-only-unit")}
                        } else {
                            {t!("eq-parts-show-all", count : hidden)}
                        }
                    }
                }
            }
            if lines.is_empty() {
                p { class: FIELD_HELP, {t!("eq-parts-empty")} }
            }
            ul { class: "divide-y divide-line rounded-md border border-line bg-surface",
                for line in lines.into_iter().filter(|l| l.applies || all()) {
                    li {
                        key: "{line.part_id}",
                        class: if line.applies { "flex items-center gap-3 px-3 py-2" } else { "flex items-center gap-3 px-3 py-2 opacity-50" },
                        "data-testid": "equipment-part",
                        span { class: "w-12 shrink-0 font-mono text-xs text-slate",
                            {line.attrs.get("position").and_then(Value::as_str).unwrap_or("").to_string()}
                        }
                        span { class: "min-w-0 flex-1 break-words text-ink", "{line.part}" }
                        span { class: "shrink-0 text-sm text-slate",
                            {
                                format!(
                                    "× {}",
                                    line.attrs.get("quantity").and_then(Value::as_f64).unwrap_or(1.0),
                                )
                            }
                        }
                        if can_write && line.applies {
                            if added().contains(&line.part_id) {
                                span { class: "shrink-0 text-sm text-ok", {t!("eq-parts-added")} }
                            } else {
                                button {
                                    r#type: "button",
                                    class: BTN_SECONDARY_SM,
                                    "data-testid": "equipment-part-add",
                                    onclick: {
                                        let line = line.clone();
                                        move |_| {
                                            let line = line.clone();
                                            spawn(async move {
                                                match add_to_basket(&obj(), &line).await {
                                                    Ok(()) => added.write().push(line.part_id.clone()),
                                                    Err(e) => crate::state::toasts::error(e),
                                                }
                                            });
                                        }
                                    },
                                    {t!("eq-parts-add")}
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// On a work order's page: its parts exported as CSV.
#[component]
pub fn BasketExport(object: ObjectView) -> Element {
    let id = object.id.clone();
    let title = object.title.clone();
    let mut busy = use_signal(|| false);
    rsx! {
        button {
            r#type: "button",
            class: BTN_SECONDARY_SM,
            disabled: busy(),
            "data-testid": "basket-export",
            onclick: move |_| {
                let (id, title) = (id.clone(), title.clone());
                busy.set(true);
                spawn(async move {
                    match basket_lines(&id).await {
                        Ok(lines) => {
                            let csv = basket_csv(&lines);
                            let name = format!("{}.csv", title.replace(['/', '\\', '"'], "-"));
                            crate::util::save_blob(&name, csv.as_bytes());
                        }
                        Err(e) => crate::state::toasts::error(e),
                    }
                    busy.set(false);
                });
            },
            {t!("eq-parts-export")}
        }
    }
}

/// The parts a work order uses: part number, title, quantity.
async fn basket_lines(id: &str) -> Result<Vec<(String, String, f64)>, crate::api::error::ApiError> {
    let links = api::ontology::object_links(id, None, None, false).await?;
    let mut out = Vec::new();
    for l in links
        .into_iter()
        .filter(|l| l.link_type == "uses_part" && l.source.id == id)
    {
        let part = api::ontology::object(&l.target.id).await?;
        let qty = l
            .attributes
            .get("quantity")
            .and_then(Value::as_f64)
            .unwrap_or(1.0);
        out.push((
            text_prop(&part, "part_number").unwrap_or_default(),
            part.title,
            qty,
        ));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(v: Value) -> Map<String, Value> {
        v.as_object().unwrap().clone()
    }

    #[test]
    fn serials_compare_as_read() {
        assert!(serial_cmp("S900", "S1000").is_lt());
        assert!(serial_cmp("s1000", "S1000").is_eq());
        assert!(serial_cmp("AB-0012", "AB-12").is_eq());
        assert!(serial_cmp("B1", "A9").is_gt());
    }

    #[test]
    fn lines_apply_by_serial_date_and_variant() {
        let unit = Unit {
            serial: Some("S5200".into()),
            built_on: Some("2024-03-01".into()),
            variant: Some("230V".into()),
        };
        assert!(applies(&line(json!({})), &unit));
        assert!(applies(&line(json!({ "serial_from": "S5000" })), &unit));
        assert!(!applies(&line(json!({ "serial_to": "S4999" })), &unit));
        assert!(!applies(&line(json!({ "date_from": "2025-01-01" })), &unit));
        assert!(applies(&line(json!({ "variant": "230v" })), &unit));
        assert!(!applies(&line(json!({ "variant": "115V" })), &unit));
        // Nothing known of the unit: every line is kept.
        assert!(applies(
            &line(json!({ "variant": "115V", "serial_to": "S1" })),
            &Unit::default()
        ));
    }

    #[test]
    fn basket_exports_quoted_csv() {
        let csv = basket_csv(&[
            ("P-1".into(), "Fan, motor".into(), 2.0),
            ("P-2".into(), "Gasket \"A\"".into(), 1.0),
        ]);
        assert_eq!(
            csv,
            "part_number,part,quantity\nP-1,\"Fan, motor\",2\nP-2,\"Gasket \"\"A\"\"\",1\n"
        );
    }
}
