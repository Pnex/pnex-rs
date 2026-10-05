//! Écran « URL du serveur » — façon Bitwarden, natif uniquement (Android/
//! desktop) : le front n'est pas servi par le backend sur ces cibles, donc
//! pas de same-origin. Le web (wasm32) n'y passe jamais : URLs relatives,
//! origine de la page.
//!
//! Affiché au boot quand `api::config::api_base()` est vide : saisie +
//! validation du schéma http/https + probe `GET {base}/` (le backend Loco
//! sert l'UI statique → toute réponse non-5xx prouve la joignabilité), puis
//! persistance `pnex.api_base` via `storage::local()` — le seam `api::config`
//! est le seul consommateur de la clé.

use dioxus::prelude::*;
use dioxus_i18n::t;

// Utiles uniquement côté natif (probe + persistance) — sur wasm l'écran
// n'est jamais affiché et probe_and_store n'existe pas.
#[cfg(not(target_arch = "wasm32"))]
use crate::storage::{self, KeyValueStorage, KEY_API_BASE};

/// Router de secours de l'écran : `false` → l'écran ServerUrl remplace le
/// routeur (main.rs). Global (pas un signal local d'App) pour que la page de
/// login — après logout — puisse rouvrir cet écran (« Changer de serveur »).
pub static SERVER_READY: GlobalSignal<bool> = GlobalSignal::new(|| !needs_setup());

/// URL courante, pour préremplir la saisie (changement de serveur — l'URL
/// est proposée en édition, jamais à retaper de zéro).
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn current_base() -> String {
    storage::local().get(KEY_API_BASE).unwrap_or_default()
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn current_base() -> String {
    String::new()
}

/// Vrai quand l'écran doit s'afficher au boot (cible native, base URL non
/// résolue). Sur wasm32 : toujours faux (same-origin).
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

/// Erreurs de configuration — énumérée pour rester traduisible : `t!()` est
/// appelé dans le rsx, jamais dans le helper async (le dictionnaire i18n vit
/// dans le scope du composant).
#[derive(Clone)]
pub(crate) enum SetupError {
    Format,
    // Construit uniquement côté natif (probe_and_store n'existe pas en wasm).
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

/// Valide la saisie (schéma http/https), probe `GET {base}/` puis persiste
/// la base dans `pnex.api_base`. Sur wasm32 : n'existe pas (écran jamais
/// affiché).
#[cfg(not(target_arch = "wasm32"))]
pub(crate) async fn probe_and_store(raw: &str) -> Result<(), SetupError> {
    let base = raw.trim().trim_end_matches('/').to_string();
    let parsed: reqwest::Url = base.parse().map_err(|_| SetupError::Format)?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(SetupError::Format);
    }

    // Probe : toute réponse non-5xx prouve la joignabilité (le backend sert
    // l'UI statique → 200 attendu sur `/` ; un 404 d'un autre service reste
    // une preuve de joignabilité). Timeout court : réseau mobile.
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

/// Chemin unique « se connecter à ce serveur » : probe + persistance +
/// porte de compatibilité (reset puis re-vérification) + bascule
/// `SERVER_READY`. Emprunté par les trois points d'entrée : `ServerUrl`
/// (premier lancement / changement), `SelfHostedSection` (login) et le scan
/// LAN (`pages/lan_scan.rs`). La porte est remise en vérification AVANT la
/// bascule : l'écran courant laisse place au spinner de `GateScreen`,
/// jamais à un écran bloquant périmé (serveur précédent).
#[cfg(not(target_arch = "wasm32"))]
pub(crate) async fn apply_server(raw: &str) -> Result<(), SetupError> {
    let base = canonical_base(raw).await;
    probe_and_store(&base).await?;
    crate::state::compat::reset();
    // La vérification est DÉTACHÉE du scope appelant (`spawn_forever`, pas
    // `spawn` ni await direct) : dès `SERVER_READY=true`, App remplace le
    // sous-arbre courant (ServerUrl/login/scan) par la porte — une tâche
    // liée au scope serait annulée en pleine vérification et GATE resterait
    // figé sur Checking (constat 2026-09-09 sur device).
    dioxus::dioxus_core::spawn_forever(async {
        crate::state::compat::check().await;
    });
    SERVER_READY.with_mut(|v| *v = true);
    Ok(())
}

#[cfg(target_arch = "wasm32")]
pub(crate) async fn apply_server(_raw: &str) -> Result<(), SetupError> {
    // Web : same-origin, jamais appelé (garde de compilation).
    Err(SetupError::Format)
}

/// Section « Serveur auto-hébergé » de la page de login (façon Bitwarden :
/// la sélection du serveur vit DANS l'écran de connexion — pattern demandé
/// 2026-09-09). Dépliable : input prérempli avec l'URL courante, probe +
/// persistance au enregistrement. Web : same-origin, ne rend rien.
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
            // apply_server (et non probe_and_store) : enregistrer = se
            // connecter — bascule SERVER_READY + porte de compatibilité.
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
                class: "flex items-center gap-1.5 mx-auto text-xs text-gray-400 hover:text-gray-600",
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
                        class: "w-full py-2 px-3 rounded-lg border border-gray-300 text-xs text-gray-900                                 focus:outline-none focus:ring-2 focus:ring-blue-500 focus:border-transparent",
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
    // Prérempli avec l'URL courante si elle existe (changement de serveur).
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
            // apply_server : probe + persistance + porte de compatibilité +
            // bascule SERVER_READY (main.rs passe alors sur la porte, puis
            // le routeur).
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
                                        focus:outline-none focus:ring-2 focus:ring-blue-500 focus:border-transparent",
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
                                        bg-gradient-to-r from-blue-600 to-blue-700 hover:from-blue-700 hover:to-blue-800 \
                                        shadow-md hover:shadow-lg transform hover:-translate-y-0.5 transition-all \
                                        disabled:opacity-50 disabled:hover:translate-y-0 disabled:hover:shadow-md",
                                disabled: busy(),
                                onclick: connect,
                                {t!("server-url-connect")}
                            }
                        }

                        // Détection LAN des serveurs PNEX (natif) — premier
                        // lancement sans URL connue : le scan trouve le
                        // serveur auto-hébergé du réseau local.
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
