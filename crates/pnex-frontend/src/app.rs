//! Routage de l'app — routes **statiques** uniquement.
//!
//! Le détail d'une organisation est piloté par le signal global `ORG` (pas
//! par un segment dynamique) : les props de route ne sont pas des signaux et
//! ne redémarrent pas `use_resource` — piège documenté (dioxus #2784).
//!
//! Sur web, `dioxus-web` fournit le `WebHistory` par défaut au launch : le
//! `Router` nu bénéficie des deep links et du back/forward navigateur sans
//! configuration.

use crate::pages::{
    self, AuthCallback, Catalog, Dashboard, Devices, NotFound, Orgs, Profile, Visualisation,
};
use dioxus::prelude::*;

#[derive(Clone, Debug, PartialEq, Routable)]
#[rustfmt::skip]
pub enum Route {
    // Callback OAuth — HORS shell (redirection plein page depuis l'IdP Rauthy).
    // Les segments query sont infaillibles : absents → chaîne vide, tester
    // avec is_empty().
    #[route("/auth/callback?:code&:error&:error_description")]
    AuthCallback { code: String, error: String, error_description: String },

    #[layout(pages::shell::Shell)]
        #[route("/")]
        Dashboard {},

        #[route("/visualisation")]
        Visualisation {},

        #[route("/devices")]
        Devices {},

        #[route("/catalog")]
        Catalog {},

        #[route("/orgs")]
        Orgs {},

        #[route("/profile")]
        Profile {},
    #[end_layout]

    #[route("/:..route")]
    NotFound { route: Vec<String> },
}
