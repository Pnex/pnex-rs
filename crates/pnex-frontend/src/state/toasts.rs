//! Toasts — notifications flottantes (porté du `Toast.tsx`/`ToastContainer.tsx`
//! React) : auto-dismiss 5 s, types success/error/info.
//!
//! Deux sortes de messages : `Text` (message serveur relayé tel quel —
//! convention projet) et `Key` (libellé local, traduit à l'affichage).

// API complète du socle — success/info sont consommés par les pages suivantes.
#![allow(dead_code)]

use std::time::Duration;

use dioxus::prelude::*;

#[derive(Clone, Debug, PartialEq)]
pub enum ToastKind {
    Success,
    Error,
    Info,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Toast {
    pub id: u64,
    pub kind: ToastKind,
    pub message: ToastMessage,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ToastMessage {
    /// Texte relayé tel quel (erreur serveur).
    Text(String),
    /// Clé i18n traduite à l'affichage.
    Key(&'static str),
}

pub static TOASTS: GlobalSignal<Vec<Toast>> = GlobalSignal::new(Vec::new);

fn next_id() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// Attente portable : `futures_timer::Delay` panique sur wasm32
/// (`Instant::now()` → « time not implemented on this platform ») —
/// `gloo-timers` (setTimeout) côté navigateur.
async fn sleep(duration: Duration) {
    #[cfg(target_arch = "wasm32")]
    gloo_timers::future::TimeoutFuture::new(
        duration.as_millis().min(u32::MAX as u128) as u32,
    )
    .await;
    #[cfg(not(target_arch = "wasm32"))]
    futures_timer::Delay::new(duration).await;
}

/// Affiche un toast (auto-dismiss 5 s).
pub fn show(kind: ToastKind, message: ToastMessage) {
    let toast = Toast {
        id: next_id(),
        kind,
        message,
    };
    let id = toast.id;
    TOASTS.with_mut(|toasts| toasts.push(toast));
    spawn(async move {
        sleep(Duration::from_secs(5)).await;
        dismiss(id);
    });
}

pub fn success(message: impl Into<ToastMessage>) {
    show(ToastKind::Success, message.into());
}

pub fn error(message: impl Into<ToastMessage>) {
    show(ToastKind::Error, message.into());
}

pub fn info(message: impl Into<ToastMessage>) {
    show(ToastKind::Info, message.into());
}

impl From<String> for ToastMessage {
    fn from(value: String) -> Self {
        ToastMessage::Text(value)
    }
}

impl From<&'static str> for ToastMessage {
    fn from(key: &'static str) -> Self {
        ToastMessage::Key(key)
    }
}

/// Ferme un toast (bouton ×).
pub fn dismiss(id: u64) {
    TOASTS.with_mut(|toasts| toasts.retain(|toast| toast.id != id));
}
