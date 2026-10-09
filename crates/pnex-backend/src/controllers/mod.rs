pub mod ai;
pub mod annotation_layers;
/// Profils de board (catalogue /api/v1/boards — éditeur pinout + wizard).
pub mod boards;
pub mod builds;
pub mod camera_recordings;
/// Cameras settings + recorded segments API (D76/D79).
pub mod cameras;
pub mod controls;
pub mod dashboard;
pub mod dashboards;
pub mod devices;
/// Référentiels Edge — credentials WiFi + hosts serveur PNeX (wizard device).
pub mod edge_agents;
pub mod edge_refs;
/// JSON events stored as OpenObserve logs (D84).
pub mod events;
pub mod firmware_projects;
pub mod flow_cluster;
pub mod flows;
pub mod fluid_mixtures;
pub mod functions;
/// Global typeahead across the org's objects (D69).
pub mod global_search;
pub mod health;
pub mod internal_flow;
/// Endpoint interne de livraison websocket (runtime de flows / loopback).
pub mod internal_notify;
pub mod media;
pub mod memory;
pub mod meta;
/// Vision model registry (D81) + internal model fetch for the flow runtime.
pub mod ml_models;
/// Notifications (D49–D54) — CRUD canaux/templates + test/preview/journal.
pub mod notify;
pub mod oauth2;
pub mod orgs;
pub mod ota;
pub mod pagination;
pub mod pins;
pub mod pois;
pub mod public_tours;
/// Couche d'organisation transverse (D42) — endpoints kind-agnostiques.
pub mod resources;
/// Org secrets vault (D110–D118).
pub mod secrets;
pub mod stitch_jobs;
/// System: O2 retention/cleanup + platform status (D72).
pub mod system;
pub mod thermo;
pub mod tours;
pub mod user_info;
pub mod visualization;
pub mod viz_widgets;
/// Camera uplink + browser live view (camera-video.md D73/D75).
pub mod ws_camera;
pub mod ws_device;
pub mod ws_ingest;
/// Bus WS des notifications (canal `websocket`, D51).
pub mod ws_notify;
pub mod ws_ticket;

/// Error answer of the API: machine `code` (resolved by the frontend to
/// `err-<code>`), canonical English `description`, optional fluent `args`.
pub(crate) fn coded_error(
    status: axum::http::StatusCode,
    code: &str,
    description: impl Into<String>,
    args: Option<serde_json::Value>,
) -> loco_rs::Error {
    loco_rs::Error::CustomError(
        status,
        loco_rs::controller::ErrorDetail {
            error: Some(code.to_string()),
            description: Some(description.into()),
            errors: args.map(|a| serde_json::json!({ "args": a })),
        },
    )
}
