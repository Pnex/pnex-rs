//! "Server root certificate" card (D70): fingerprint, download link and a QR
//! code pointing at the public `META_CA_PATH`, so phones can scan it and
//! install the local edge CA. Renders nothing when the server has no local
//! CA to hand out (404: no edge, or a public Let's Encrypt certificate).

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_api_contract::META_CA_PATH;

#[component]
pub fn ServerCaCard() -> Element {
    let url = format!("{}{META_CA_PATH}", crate::api::config::api_base());
    let fetch_url = url.clone();
    let ca = use_resource(move || {
        let fetch_url = fetch_url.clone();
        async move { fetch_ca(&fetch_url).await }
    });
    let Some(Some(fingerprint)) = ca.cloned() else {
        return rsx! {};
    };
    let qr_svg = qr_svg(&url);

    rsx! {
        div { class: "bg-white rounded-lg shadow-sm",
            div { class: "p-6 border-b border-gray-200",
                h3 { class: "text-lg font-semibold text-gray-900", {t!("about-ca-title")} }
            }
            div { class: "p-6 space-y-4 text-sm",
                p { class: "text-gray-600", {t!("about-ca-description")} }
                if let Some(svg) = qr_svg {
                    div {
                        class: "mx-auto w-40 h-40 bg-white p-2 rounded-lg border border-gray-200",
                        dangerous_inner_html: svg,
                    }
                }
                div { class: "space-y-1",
                    p { class: "text-xs font-medium text-gray-500", {t!("about-ca-fingerprint")} }
                    p { class: "text-xs font-mono text-gray-900 break-all bg-gray-50 rounded-lg p-2",
                        "{fingerprint}"
                    }
                }
                a {
                    class: "block w-full text-center py-2 px-4 rounded-lg text-sm font-medium \
                            text-blue-700 bg-blue-50 hover:bg-blue-100 transition-colors",
                    href: "{url}",
                    download: "pnex-ca.crt",
                    {t!("about-ca-download")}
                }
            }
        }
    }
}

/// Fingerprint of the served CA, `None` when absent or unreadable.
async fn fetch_ca(url: &str) -> Option<String> {
    let response = crate::api::tls::client(None).get(url).send().await.ok()?;
    if !response.status().is_success() {
        return None;
    }
    crate::api::tls::fingerprint(&response.text().await.ok()?)
}

/// SVG QR code of `url`, scaled by its container.
fn qr_svg(url: &str) -> Option<String> {
    let code = qrcode::QrCode::new(url.as_bytes()).ok()?;
    let svg = code
        .render::<qrcode::render::svg::Color>()
        .quiet_zone(false)
        .build();
    // Drop the fixed width/height so the SVG fills its box.
    Some(
        svg.replacen(" width=\"", " data-w=\"", 1)
            .replacen(" height=\"", " data-h=\"", 1)
            .replacen("<svg", "<svg style=\"width:100%;height:100%\"", 1),
    )
}
