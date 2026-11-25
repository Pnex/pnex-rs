//! Page `/parts` (parts-docs.md lot PD0, D961): one field for a part
//! reference typed any way (spaces, dashes, case, a prefix), the parts its
//! references designate, and for a superseded part the one that replaces
//! it. Reads only: the generic ontology query and links, nothing of its own.

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::ontology::api::{
    Direction, FilterOp, Hop, LinkView, ObjectView, OntologyQuery, PropertyFilter,
};
use pnex_core::ontology::schema::Normalize;

use crate::api;
use crate::app::Route;
use crate::components::crud::layout::ListLayout;
use crate::components::crud::pager::{ListPager, PAGE_SIZE};
use crate::components::crud::states::ListStates;
use crate::components::crud::table::{Column, DataTable, RowKey};
use crate::components::ui::INPUT;

/// Shortest normalized reference searched: one or two characters would
/// list half the catalogue.
const MIN_CHARS: usize = 3;
/// References listed per part; the others are counted.
const REFS_SHOWN: usize = 3;
/// Supersessions followed at most from a part to the newest one.
const HOPS_MAX: usize = 5;

/// A part found, its references and the newest part replacing it.
#[derive(Clone, PartialEq)]
struct Found {
    part: ObjectView,
    references: Vec<String>,
    /// Parts from the one found to the newest, oldest first; empty when the
    /// part is current.
    replaced_by: Vec<(String, String)>,
}

/// The parts whose references start with `normalized`, one page.
async fn search(
    normalized: String,
    page: i64,
) -> Result<(i64, Vec<Found>), crate::api::error::ApiError> {
    let q = OntologyQuery {
        type_key: Some("part_reference".into()),
        filters: vec![PropertyFilter {
            key: "reference".into(),
            op: FilterOp::StartsWith,
            value: normalized.into(),
        }],
        traverse: vec![Hop {
            link_type: "refers_to".into(),
            direction: Direction::Out,
        }],
        limit: Some(PAGE_SIZE as u64),
        offset: Some((page * PAGE_SIZE) as u64),
        ..Default::default()
    };
    let r = api::ontology::query(&q).await?;
    // shortcut: one links read per part found (20 a page) to show its
    // references and successors; a server read joining them comes if a
    // measure shows the page slow.
    let found = futures::future::join_all(r.rows.into_iter().map(|row| async move {
        let links = api::ontology::object_links(&row.object.id, None, None, false)
            .await
            .unwrap_or_default();
        let references = links
            .iter()
            .filter(|l| l.link_type == "refers_to" && l.target.id == row.object.id)
            .map(|l| l.source.title.clone())
            .collect();
        let replaced_by = successors(&row.object.id, links).await;
        Found {
            part: row.object,
            references,
            replaced_by,
        }
    }))
    .await;
    Ok((r.count, found))
}

/// The parts superseding `id`, one after the other, up to the newest.
async fn successors(id: &str, mut links: Vec<LinkView>) -> Vec<(String, String)> {
    let mut chain: Vec<(String, String)> = Vec::new();
    let mut at = id.to_string();
    for _ in 0..HOPS_MAX {
        let Some(newer) = links
            .iter()
            .find(|l| l.link_type == "supersedes" && l.target.id == at)
            .map(|l| (l.source.id.clone(), l.source.title.clone()))
        else {
            break;
        };
        // A loop in the data stops the walk.
        if newer.0 == id || chain.iter().any(|(c, _)| *c == newer.0) {
            break;
        }
        at = newer.0.clone();
        chain.push(newer);
        links = api::ontology::object_links(&at, None, None, false)
            .await
            .unwrap_or_default();
    }
    chain
}

