//! Stockage clé/valeur du front, abstrait de la plateforme.
//!
//! - **web (wasm32)** : `localStorage` (persistance) et `sessionStorage`
//!   (verifier PKCE — survit à la redirection vers Keycloak mais pas à la
//!   fermeture de l'onglet) via web-sys. Les navigateurs hostiles au storage
//!   (mode privé Safari) dégradent silencieusement en no-op.
//! - **natif (future cible desktop/mobile)** : implémentation mémoire. Une
//!   persistance fichier viendra avec la phase desktop — l'API ne bougera pas.

// Socle posé avant ses consommateurs (session, PKCE, sélecteur d'org) : les
// clés et le storage session deviennent utilisés aux commits suivants.
#![allow(dead_code)]

/// Clés utilisées par l'app (préfixe `pnex.`).
pub const KEY_ACCESS_TOKEN: &str = "pnex.access_token";
pub const KEY_REFRESH_TOKEN: &str = "pnex.refresh_token";
pub const KEY_ORG: &str = "pnex.org";
pub const KEY_LOCALE: &str = "pnex.locale";
/// URL du serveur auto-hébergé — cible desktop/mobile uniquement (le web est
/// same-origin, la clé n'est jamais écrite).
pub const KEY_API_BASE: &str = "pnex.api_base";
/// Verifier PKCE — stockage *session* (consommé au callback, jamais persisté).
pub const KEY_PKCE_VERIFIER: &str = "pnex.pkce_verifier";

pub trait KeyValueStorage {
    fn get(&self, key: &str) -> Option<String>;
    fn set(&self, key: &str, value: &str);
    fn remove(&self, key: &str);
    /// Supprime plusieurs clés d'un coup (logout).
    fn purge(&self, keys: &[&str]) {
        for key in keys {
            self.remove(key);
        }
    }
}

/// Stockage persistant (tokens, org, locale).
pub fn local() -> impl KeyValueStorage {
    LocalStorage
}

/// Stockage volatil onglet (verifier PKCE).
pub fn session() -> impl KeyValueStorage {
    SessionStorage
}

pub struct LocalStorage;
pub struct SessionStorage;

#[cfg(target_arch = "wasm32")]
mod imp {
    use super::{KeyValueStorage, LocalStorage, SessionStorage};

    fn web_storage(persistent: bool) -> Option<web_sys::Storage> {
        web_sys::window().and_then(|w| {
            if persistent {
                w.local_storage().ok().flatten()
            } else {
                w.session_storage().ok().flatten()
            }
        })
    }

    macro_rules! web_impl {
        ($ty:ty, $persistent:expr) => {
            impl KeyValueStorage for $ty {
                fn get(&self, key: &str) -> Option<String> {
                    web_storage($persistent).and_then(|s| s.get_item(key).ok().flatten())
                }
                fn set(&self, key: &str, value: &str) {
                    if let Some(s) = web_storage($persistent) {
                        let _ = s.set_item(key, value);
                    }
                }
                fn remove(&self, key: &str) {
                    if let Some(s) = web_storage($persistent) {
                        let _ = s.remove_item(key);
                    }
                }
            }
        };
    }

    web_impl!(LocalStorage, true);
    web_impl!(SessionStorage, false);
}

#[cfg(not(target_arch = "wasm32"))]
mod imp {
    use super::{KeyValueStorage, LocalStorage, SessionStorage};
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};

    fn memory(persistent: bool) -> &'static Mutex<HashMap<String, String>> {
        static LOCAL: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
        static SESSION: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
        if persistent {
            LOCAL.get_or_init(Default::default)
        } else {
            SESSION.get_or_init(Default::default)
        }
    }

    macro_rules! mem_impl {
        ($ty:ty, $persistent:expr) => {
            impl KeyValueStorage for $ty {
                fn get(&self, key: &str) -> Option<String> {
                    memory($persistent).lock().ok()?.get(key).cloned()
                }
                fn set(&self, key: &str, value: &str) {
                    if let Ok(mut map) = memory($persistent).lock() {
                        map.insert(key.to_string(), value.to_string());
                    }
                }
                fn remove(&self, key: &str) {
                    if let Ok(mut map) = memory($persistent).lock() {
                        map.remove(key);
                    }
                }
            }
        };
    }

    mem_impl!(LocalStorage, true);
    mem_impl!(SessionStorage, false);
}
