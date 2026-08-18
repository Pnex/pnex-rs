//! Petits utilitaires portables wasm32/natif.

use std::time::Duration;

/// Attente portable : `futures_timer::Delay` panique sur wasm32
/// (`Instant::now()` → « time not implemented on this platform ») —
/// `gloo-timers` (setTimeout) côté navigateur.
pub async fn sleep(duration: Duration) {
    #[cfg(target_arch = "wasm32")]
    gloo_timers::future::TimeoutFuture::new(
        duration.as_millis().min(u32::MAX as u128) as u32,
    )
    .await;
    #[cfg(not(target_arch = "wasm32"))]
    futures_timer::Delay::new(duration).await;
}

/// Déclenche le téléchargement navigateur d'octets (binaire firmware).
///
/// Data URI base64 plutôt que Blob/URL : zéro dépendance js-sys, et les
/// binaires concernés (1–4 Mo) passent sans problème. No-op en natif (la
/// cible desktop chosera sa propre boîte de dialogue).
pub fn save_blob(filename: &str, bytes: &[u8]) {
    #[cfg(target_arch = "wasm32")]
    {
        use base64::Engine as _;
        use web_sys::wasm_bindgen::JsCast;
        let data = format!(
            "data:application/octet-stream;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(bytes)
        );
        let Some(window) = web_sys::window() else { return };
        let Some(document) = window.document() else { return };
        let Ok(element) = document.create_element("a") else { return };
        let Ok(anchor) = element.dyn_into::<web_sys::HtmlAnchorElement>() else {
            return;
        };
        anchor.set_href(&data);
        anchor.set_download(filename);
        anchor.click();
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = (filename, bytes);
    }
}

/// Hôte du serveur prérempli (formulaires de build) : l'origine courante
/// du front — le backend sert le front en same-origin, le device doit
/// pouvoir joindre cet hôte.
pub fn default_host() -> String {
    #[cfg(target_arch = "wasm32")]
    {
        web_sys::window()
            .and_then(|w| w.location().host().ok())
            .filter(|h| !h.is_empty())
            .unwrap_or_else(|| "localhost:5150".to_string())
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        "localhost:5150".to_string()
    }
}

/// Copie dans le presse-papier navigateur : textarea temporaire hors écran
/// puis `exec_command("copy")` (synchrone, sans API Clipboard ni promise —
/// la valeur reste affichée et sélectionnable à côté). No-op en natif.
pub fn copy_text(text: &str) {
    #[cfg(target_arch = "wasm32")]
    {
        use web_sys::wasm_bindgen::JsCast;
        let Some(window) = web_sys::window() else { return };
        let Some(document) = window.document() else { return };
        let Ok(element) = document.create_element("textarea") else { return };
        let Ok(area) = element.dyn_into::<web_sys::HtmlTextAreaElement>() else {
            return;
        };
        area.set_value(text);
        let _ = area.set_attribute("style", "position:fixed;left:-9999px");
        if let Some(body) = document.body() {
            let _ = body.append_child(&area);
            area.select();
            // exec_command vit sur HtmlDocument (cast d'une seconde vue).
            if let Ok(html_doc) = document.clone().dyn_into::<web_sys::HtmlDocument>() {
                let _ = html_doc.exec_command("copy");
            }
            let _ = body.remove_child(&area);
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = text;
    }
}

/// Page courante servie en https → les WebSocket du device parlent wss.
/// En natif (desktop) : faux.
pub fn page_secure() -> bool {
    #[cfg(target_arch = "wasm32")]
    {
        web_sys::window()
            .and_then(|w| w.location().protocol().ok())
            .is_some_and(|p| p == "https:")
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        false
    }
}

/// Valeur par défaut du toggle « WebSocket SSL » des formulaires de build :
/// suit le protocole de la page (déploiement local http → ws, industriel
/// https → wss). L'utilisateur peut inverser.
pub fn default_ws_ssl() -> bool {
    page_secure()
}

/// URL WebSocket d'ingestion pour les devices custom (snippet Python du
/// wizard) : schéma ws/wss selon le protocole de la page, hôte courant.
pub fn ws_ingest_url() -> String {
    let scheme = if page_secure() { "wss" } else { "ws" };
    format!("{scheme}://{}/ws/sensor/ingest", default_host())
}
