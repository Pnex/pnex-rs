//! Native login: system browser + backend bridge (polling).
//!
//! dioxus-desktop opens any http(s) navigation in the external browser
//! (`webbrowser::open` hard-coded in its navigation wrapper, with no
//! config switch): an embedded webview is impossible without a fork.
//! Chosen pattern (Bitwarden style): the app generates the PKCE, opens the
//! Rauthy login in the browser with `redirect_uri = {base}/api/v1/oauth2/native`
//! (the backend bridge, see controller `oauth2.rs`); Rauthy sends the code there,
//! and the app — left in the background — fetches it by polling the bridge,
//! exchanges it (local PKCE verifier) and opens the session: the user
//! finds the app already logged in on return.
//!
//! `webbrowser` is **vendored and patched** (`[patch.crates-io]`, see
//! `vendor/patches/README.md`): the upstream Android implementation SIGABRTs
//! the application (assert of the `jni` crate on Java exceptions, observed
//! 2026-09-08) — the patch rewrites it in raw JNI without panic. It is also what
//! makes the hard-coded `webbrowser::open` calls of dioxus-desktop safe
//! (webview + `<a>` links): a single choke point.

use dioxus_i18n::t;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::api::{auth, client, user};
use crate::state;

/// A single login at a time (double click, re-entry).
static LOGIN_ACTIVE: AtomicBool = AtomicBool::new(false);

/// Starts the native login: system browser on the absolute SSO URL, then
/// polling of the backend bridge (`state` = code_challenge of the flow, unique) to
/// fetch the authorization code and finalise the session. 10 min timeout.
pub fn open_login(abs_sso_url: String, state: String) {
    if LOGIN_ACTIVE.swap(true, Ordering::SeqCst) {
        eprintln!("pnex-login: open_login ignored (login already active)");
        return;
    }
    eprintln!("pnex-login: open_login started (state={state})");
    // Resolved before the spawn: no t! inside a detached task.
    let browser_error = t!("login-browser-error");
    dioxus::prelude::spawn(async move {
        // Explicit error toast: opening the browser may fail
        // (no browser on the device, ndk-context missing…) — never
        // a silent click. Polling starts ANYWAY: on Android, an
        // apparent Intent failure may hide a successful launch
        // (observed 2026-09-08: post-launch exception while Chrome
        // did open) — in the real worst case, polling expires
        // harmlessly.
        if let Err(err) = webbrowser::open(&abs_sso_url) {
            eprintln!("pnex-login: browser opening reported failed (polling kept): {err}");
            state::toasts::error(format!("{browser_error}: {err}"));
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
                    eprintln!("pnex-login: code captured, opening session");
                    finish(code).await;
                    break;
                }
            }
            // Pending or transient network error: retry until the
            // timeout (10 min) — the user is allowed to be slow.
        }
        LOGIN_ACTIVE.store(false, Ordering::SeqCst);
    });
}

/// Exchanges code+verifier → tokens → session. The error message is relayed
/// as is (project convention); success is silent: `session::login`
/// switches the UI and polling stops.
async fn finish(code: &str) {
    let Some(verifier) = auth::take_pkce_verifier() else {
        eprintln!("pnex-login: ABORT — PKCE verifier not found");
        return;
    };
    let redirect = auth::redirect_uri();
    eprintln!("pnex-login: exchange started (redirect={redirect})");
    let outcome: Result<(), crate::api::error::ApiError> = async {
        let tokens = auth::exchange_code(code, &verifier, &redirect).await?;
        eprintln!("pnex-login: exchange OK, storing tokens + user_info");
        auth::store_tokens(&tokens);
        let user = user::get_user_info().await?;
        state::session::login(user);
        eprintln!("pnex-login: session opened");
        Ok(())
    }
    .await;
    if let Err(err) = outcome {
        eprintln!("pnex-login: exchange/user_info failed: {err}");
        state::toasts::error(err.to_string());
    }
    LOGIN_ACTIVE.store(false, Ordering::SeqCst);
}
