//! "Server URL" screen — Bitwarden style, native only (Android/
//! desktop): the front is not served by the backend on these targets, so
//! no same-origin. The web (wasm32) never goes through it: relative URLs,
//! page origin.
//!
//! Shown at boot when `api::config::api_base()` is empty: input +
//! http/https scheme validation + probe `GET {base}/` (the Loco backend
//! serves the static UI → any non-5xx response proves reachability), then
//! persistence of `pnex.api_base` via `storage::local()` — the `api::config`
//! seam is the only consumer of the key.

use dioxus::prelude::*;
use dioxus_i18n::t;

// Only useful natively (probe + persistence) — on wasm the screen
// is never shown and probe_and_store does not exist.
#[cfg(not(target_arch = "wasm32"))]
use crate::storage::{self, KeyValueStorage, KEY_API_BASE};

/// Fallback router of the screen: `false` → the ServerUrl screen replaces the
/// router (main.rs). Global (not a local App signal) so that the login
/// page — after logout — can reopen this screen ("Change server").
pub static SERVER_READY: GlobalSignal<bool> = GlobalSignal::new(|| !needs_setup());

/// Current URL, to prefill the input (server change — the URL is
/// offered for editing, never retyped from scratch).
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn current_base() -> String {
    storage::local().get(KEY_API_BASE).unwrap_or_default()
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn current_base() -> String {
    String::new()
}

/// True when the screen must show at boot (native target, base URL not
/// resolved). On wasm32: always false (same-origin).
pub fn needs_setup() -> bool {
    #[cfg(target_arch = "wasm32")]
    {
        false
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        crate::api::config::api_base().is_empty()
    }
}

/// Configuration errors — enumerated to stay translatable: `t!()` is
/// called in the rsx, never in the async helper (the i18n dictionary lives
/// in the component scope).
#[derive(Clone)]
pub(crate) enum SetupError {
    Format,
    // Built only natively (probe_and_store does not exist in wasm).
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    Probe(String),
    /// HTTPS server signed by an unknown CA that the server hands out: the
    /// trust dialog (`pages/trust_ca.rs`) takes over, callers show nothing.
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    AwaitingTrust,
}

/// Root CA offered by a server whose certificate is not trusted yet —
/// shown by `TrustCaDialog` for a fingerprint confirmation.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PendingCa {
    pub base: String,
    pub pem: String,
    pub fingerprint: String,
}

/// Set by `apply_server` when the server needs a CA confirmation; rendered
/// as an overlay by `App` whatever the screen underneath.
pub(crate) static PENDING_CA: GlobalSignal<Option<PendingCa>> = GlobalSignal::new(|| None);

/// Validates the input (http/https scheme), probes `GET {base}/` then persists
/// the base in `pnex.api_base`. On wasm32: does not exist (screen never
/// shown).
#[cfg(not(target_arch = "wasm32"))]
pub(crate) async fn probe_and_store(raw: &str) -> Result<(), SetupError> {
    let base = raw.trim().trim_end_matches('/').to_string();
    let parsed: reqwest::Url = base.parse().map_err(|_| SetupError::Format)?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(SetupError::Format);
    }

    // Probe: any non-5xx response proves reachability (the backend serves
    // the static UI → 200 expected on `/`; a 404 from another service is still
    // proof of reachability). Short timeout: mobile network.
    let client = crate::api::tls::client(Some(std::time::Duration::from_secs(10)));
    let response = match client.get(format!("{base}/")).send().await {
        Ok(response) => response,
        Err(err) if parsed.scheme() == "https" => return Err(offer_server_ca(&base, &err).await),
        Err(err) => return Err(SetupError::Probe(err.to_string())),
    };
    if response.status().is_server_error() {
        return Err(SetupError::Probe(format!("HTTP {}", response.status())));
    }

    storage::local().set(KEY_API_BASE, &base);
    Ok(())
}

