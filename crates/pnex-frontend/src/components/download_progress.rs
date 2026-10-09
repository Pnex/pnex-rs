//! Download progress of a large media (splat, panorama) over its dark
//! viewer box: spinner, bar and MB count while the bytes arrive, so a
//! 100 MB splat no longer looks like a frozen black box.

use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::util::DownloadProgress;

/// Overlay shown while `progress` is `Some`; place it in a `relative`
/// parent. Indeterminate (no bar) when the server sent no length.
#[component]
pub fn MediaDownloadProgress(progress: Option<DownloadProgress>) -> Element {
    let Some((received, total)) = progress else {
        return rsx! {};
    };
    let mb = |n: u64| format!("{:.1}", n as f64 / 1_000_000.0);
    let percent = total.map(|t| (received.min(t) * 100 / t.max(1)) as u32);
    let size = match total {
        Some(t) => t!("media-downloading-size", received: mb(received), total: mb(t)).to_string(),
        None => t!("media-downloading-received", received: mb(received)).to_string(),
    };
    rsx! {
        div { class: "absolute inset-0 z-20 flex items-center justify-center bg-gray-900/80 pointer-events-none",
            div { class: "flex w-64 max-w-[80%] flex-col items-center gap-2",
                span { class: "animate-spin inline-block rounded-full h-8 w-8 border-2 border-white border-t-transparent" }
                span { class: "text-sm font-medium text-white", {t!("media-downloading")} }
                if let Some(p) = percent {
                    div { class: "h-1.5 w-full overflow-hidden rounded-full bg-white/20",
                        div {
                            class: "h-full rounded-full bg-blue-500 transition-[width]",
                            style: "width: {p}%",
                        }
                    }
                    span { class: "text-xs tabular-nums text-gray-300", "{size} · {p} %" }
                } else {
                    span { class: "text-xs tabular-nums text-gray-300", "{size}" }
                }
            }
        }
    }
}
