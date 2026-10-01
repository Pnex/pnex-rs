//! Login natif : navigateur système + pont backend (polling).
//!
//! dioxus-desktop ouvre toute navigation http(s) dans le navigateur externe
//! (`webbrowser::open` codé en dur dans son wrapper de navigation, sans
//! interrupteur de config) : la webview embarquée est impossible sans fork.
//! Pattern retenu (façon Bitwarden) : l'app génère le PKCE, ouvre le login
//! Rauthy dans le navigateur avec `redirect_uri = {base}/api/v1/oauth2/native`
//! (le pont backend, cf. contrôleur `oauth2.rs`) ; Rauthy y rapatrie le code,
//! et l'app — restée en arrière-plan — le récupère par polling du pont,
//! l'échange (verifier PKCE local) et ouvre la session : l'utilisateur
//! retrouve l'app déjà connectée au retour.
//!
//! `webbrowser` est **vendu et patché** (`[patch.crates-io]`, cf.
//! `vendor/patches/README.md`) : l'implémentation Android amont SIGABRT
//! l'application (assert du crate `jni` sur les exceptions Java, constat
//! 2026-09-08) — le patch la réécrit en JNI brut sans panic. C'est aussi ce
//! qui rend sûrs les appels `webbrowser::open` codés en dur de
//! dioxus-desktop (webview + liens `<a>`) : un seul point de passage.

use std::sync::atomic::{AtomicBool, Ordering};

use crate::api::{auth, client, user};
use crate::state;

/// Un seul login à la fois (double-clic, re-entrée).
static LOGIN_ACTIVE: AtomicBool = AtomicBool::new(false);

/// Démarre le login natif : navigateur système sur l'URL SSO absolue, puis
/// polling du pont backend (`state` = code_challenge du flow, unique) pour
/// récupérer le code d'autorisation et finaliser la session. Timeout 10 min.
pub fn open_login(abs_sso_url: String, state: String) {
    if LOGIN_ACTIVE.swap(true, Ordering::SeqCst) {
        eprintln!("pnex-login: open_login ignoré (login déjà actif)");
        return;
    }
    eprintln!("pnex-login: open_login démarré (state={state})");
    dioxus::prelude::spawn(async move {
        // Erreur explicite en toast : l'ouverture navigateur peut échouer
        // (pas de navigateur sur l'appareil, ndk-context absent…) — jamais
        // de clic muet. Le polling démarre QUAND MÊME : sur Android, un
        // échec apparent de l'Intent peut masquer un lancement réussi
        // (constat 2026-09-08 : exception post-lancement alors que Chrome
        // s'ouvrait bel et bien) — dans le pire cas réel, le polling expire
        // sans conséquence.
        if let Err(err) = webbrowser::open(&abs_sso_url) {
            eprintln!(
                "pnex-login: ouverture navigateur signalée échouée (polling maintenu) : {err}"
            );
            state::toasts::error(format!("navigateur : {err}"));
        }
        for _ in 0..1200 {
            crate::util::sleep(std::time::Duration::from_millis(500)).await;
            let polled = client::request_opt::<serde_json::Value>(
                reqwest::Method::GET,
                &format!("/api/v1/oauth2/native/{state}"),
                None,
            )
            .await;
            if let Ok(Some(value)) = polled {
                if let Some(code) = value.get("code").and_then(|c| c.as_str()) {
                    eprintln!("pnex-login: code capté, ouverture de session");
                    finish(code).await;
                    break;
                }
            }
            // En attente ou erreur réseau passagère : on réessaie jusqu'au
            // timeout (10 min) — l'utilisateur a le droit d'être lent.
        }
        LOGIN_ACTIVE.store(false, Ordering::SeqCst);
    });
}

/// Échange code+verifier → tokens → session. Le message d'erreur est relayé
/// tel quel (convention projet) ; le succès est muet : `session::login`
/// bascule l'UI et le polling s'arrête.
async fn finish(code: &str) {
    let Some(verifier) = auth::take_pkce_verifier() else {
        eprintln!("pnex-login: ABANDON — verifier PKCE introuvable");
        return;
    };
    let redirect = auth::redirect_uri();
    eprintln!("pnex-login: exchange lancé (redirect={redirect})");
    let outcome: Result<(), crate::api::error::ApiError> = async {
        let tokens = auth::exchange_code(code, &verifier, &redirect).await?;
        eprintln!("pnex-login: exchange OK, stockage tokens + user_info");
        auth::store_tokens(&tokens);
        let user = user::get_user_info().await?;
        state::session::login(user);
        eprintln!("pnex-login: session ouverte");
        Ok(())
    }
    .await;
    if let Err(err) = outcome {
        eprintln!("pnex-login: exchange/user_info échoué : {err}");
        state::toasts::error(err.to_string());
    }
    LOGIN_ACTIVE.store(false, Ordering::SeqCst);
}