/// HTTPS probe failed: if the server hands out a root CA (`META_CA_PATH`,
/// fetched without verification) that DOES validate its certificate, park
/// it in `PENDING_CA` for a fingerprint confirmation. Otherwise the original
/// network error stands (server down, wrong URL, CA mismatch).
#[cfg(not(target_arch = "wasm32"))]
async fn offer_server_ca(base: &str, err: &reqwest::Error) -> SetupError {
    let failure = || SetupError::Probe(err.to_string());
    let timeout = std::time::Duration::from_secs(5);
    let Ok(response) = crate::api::tls::discovery_client()
        .get(format!("{base}{}", pnex_api_contract::META_CA_PATH))
        .timeout(timeout)
        .send()
        .await
    else {
        return failure();
    };
    if !response.status().is_success() {
        return failure();
    }
    let pem = response.text().await.unwrap_or_default();
    let Some(fingerprint) = crate::api::tls::fingerprint(&pem) else {
        return failure();
    };
    // The offered CA must actually sign the served certificate — otherwise
    // pinning it would not help, and the user would confirm for nothing.
    let verified = crate::api::tls::client_with_ca(Some(&pem), Some(timeout))
        .get(format!("{base}/"))
        .send()
        .await;
    if verified.is_err() {
        return failure();
    }
    PENDING_CA.with_mut(|slot| {
        *slot = Some(PendingCa {
            base: base.to_string(),
            pem,
            fingerprint,
        });
    });
    SetupError::AwaitingTrust
}

/// Base kept for `raw` (`ServerInfo::origin`): a server reached by IP
/// literal (LAN scan, typed address) that announces its canonical origin is
/// stored under that origin, since its OIDC redirect URIs and certificate
/// name are issued for it, not for the IP. The origin is adopted only when
/// it is an https domain name that answers as a PNEX server AT THAT SAME
/// IP: a LAN host cannot send the app to another machine. Otherwise (older
/// server, name not resolvable on this client, different machine) `raw`
/// stays, as before. Identity reads only, no trust: the probes use the
/// discovery client, TLS is settled afterwards by `probe_and_store`.
#[cfg(not(target_arch = "wasm32"))]
async fn canonical_base(raw: &str) -> String {
    let base = raw.trim().trim_end_matches('/').to_string();
    let Some(ip) = ip_literal_host(&base) else {
        return base;
    };
    let timeout = std::time::Duration::from_secs(5);
    let client = crate::api::tls::discovery_client();
    let Ok(info) = crate::api::meta::probe(&client, timeout, &base).await else {
        return base;
    };
    let Some(origin) = info.origin.as_deref().and_then(domain_origin) else {
        return base;
    };
    match crate::api::meta::probe_with_peer(&client, timeout, &origin).await {
        Ok((_, Some(peer))) if peer.ip() == ip => origin,
        _ => base,
    }
}

/// Host of `base` when it is an IP literal (`https://192.168.1.20`).
#[cfg(not(target_arch = "wasm32"))]
fn ip_literal_host(base: &str) -> Option<std::net::IpAddr> {
    let url = reqwest::Url::parse(base).ok()?;
    let host = url.host_str()?;
    host.trim_start_matches('[')
        .trim_end_matches(']')
        .parse()
        .ok()
}

/// `origin` normalised (`https://host[:port]`) when it is an https URL on a
/// domain name — an IP origin brings nothing over the probed IP.
#[cfg(not(target_arch = "wasm32"))]
fn domain_origin(origin: &str) -> Option<String> {
    let url = reqwest::Url::parse(origin.trim()).ok()?;
    if url.scheme() != "https" {
        return None;
    }
    let host = url.host_str()?;
    if ip_literal_host(&format!("https://{host}")).is_some() {
        return None;
    }
    Some(match url.port() {
        Some(port) => format!("https://{host}:{port}"),
        None => format!("https://{host}"),
    })
}

/// Single "connect to this server" path: probe + persistence +
/// compatibility gate (reset then re-check) + `SERVER_READY`
/// switch. Used by the three entry points: `ServerUrl`
/// (first launch / change), `SelfHostedSection` (login) and the LAN
/// scan (`pages/lan_scan.rs`). The gate is put back into checking BEFORE the
/// switch: the current screen gives way to the `GateScreen` spinner,
/// never to a stale blocking screen (previous server).
#[cfg(not(target_arch = "wasm32"))]
pub(crate) async fn apply_server(raw: &str) -> Result<(), SetupError> {
    let base = canonical_base(raw).await;
    probe_and_store(&base).await?;
    crate::state::compat::reset();
    // The check is DETACHED from the calling scope (`spawn_forever`, not
    // `spawn` nor a direct await): as soon as `SERVER_READY=true`, App replaces
    // the current subtree (ServerUrl/login/scan) with the gate — a task
    // bound to the scope would be cancelled mid-check and GATE would stay
    // stuck on Checking (finding 2026-09-09 on device).
    dioxus::dioxus_core::spawn_forever(async {
        crate::state::compat::check().await;
    });
    SERVER_READY.with_mut(|v| *v = true);
    Ok(())
}

