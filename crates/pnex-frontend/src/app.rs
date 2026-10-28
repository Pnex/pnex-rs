//! App routing — **static** routes only.
//!
//! An organisation's detail is driven by the global `ORG` signal (not
//! by a dynamic segment): route props are not signals and do not
//! restart `use_resource` — documented pitfall (dioxus #2784).
//!
//! On web, `dioxus-web` provides the default `WebHistory` at launch: the
//! bare `Router` gets deep links and browser back/forward without
//! configuration.

use crate::pages::access::Access;
use crate::pages::browser_agents::BrowserAgents;
use crate::pages::edge_refs::EdgeRefs;
use crate::pages::mailbox_messages::MailboxMessages;
use crate::pages::mailboxes::Mailboxes;
use crate::pages::proposals::Proposals;
use crate::pages::web_sites::{WebSiteStudio, WebSites};
use crate::pages::{
    self, AdminStatus, Annotations, AuthCallback, Calendar, Cameras, CaseWorkspace, Cases, Catalog,
    Controls, Dashboard, Dashboards, Devices, Docs, Events, Firmware, Flows, FluidMixtures,
    Functions, Lab, LabProjectPage, Map, Media, MediaReview, Models, NotFound, Notifications,
    Ontology, OntologyObject, OntologyType, Orgs, OrgsCurrent, Profile, Secrets, ShareTour,
    Showcase, Sites, StreamReview, Streams, Studio, System, Visualisation, VoiceId,
};
use dioxus::prelude::*;

#[derive(Clone, Debug, PartialEq, Routable)]
#[rustfmt::skip]
pub enum Route {
    // OAuth callback — OUTSIDE the shell (full-page redirect from the Rauthy IdP).
    // Query segments are infallible: absent → empty string, test
    // with is_empty().
    #[route("/auth/callback?:code&:error&:error_description")]
    AuthCallback { code: String, error: String, error_description: String },

    // PUBLIC tour (share link) — outside the shell: the Shell carries the session
    // guard, here viewing is anonymous (token = the only key).
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

        // Review of an audio or video file: media, transcript, speakers (media-review.md D271).
        #[route("/media/:id/review")]
        MediaReview { id: String },

        // Collaborative pages (pages.md #21); `?id=` opens the live editor.
        #[route("/pages?:id")]
        Docs { id: String },

        // Camera live view, settings and recordings (camera-video.md D80).
        #[route("/cameras")]
        Cameras {},

        // Vision model registry (camera-video.md D81).
        #[route("/models")]
        Models {},

        // Captured audio streams, transcribed by the queue (media-ingest.md P2.13).
        #[route("/streams")]
        Streams {},

        // Review of a window of a stream: text, speakers, kept audio (media-review.md D271).
        #[route("/streams/:id/review")]
        StreamReview { id: String },

        // Consented voice profiles of persons (voice-id.md).
        #[route("/voice-id")]
        VoiceId {},

        // Vision Lab: datasets of custom object detectors (media-vision-studio.md D198).
        #[route("/lab")]
        Lab {},

        #[route("/lab/:id")]
        LabProjectPage { id: String },

        // JSON events stored in OpenObserve (camera-video.md D84).
        #[route("/events")]
        Events {},

        #[route("/studio")]
        Studio {},

        #[route("/annotations")]
        Annotations {},

        #[route("/controls")]
        Controls {},

        // Org time ranges, recurrence occurrences included (ontology.md annex B).
        #[route("/calendar")]
        Calendar {},

        // Investigation cases and their workspace (cases.md).
        #[route("/cases")]
        Cases {},

        #[route("/cases/:id")]
        CaseWorkspace { id: String },

        // Ontology explorer, object page and type editor (ontology.md D186).
        #[route("/ontology?:tab&:type_key")]
        Ontology { tab: String, type_key: String },

        #[route("/ontology/object/:id")]
        OntologyObject { id: String },

        #[route("/ontology/types/:type_key")]
        OntologyType { type_key: String },

        // What machines proposed, waiting for a human (proposals.md D374).
        #[route("/proposals")]
        Proposals {},

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

        // Edge references (WiFi / PNeX servers) used by the device wizard
        // and the firmware build queue.
        #[route("/edges/refs")]
        EdgeRefs {},

        // Browser agents and their missions (extension-collector.md D255).
        #[route("/edges/browser-agents")]
        BrowserAgents {},

        // Web sites and their navigation studio (site-recipes.md D323–D326).
        #[route("/web-sites")]
        WebSites {},

        // Mailboxes polled by PneX (mail.md D453).
        #[route("/mailboxes")]
        Mailboxes {},

        #[route("/mailboxes/:id")]
        MailboxMessages { id: String },

        #[route("/web-sites/:site")]
        WebSiteStudio { site: String },

        #[route("/orgs")]
        Orgs {},

        // Detail of the current org opened directly (deep-link target).
        #[route("/orgs/current")]
        OrgsCurrent {},

        // Org secrets vault (D110–D117): names and usages, never a value.
        #[route("/secrets")]
        Secrets {},

        // Access management (iam.md §9): roles, teams, assignments,
        // service accounts and their access keys.
        #[route("/access")]
        Access {},

        #[route("/profile")]
        Profile {},

        // System (D72): O2 retention + cleanup of the current org.
        #[route("/system")]
        System {},

        // Platform status (D72) — platform admins only (server enforces).
        #[route("/admin/status")]
        AdminStatus {},

        // CRUD foundation showcase (dev visual reference, outside nav — direct
        // URL access; session guard carried by the Shell).
        #[route("/_showcase")]
        Showcase {},

        // Unknown route: rendered inside the shell (O57).
        // The shell layout runs to the end of the enum: no `#[end_layout]` after the last variant.
        #[route("/:..route")]
        NotFound { route: Vec<String> },
}