#[component]
pub fn Parts() -> Element {
    let mut typed = use_signal(String::new);
    let mut page = use_signal(|| 0i64);
    let normalized = use_memo(move || Normalize::Reference.apply(&typed()));
    let mut results = use_resource(move || {
        let (n, p) = (normalized(), page());
        async move {
            if n.chars().count() < MIN_CHARS {
                return None;
            }
            Some(search(n, p).await)
        }
    });
    let nav = use_navigator();
    let (state, count, rows) = match &*results.value().read() {
        Some(Some(Ok((count, rows)))) => (Some(Ok(())), *count, rows.clone()),
        Some(Some(Err(e))) => (Some(Err(e.clone())), 0, Vec::new()),
        Some(None) => (Some(Ok(())), 0, Vec::new()),
        None => (None, 0, Vec::new()),
    };
    let short = normalized().chars().count() < MIN_CHARS;
    let columns = vec![
        Column::new(t!("parts-col-part").to_string(), |f: &Found| {
            let number = f
                .part
                .properties
                .get("part_number")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            rsx! {
                span { class: "block font-medium text-ink break-words", "{f.part.title}" }
                if !number.is_empty() {
                    span { class: "block text-xs text-slate break-words", "{number}" }
                }
            }
        }),
        Column::new(t!("parts-col-references").to_string(), |f: &Found| {
            // A part may carry dozens of references: the first three, then a count.
            let shown = f
                .references
                .iter()
                .take(REFS_SHOWN)
                .cloned()
                .collect::<Vec<_>>()
                .join(", ");
            let more = f.references.len().saturating_sub(REFS_SHOWN);
            rsx! {
                span { class: "break-words", "{shown}" }
                if more > 0 {
                    span { class: "ml-1 whitespace-nowrap text-xs text-slate", {t!("parts-more", count : more)} }
                }
            }
        }),
        Column::new(t!("parts-col-current").to_string(), |f: &Found| {
            match f.replaced_by.last() {
                Some((_, title)) => rsx! {
                    span { class: "inline-flex items-start gap-1.5 text-warn break-words",
                        span { class: "mt-1.5 h-2 w-2 shrink-0 rounded-full bg-warn", aria_hidden: "true" }
                        {t!("parts-replaced-by", title : title.clone())}
                    }
                },
                None => rsx! {
                    span { class: "text-slate", {t!("parts-current")} }
                },
            }
        }),
    ];
    rsx! {
        ListLayout {
            title: t!("parts-title").to_string(),
            subtitle: Some(t!("parts-subtitle").to_string()),
            // Parts come from parts lists: a pipeline imports one (D1050).
            can_write: crate::state::access::can("ontology:write"),
            add_label: Some(t!("parts-import").to_string()),
            on_add: move |_| {
                nav.push(Route::Pipelines {});
            },
            on_refresh: move |_| results.restart(),
            div { class: "space-y-4",
                div {
                    label { class: "sr-only", r#for: "parts-search", {t!("parts-search-label")} }
                    input {
                        id: "parts-search",
                        "data-testid": "parts-search",
                        r#type: "search",
                        class: "{INPUT} text-lg",
                        placeholder: t!("parts-search-placeholder").to_string(),
                        autocomplete: "off",
                        maxlength: "64",
                        value: "{typed}",
                        oninput: move |e| {
                            typed.set(e.value());
                            page.set(0);
                        },
                    }
                    p {
                        class: "mt-1 text-xs text-slate",
                        "data-testid": "parts-searched-as",
                        if short {
                            {t!("parts-search-help")}
                        } else {
                            {t!("parts-searched-as", reference : normalized())}
                        }
                    }
                }
                if !short {
                    ListStates {
                        state,
                        is_empty: count == 0,
                        empty_message: t!("parts-empty").to_string(),
                        div { class: "space-y-4",
                            DataTable {
                                columns,
                                rows,
                                row_key: RowKey::new(|f: &Found| f.part.id.clone()),
                                on_row_click: move |id: String| {
                                    nav.push(Route::OntologyObject { id });
                                },
                            }
                            ListPager { count, page }
                        }
                    }
                }
            }
        }
    }
}
