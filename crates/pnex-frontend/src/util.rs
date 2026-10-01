//! Petits utilitaires portables wasm32/natif.

use std::time::Duration;

/// Date/heure locale « AAAA-MM-JJ HH:MM » — noms par défaut des créations
/// (flows, dashboards). Via `Date` JS sur le web (chrono::Local panique sur
/// wasm32 sans la feature wasmbind), chrono en natif (tests).
pub fn now_label() -> String {
    #[cfg(target_arch = "wasm32")]
    {
        let now = js_sys::Date::new_0();
        format!(
            "{:04}-{:02}-{:02} {:02}:{:02}",
            now.get_full_year(),
            now.get_month() + 1,
            now.get_date(),
            now.get_hours(),
            now.get_minutes()
        )
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        chrono::Local::now().format("%Y-%m-%d %H:%M").to_string()
    }
}

/// Same as `now_label` with second precision — auto-saved template names on
/// test/preview (datetime in the name, collisions unlikely within a minute).
pub fn now_label_secs() -> String {
    #[cfg(target_arch = "wasm32")]
    {
        let now = js_sys::Date::new_0();
        format!(
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
            now.get_full_year(),
            now.get_month() + 1,
            now.get_date(),
            now.get_hours(),
            now.get_minutes(),
            now.get_seconds()
        )
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string()
    }
}

