//! Shared pieces of the ontology UI (ontology.md D186): labels of types
//! and link types, typed property inputs and values, the local graph view
//! and the "object page" link shown on the specialised pages.

pub mod action_dialog;
pub mod add_to_case;
pub mod gallery;
pub mod graph_view;
pub mod hierarchy;
pub mod object_link;
pub mod open_case;
pub mod page_from_template;
pub mod property_input;

use dioxus::prelude::*;
use dioxus_i18n::prelude::i18n;
use dioxus_i18n::t;
use pnex_core::ontology::api::ObjectTypeView;
use pnex_core::ontology::{LinkTypeDef, ObjectTypeDef};

use crate::components::icons;

/// `name` in the UI language when a translation exists (packs, O43).
pub fn tr(name: &str, names: &pnex_core::ontology::Names) -> String {
    pnex_core::ontology::localized(name, names, &crate::i18n::current_tag()).to_string()
}

/// Display name of a type: its own name, or the translated system key.
pub fn type_label(def: &ObjectTypeDef) -> String {
    if !def.name.trim().is_empty() {
        return tr(&def.name, &def.names);
    }
    key_label(&def.key)
}

/// Display name of a type key alone (`onto-type-<key>`, else the key).
pub fn key_label(key: &str) -> String {
    i18n()
        .try_translate(&format!("onto-type-{key}"))
        .unwrap_or_else(|_| key.to_string())
}

/// Label of a type key among the known types.
pub fn type_label_of(types: &[ObjectTypeView], key: &str) -> String {
    types
        .iter()
        .find(|t| t.def.key == key)
        .map(|t| type_label(&t.def))
        .unwrap_or_else(|| key_label(key))
}

/// Link label read from the source (`inverse` = read from the target).
pub fn link_label(def: Option<&LinkTypeDef>, key: &str, inverse: bool) -> String {
    let own = def.map(|d| {
        if inverse {
            (&d.inverse_name, &d.inverse_names)
        } else {
            (&d.name, &d.names)
        }
    });
    match own.filter(|(n, _)| !n.trim().is_empty()) {
        Some((n, names)) => tr(n, names),
        None => {
            let k = if inverse {
                format!("onto-link-{key}-inverse")
            } else {
                format!("onto-link-{key}")
            };
            i18n().try_translate(&k).unwrap_or_else(|_| key.to_string())
        }
    }
}

/// Translated precision of a `fuzzy_time` (`fuzzy-precision-<p>`).
pub fn precision_label(p: &str) -> String {
    i18n()
        .try_translate(&format!("fuzzy-precision-{p}"))
        .unwrap_or_else(|_| p.to_string())
}

/// A `fuzzy_time` value read at its precision: `2019-03` for a month,
/// `≈ a → b` for an approximation (UTC).
/// « 1 250,50 EUR (estimé) », in the UI language.
pub fn money_label(v: &serde_json::Value) -> String {
    let Some(m) = pnex_core::ontology::schema::Money::parse(v) else {
        return v.to_string();
    };
    let amount = pnex_core::ontology::money::full_amount(
        m.amount,
        crate::i18n::current_tag().starts_with("fr"),
    );
    if m.estimated {
        format!(
            "{amount} {} {}",
            m.currency,
            t!("onto-money-estimated-mark")
        )
    } else {
        format!("{amount} {}", m.currency)
    }
}

pub fn fuzzy_label(v: &serde_json::Value) -> String {
    let get = |k: &str| {
        v.get(k)
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
    };
    let (from, to, p) = (get("from"), get("to"), get("precision"));
    let cut = |s: &str, n: usize| s.get(..n).unwrap_or(s).replace('T', " ");
    match p {
        "year" => cut(from, 4),
        "month" => cut(from, 7),
        "day" => cut(from, 10),
        "hour" => format!("{} h", cut(from, 13)),
        "minute" | "instant" => cut(from, 16),
        "approx" => format!("≈ {} → {}", cut(from, 16), cut(to, 16)),
        _ => format!("{} → {}", cut(from, 16), cut(to, 16)),
    }
}

/// Icon of a type from its `icon` name (a few names of the icon set).
#[component]
pub fn TypeIcon(icon: String, class: Option<String>) -> Element {
    let class = class.unwrap_or_else(|| "h-4 w-4".into());
    let c = Some(class);
    match icon.as_str() {
        "map-pin" => rsx! {
            icons::MapPin { class: c }
        },
        "workflow" => rsx! {
            icons::Workflow { class: c }
        },
        "cog" | "wrench" => rsx! {
            icons::Wrench { class: c }
        },
        "droplet" | "thermometer" => rsx! {
            icons::Thermometer { class: c }
        },
        "cpu" => rsx! {
            icons::Cpu { class: c }
        },
        "building" => rsx! {
            icons::Building { class: c }
        },
        "user" => rsx! {
            icons::User { class: c }
        },
        "folder" => rsx! {
            icons::Folder { class: c }
        },
        "image" => rsx! {
            icons::Image { class: c }
        },
        "layers" => rsx! {
            icons::Layers { class: c }
        },
        "book" => rsx! {
            icons::BookOpen { class: c }
        },
        "tag" => rsx! {
            icons::Tag { class: c }
        },
        "radio" => rsx! {
            icons::Radio { class: c }
        },
        "camera" => rsx! {
            icons::Camera { class: c }
        },
        "server" => rsx! {
            icons::Server { class: c }
        },
        "globe" => rsx! {
            icons::Globe { class: c }
        },
        "flag" => rsx! {
            icons::Flag { class: c }
        },
        "landmark" => rsx! {
            icons::Landmark { class: c }
        },
        "newspaper" => rsx! {
            icons::Newspaper { class: c }
        },
        "file-text" => rsx! {
            icons::FileText { class: c }
        },
        _ => rsx! {
            icons::Cube { class: c }
        },
    }
}

