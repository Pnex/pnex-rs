//! Failure reason of a firmware build (O4): the translated code stored by
//! the worker and, for a compiler failure, its output (credentials masked
//! server side).

use dioxus::prelude::*;
use dioxus_i18n::t;

/// Localized text of a build failure code (`build-fail-<code>`); an
/// unknown code (newer server) falls back to the generic text.
pub fn build_failure_text(code: &str) -> String {
    dioxus_i18n::prelude::i18n()
        .try_translate(&format!("build-fail-{}", code.replace('_', "-")))
        .unwrap_or_else(|_| t!("build-fail-unknown").to_string())
}

/// Red note: why the build failed, the tool output folded below.
#[component]
pub fn BuildFailureNote(code: Option<String>, detail: Option<String>) -> Element {
    let text = code
        .as_deref()
        .map(build_failure_text)
        .unwrap_or_else(|| t!("build-fail-unknown").to_string());
    rsx! {
        div { class: "rounded-lg border border-red-200 bg-red-50 p-3 text-sm text-red-700",
            p { class: "font-medium", {t!("build-fail-title")} }
            p { "{text}" }
            if let Some(detail) = detail {
                details { class: "mt-2",
                    summary { class: "cursor-pointer text-xs text-red-600", {t!("build-fail-output")} }
                    pre { class: "mt-1 max-h-64 overflow-auto whitespace-pre-wrap break-all rounded bg-white p-2 font-mono text-[11px] text-gray-800",
                        "{detail}"
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    /// Every failure code of the worker has its text in both locales.
    #[test]
    fn every_failure_code_is_translated() {
        for ftl in [
            include_str!("../../locales/en-US.ftl"),
            include_str!("../../locales/fr-FR.ftl"),
        ] {
            for code in pnex_core::BUILD_FAILURE_CODES {
                let key = format!("\nbuild-fail-{} =", code.replace('_', "-"));
                assert!(ftl.contains(&key), "missing {key}");
            }
        }
    }
}
