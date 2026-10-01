//! Toasts — notifications flottantes (porté du `Toast.tsx`/`ToastContainer.tsx`
//! React) : auto-dismiss 5 s, types success/error/info.
//!
//! Three message kinds: `Api` (structured error — machine code resolved via
//! `api::error_i18n` at render, verbatim fallback), `Text` (raw text:
//! code-less errors, runtime text) and `Key` (local label, translated at
//! display).

// API complète du socle — success/info sont consommés par les pages suivantes.
#![allow(dead_code)]

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
    /// Structured API error: machine code resolved at render time
    /// (`api::error_i18n::localize`), verbatim fallback for unknown codes.
    Api(crate::api::error::ApiError),
    /// Raw text relayed as-is (code-less errors, runtime text).
    Text(String),
    /// i18n key translated at display time.
    Key(&'static str),
}

pub static TOASTS: GlobalSignal<Vec<Toast>> = GlobalSignal::new(Vec::new);

fn next_id() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// Auto-dismiss delay — the timer runs in the toast card component
/// (`components::toasts`), never in the caller's scope.
pub const AUTO_DISMISS_SECS: u64 = 5;

/// Affiche un toast (auto-dismiss 5 s).
pub fn show(kind: ToastKind, message: ToastMessage) {
    let toast = Toast {
        id: next_id(),
        kind,
        message,
    };
    TOASTS.with_mut(|toasts| toasts.push(toast));
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

impl From<crate::api::error::ApiError> for ToastMessage {
    fn from(err: crate::api::error::ApiError) -> Self {
        ToastMessage::Api(err)
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