/// Attente portable : `futures_timer::Delay` panique sur wasm32
/// (`Instant::now()` → « time not implemented on this platform ») —
/// `gloo-timers` (setTimeout) côté navigateur.
pub async fn sleep(duration: Duration) {
    #[cfg(target_arch = "wasm32")]
    gloo_timers::future::TimeoutFuture::new(duration.as_millis().min(u32::MAX as u128) as u32)
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
        let Some(window) = web_sys::window() else {
            return;
        };
        let Some(document) = window.document() else {
            return;
        };
        let Ok(element) = document.create_element("a") else {
            return;
        };
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

/// Un hôte joignable par le navigateur mais pas par un device : boucle
/// locale. Le wizard pré-remplit `localhost:5150` quand l'UI est ouverte
/// sur la machine du serveur — un ESP8266 dialant `localhost` se connecte
/// à LUI-MÊME (leçon du 2026-09-02, builds « échec de connexion »).
pub fn is_loopback_host(host: &str) -> bool {
    let host = host.trim();
    host.starts_with("localhost")
        || host.starts_with("127.")
        || host.starts_with("[::1]")
        || host == "::1"
}

/// Hôte prérempli pour les formulaires de build FIRMWARE : comme
/// [`default_host`] mais vide sur une origine en boucle locale — le champ
/// reste à remplir consciemment (IP LAN du serveur), l'UI affiche un
/// avertissement si l'utilisateur saisit quand même une adresse locale.
/// L'URL du snippet Python des customs garde [`default_host`] : un script
/// sur le PC, lui, JOINT localhost.
pub fn default_device_host() -> String {
    if is_loopback_host(&default_host()) {
        String::new()
    } else {
        default_host()
    }
}

/// Copie dans le presse-papier navigateur : textarea temporaire hors écran
/// puis `exec_command("copy")` (synchrone, sans API Clipboard ni promise —
/// la valeur reste affichée et sélectionnable à côté). No-op en natif.
pub fn copy_text(text: &str) {
    #[cfg(target_arch = "wasm32")]
    {
        use web_sys::wasm_bindgen::JsCast;
        let Some(window) = web_sys::window() else {
            return;
        };
        let Some(document) = window.document() else {
            return;
        };
        let Ok(element) = document.create_element("textarea") else {
            return;
        };
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

/// Déclenche le téléchargement des octets d'un média : wasm → ancre `<a
/// download>` sur URL `blob:` (gros fichiers — pas de base64, école
/// save_blob) ; Android → MediaStore `Download/Pnex` via JNI (école
/// capture.rs, cf. `download.rs`) ; autre natif : no-op. `true` si le
/// téléchargement a été déclenché/enregistré.
pub fn trigger_download(filename: &str, mime: &str, bytes: &[u8]) -> bool {
    #[cfg(target_arch = "wasm32")]
    {
        use web_sys::wasm_bindgen::JsCast as _;
        let _ = mime; // seul le chemin natif consomme le mime (MediaStore)
        let Some(url) = bytes_to_blob_url(bytes) else {
            return false;
        };
        let Some(window) = web_sys::window() else {
            return false;
        };
        let Some(document) = window.document() else {
            return false;
        };
        let Ok(element) = document.create_element("a") else {
            return false;
        };
        let Ok(anchor) = element.dyn_into::<web_sys::HtmlAnchorElement>() else {
            return false;
        };
        anchor.set_href(&url);
        anchor.set_download(filename);
        anchor.click();
        // Révocation différée : immédiate, elle peut annuler le download
        // démarré — 60 s puis fire-and-forget (closure fuie délibérément :
        // one-shot). L'URL blob vit sinon jusqu'au déchargement du document.
        let Some(window) = web_sys::window() else {
            return true;
        };
        let cb: wasm_bindgen::closure::Closure<dyn FnMut()> =
            wasm_bindgen::closure::Closure::new(move || {
                let _ = web_sys::Url::revoke_object_url(&url);
            });
        let _ = window.set_timeout_with_callback_and_timeout_and_arguments_0(
            cb.as_ref().unchecked_ref(),
            60_000,
        );
        std::mem::forget(cb); // one-shot : le callback doit survivre au fire
        true
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        // Android : MediaStore Download/Pnex (JNI) ; autre natif : le stub
        // de download.rs renvoie false (desktop sans dialog natif — no-op,
        // école save_blob).
        crate::download::save_to_downloads(filename, mime, bytes)
    }
}

/// URL `blob:` d'un endpoint d'octets authentifié — les `<img>` et les
/// viewers WebGL ne peuvent pas poser d'en-têtes, l'octet est donc fetché
/// par le client Rust (Bearer/X-Org-Id/refresh 401 gérés) puis converti en
/// Blob JS (`js_sys` pur : `Reflect` sur `URL.createObjectURL`, aucune
/// feature web-sys). `None` en cas d'échec (ou en natif : le blob vive dans
/// la webview, CORS interdirait le fetch JS cross-origin — limitation V1,
/// cf. docs/architecture/media.md).
pub async fn media_blob_url(path: &str, content_type: Option<&str>) -> Option<String> {
    let bytes = crate::api::client::request_bytes(reqwest::Method::GET, path)
        .await
        .ok()?;
    // Native : data URI base64 (école save_blob) — le blob JS n'est pas
    // jouable cross-origin depuis la webview ; les photos deviennent
    // visualisables sur Android.
    #[cfg(not(target_arch = "wasm32"))]
    {
        use base64::Engine as _;
        let mime = content_type.unwrap_or("application/octet-stream");
        Some(format!(
            "data:{mime};base64,{}",
            base64::engine::general_purpose::STANDARD.encode(&bytes)
        ))
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = content_type;
        bytes_to_blob_url(&bytes)
    }
}

/// Convertit des octets en URL blob: (wasm uniquement).
#[cfg(target_arch = "wasm32")]
fn bytes_to_blob_url(bytes: &[u8]) -> Option<String> {
    let array = js_sys::Uint8Array::from(bytes);
    let seq = js_sys::Array::of1(&array);
    let blob = web_sys::Blob::new_with_u8_array_sequence(&seq).ok()?;
    web_sys::Url::create_object_url_with_blob(&blob).ok()
}

/// Rect d'un élément DOM par id — (left, top, width, height). Wasm :
/// `get_bounding_client_rect` ; natif : None (les marqueurs plats de la
/// page annotations ne rendent qu'en web, école canvas_rect).
pub fn element_rect(element_id: &str) -> Option<(f64, f64, f64, f64)> {
    #[cfg(target_arch = "wasm32")]
    {
        let document = web_sys::window()?.document()?;
        let element = document.get_element_by_id(element_id)?;
        let rect = element.get_bounding_client_rect();
        Some((rect.left(), rect.top(), rect.width(), rect.height()))
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = element_id;
        None
    }
}

/// Image URL of in-memory bytes (camera frames, test images): blob URL on
/// web, `data:` URI on native (the webview cannot fetch cross-origin blobs).
/// Release it with [`release_image_url`].
pub fn image_url_from_bytes(bytes: &[u8], mime: &str) -> Option<String> {
    #[cfg(target_arch = "wasm32")]
    {
        let _ = mime;
        bytes_to_blob_url(bytes)
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        use base64::Engine as _;
        Some(format!(
            "data:{mime};base64,{}",
            base64::engine::general_purpose::STANDARD.encode(bytes)
        ))
    }
}

/// Releases a URL from [`image_url_from_bytes`] (blob URLs only).
pub fn release_image_url(url: &str) {
    #[cfg(target_arch = "wasm32")]
    {
        let _ = web_sys::Url::revoke_object_url(url);
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = url;
    }
}
