//! Détection LAN des serveurs PNEX — natif uniquement (wasm : `rsx!{}` vide,
//! même garde que `SelfHostedSection`).
//!
//! Probes `GET {base}{META_VERSION_PATH}` on the 254 hosts of a /24, 48
//! hosts in parallel, short timeout per probe. Each host is probed on both
//! `https://{ip}` (TLS edge, D70 — preferred) and `http://{ip}:5150` (direct
//! backend, fallback); the HTTPS probe skips certificate checks because it
//! only reads the public identity card — trust is settled at connect time
//! (`apply_server` → CA fingerprint confirmation). A host is kept only if
//! it identifies as `pnex-server` (contract `service` field). Connecting to
//! a result goes through the single `apply_server` path (probe + storage +
//! compatibility gate + `SERVER_READY`).
//!
//! Sous-réseau sondé : dérivé de l'URL serveur courante si c'est une IP
//! privée, sinon détection JNI de l'interface active (Android), sinon
//! `192.168.1.` éditable. JNI brut même école que `storage::imp` : jamais de
//! panic, dégradation silencieuse en fallback.

use std::net::Ipv4Addr;
use std::time::Duration;

use dioxus::prelude::*;
use dioxus_i18n::t;
use futures::StreamExt;
use pnex_api_contract::{compatible, ServerInfo, CONTRACT};

use crate::api::meta;
use crate::pages::server_url::{apply_server, current_base};

/// PNEX server port (default binding, config/*.yaml) — plain HTTP fallback
/// when the host has no TLS edge.
const SCAN_PORT: u16 = 5150;
/// Timeout par sonde : une IP injoignable doit libérer le slot vite.
const SCAN_TIMEOUT: Duration = Duration::from_millis(1200);
/// Parallélisme : 48 sondes simultanées → /24 complet en quelques secondes
/// (réseau WiFi mobile inclus).
const SCAN_CONCURRENCY: usize = 48;
/// Préfixe de repli (le plus commun des réseaux domestiques) — éditable.
const DEFAULT_PREFIX: &str = "192.168.1.";

/// Un serveur PNEX détecté : base URL (`http://192.168.1.16:5150`) + carte
/// d'identité renvoyée par le endpoint meta.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ScanHit {
    pub url: String,
    pub info: ServerInfo,
}

#[component]
pub(crate) fn LanScanSection() -> Element {
    if cfg!(target_arch = "wasm32") {
        return rsx! {};
    }

    let mut scanning = use_signal(|| false);
    let mut prefix = use_signal(default_prefix);
    // `None` = pas encore scanné (rien d'affiché), `Some(vec)` = résultat.
    let mut hits = use_signal(|| None::<Vec<ScanHit>>);
    let mut error = use_signal(|| None::<String>);
    // Progression déterminée : chaque sonde résolue incrémente `done`
    // (total = candidats réellement sondés, fixé au démarrage du scan).
    let mut done = use_signal(|| 0usize);
    let mut total = use_signal(|| 0usize);

    let scan = move |_| {
        let p = prefix.read().clone();
        scanning.set(true);
        error.set(None);
        done.set(0);
        total.set(0);
        spawn(async move {
            match run_scan(&p, done, total).await {
                Ok(found) => hits.set(Some(found)),
                Err(msg) => {
                    error.set(Some(msg));
                    hits.set(Some(Vec::new()));
                }
            }
            scanning.set(false);
        });
    };

    let scanned = done.cloned();
    let targets = total.cloned();
    let pct = scanned
        .checked_mul(100)
        .and_then(|v| v.checked_div(targets))
        .unwrap_or(0);

    rsx! {
        div { class: "pt-4 border-t border-gray-200 mt-4 space-y-2",
            p { class: "text-xs text-gray-500", {t!("scan-prefix-label")} }
            div { class: "flex items-center gap-2",
                input {
                    class: "flex-1 py-2 px-3 rounded-lg border border-gray-300 text-xs text-gray-900 \
                            focus:outline-none focus:ring-2 focus:ring-blue-500 focus:border-transparent",
                    r#type: "text",
                    placeholder: DEFAULT_PREFIX,
                    value: "{prefix}",
                    oninput: move |e| prefix.set(e.value()),
                }
                button {
                    class: "py-2 px-3 rounded-lg text-xs font-semibold text-white \
                            bg-gray-700 hover:bg-gray-800 transition-colors disabled:opacity-50",
                    disabled: scanning(),
                    onclick: scan,
                    {t!("scan-toggle")}
                }
            }
            if scanning() {
                div { class: "space-y-1",
                    div { class: "h-2 w-full bg-gray-200 rounded-full overflow-hidden",
                        div {
                            class: "h-full bg-blue-600 rounded-full transition-all duration-200",
                            style: "width: {pct}%",
                        }
                    }
                    p { class: "text-xs text-gray-500 text-center",
                        {t!("scan-progress", done : scanned.to_string(), total : targets.to_string())}
                    }
                }
            }
            if let Some(err) = error.read().as_ref() {
                p { class: "text-xs text-red-600 text-center", {err.clone()} }
            }
            if let Some(list) = hits.cloned() {
                if list.is_empty() {
                    p { class: "text-xs text-gray-400 text-center", {t!("scan-none")} }
                }
                for hit in list {
                    ScanRow { hit }
                }
            }
        }
    }
}

