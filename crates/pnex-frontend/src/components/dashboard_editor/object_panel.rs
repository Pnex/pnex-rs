//! Type dashboard settings (ontology D187): the object type the dashboard
//! is drawn for, and, per widget, the `series` property of the shown
//! object that it reads instead of a fixed device.

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::ontology::PropertyKind;
use pnex_core::Widget;

use super::EditorCx;
use crate::api;
use crate::components::ontology::{key_label, type_label};

const SELECT: &str = "w-full px-2 py-1.5 border border-gray-300 rounded text-sm bg-white";

#[component]
pub fn ObjectSourcePanel(cx: EditorCx, widget: Widget, can_write: bool) -> Element {
    let mut layout = cx.layout;
    let types = use_resource(|| async { api::ontology::types().await.unwrap_or_default() });
    let all = types.read().clone().unwrap_or_default();
    let object_type = layout.read().object_type.clone();
    let props: Vec<(String, String)> = all
        .iter()
        .find(|t| Some(&t.def.key) == object_type.as_ref())
        .map(|t| {
            t.def
                .properties
                .iter()
                .filter(|p| matches!(p.kind, PropertyKind::Series { .. }))
                .map(|p| {
                    (
                        p.key.clone(),
                        if p.name.is_empty() {
                            p.key.clone()
                        } else {
                            p.name.clone()
                        },
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    let current = widget
        .source
        .first()
        .and_then(|s| s.object_property.clone())
        .unwrap_or_default();
    let wid = widget.id.clone();
    let type_choices: Vec<(String, String)> = all
        .iter()
        .filter(|t| !t.def.system)
        .map(|t| (t.def.key.clone(), type_label(&t.def)))
        .collect();
    let shown_type = object_type
        .clone()
        .map(|k| key_label(&k))
        .unwrap_or_default();
    rsx! {
        div {
            class: "space-y-2 border-t border-gray-100 pt-3",
            "data-testid": "object-source-panel",
            label { class: "block text-xs font-medium text-gray-500", {t!("insp-object-type")} }
            select {
                class: SELECT,
                disabled: !can_write,
                "data-testid": "insp-object-type",
                onchange: move |e| {
                    let v = e.value();
                    layout.with_mut(|l| l.object_type = (!v.is_empty()).then_some(v));
                },
                option { value: "", selected: object_type.is_none(), {t!("insp-object-type-none")} }
                for (k, l) in type_choices {
                    option {
                        key: "{k}",
                        value: "{k}",
                        selected: object_type.as_deref() == Some(k.as_str()),
                        "{l}"
                    }
                }
            }
            if object_type.is_some() {
                label { class: "block text-xs font-medium text-gray-500",
                    {t!("insp-object-property", kind : shown_type)}
                }
                select {
                    class: SELECT,
                    disabled: !can_write,
                    "data-testid": "insp-object-property",
                    onchange: move |e| {
                        let v = e.value();
                        let wid = wid.clone();
                        layout.with_mut(|l| bind_widget(l, &wid, v));
                    },
                    option { value: "", selected: current.is_empty(),
                        {t!("insp-object-property-none")}
                    }
                    for (k, l) in props {
                        option {
                            key: "{k}",
                            value: "{k}",
                            selected: current == k,
                            "{l}"
                        }
                    }
                }
                p { class: "text-[10px] text-gray-400", {t!("insp-object-help")} }
            }
        }
    }
}

/// Points the widget's primary source at an object property (or back to a
/// device source when `property` is empty).
fn bind_widget(l: &mut pnex_core::DashboardLayout, widget_id: &str, property: String) {
    let Some(w) = l.widgets.iter_mut().find(|w| w.id == widget_id) else {
        return;
    };
    if w.source.is_empty() {
        w.source.push(pnex_core::SourceRef {
            role: "primary".into(),
            metric: String::new(),
            device_id: String::new(),
            window: "1h".into(),
            memory: None,
            labels: Default::default(),
            object_property: None,
        });
    }
    let s = &mut w.source[0];
    s.memory = None;
    s.labels.clear();
    s.metric.clear();
    s.device_id.clear();
    s.object_property = (!property.is_empty()).then_some(property);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binding_a_widget_clears_the_device_source() {
        let mut l: pnex_core::DashboardLayout = serde_json::from_value(serde_json::json!({
            "canvas": {"width": 800, "height": 600},
            "widgets": [{"id": "w1", "type": "gauge", "x": 0, "y": 0, "w": 100, "h": 100,
                "source": [{"metric": "temp", "device_id": "c47", "window": "1h"}]}]
        }))
        .unwrap();
        bind_widget(&mut l, "w1", "temperature".into());
        let s = &l.widgets[0].source[0];
        assert_eq!(
            (s.metric.as_str(), s.object_property.as_deref()),
            ("", Some("temperature"))
        );
        let bound = pnex_core::bind_object(
            &l,
            &[pnex_core::SeriesBinding {
                property: "temperature".into(),
                device_id: "c88".into(),
                metric: "temp".into(),
            }],
        );
        assert_eq!(bound.widgets[0].source[0].device_id, "c88");
    }
}
