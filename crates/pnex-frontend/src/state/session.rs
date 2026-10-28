//! User session — global signal + restore at boot.
//!
//! `SessionState`: `Booting` (restore in progress) → `LoggedOut` |
//! `Authenticated`. The shell renders the login page in place of the Outlet
//! until the session is established (no `/login` route).

use dioxus::prelude::*;
use pnex_core::UserInfo;

use crate::api;
use crate::storage::{self, KeyValueStorage, KEY_LOCALE, KEY_REFRESH_TOKEN};

#[derive(Clone, Debug)]
pub enum SessionState {
    Booting,
    LoggedOut,
    /// Boxed: UserInfo is large (clippy large_enum_variant).
    Authenticated {
        user: Box<UserInfo>,
    },
}

pub static SESSION: GlobalSignal<SessionState> = GlobalSignal::new(|| SessionState::Booting);

/// Restores the session at app start: no refresh token →
/// logged out; otherwise `user-info` (a possible 401 goes through the client's
/// single-flight refresh) → authenticated + org + profile language.
pub async fn boot() {
    if storage::local().get(KEY_REFRESH_TOKEN).is_none() {
        SESSION.with_mut(|s| *s = SessionState::LoggedOut);
        return;
    }
    match api::user::get_user_info().await {
        Ok(user) => login(user),
        Err(_) => logout(),
    }
}

/// Session established: stores the user, restores the current org and
/// applies the profile language (unless a local choice was already made).
pub fn login(user: UserInfo) {
    crate::state::org::restore(&user.orgs);
    apply_profile_language(&user);
    if let Some(profile) = &user.profile {
        crate::state::theme::apply(profile.theme.as_str());
    }
    SESSION.with_mut(|s| {
        *s = SessionState::Authenticated {
            user: Box::new(user),
        }
    });
}

/// Logout: local purge (tokens + org, the locale is kept) then
/// Rauthy end-session in a side tab — otherwise the SSO cookie survives and the
/// next login re-authenticates without a form. On session expiry
/// (refresh failed), `expire()` does the purge alone.
pub fn logout() {
    // id_token read BEFORE the purge: without `id_token_hint`, the Rauthy
    // logout page shows its yes/no confirmation instead of destroying the SSO
    // directly (the auto-POST only happens when a hint is present).
    let id_token = api::auth::stored_id_token();
    api::auth::clear_tokens();
    // No media of this account left in the browser after logout.
    crate::media_cache::clear();
    crate::state::org::clear();
    SESSION.with_mut(|s| *s = SessionState::LoggedOut);
    // Without a hint Rauthy answers a raw 400 when its SSO cookie is gone:
    // stay on the app's login screen instead.
    if id_token.is_some() {
        api::auth::end_session(id_token);
    } else {
        api::auth::force_next_login();
    }
}

/// Session expired (refresh failed) — purge + notification.
pub fn expire() {
    logout();
    crate::state::toasts::error("toast-session-expired");
}

/// Current user, if logged in.
pub fn user() -> Option<UserInfo> {
    match &*SESSION.read() {
        SessionState::Authenticated { user } => Some((**user).clone()),
        _ => None,
    }
}

/// Profile language applied only if the user has not already chosen
/// locally (`pnex.locale`): profile.language ("en"/"fr") → UI locale.
fn apply_profile_language(user: &UserInfo) {
    if storage::local().get(KEY_LOCALE).is_some() {
        return;
    }
    if let Some(profile) = &user.profile {
        crate::i18n::set_locale(&profile.language);
    }
}
