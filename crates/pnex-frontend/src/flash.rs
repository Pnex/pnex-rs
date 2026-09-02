//! Pont vers le glue JS esptool-js (`js/flasher.js`, bundlé esbuild en
//! `assets/flasher.js`) — flash firmware via Web Serial (Chromium uniquement).
//!
//! Le module JS expose deux globales :
//!   - `window.pnexFlashSupported()` → Web Serial dispo ?
//!   - `window.pnexFlash(entries, onEvent)` → promise du flow complet
//!     (requestPort → sync → writeFlash @0x0 → hard reset) ;
//!     `onEvent` reçoit des chaînes JSON décodées ici en `FlashEvent`
//!     (serde_json — pas de dépendance serde-wasm-bindgen).
//!
//! Cible non-wasm32 : stubs (le flash navigateur n'existe que sur le web ;
//! garde `task check` natif vert).

use serde::Deserialize;

#[cfg(target_arch = "wasm32")]
use wasm_bindgen::{closure::Closure, JsCast, JsValue};

/// Événement du flow de flash, sérialisé en JSON par `js/flasher.js`
/// (`{"type":"stage","stage":"write"}`, `{"type":"progress","percent":42}`…).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FlashEvent {
    /// Changement d'étape : `connect` | `write` | `reset`.
    Stage {
        stage: String,
    },
    /// Chip détecté au sync (ex. « ESP32-D0WD-V3 ») — affiché tel quel.
    Chip {
        chip: String,
    },
    /// Progression d'écriture, 0-100.
    Progress {
        percent: u8,
    },
    Done,
    Error {
        message: String,
    },
}

/// Web Serial disponible ? Chrome/Edge/Opera uniquement — false sur
/// Firefox/Safari et sur toute cible non-web.
#[cfg(target_arch = "wasm32")]
pub fn supported() -> bool {
    global_function("pnexFlashSupported")
        .and_then(|f| f.call0(&JsValue::NULL).ok())
        .and_then(|value| value.as_bool())
        .unwrap_or(false)
}

/// Flash les `entries` [(adresse, octets)] en un seul writeFlash (firmware
/// de ports natif. Doit être appelé depuis un handler de clic :
/// `requestPort()` exige un geste utilisateur.
#[cfg(target_arch = "wasm32")]
pub async fn flash<F>(entries: Vec<(u32, Vec<u8>)>, mut on_event: F) -> Result<(), String>
where
    F: FnMut(FlashEvent),
{
    let flash_fn = global_function("pnexFlash")
        .ok_or_else(|| "flasher.js non chargé (window.pnexFlash absent)".to_string())?;

    // [{ data: Uint8Array, address: Number }, ...] — un seul writeFlash
    // esptool-js, toutes entrées confondues (aujourd'hui : l'image unique
    // @0x0 ; le multi-entrées reste disponible si besoin).
    let entries_array = js_sys::Array::new();
    for (address, bytes) in entries {
        let array = js_sys::Uint8Array::new_with_length(bytes.len() as u32);
        array.copy_from(&bytes);
        let entry = js_sys::Object::new();
        js_sys::Reflect::set(&entry, &"data".into(), &array).ok();
        js_sys::Reflect::set(&entry, &"address".into(), &js_sys::Number::from(address)).ok();
        entries_array.push(&entry);
    }

    // Callback d'événements : la Closure reste vivante jusqu'au retour de
    // l'await (locale possédée), le JS n'en garde qu'un emprunt.
    let closure = Closure::wrap(Box::new(move |json: String| {
        if let Ok(event) = serde_json::from_str::<FlashEvent>(&json) {
            on_event(event);
        }
    }) as Box<dyn FnMut(String)>);

    let promise = flash_fn
        .call2(&JsValue::NULL, &entries_array, closure.as_js_value())
        .and_then(|value| {
            value
                .dyn_into::<js_sys::Promise>()
                .map_err(|_| JsValue::from_str("pnexFlash n'a pas renvoyé de promise"))
        })
        .map_err(|err| js_error_message(&err))?;

    wasm_bindgen_futures::JsFuture::from(promise)
        .await
        .map_err(|err| js_error_message(&err))?;
    Ok(())
}

/// Résout une fonction globale exposée par le bundle flasher.js.
#[cfg(target_arch = "wasm32")]
fn global_function(name: &str) -> Option<js_sys::Function> {
    js_sys::Reflect::get(&js_sys::global(), &JsValue::from_str(name))
        .ok()
        .and_then(|value| value.dyn_into::<js_sys::Function>().ok())
}

/// Message lisible d'une `JsValue` d'erreur (rejet de promise ou exception).
#[cfg(target_arch = "wasm32")]
fn js_error_message(value: &JsValue) -> String {
    if let Some(error) = value.dyn_ref::<js_sys::Error>() {
        let message = js_sys::Reflect::get(error, &JsValue::from_str("message"))
            .ok()
            .and_then(|m| m.as_string());
        if let Some(message) = message.filter(|m| !m.is_empty()) {
            return message;
        }
    }
    value
        .as_string()
        .unwrap_or_else(|| "erreur flasher inconnue".to_string())
}

#[cfg(not(target_arch = "wasm32"))]
pub fn supported() -> bool {
    false
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn flash<F>(_entries: Vec<(u32, Vec<u8>)>, _on_event: F) -> Result<(), String>
where
    F: FnMut(FlashEvent),
{
    Err("flash navigateur indisponible hors cible web".to_string())
}

#[cfg(test)]
mod tests {
    use super::FlashEvent;

    /// Contrat JSON du glue `js/flasher.js` — toute divergence cassera ces
    /// décodages (le JS n'est pas couvert par les tests Rust).
    #[test]
    fn decode_evenements_flasher_js() {
        assert_eq!(
            serde_json::from_str::<FlashEvent>(r#"{"type":"stage","stage":"write"}"#).unwrap(),
            FlashEvent::Stage {
                stage: "write".into()
            }
        );
        assert_eq!(
            serde_json::from_str::<FlashEvent>(r#"{"type":"chip","chip":"ESP32-D0WD-V3"}"#)
                .unwrap(),
            FlashEvent::Chip {
                chip: "ESP32-D0WD-V3".into()
            }
        );
        assert_eq!(
            serde_json::from_str::<FlashEvent>(r#"{"type":"progress","percent":42}"#).unwrap(),
            FlashEvent::Progress { percent: 42 }
        );
        assert_eq!(
            serde_json::from_str::<FlashEvent>(r#"{"type":"done"}"#).unwrap(),
            FlashEvent::Done
        );
        assert!(matches!(
            serde_json::from_str::<FlashEvent>(r#"{"type":"error","message":"No port selected"}"#)
                .unwrap(),
            FlashEvent::Error { .. }
        ));
    }
}
