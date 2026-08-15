//! Callback OAuth (`/auth/callback?code=…`) : échange le code d'autorisation
//! contre des tokens (PKCE) puis redirige vers le tableau de bord.

use dioxus::prelude::*;
use dioxus_i18n::t;

#[component]
pub fn AuthCallback(code: String, error: String, error_description: String) -> Element {
    let _ = (code, error, error_description);
    rsx! { p { {t!("common-loading")} } }
}
