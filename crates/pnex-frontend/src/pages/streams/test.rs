//! Stream test (media-ingest.md §7): a first 10 s extract captured by the
//! server and transcribed with the stream's profile, nothing stored.

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::media_ingest::MediaStream;

use crate::api;
use crate::components::modal::Modal;

fn transcribe_error_label(code: &str) -> String {
    match code {
        "no-profile" => t!("streams-seg-error-no-profile").to_string(),
        "model-invalid" => t!("streams-seg-error-model-invalid").to_string(),
        _ => t!("streams-seg-error-asr-failed").to_string(),
    }
}

#[component]
pub fn StreamTestDialog(stream: MediaStream, on_close: Callback<()>) -> Element {
    let id = stream.id.clone();
    let result = use_resource(move || {
        let id = id.clone();
        async move { api::media_streams::test_stream(&id).await }
    });
    let body = match &*result.value().read() {
        None => rsx! {
            p { class: "text-sm text-gray-600", {t!("streams-test-running")} }
        },
        Some(Err(err)) => rsx! {
            p { class: "text-sm text-red-700", "{err}" }
        },
        Some(Ok(r)) => {
            let r = r.clone();
            let secs = format!("{:.0}", r.audio_ms as f64 / 1000.0);
            rsx! {
                if let Some(code) = r.capture_error.as_deref() {
                    p { class: "text-sm text-red-700",
                        {t!("streams-test-capture-failed", reason : super::capture_error_label(code))}
                    }
                } else {
                    p { class: "text-sm text-emerald-700",
                        {t!("streams-test-captured", secs : secs)}
                    }
                    if let Some(code) = r.transcribe_error.as_deref() {
                        p { class: "text-sm text-amber-700", {transcribe_error_label(code)} }
                    }
                    if let Some(text) = r.text.as_deref() {
                        if text.trim().is_empty() {
                            p { class: "text-sm text-gray-500", {t!("asr-test-no-speech")} }
                        } else {
                            p { class: "text-sm text-gray-900 whitespace-pre-wrap rounded-lg border border-gray-200 bg-gray-50 p-3",
                                "{text}"
                            }
                        }
                    }
                }
            }
        }
    };
    rsx! {
        Modal {
            title: t!("streams-test-title", name : stream.name.clone()).to_string(),
            max_width: "max-w-xl".to_string(),
            on_close,
            div { class: "space-y-3",
                p { class: "text-xs text-gray-500", {t!("streams-test-help")} }
                {body}
            }
        }
    }
}