/// Une ligne de résultat : URL + version, badge de compatibilité de contrat
/// et bouton « Se connecter » (désactivé si le contrat du serveur diffère —
/// la porte refuserait la connexion).
#[component]
fn ScanRow(hit: ScanHit) -> Element {
    let ScanHit { url, info } = hit;
    let ok = compatible(CONTRACT, info.contract);
    let url_for_click = url.clone();

    rsx! {
        div { class: "flex items-center justify-between gap-2 py-1.5",
            div { class: "min-w-0 text-left",
                p { class: "text-xs font-medium text-gray-900 truncate", "{url}" }
                p { class: "text-xs text-gray-500", "v{info.version}" }
            }
            div { class: "flex items-center gap-2 shrink-0",
                if ok {
                    span { class: "text-xs text-green-600", {t!("scan-compatible")} }
                } else {
                    span { class: "text-xs text-red-500",
                        {
                            t!(
                                "scan-incompatible", app : CONTRACT.to_string(), server : info.contract
                                .to_string()
                            )
                        }
                    }
                }
                button {
                    class: "py-1.5 px-3 rounded-lg text-xs font-semibold text-white \
                            bg-blue-600 hover:bg-blue-700 transition-colors disabled:opacity-40",
                    disabled: !ok,
                    onclick: move |_| {
                        let target = url_for_click.clone();
                        spawn(async move {
                            let _ = apply_server(&target).await;
                        });
                    },
                    {t!("scan-connect")}
                }
            }
        }
    }
}

/// Sonde la plage et retourne les serveurs PNEX détectés, triés par URL.
/// Le serveur courant (si configuré) est toujours sondé, même hors plage.
/// La progression est publiée dans les signals `total` (candidats) et `done`
/// (sondes résolues) pour la barre de progression de `LanScanSection`.
async fn run_scan(
    prefix: &str,
    mut done: Signal<usize>,
    mut total: Signal<usize>,
) -> Result<Vec<ScanHit>, String> {
    let client = crate::api::tls::discovery_client();

    // One entry per host: its base URLs in preference order.
    let mut candidates: Vec<Vec<String>> = Vec::new();
    let base = current_base();
    if !base.is_empty() {
        candidates.push(vec![base]);
    }
    candidates.extend(lan_candidates(prefix));
    candidates.sort();
    candidates.dedup();
    total.with_mut(|v| *v = candidates.len());

    let mut found: Vec<ScanHit> = futures::stream::iter(candidates)
        .map(|urls| {
            // reqwest::Client est un Arc interne : cloner par future est
            // négligeable et rend la closure FnMut.
            let client = client.clone();
            async move {
                // Both schemes probed concurrently: a dead host costs one
                // timeout, not two.
                let results = futures::future::join_all(
                    urls.iter()
                        .map(|url| meta::probe(&client, SCAN_TIMEOUT, url)),
                )
                .await;
                let hit = urls
                    .into_iter()
                    .zip(results)
                    .find_map(|(url, result)| result.ok().map(|info| ScanHit { url, info }));
                done.with_mut(|v| *v += 1);
                hit
            }
        })
        .buffer_unordered(SCAN_CONCURRENCY)
        .filter_map(|hit| async move { hit })
        .collect::<Vec<_>>()
        .await;
    found.sort_by(|a, b| a.url.cmp(&b.url));
    found.dedup_by(|a, b| a.url == b.url);
    let urls: Vec<String> = found.iter().map(|hit| hit.url.clone()).collect();
    found.retain(|hit| !is_shadowed_by_https(&hit.url, &urls));
    Ok(found)
}

