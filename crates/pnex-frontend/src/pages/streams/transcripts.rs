//! "Transcriptions" tab of /streams (media-ingest.md D165): full-text
//! search over the timestamped text of the org's streams, newest first.

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::media_ingest::MediaStream;

use crate::api;
use crate::components::badges::date_label;
use crate::components::crud::pager::{ListPager, PAGE_SIZE};
use crate::components::crud::states::ListStates;

const INPUT: &str = "px-3 py-2 border border-gray-300 rounded-lg text-sm bg-white";

#[component]
pub fn TranscriptsTab(streams: Vec<MediaStream>, reload: Signal<u32>) -> Element {
    let mut stream = use_signal(String::new);
    let mut query = use_signal(String::new);
    let mut page = use_signal(|| 0i64);
    let results = use_resource(move || {
        let stream = stream();
        let query = query();
        let offset = page() * PAGE_SIZE;
        async move {
            let _ = reload();
            api::media_streams::transcripts(&stream, &query, PAGE_SIZE, offset).await
        }
    });
    let (state, is_empty, count, rows) = match &*results.value().read() {
        None => (None, false, 0, Vec::new()),
        Some(Ok(p)) => (
            Some(Ok(())),
            p.results.is_empty(),
            p.count,
            p.results.clone(),
        ),
        Some(Err(err)) => (Some(Err(err.clone())), false, 0, Vec::new()),
    };
    let names: Vec<(String, String)> = streams
        .iter()
        .map(|s| (s.slug.clone(), s.name.clone()))
        .collect();
    let name_of = move |slug: &str| {
        names
            .iter()
            .find(|(s, _)| s == slug)
            .map(|(_, n)| n.clone())
            .unwrap_or_else(|| slug.to_string())
    };

    rsx! {
        div { class: "space-y-4",
            div { class: "flex flex-col gap-2 sm:flex-row",
                select {
                    class: INPUT,
                    aria_label: t!("transcripts-stream"),
                    onchange: move |e| {
                        stream.set(e.value());
                        page.set(0);
                    },
                    option { value: "", selected: stream().is_empty(), {t!("transcripts-all-streams")} }
                    for s in streams.clone() {
                        option { value: "{s.slug}", selected: stream() == s.slug, "{s.name}" }
                    }
                }
                input {
                    class: "{INPUT} flex-1",
                    r#type: "search",
                    placeholder: t!("transcripts-search"),
                    value: "{query}",
                    oninput: move |e| {
                        query.set(e.value());
                        page.set(0);
                    },
                }
            }
            ListStates {
                state,
                is_empty,
                empty_message: t!("transcripts-empty").to_string(),
                ul { class: "divide-y divide-gray-100 bg-white rounded-lg shadow border border-gray-200",
                    for r in rows {
                        li {
                            key: "{r.segment_id}",
                            class: "px-4 py-3 space-y-1",
                            p { class: "text-xs text-gray-500",
                                {format!("{} · {}", name_of(&r.stream), date_label(&r.ts))}
                            }
                            p { class: "text-sm text-gray-900 whitespace-pre-wrap",
                                "{r.text}"
                            }
                        }
                    }
                }
                ListPager { count, page }
            }
        }
    }
}
