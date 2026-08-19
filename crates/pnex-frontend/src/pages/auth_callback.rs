//! Callback OAuth (`/auth/callback?code=…`) : échange le code contre des
//! tokens (PKCE, verifier en sessionStorage), récupère le `user-info` (le
//! JIT provisioning côté backend crée l'org personnelle à cette occasion),
//! établit la session puis redirige vers le tableau de bord.
//!
//! Les segments query du routeur sont infaillibles : absents → chaîne vide,
//! on teste `is_empty()` et non l'existence.

use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::api;
use crate::app::Route;
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
                        Link {
                            to: Route::Dashboard {},
                            class: "inline-block px-4 py-2 bg-blue-600 text-white rounded-lg hover:bg-blue-700 transition-colors text-sm font-semibold",
                            {t!("callback-back")}
                        }
                    }
                },
                _ => rsx! {
                    div { class: "text-center space-y-4",
                        span { class: "animate-spin inline-block rounded-full h-10 w-10 border-b-2 border-blue-400" }
                        p { class: "text-sm text-gray-300", {t!("callback-exchanging")} }
                    }
                },
            }
        }
    }
}

/// Échange + session + navigation. Le message d'erreur renvoyé est celui du
/// serveur, tel quel (convention projet) ; les conditions purement locales
/// (code/verifier absents) renvoient une chaîne vide, la page affiche alors
/// le libellé générique.
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