/// Préfixe « a.b.c. » prérempli : URL serveur courante si c'est une IP
/// privée, sinon détection JNI de l'interface active (Android), sinon repli.
fn default_prefix() -> String {
    prefix_from_base(&current_base())
        .or_else(detect_prefix)
        .unwrap_or_else(|| DEFAULT_PREFIX.to_string())
}

/// `http://192.168.1.16:5150` → `Some("192.168.1.")` — IP privée
/// uniquement (un nom d'hôte ou une IP publique ne dérivent pas de plage).
fn prefix_from_base(base: &str) -> Option<String> {
    let host = reqwest::Url::parse(base).ok()?.host_str()?.to_string();
    let ip: Ipv4Addr = host.parse().ok()?;
    if !ip.is_private() {
        return None;
    }
    let octets = ip.octets();
    Some(format!("{}.{}.{}.", octets[0], octets[1], octets[2]))
}

/// The 254 hosts `{prefix}{1..254}`, each as `[https://{ip},
/// http://{ip}:{SCAN_PORT}]` (preference order). The prefix must be exactly
/// "a.b.c." AND private (never probe a public range by mistake); otherwise
/// the list is empty.
fn lan_candidates(prefix: &str) -> Vec<Vec<String>> {
    let first = format!("{prefix}1");
    match first.parse::<Ipv4Addr>() {
        Ok(ip) if ip.is_private() => (1..=254)
            .map(|i| {
                vec![
                    format!("https://{prefix}{i}"),
                    format!("http://{prefix}{i}:{SCAN_PORT}"),
                ]
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// A plain-HTTP hit is hidden when the same host also answered over HTTPS
/// (e.g. the saved `http://…:5150` base of a server now behind the edge).
fn is_shadowed_by_https(url: &str, all: &[String]) -> bool {
    let Ok(parsed) = reqwest::Url::parse(url) else {
        return false;
    };
    parsed.scheme() == "http"
        && all.iter().any(|other| {
            reqwest::Url::parse(other)
                .is_ok_and(|o| o.scheme() == "https" && o.host_str() == parsed.host_str())
        })
}

/// Interface réseau active de l'appareil (Android) — JNI brut, miroir de
/// `storage::imp::app_files_dir` : attach JVM daemon, `(**env).v1_1` partout,
/// `ExceptionCheck`/`Clear` après tout appel pouvant lever, jamais de panic.
/// Retourne le préfixe « a.b.c. » de la première IPv4 site-locale (Java :
/// 10/8, 172.16/12, 192.168/16 — tout ce qui n'est pas IPv4 répond `false`).
#[cfg(target_os = "android")]
fn detect_prefix() -> Option<String> {
    type EnvPtr = *mut ::jni::sys::JNIEnv;

    let ctx = ndk_context::try_android_context()?;
    let vm = ctx.vm() as *mut ::jni::sys::JavaVM;

    unsafe {
        // Threads du glue déjà attachés : GetEnv suffit ; sinon attach
        // daemon en filet de sécurité (tâches async de l'exécuteur).
        let mut env: EnvPtr = std::ptr::null_mut();
        let status = ((**vm).v1_2.GetEnv)(
            vm,
            (&mut env as *mut EnvPtr).cast::<*mut std::ffi::c_void>(),
            ::jni::sys::JNI_VERSION_1_6,
        );
        if status != ::jni::sys::JNI_OK || env.is_null() {
            let args = ::jni::sys::JavaVMAttachArgs {
                version: ::jni::sys::JNI_VERSION_1_6,
                name: c"pnex-lan-scan".as_ptr().cast_mut(),
                group: std::ptr::null_mut(),
            };
            let attached = ((**vm).v1_4.AttachCurrentThreadAsDaemon)(
                vm,
                (&mut env as *mut EnvPtr).cast::<*mut std::ffi::c_void>(),
                std::ptr::from_ref(&args)
                    .cast::<std::ffi::c_void>()
                    .cast_mut(),
            );
            if attached != ::jni::sys::JNI_OK || env.is_null() {
                eprintln!("pnex-lan-scan: attach JVM impossible — plage par défaut");
                return None;
            }
        }

        (|| {
            // java.net.NetworkInterface.getNetworkInterfaces() → Enumeration
            let ni_class = ((**env).v1_1.FindClass)(env, c"java/net/NetworkInterface".as_ptr());
            if ni_class.is_null() {
                return None;
            }
            let mid_get = ((**env).v1_1.GetStaticMethodID)(
                env,
                ni_class,
                c"getNetworkInterfaces".as_ptr(),
                c"()Ljava/util/Enumeration;".as_ptr(),
            );
            if mid_get.is_null() {
                return None;
            }
            let interfaces =
                ((**env).v1_1.CallStaticObjectMethodA)(env, ni_class, mid_get, std::ptr::null())
                    as ::jni::sys::jobject;
            if interfaces.is_null() || ((**env).v1_2.ExceptionCheck)(env) {
                ((**env).v1_1.ExceptionClear)(env);
                return None;
            }

            // hasMoreElements / nextElement — partagés par les deux énumérations.
            let en_class = ((**env).v1_1.FindClass)(env, c"java/util/Enumeration".as_ptr());
            if en_class.is_null() {
                return None;
            }
            let mid_more = ((**env).v1_1.GetMethodID)(
                env,
                en_class,
                c"hasMoreElements".as_ptr(),
                c"()Z".as_ptr(),
            );
            let mid_next = ((**env).v1_1.GetMethodID)(
                env,
                en_class,
                c"nextElement".as_ptr(),
                c"()Ljava/lang/Object;".as_ptr(),
            );
            if mid_more.is_null() || mid_next.is_null() {
                return None;
            }

            while ((**env).v1_1.CallBooleanMethodA)(env, interfaces, mid_more, std::ptr::null()) {
                if ((**env).v1_2.ExceptionCheck)(env) {
                    ((**env).v1_1.ExceptionClear)(env);
                    break;
                }
                let iface =
                    ((**env).v1_1.CallObjectMethodA)(env, interfaces, mid_next, std::ptr::null())
                        as ::jni::sys::jobject;
                if iface.is_null() {
                    break;
                }

                // iface.getInetAddresses() → Enumeration<InetAddress>
                let iface_class = ((**env).v1_1.GetObjectClass)(env, iface);
                let mid_addrs = ((**env).v1_1.GetMethodID)(
                    env,
                    iface_class,
                    c"getInetAddresses".as_ptr(),
                    c"()Ljava/util/Enumeration;".as_ptr(),
                );
                if mid_addrs.is_null() {
                    continue;
                }
                let addrs =
                    ((**env).v1_1.CallObjectMethodA)(env, iface, mid_addrs, std::ptr::null())
                        as ::jni::sys::jobject;
                if addrs.is_null() || ((**env).v1_2.ExceptionCheck)(env) {
                    ((**env).v1_1.ExceptionClear)(env);
                    continue;
                }

                while ((**env).v1_1.CallBooleanMethodA)(env, addrs, mid_more, std::ptr::null()) {
                    if ((**env).v1_2.ExceptionCheck)(env) {
                        ((**env).v1_1.ExceptionClear)(env);
                        break;
                    }
                    let addr =
                        ((**env).v1_1.CallObjectMethodA)(env, addrs, mid_next, std::ptr::null())
                            as ::jni::sys::jobject;
                    if addr.is_null() {
                        break;
                    }

                    let addr_class = ((**env).v1_1.GetObjectClass)(env, addr);
                    // isSiteLocalAddress() : false pour tout non-IPv4 — pas
                    // besoin de tester le type concret (Inet4Address).
                    let mid_site = ((**env).v1_1.GetMethodID)(
                        env,
                        addr_class,
                        c"isSiteLocalAddress".as_ptr(),
                        c"()Z".as_ptr(),
                    );
                    let mid_host = ((**env).v1_1.GetMethodID)(
                        env,
                        addr_class,
                        c"getHostAddress".as_ptr(),
                        c"()Ljava/lang/String;".as_ptr(),
                    );
                    if mid_site.is_null() || mid_host.is_null() {
                        continue;
                    }
                    if !((**env).v1_1.CallBooleanMethodA)(env, addr, mid_site, std::ptr::null()) {
                        continue;
                    }
                    if ((**env).v1_2.ExceptionCheck)(env) {
                        ((**env).v1_1.ExceptionClear)(env);
                        continue;
                    }

                    let jstr =
                        ((**env).v1_1.CallObjectMethodA)(env, addr, mid_host, std::ptr::null())
                            as ::jni::sys::jstring;
                    if jstr.is_null() || ((**env).v1_2.ExceptionCheck)(env) {
                        ((**env).v1_1.ExceptionClear)(env);
                        continue;
                    }
                    let chars = ((**env).v1_1.GetStringUTFChars)(env, jstr, std::ptr::null_mut());
                    if chars.is_null() {
                        continue;
                    }
                    let host = std::ffi::CStr::from_ptr(chars)
                        .to_string_lossy()
                        .into_owned();
                    ((**env).v1_1.ReleaseStringUTFChars)(env, jstr, chars);

                    if let Ok(ip) = host.parse::<Ipv4Addr>() {
                        if ip.is_private() {
                            let o = ip.octets();
                            return Some(format!("{}.{}.{}.", o[0], o[1], o[2]));
                        }
                    }
                }
            }
            None
        })()
    }
}

/// Hors Android : pas de détection d'interface — repli sur le préfixe
/// éditable (desktop non activé, web non concerné).
#[cfg(not(target_os = "android"))]
fn detect_prefix() -> Option<String> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candidats_plage_privee_completes() {
        let hosts = lan_candidates("192.168.1.");
        assert_eq!(hosts.len(), 254);
        assert_eq!(hosts[0], ["https://192.168.1.1", "http://192.168.1.1:5150"]);
        assert_eq!(
            hosts[253],
            ["https://192.168.1.254", "http://192.168.1.254:5150"]
        );
    }

    #[test]
    fn http_hit_hidden_when_same_host_answers_https() {
        let all = vec![
            "http://192.168.1.185:5150".to_string(),
            "https://192.168.1.185".to_string(),
            "http://192.168.1.20:5150".to_string(),
        ];
        assert!(is_shadowed_by_https(&all[0], &all));
        assert!(!is_shadowed_by_https(&all[1], &all));
        assert!(!is_shadowed_by_https(&all[2], &all));
    }

    #[test]
    fn candidats_plage_non_privee_refusee() {
        // Jamais sonder une plage publique par inadvertance.
        assert!(lan_candidates("8.8.8.").is_empty());
        assert!(lan_candidates("172.32.").is_empty());
    }

    #[test]
    fn candidats_prefixe_invalide_refuse() {
        assert!(lan_candidates("192.168.").is_empty()); // 3 octets
        assert!(lan_candidates("").is_empty());
        assert!(lan_candidates("192.168.1..").is_empty());
        assert!(lan_candidates("pnex.local.").is_empty());
    }

    #[test]
    fn prefixe_depuis_base_privee() {
        assert_eq!(
            prefix_from_base("http://192.168.1.16:5150").as_deref(),
            Some("192.168.1.")
        );
        assert_eq!(
            prefix_from_base("http://10.0.0.2:5150/").as_deref(),
            Some("10.0.0.")
        );
    }

    #[test]
    fn prefixe_depuis_base_non_ip_ou_publique_vide() {
        assert_eq!(prefix_from_base("http://pnex.local:5150"), None);
        assert_eq!(prefix_from_base("http://8.8.8.8:5150"), None);
        assert_eq!(prefix_from_base("pas une URL"), None);
    }
}
