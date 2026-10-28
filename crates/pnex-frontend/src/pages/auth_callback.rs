//! OAuth callback (`/auth/callback?code=…`): exchanges the code for
//! tokens (PKCE, verifier in sessionStorage), fetches the `user-info` (the
//! backend JIT provisioning creates the personal org on that occasion),
//! establishes the session then redirects to the dashboard.
//!
//! The router query segments are infallible: missing → empty string,
//! so we test `is_empty()` and not existence.

use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::api;
use crate::app::Route;
use crate::components::gerbe::Gerbe;
use crate::state;

#[component]
pub fn AuthCallback(code: String, error: String, error_description: String) -> Element {
    let nav = navigator();
    let outcome = use_resource(move || {
        let nav = nav;
        let code = code.clone();
        let error = error.clone();
        let error_description = error_description.clone();
        async move { run(&code, &error, &error_description, nav).await }
    });

    rsx! {
        div { class: "min-h-screen flex items-center justify-center bg-gray-900 px-4",
            match outcome.value().cloned() {
                Some(Err(message)) => rsx! {
                    div { class: "max-w-md w-full bg-white/95 rounded-2xl shadow-2xl p-8 text-center space-y-4",
                        crate::components::icons::AlertTriangle { class: "h-10 w-10 text-red-500 mx-auto" }
                        h2 { class: "text-xl font-bold text-gray-900", {t!("callback-failed")} }
                        p { class: "text-sm text-gray-700 break-words", {message} }
                        Link { to: Route::Dashboard {}, class: crate::components::ui::BTN_PRIMARY, {t!("callback-back")} }
                    }
                },
                _ => rsx! {
                    div { class: "text-center space-y-4",
                        img {
                            src: asset!("/assets/logo-light.png"),
                            alt: "PNeX",
                            class: "h-12 w-auto",
                        }
                        Gerbe { size: 44 }
                        p { class: "text-sm text-gray-300", {t!("callback-exchanging")} }
                    }
                },
            }
        }
    }
}

/// Exchange + session + navigation. The returned error message is the
/// server's, as is (project convention); purely local conditions
/// (code/verifier missing) return an empty string, the page then shows
/// the generic label.
async fn run(
    code: &str,
    error: &str,
    error_description: &str,
    nav: dioxus::router::Navigator,
) -> Result<(), String> {
    if !error.is_empty() {
        let detail = if error_description.is_empty() {
            error.to_string()
        } else {
            format!("{error} : {error_description}")
        };
        return Err(detail);
    }
    if code.is_empty() {
        return Err(String::new());
    }
    let Some(verifier) = api::auth::take_pkce_verifier() else {
        return Err(String::new());
    };
    let result: Result<(), api::error::ApiError> = async {
        let tokens = api::auth::exchange_code(code, &verifier, &api::auth::redirect_uri()).await?;
        api::auth::store_tokens(&tokens);
        let user = api::user::get_user_info().await?;
        state::session::login(user);
        Ok(())
    }
    .await;
    match result {
        Ok(()) => {
            nav.replace(Route::Dashboard {});
            Ok(())
        }
        Err(err) => Err(err.to_string()),
    }
}