#[cfg(target_arch = "wasm32")]
pub(crate) async fn apply_server(_raw: &str) -> Result<(), SetupError> {
    // Web: same-origin, never called (compilation guard).
    Err(SetupError::Format)
}

/// "Self-hosted server" section of the login page (Bitwarden style:
/// server selection lives IN the login screen — pattern requested
/// 2026-09-09). Collapsible: input prefilled with the current URL, probe +
/// persistence on save. Web: same-origin, renders nothing.
#[component]
pub(crate) fn SelfHostedSection() -> Element {
    if cfg!(target_arch = "wasm32") || crate::api::config::locked_server().is_some() {
        return rsx! {};
    }

    let mut open = use_signal(|| false);
    let mut url = use_signal(current_base);
    let mut busy = use_signal(|| false);
    let mut error: Signal<Option<SetupError>> = use_signal(|| None);
    let mut saved = use_signal(|| false);

    let save = move |_| {
        let raw = url.read().clone();
        if raw.trim().is_empty() {
            error.set(Some(SetupError::Format));
            saved.set(false);
            return;
        }
        busy.set(true);
        error.set(None);
        saved.set(false);
        spawn(async move {
            // apply_server (not probe_and_store): saving = connecting
            // — SERVER_READY switch + compatibility gate.
            match apply_server(&raw).await {
                Ok(()) => {
                    saved.set(true);
                    open.set(false);
                }
                Err(e) => error.set(Some(e)),
            }
            busy.set(false);
        });
    };

    rsx! {
        div { class: "pt-1",
            button {
                class: "flex items-center gap-1.5 mx-auto text-xs text-gray-500 hover:text-gray-600",
                onclick: move |_| {
                    open.with_mut(|o| *o = !*o);
                    saved.set(false);
                },
                crate::components::icons::Server { class: "h-3.5 w-3.5" }
                {t!("login-selfhosted-toggle")}
            }
            if open() {
                div { class: "mt-3 space-y-2",
                    input {
                        class: "w-full py-2 px-3 rounded-lg border border-gray-300 text-xs text-gray-900                                 focus:outline-none focus:ring-2 focus:ring-signal-500 focus:border-transparent",
                        r#type: "url",
                        placeholder: t!("server-url-placeholder"),
                        value: "{url}",
                        oninput: move |e| {
                            url.set(e.value());
                            saved.set(false);
                        },
                    }
                    if busy() {
                        p { class: "text-xs text-gray-500 text-center", {t!("server-url-testing")} }
                    }
                    if saved() {
                        p { class: "text-xs text-green-600 text-center",
                            {t!("login-selfhosted-saved")}
                        }
                    }
                    if matches!(error.read().as_ref(), Some(SetupError::Format)) {
                        p { class: "text-xs text-red-600 text-center",
                            {t!("server-url-error-format")}
                        }
                    }
                    if let Some(SetupError::Probe(msg)) = error.read().as_ref() {
                        p { class: "text-xs text-red-600 text-center",
                            {t!("server-url-error-network", msg : msg)}
                        }
                    }
                    button {
                        class: "w-full py-2 px-3 rounded-lg text-xs font-semibold text-white                                 bg-gray-700 hover:bg-gray-800 transition-colors                                 disabled:opacity-50",
                        disabled: busy(),
                        onclick: save,
                        {t!("login-selfhosted-save")}
                    }
                    // LAN discovery of PNEX servers (native) — complements
                    // manual entry: probes https + :5150 on the local /24.
                    crate::pages::lan_scan::LanScanSection {}
                }
            }
        }
    }
}

