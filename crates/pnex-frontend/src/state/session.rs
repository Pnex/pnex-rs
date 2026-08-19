//! Session utilisateur — signal global + restauration au boot.
//!
//! `SessionState` : `Booting` (restauration en cours) → `LoggedOut` |
//! `Authenticated`. Le shell rend la page de login en place de l'Outlet tant
//! que la session n'est pas établie (parité `AuthWrapper` React — pas de
//! route `/login`).

use dioxus::prelude::*;
use pnex_core::UserInfo;

use crate::api;
use crate::storage::{self, KeyValueStorage, KEY_LOCALE, KEY_REFRESH_TOKEN};

#[derive(Clone, Debug)]
pub enum SessionState {
    Booting,
    LoggedOut,
    /// Boxé : UserInfo est volumineux (clippy large_enum_variant).
    Authenticated {
        user: Box<UserInfo>,
    },
}

pub static SESSION: GlobalSignal<SessionState> = GlobalSignal::new(|| SessionState::Booting);

/// Restaure la session au démarrage de l'app : sans refresh token →
/// déconnecté ; sinon `user-info` (le 401 éventuel passe par le refresh
/// single-flight du client) → authentifié + org + langue du profil.
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

/// Session établie : enregistre l'utilisateur, restaure l'org courante et
/// applique la langue du profil (si pas de choix local déjà exprimé).
pub fn login(user: UserInfo) {
    crate::state::org::restore(&user.orgs);
    apply_profile_language(&user);
    SESSION.with_mut(|s| {
        *s = SessionState::Authenticated {
            user: Box::new(user),
        }
    });
}

/// Déconnexion : purge locale (tokens + org, la locale est conservée) puis
/// end-session Keycloak en pleine page — sinon le cookie SSO survit et le
/// login suivant ré-authentifie sans formulaire. En expiration de session
/// (refresh échoué), `expire()` fait la purge seule.
pub fn logout() {
    api::auth::clear_tokens();
    crate::state::org::clear();
    SESSION.with_mut(|s| *s = SessionState::LoggedOut);
    api::auth::end_session();
}

/// Session expirée (refresh échoué) — purge + notification.
pub fn expire() {
    logout();
    crate::state::toasts::error("toast-session-expired");
}

/// Utilisateur courant, si connecté.
pub fn user() -> Option<UserInfo> {
    match &*SESSION.read() {
        SessionState::Authenticated { user } => Some((**user).clone()),
        _ => None,
    }
}

/// Langue du profil appliquée seulement si l'utilisateur n'a pas déjà choisi
/// localement (`pnex.locale`) : profile.language ("en"/"fr") → locale UI.
fn apply_profile_language(user: &UserInfo) {
    if storage::local().get(KEY_LOCALE).is_some() {
        return;
    }
    if let Some(profile) = &user.profile {
        crate::i18n::set_locale(&profile.language);
    }
}
