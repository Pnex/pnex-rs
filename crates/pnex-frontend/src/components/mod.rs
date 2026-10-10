//! Composants partagés du front : icônes (SVG lucide inline), toasts, modales.

pub mod agent_panel;
pub mod ai_retention;
/// Read-only media view with its published annotations (map, media page,
/// annotation page preview).
pub mod annotated_media;
/// Annotations sur médias (D55–D60) — popover live partagée + panneau
/// d'édition studio (tranche 4).
pub mod annotation_editor;
pub mod assistant;
pub mod badges;
pub mod board_pinout_editor;
pub mod build_failure;
pub mod charts;
pub mod code_editing;
pub mod code_highlight;
pub mod confirm;
/// Socle CRUD standardisé (listes) — layout, états, pagination, table,
/// filtres, formulaire (doctrine et checklist : README du module).
pub mod crud;
pub mod dashboard_editor;
/// Rendu live read-only d'un dashboard (page Dashboards + aperçu POI).
pub mod dashboard_live;
/// Live view of mobile dashboards: pages, rooms, chips, detail (D139).
pub mod dashboard_live_mobile;
pub mod dashboard_widget;
pub mod device_wizard;
pub mod dom_rect;
/// Overlay de chargement léger réutilisable (voile + spinner, cf. login
/// allégé) — opérations serveur longues : delete de flow, builds, etc.
pub mod download_progress;
/// Pickers du référentiel Edge (WiFi + hosts) pour le wizard device.
pub mod edge_refs_picker;
/// Coquille d'éditeur unifiée (flows/dashboards/studio) — barre 3 zones,
/// palette à la demande, inspecteur divulgatif, invitation d'état vide.
pub mod editor_shell;
pub mod flash_modal;
pub mod flow_editor;
pub mod function_analysis;
pub mod function_node_preview;
pub mod function_ports;
pub mod function_templates;
pub mod functions_reference;
/// Home icon catalog of the dashboard cards (D136).
pub mod home_icons;
pub mod icons;
/// Éditeur de labels transverse (D42) — réutilisable sur tout kind.
pub mod labels_editor;
pub mod lan_detect;
/// Secret input shared by every functional form (secrets.md D113).
pub mod llm_providers;
pub mod loading_overlay;
pub mod location_breadcrumb;
pub mod markdown;
pub mod modal;
/// Notifications (D49–D54) — formulaires canal (field_spec) et template.
pub mod notify_channel_form;
pub mod notify_deliveries;
pub mod notify_template_form;
pub mod org_switcher;
pub mod pager;
pub mod pins_panel;
/// Panneau d'aperçu read-only des objets attachés (média/tour/dashboard) +
/// épingle par POI — couvre la carte, commutable d'asset à asset.
pub mod poi_preview;
/// Arbre de dossiers du drawer POI (containment D42 + drag & drop).
pub mod poi_tree;
pub mod range_bars;
pub mod refresh_rate;
/// Sélecteur générique d'objets attachables (map drawer, D42) —
/// généralisation du MediaPicker.
pub mod resource_picker;
pub mod secret_field;
/// Root CA download card with fingerprint + QR code (D70).
pub mod server_ca_card;
pub mod surface;
pub mod symbols;
pub mod toasts;
pub mod tour_editor;
pub mod tour_viewer;