/// Icon names offered by the type editor.
pub const ICON_NAMES: &[&str] = &[
    "cube",
    "map-pin",
    "building",
    "workflow",
    "cog",
    "droplet",
    "cpu",
    "user",
    "folder",
    "image",
    "layers",
    "tag",
    "radio",
    "camera",
    "server",
    "globe",
    "book",
    "flag",
    "landmark",
    "newspaper",
    "file-text",
];

/// Icon of a system type key.
pub fn system_icon(key: &str) -> &'static str {
    match key {
        "device" => "cpu",
        "map_pin" => "map-pin",
        "media_asset" => "image",
        "dashboard" => "layers",
        "tour" => "camera",
        "flow" => "workflow",
        "doc" => "book",
        "folder" => "folder",
        _ => "cube",
    }
}

pub fn icon_of(def: &ObjectTypeDef) -> String {
    if def.icon.is_empty() {
        system_icon(&def.key).to_string()
    } else {
        def.icon.clone()
    }
}

/// Short date-time for tables (`YYYY-MM-DD HH:MM`, UTC).
pub fn short_time(rfc3339: &str) -> String {
    rfc3339
        .get(..16)
        .map(|s| s.replace('T', " "))
        .unwrap_or_default()
}

/// Dot classes of a property colour token (D339); unknown → gray.
pub fn mark_class(color: &str) -> &'static str {
    match color {
        "green" => "bg-green-500",
        "amber" => "bg-amber-500",
        "red" => "bg-red-500",
        "blue" => "bg-signal-500",
        "violet" => "bg-violet-500",
        _ => "bg-gray-400",
    }
}

/// Hex of a property colour token, for SVG strokes (same palette as
/// [`mark_class`] and map.js).
pub fn tone_hex(color: &str) -> &'static str {
    match color {
        "green" => "#22c55e",
        "amber" => "#f59e0b",
        "red" => "#ef4444",
        "blue" => "#3b82f6",
        "violet" => "#8b5cf6",
        _ => "#9ca3af",
    }
}

/// Display name of a property key, from the first type that has it.
pub fn property_name(types: &[ObjectTypeView], key: &str) -> String {
    types
        .iter()
        .flat_map(|t| t.def.properties.iter())
        .find(|p| p.key == key)
        .map(|p| tr(&p.name, &p.names))
        .unwrap_or_else(|| key.to_string())
}

/// Display form of an enum value: its key is user data (`non_conforme`),
/// shown as words (`Non conforme`); the stored value is unchanged.
pub fn enum_label(value: &str) -> String {
    let words = value.replace(['_', '-'], " ");
    let mut chars = words.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// Field errors of a `field-invalid` body.
pub fn field_errors(
    e: &crate::api::error::ApiError,
) -> Option<serde_json::Map<String, serde_json::Value>> {
    e.fields().cloned()
}

/// Object picker of a type dashboard (D187): the objects of `type_key`;
/// the first one is selected when nothing is.
#[component]
pub fn ObjectPicker(type_key: String, selected: Signal<String>) -> Element {
    let mut selected = selected;
    let tk = type_key.clone();
    let list = use_resource(move || {
        let q = pnex_core::ontology::api::OntologyQuery {
            type_key: Some(tk.clone()),
            limit: Some(500),
            ..Default::default()
        };
        async move {
            crate::api::ontology::query(&q)
                .await
                .map(|r| r.rows)
                .unwrap_or_default()
        }
    });
    use_effect(move || {
        if let Some(rows) = list.read().as_ref() {
            if selected.peek().is_empty() {
                if let Some(first) = rows.first() {
                    selected.set(first.object.id.clone());
                }
            }
        }
    });
    let rows = list.read().clone().unwrap_or_default();
    let label = key_label(&type_key);
    rsx! {
        label { class: "flex items-center gap-2 text-sm text-gray-700",
            span { "{label}" }
            select {
                class: "px-3 py-1.5 border border-gray-300 rounded-lg text-sm bg-white",
                "data-testid": "dashboard-object-picker",
                onchange: move |e| selected.set(e.value()),
                for r in rows {
                    option {
                        key: "{r.object.id}",
                        value: "{r.object.id}",
                        selected: r.object.id == selected(),
                        "{r.object.title}"
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enum_label_turns_keys_into_words() {
        assert_eq!(enum_label("non_conforme"), "Non conforme");
        assert_eq!(enum_label("3x8"), "3x8");
        assert_eq!(enum_label("élevé-critique"), "Élevé critique");
        assert_eq!(enum_label(""), "");
    }

    #[test]
    fn every_pack_icon_is_drawn() {
        let packs = [
            include_str!("../../../../pnex-backend/packs/investigation.yaml"),
            include_str!("../../../../pnex-backend/packs/maintenance.yaml"),
            include_str!("../../../../pnex-backend/packs/media_coverage.yaml"),
            include_str!("../../../../pnex-backend/packs/entreprises.yaml"),
            include_str!("../../../../pnex-backend/packs/entreprises_fr.yaml"),
            include_str!("../../../../pnex-backend/packs/quali_watch.yaml"),
            include_str!("../../../../pnex-backend/packs/prospection_b2b.yaml"),
        ];
        for line in packs.iter().flat_map(|p| p.lines()) {
            if let Some(icon) = line.trim().strip_prefix("icon:") {
                let icon = icon.trim();
                assert!(
                    ICON_NAMES.contains(&icon),
                    "pack icon `{icon}` falls back to the cube"
                );
            }
        }
    }

    #[test]
    fn short_time_drops_seconds_and_zone() {
        assert_eq!(short_time("2026-10-18T06:00:00Z"), "2026-10-18 06:00");
    }
}
