//! Routage de l'app — routes **statiques** uniquement.
//!
//! Le détail d'une organisation est piloté par le signal global `ORG` (pas
//! par un segment dynamique) : les props de route ne sont pas des signaux et
//! ne redémarrent pas `use_resource` — piège documenté (dioxus #2784).
//!
//! Sur web, `dioxus-web` fournit le `WebHistory` par défaut au launch : le
//! `Router` nu bénéficie des deep links et du back/forward navigateur sans
//! configuration.

use crate::pages::edge_refs::EdgeRefs;
use crate::pages::{
    self, AdminStatus, Annotations, AuthCallback, Cameras, Catalog, Controls, Dashboard,
    Dashboards, Devices, Events, Firmware, Flows, FluidMixtures, Functions, Map, Media, Models,
    NotFound, Notifications, Orgs, OrgsCurrent, Profile, Secrets, ShareTour, Showcase, Sites,
    Studio, System, Visualisation,
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

    // Tour PUBLIC (lien de partage) — hors shell : la Shell porte la garde
    // de session, ici la consultation est anonyme (token = seule clé).
    #[route("/share/:token")]
    ShareTour { token: String },

    #[layout(pages::shell::Shell)]
        #[route("/")]
        Dashboard {},

        #[route("/visualisation")]
        Visualisation {},

        #[route("/map")]
        Map {},

        #[route("/sites")]
        Sites {},

        #[route("/dashboards?:id&:mode")]
        Dashboards { id: String, mode: String },

        #[route("/media")]
        Media {},

        // Camera live view, settings and recordings (camera-video.md D80).
        #[route("/cameras")]
        Cameras {},

        // Vision model registry (camera-video.md D81).
        #[route("/models")]
        Models {},

        // JSON events stored in OpenObserve (camera-video.md D84).
        #[route("/events")]
        Events {},

        #[route("/studio")]
        Studio {},

        #[route("/annotations")]
        Annotations {},

        #[route("/controls")]
        Controls {},

        #[route("/devices")]
        Devices {},

        #[route("/mixtures")]
        FluidMixtures {},

        // `?id=` keeps the open flow across a reload (O31).
        #[route("/flows?:id")]
        Flows { id: String },

        #[route("/functions")]
        Functions {},

        // Custom firmware IDE (custom-firmware.md D87–D94).
        #[route("/firmware")]
        Firmware {},

        #[route("/notifications")]
        Notifications {},

        #[route("/catalog")]
        Catalog {},

        // Référentiels Edge (WiFi / serveurs PNeX) consommés par le wizard
        // device et la file de build firmware.
        #[route("/edges/refs")]
        EdgeRefs {},

        #[route("/orgs")]
        Orgs {},

        // Detail of the current org opened directly (deep-link target).
        #[route("/orgs/current")]
        OrgsCurrent {},

        // Org secrets vault (D110–D117): names and usages, never a value.
        #[route("/secrets")]
        Secrets {},

        #[route("/profile")]
        Profile {},

        // System (D72): O2 retention + cleanup of the current org.
        #[route("/system")]
        System {},

        // Platform status (D72) — platform admins only (server enforces).
        #[route("/admin/status")]
        AdminStatus {},

        // Vitrine du socle CRUD (référence visuelle dev, hors nav — accès
        // URL direct ; garde de session portée par la Shell).
        #[route("/_showcase")]
        Showcase {},
    #[end_layout]

    #[route("/:..route")]
    NotFound { route: Vec<String> },
}
