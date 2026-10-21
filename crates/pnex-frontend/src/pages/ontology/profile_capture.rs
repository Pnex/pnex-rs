//! Profile capture of a person (account-mapping.md D333) and its
//! prospecting objection (D341): one pasted profile address, captured once
//! by a browser agent of the org in its signed-in session.

use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::api;
use crate::components::crud::form::FormDialog;
use crate::state::toasts;

#[component]
pub fn ProfileCaptureDialog(person_id: String, on_close: EventHandler<()>) -> Element {
    let mut url = use_signal(String::new);
    let mut busy = use_signal(|| false);
    let mut reload = use_signal(|| 0u32);
    // The objection is final: a first click asks to confirm.
    let mut confirming = use_signal(|| false);
    let pid = person_id.clone();
    let opposition = use_resource(move || {
        let id = pid.clone();
        async move {
            let _ = reload();
            api::extension::opposition(&id).await.ok()
        }
    });
    let opposed = opposition
        .read()
        .clone()
        .flatten()
        .is_some_and(|o| o.opposed);
    let submit_id = person_id.clone();
    let oppose_id = person_id.clone();

    rsx! {
        FormDialog {
            title: t!("profile-capture-title").to_string(),
            submit_label: t!("profile-capture-submit").to_string(),
            on_close: move |_| on_close.call(()),
            busy: busy(),
            valid: !opposed && !url().trim().is_empty(),
            on_submit: move |_| {
                let id = submit_id.clone();
                let raw = url();
                let done = t!("profile-capture-requested").to_string();
                busy.set(true);
                spawn(async move {
                    match api::extension::capture_profile(&id, &raw).await {
                        Ok(_) => {
                            toasts::success(done);
                            on_close.call(());
                        }
                        Err(e) => toasts::error(e),
                    }
                    busy.set(false);
                });
            },
            if opposed {
                p {
                    class: "rounded-lg bg-amber-50 px-3 py-2 text-sm text-amber-800",
                    "data-testid": "profile-opposed",
                    {t!("profile-capture-opposed")}
                }
            } else {
                label { class: "block text-sm font-medium text-gray-700",
                    {t!("profile-capture-url")}
                    input {
                        class: "mt-1 w-full px-3 py-2 border border-gray-300 rounded-lg text-sm bg-white",
                        r#type: "url",
                        "data-testid": "profile-url",
                        placeholder: "https://www.linkedin.com/in/…",
                        value: "{url}",
                        oninput: move |e| url.set(e.value()),
                    }
                }
                p { class: "text-xs text-gray-500", {t!("profile-capture-help")} }
                // D254: the platform's terms may forbid automated access.
                p { class: "rounded-lg bg-amber-50 px-3 py-2 text-xs text-amber-800",
                    {t!("profile-capture-terms-warning")}
                }
                button {
                    class: "text-xs text-red-700 underline",
                    r#type: "button",
                    "data-testid": "profile-oppose",
                    disabled: busy(),
                    onclick: move |_| {
                        if !confirming() {
                            confirming.set(true);
                            return;
                        }
                        let id = oppose_id.clone();
                        let done = t!("profile-opposition-recorded").to_string();
                        spawn(async move {
                            match api::extension::oppose(&id).await {
                                Ok(_) => {
                                    toasts::success(done);
                                    reload.with_mut(|r| *r += 1);
                                }
                                Err(e) => toasts::error(e),
                            }
                        });
                    },
                    if confirming() {
                        {t!("profile-opposition-confirm")}
                    } else {
                        {t!("profile-opposition-record")}
                    }
                }
            }
        }
    }
}
