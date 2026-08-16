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
