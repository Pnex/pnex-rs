//! Shared front components: icons (inline lucide SVG), toasts, modals.

pub mod agent_panel;
pub mod ai_retention;
/// Read-only media view with its published annotations (map, media page,
/// annotation page preview).
pub mod annotated_media;
/// Annotations on media (D55–D60) — shared live popover + studio
/// editing panel (slice 4).
pub mod annotation_editor;
pub mod assistant;
pub mod badges;
pub mod board_pinout_editor;
/// Browser endpoints of the org (site-recipes.md D325).
pub mod browser_endpoints;
pub mod build_failure;
pub mod charts;
pub mod code_editing;
pub mod code_highlight;
pub mod confirm;
/// Standardised CRUD foundation (lists) — layout, states, pagination, table,
/// filters, form (doctrine and checklist: the module's README).
pub mod crud;
pub mod dashboard_editor;
/// Read-only live rendering of a dashboard (Dashboards page + POI preview).
pub mod dashboard_live;
/// Live view of mobile dashboards: pages, rooms, chips, detail (D139).
pub mod dashboard_live_mobile;
pub mod dashboard_widget;
pub mod device_wizard;
pub mod dom_rect;
/// Reusable lightweight loading overlay (veil + spinner, see the slimmed-down
/// login) — long server operations: flow delete, builds, etc.
pub mod download_progress;
/// Edge reference pickers (WiFi + hosts) for the device wizard.
pub mod edge_refs_picker;
/// Unified editor shell (flows/dashboards/studio) — 3-zone bar,
/// palette on demand, disclosing inspector, empty-state invitation.
pub mod editor_shell;
pub mod flash_modal;
pub mod flow_editor;
pub mod function_analysis;
pub mod function_node_preview;
pub mod function_ports;
pub mod function_templates;
pub mod functions_reference;
/// Secret input shared by every functional form (secrets.md D113).
pub mod geo_providers;
pub mod gerbe;
/// Home icon catalog of the dashboard cards (D136).
pub mod home_icons;
pub mod icons;
/// Cross-cutting label editor (D42) — reusable on any kind.
pub mod labels_editor;
pub mod lan_detect;
pub mod llm_providers;
pub mod loading_overlay;
pub mod location_breadcrumb;
pub mod map_panel;
pub mod markdown;
pub mod mesh_viewer;
pub mod modal;
/// Notifications (D49–D54) — channel (field_spec) and template forms.
pub mod notify_channel_form;
pub mod notify_deliveries;
pub mod notify_template_form;
pub mod ontology;
pub mod org_switcher;
pub mod pager;
pub mod pins_panel;
/// Read-only preview panel of attached objects (media/tour/dashboard) +
/// pin per POI — covers the map, switchable from asset to asset.
pub mod poi_preview;
/// Folder tree of the POI drawer (D42 containment + drag & drop).
pub mod poi_tree;
pub mod proposals_table;
pub mod range_bars;
pub mod refresh_rate;
/// Generic picker of attachable objects (map drawer, D42) —
/// generalisation of the MediaPicker.
pub mod resource_picker;
pub mod secret_field;
/// Root CA download card with fingerprint + QR code (D70).
pub mod server_ca_card;
pub mod stepper;
pub mod surface;
pub mod symbols;
/// Generic timeline (ruler, lanes of marks and spans, cursor): cases,
/// Vision Lab, media analyses.
pub mod timeline;
pub mod toasts;
pub mod tour_editor;
pub mod tour_viewer;
/// Shared playback transport (play, clock, scrub strip, speed) and the
/// audio/video player built on it: one look for everything timed.
pub mod transport;
pub mod ui;
/// Consented voice profiles of persons (voice-id.md).
pub mod voice_id;
pub mod web_search_providers;