#[component]
pub fn ServerUrl() -> Element {
    // Prefilled with the current URL if it exists (server change).
    let mut url = use_signal(current_base);
    let mut busy = use_signal(|| false);
    let mut error: Signal<Option<SetupError>> = use_signal(|| None);

    let connect = move |_| {
        if url.read().trim().is_empty() {
            error.set(Some(SetupError::Format));
            return;
        }
        let raw = url.read().clone();
        busy.set(true);
        error.set(None);
        spawn(async move {
            // apply_server: probe + persistence + compatibility gate +
            // SERVER_READY switch (main.rs then moves to the gate, then
            // the router).
            match apply_server(&raw).await {
                Ok(()) => {}
                Err(e) => error.set(Some(e)),
            }
            busy.set(false);
        });
    };

    rsx! {
        div { class: "relative min-h-screen overflow-hidden bg-gray-900",
            div {
                class: "absolute inset-0",
                style: "background: linear-gradient(135deg, #040d10 0%, #0b2830 100%)",
            }

            div { class: "relative z-10 min-h-screen flex items-center justify-center px-4",
                div { class: "max-w-md w-full",
                    div { class: "bg-white/95 backdrop-blur-sm rounded-2xl shadow-2xl p-8 space-y-8",
                        div { class: "text-center",
                            img {
                                src: asset!("/assets/logo.png"),
                                alt: "PNeX",
                                class: "mx-auto h-16 w-auto mb-5",
                            }
                            h1 { class: "text-lg font-semibold text-gray-900",
                                {t!("server-url-title")}
                            }
                            p { class: "text-xs text-gray-500 mt-1", {t!("server-url-description")} }
                        }

                        div { class: "space-y-4",
                            input {
                                class: "w-full py-3 px-4 rounded-lg border border-gray-300 text-sm text-gray-900 \
                                        focus:outline-none focus:ring-2 focus:ring-signal-500 focus:border-transparent",
                                r#type: "url",
                                placeholder: t!("server-url-placeholder"),
                                value: "{url}",
                                oninput: move |e| url.set(e.value()),
                            }
                            if busy() {
                                p { class: "text-sm text-gray-500 text-center",
                                    {t!("server-url-testing")}
                                }
                            }
                            if matches!(error.read().as_ref(), Some(SetupError::Format)) {
                                p { class: "text-sm text-red-600 text-center",
                                    {t!("server-url-error-format")}
                                }
                            }
                            if let Some(SetupError::Probe(msg)) = error.read().as_ref() {
                                p { class: "text-sm text-red-600 text-center",
                                    {t!("server-url-error-network", msg : msg)}
                                }
                            }
                            button {
                                class: "w-full flex justify-center items-center py-3 px-4 rounded-lg text-sm font-semibold text-white \
                                        bg-gradient-to-r from-signal-600 to-signal-700 hover:from-signal-700 hover:to-signal-800 \
                                        shadow-md hover:shadow-lg transform hover:-translate-y-0.5 transition-[translate,box-shadow] duration-150 ease-out-ui \
                                        disabled:opacity-50 disabled:hover:translate-y-0 disabled:hover:shadow-md",
                                disabled: busy(),
                                onclick: connect,
                                {t!("server-url-connect")}
                            }
                        }

                        // LAN detection of PNEX servers (native) — first
                        // launch without a known URL: the scan finds the
                        // self-hosted server of the local network.
                        crate::pages::lan_scan::LanScanSection {}

                        div { class: "pt-6 border-t border-gray-200",
                            p { class: "text-xs text-gray-500", {t!("server-url-footer")} }
                        }
                    }
                }
            }
        }
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::{domain_origin, ip_literal_host};

    #[test]
    fn only_ip_literal_bases_are_canonicalised() {
        assert_eq!(
            ip_literal_host("https://192.168.1.20"),
            Some("192.168.1.20".parse().unwrap())
        );
        assert_eq!(
            ip_literal_host("http://[fe80::1]:5150"),
            Some("fe80::1".parse().unwrap())
        );
        assert_eq!(ip_literal_host("https://pnex.local"), None);
        assert_eq!(ip_literal_host("not a url"), None);
    }

    #[test]
    fn origin_must_be_an_https_domain() {
        assert_eq!(
            domain_origin("https://pnex.local/").as_deref(),
            Some("https://pnex.local")
        );
        assert_eq!(
            domain_origin("https://pnex.home:8443").as_deref(),
            Some("https://pnex.home:8443")
        );
        // The dev issuer is an IP (Rauthy's own port): never adopted.
        assert_eq!(domain_origin("https://192.168.1.185:8443"), None);
        assert_eq!(domain_origin("http://pnex.local"), None);
        assert_eq!(domain_origin(""), None);
    }
}
