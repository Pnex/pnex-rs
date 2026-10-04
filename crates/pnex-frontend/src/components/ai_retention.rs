//! Retention of the assistant conversations (D145): how long inactive
//! conversations are kept before automatic erasure. On the org page
//! (`platform: false`) an owner/admin may only SHORTEN the platform value;
//! on the platform page (`platform: true`) a platform admin sets it.

use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::api;
use crate::pages::system::parse_days;
use crate::state::toasts;

#[component]
pub fn AiRetentionCard(platform: bool) -> Element {
    let mut reload = use_signal(|| 0u32);
    let info = use_resource(move || async move {
        let _ = reload();
        let _ = crate::state::org::current();
        api::ai::retention().await
    });
    let mut input = use_signal(String::new);
    let mut saving = use_signal(|| false);

    let Some(Ok(current)) = info.value().read().clone() else {
        return rsx! {};
    };
    let editable = if platform {
        current.platform_editable
    } else {
        current.editable
    };

    let mut save = move |raw: String| {
        let Ok(days) = parse_days(&raw) else {
            toasts::error(t!("ai-retention-invalid").to_string());
            return;
        };
        saving.set(true);
        spawn(async move {
            let res = if platform {
                api::ai::set_platform_retention(days).await.map(|_| ())
            } else {
                api::ai::set_retention(days).await.map(|_| ())
            };
            match res {
                Ok(()) => {
                    input.set(String::new());
                    reload.with_mut(|r| *r += 1);
                }
                Err(err) => toasts::error(err),
            }
            saving.set(false);
        });
    };

    rsx! {
        section { class: "bg-white rounded-lg shadow p-6 space-y-3",
            h2 { class: "text-lg font-semibold text-gray-900", {t!("ai-retention-title")} }
            p { class: "text-sm text-gray-600", {t!("ai-retention-help")} }
            div { class: "flex items-baseline gap-2",
                span { class: "text-4xl font-bold text-gray-900",
                    if platform {
                        "{current.platform_days}"
                    } else {
                        "{current.days}"
                    }
                }
                span { class: "text-gray-600", {t!("ai-retention-days")} }
            }
            if !platform {
                p { class: "text-sm text-gray-500",
                    {t!("ai-retention-platform", days : current.platform_days)}
                }
            }
            if editable {
                div { class: "flex flex-wrap items-center gap-2",
                    input {
                        class: "w-28 border border-gray-300 rounded-md px-2 py-1 text-sm",
                        r#type: "number",
                        min: "1",
                        max: if platform { "3650".to_string() } else { current.platform_days.to_string() },
                        placeholder: t!("ai-retention-placeholder"),
                        value: "{input}",
                        oninput: move |e| input.set(e.value()),
                    }
                    button {
                        class: "px-3 py-1.5 rounded-md bg-blue-600 text-white text-sm hover:bg-blue-700 disabled:opacity-50",
                        disabled: saving(),
                        onclick: move |_| save(input()),
                        {t!("ai-retention-save")}
                    }
                    button {
                        class: "px-3 py-1.5 rounded-md border border-gray-300 text-sm text-gray-700 hover:bg-gray-50 disabled:opacity-50",
                        disabled: saving(),
                        onclick: move |_| save(String::new()),
                        if platform {
                            {t!("ai-retention-reset-platform")}
                        } else {
                            {t!("ai-retention-follow-platform")}
                        }
                    }
                }
            }
        }
    }
}
