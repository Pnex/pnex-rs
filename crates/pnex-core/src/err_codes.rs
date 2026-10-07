//! Machine error codes shared by the backend controllers and the frontend
//! error resolver (`crates/pnex-frontend/src/api/error_i18n.rs`).
//!
//! Doctrine (i18n chantier, 2026-09-22):
//! - every user-facing error carries a machine code;
//! - the frontend resolves `err-<code-kebab>` against its fluent locales
//!   (render time only — `t!` panics outside a reactive render scope);
//! - the human-readable `description`/`message` is the **canonical English
//!   fallback**, displayed verbatim for unregistered codes (documented
//!   exception: runtime diagnostics with arbitrary data — `last_error`,
//!   debug feed, node/starlark errors);
//! - interpolatable data travels in `args` (JSON object of strings), never
//!   pre-rendered into the message.
//!
//! Adding a code = 1 const + entry in `ALL` + the `err-<kebab>` key in BOTH
//! `crates/pnex-frontend/locales/*.ftl` files. Enforced by the cross-checks:
//! `crates/pnex-backend/tests/error_codes.rs` (every literal first argument
//! of `ErrorDetail::new(` must be in `ALL`) and the frontend test
//! `err_codes_resolvent_en_fr` (every `ALL` code has its key in both
//! locales).

// ── Client-side codes (pnex-frontend, resolved at display time) ──────────

/// Network failure (api/client.rs `send`/`send_bytes`).
pub const CLIENT_NETWORK: &str = "client-network";
/// Response body failed JSON decoding (api/client.rs).
pub const CLIENT_UNREADABLE_BODY: &str = "client-unreadable-body";
/// 2xx response with an empty body where a body was required.
pub const CLIENT_EMPTY_BODY: &str = "client-empty-body";
/// Refresh-token flow failed → session expired.
pub const CLIENT_SESSION_EXPIRED: &str = "client-session-expired";

// ── Flash bridge errors (pnex-frontend `flash.rs`, OTA modal) ────────────

/// The `window.pnexFlash` JS bridge is not loaded (non-secure context).
pub const FLASH_FLASHER_MISSING: &str = "flash-flasher-missing";
/// The bridge did not return a promise (unexpected JS glue).
pub const FLASH_NOT_A_PROMISE: &str = "flash-not-a-promise";
/// Web-serial unavailable or device port lost (bridge-level failure).
pub const FLASH_UNAVAILABLE: &str = "flash-unavailable";

// ── Flow editor client diagnostics (node inspector) ──────────────────────

/// Device API unreachable — nothing can be said about the device.
pub const PIN_CHECK_UNAVAILABLE: &str = "pin_check_unavailable";
/// Device absent from the registry (deleted, or never existed).
pub const DEVICE_DELETED: &str = "device_deleted";
/// Device present but disabled (`active = false`).
pub const DEVICE_INACTIVE: &str = "device_inactive";
/// Device online in DB but not connected (`pinout.connected = false`).
pub const DEVICE_OFFLINE: &str = "device_offline";
/// Pin label absent from the device pinout.
pub const PIN_MISSING: &str = "pin_missing";
/// Pin mode is an output while the node performs a read.
pub const PIN_IS_OUTPUT: &str = "pin_is_output";
/// Pin mode is not an output while the node performs a write.
pub const DEVICE_WRITE_PIN_MODE: &str = "device_write_pin_mode";
/// Input pin with no subscription: the generic firmware publishes nothing.
pub const PIN_NOT_SUBSCRIBED: &str = "pin_not_subscribed";

// ── Controller codes (batch orgs/pois/tours/dashboards/media/functions/annotation_layers) ──

pub const ORG_NAME_DUPLICATE: &str = "org-name-duplicate";
pub const ORG_RENAME_FORBIDDEN: &str = "org-rename-forbidden";
pub const ORG_DELETE_FORBIDDEN: &str = "org-delete-forbidden";
pub const ORG_NOT_EMPTY: &str = "org-not-empty";
pub const ORG_MEMBER_ADD_FORBIDDEN: &str = "org-member-add-forbidden";
pub const ORG_OWNER_GRANT_FORBIDDEN: &str = "org-owner-grant-forbidden";
pub const ORG_MEMBER_UNKNOWN_USER: &str = "org-member-unknown-user";
pub const ORG_MEMBER_DUPLICATE: &str = "org-member-duplicate";
pub const ORG_MEMBER_ROLE_FORBIDDEN: &str = "org-member-role-forbidden";
pub const ORG_OWNER_EDIT_FORBIDDEN: &str = "org-owner-edit-forbidden";
pub const ORG_LAST_OWNER: &str = "org-last-owner";
pub const ORG_MEMBER_REMOVE_FORBIDDEN: &str = "org-member-remove-forbidden";
pub const ORG_OWNER_REMOVE_FORBIDDEN: &str = "org-owner-remove-forbidden";
pub const POI_WRITE_FORBIDDEN: &str = "poi-write-forbidden";
pub const POI_LINK_WRITE_FORBIDDEN: &str = "poi-link-write-forbidden";
pub const POI_POSITION_WRITE_FORBIDDEN: &str = "poi-position-write-forbidden";
pub const POI_DEVICE_ALREADY_PLACED: &str = "poi-device-already-placed";
pub const POI_LINK_DUPLICATE: &str = "poi-link-duplicate";
pub const TOUR_WRITE_FORBIDDEN: &str = "tour-write-forbidden";
pub const TOUR_PUBLISH_FORBIDDEN: &str = "tour-publish-forbidden";
pub const TOUR_SHARE_FORBIDDEN: &str = "tour-share-forbidden";
pub const TOUR_VERSION_CONFLICT: &str = "tour-version-conflict";
pub const TOUR_NOT_PUBLISHED: &str = "tour-not-published";
pub const DASHBOARD_WRITE_FORBIDDEN: &str = "dashboard-write-forbidden";
pub const DASHBOARD_VERSION_CONFLICT: &str = "dashboard-version-conflict";
pub const MEDIA_WRITE_FORBIDDEN: &str = "media-write-forbidden";
pub const MEDIA_LAST_VERSION: &str = "media-last-version";
pub const MEDIA_FILE_TOO_LARGE: &str = "media-file-too-large";
pub const FUNCTIONS_WRITE_FORBIDDEN: &str = "functions-write-forbidden";
pub const FUNCTIONS_RUNTIME_DISABLED: &str = "functions-runtime-disabled";
pub const ANNOT_WRITE_FORBIDDEN: &str = "annot-write-forbidden";
pub const ANNOT_PUBLISH_FORBIDDEN: &str = "annot-publish-forbidden";
pub const ANNOT_VERSION_CONFLICT: &str = "annot-version-conflict";

// ── Controller codes (batch devices/pins/flows/resources/edge_refs/builds/stitch/fluid/viz/ai/notify) + board/caps display codes ──

pub const DEVICE_WRITE_FORBIDDEN: &str = "device-write-forbidden";
pub const PIN_COMMANDS_FORBIDDEN: &str = "pin-commands-forbidden";
pub const PIN_INVALID_BODY: &str = "pin-invalid-body";
pub const PIN_GPIO_NOT_ADMITTED: &str = "pin-gpio-not-admitted";
pub const PIN_FLOW_CONFLICT: &str = "pin-flow-conflict";
pub const PIN_DEVICE_OFFLINE: &str = "pin-device-offline";
pub const PIN_WRITE_VALUE_REQUIRED: &str = "pin-write-value-required";
pub const PIN_WRITE_VALUE_INVALID: &str = "pin-write-value-invalid";
pub const PIN_WRITE_DUTY_TYPE: &str = "pin-write-duty-type";
pub const PIN_WRITE_DUTY_RANGE: &str = "pin-write-duty-range";
pub const PIN_WRITE_MODE_NOT_OUTPUT: &str = "pin-write-mode-not-output";
pub const PIN_SUBSCRIBE_INTERVAL_REQUIRED: &str = "pin-subscribe-interval-required";
pub const PIN_SUBSCRIBE_INTERVAL_MIN: &str = "pin-subscribe-interval-min";
pub const PIN_SUBSCRIBE_INTERVAL_MAX: &str = "pin-subscribe-interval-max";
pub const PIN_OP_UNKNOWN: &str = "pin-op-unknown";
/// Deploy rejected: the candidate flow writes an output pin already driven
/// by another deployed flow (one write source per output).
pub const PIN_ALREADY_ASSIGNED: &str = "pin-already-assigned";
/// Manual write refused: the pin is written by a deployed flow.
pub const PIN_RESERVED_BY_FLOW: &str = "pin-reserved-by-flow";
pub const FLOW_WRITE_FORBIDDEN: &str = "flow-write-forbidden";
pub const FLOW_DEPLOY_FORBIDDEN: &str = "flow-deploy-forbidden";
pub const FLOW_EXEC_FORBIDDEN: &str = "flow-exec-forbidden";
pub const FLOW_DEBUG_DISABLED: &str = "flow-debug-disabled";
pub const FLOW_STALE_VERSION: &str = "flow-stale-version";
pub const FLOW_STOP_NOT_DEPLOYED: &str = "flow-stop-not-deployed";
pub const FLOW_START_NOT_STOPPED: &str = "flow-start-not-stopped";
pub const FLOW_NO_DEPLOYED_VERSION: &str = "flow-no-deployed-version";
pub const FLOW_DEVICE_WRITE_VALUES_EMPTY: &str = "flow-device-write-values-empty";
pub const RESOURCE_WRITE_FORBIDDEN: &str = "resource-write-forbidden";
pub const RESOURCE_EDGE_DUPLICATE: &str = "resource-edge-duplicate";
pub const EDGE_WRITE_FORBIDDEN: &str = "edge-write-forbidden";
/// Server host referential is frozen by the deployment (`PNEX_PROD_HOST`).
pub const EDGE_HOST_LOCKED: &str = "edge-host-locked";
/// Org secrets vault (secrets.md D110–D118).
pub const SECRET_NOT_FOUND: &str = "secret-not-found";
pub const SECRET_IN_USE: &str = "secret-in-use";
pub const SECRET_NOT_REFERENCED: &str = "secret-not-referenced";
pub const SECRET_NAME_TAKEN: &str = "secret-name-taken";
pub const SECRET_WRITE_FORBIDDEN: &str = "secret-write-forbidden";
/// Strict R9: only owner/admin attach a vault secret to a field or change
/// its destination; others keep it as is. Args: `field`.
pub const SECRET_DESTINATION_LOCKED: &str = "secret-destination-locked";
pub const BUILD_CREATE_FORBIDDEN: &str = "build-create-forbidden";
pub const BUILD_DELETE_FORBIDDEN: &str = "build-delete-forbidden";
/// A build of this device is already queued or running.
pub const BUILD_IN_PROGRESS: &str = "build-in-progress";
pub const STITCH_WRITE_FORBIDDEN: &str = "stitch-write-forbidden";
pub const STITCH_JOB_TERMINAL: &str = "stitch-job-terminal";
pub const STITCH_STATE_UNKNOWN: &str = "stitch-state-unknown";
pub const STITCH_DISABLED: &str = "stitch-disabled";
pub const STITCH_FRAME_TOO_LARGE: &str = "stitch-frame-too-large";
pub const FLUID_WRITE_FORBIDDEN: &str = "fluid-write-forbidden";
pub const VIZ_WRITE_FORBIDDEN: &str = "viz-write-forbidden";
pub const LLM_PROVIDER_FORBIDDEN: &str = "llm-provider-forbidden";
pub const LLM_PROVIDER_NOT_FOUND: &str = "llm-provider-not-found";
pub const LLM_PROVIDER_NAME_TAKEN: &str = "llm-provider-name-taken";
/// Provider test: the provider has no API key set.
pub const LLM_PROVIDER_NO_KEY: &str = "llm-provider-no-key";
/// Provider test: its API key could not be read from the vault.
pub const LLM_PROVIDER_KEY_UNREADABLE: &str = "llm-provider-key-unreadable";
pub const NOTIFY_WRITE_FORBIDDEN: &str = "notify-write-forbidden";
pub const NOTIFY_CHANNEL_NAME_CONFLICT: &str = "notify-channel-name-conflict";
pub const NOTIFY_TEMPLATE_NAME_CONFLICT: &str = "notify-template-name-conflict";
pub const NOTIFY_TEST_BODY_INVALID: &str = "notify-test-body-invalid";
/// Template render failure (preview, channel test, save-time syntax check);
/// the minijinja detail travels verbatim in `args.detail`.
pub const NOTIFY_TEMPLATE_RENDER: &str = "notify-template-render";
pub const CAPS_OUT_OF_RANGE: &str = "caps-out-of-range";
pub const CAPS_FLASH_PINS: &str = "caps-flash-pins";
pub const CAPS_PSRAM_PINS: &str = "caps-psram-pins";
pub const CAPS_USB_PINS: &str = "caps-usb-pins";
pub const CAPS_CONSOLE_PINS: &str = "caps-console-pins";
pub const CAPS_ADC2_WIFI: &str = "caps-adc2-wifi";
pub const CAPS_ANALOG_ONLY_ON_A0: &str = "caps-analog-only-on-a0";
pub const CAPS_ADC_ONLY_ON_A0: &str = "caps-adc-only-on-a0";
pub const CAPS_STRAPPING_LOW: &str = "caps-strapping-low";
pub const CAPS_STRAPPING_HIGH: &str = "caps-strapping-high";
pub const CAPS_INPUT_ONLY_PIN: &str = "caps-input-only-pin";
pub const CAPS_NO_PULL_UP: &str = "caps-no-pull-up";
pub const CAPS_PWM_UNSUPPORTED_PIN: &str = "caps-pwm-unsupported-pin";
pub const BOARD_RESERVED_SCREEN: &str = "board-reserved-screen";

// ── System / platform administration (D72) ───────────────────────────────

/// Endpoint reserved to platform administrators (`users.platform_admin`).
pub const PLATFORM_ADMIN_REQUIRED: &str = "platform-admin-required";
/// Retention days outside the accepted range (1..=3650).
pub const RETENTION_OUT_OF_RANGE: &str = "retention-out-of-range";
/// SaaS mode: retention follows the subscription and cannot be changed here.
pub const RETENTION_LOCKED_BY_PLAN: &str = "retention-locked-by-plan";
/// OpenObserve is not configured on this server (no `settings.openobserve`).
pub const O2_NOT_CONFIGURED: &str = "o2-not-configured";
/// OpenObserve rejected or failed a data deletion.
pub const O2_DELETE_FAILED: &str = "o2-delete-failed";
/// OpenObserve does not support time-range deletion on this stream type.
pub const O2_TIME_RANGE_UNSUPPORTED: &str = "o2-time-range-unsupported";
/// Invalid time range (start must be before end).
pub const O2_TIME_RANGE_INVALID: &str = "o2-time-range-invalid";
/// Deleting telemetry data requires the owner or admin role.
pub const SYSTEM_DATA_FORBIDDEN: &str = "system-data-forbidden";
/// The typed confirmation does not match the organization name.
pub const CONFIRMATION_MISMATCH: &str = "confirmation-mismatch";

// ── Camera & video (camera-video.md D73–D80) ─────────────────────────────

/// Changing camera settings or deleting recordings requires the owner or
/// admin role.
pub const CAMERA_WRITE_FORBIDDEN: &str = "camera-write-forbidden";
/// The device is not a camera (no `camera` cap announced yet).
pub const CAMERA_UNKNOWN: &str = "camera-unknown";
/// No frame received yet for this camera (snapshot).
pub const CAMERA_NO_FRAME: &str = "camera-no-frame";
/// The camera device has no live session (D104 live test).
pub const CAMERA_OFFLINE: &str = "camera-offline";
/// Recording range query without a valid `from` < `to` window (or wider
/// than allowed).
pub const CAMERA_RANGE_INVALID: &str = "camera-range-invalid";
/// The requested recording export exceeds the single-file size cap.
pub const CAMERA_EXPORT_TOO_LARGE: &str = "camera-export-too-large";
/// No recording in the requested range.
pub const CAMERA_EXPORT_EMPTY: &str = "camera-export-empty";
/// Deploy gate: another deployed flow already records this camera (one
/// `video-record` per camera).
pub const CAMERA_ALREADY_RECORDED: &str = "camera-already-recorded";
/// A recorded segment upload from the flow runtime is malformed.
pub const VIDEO_SEGMENT_INVALID: &str = "video-segment-invalid";
/// The configured media store (fs | s3) is unavailable.
pub const VIDEO_STORE_UNAVAILABLE: &str = "video-store-unavailable";
/// The events storage (OpenObserve logs) is unreachable or failed.
pub const EVENTS_UNAVAILABLE: &str = "events-unavailable";
/// The notification journal (OpenObserve, D86) could not be read.
pub const NOTIFY_JOURNAL_UNAVAILABLE: &str = "notify-journal-unavailable";
/// An event stream name is invalid (`ev_[a-z0-9_]+`).
pub const EVENT_STREAM_INVALID: &str = "event-stream-invalid";
/// Managing vision models requires the owner, admin or member role.
pub const ML_MODEL_WRITE_FORBIDDEN: &str = "ml-model-write-forbidden";
/// The model's ONNX file (media version) is missing.
pub const ML_MODEL_MISSING_BYTES: &str = "ml-model-missing-bytes";
/// The ONNX model could not be loaded (unsupported ops, wrong input size…).
pub const ML_MODEL_LOAD_FAILED: &str = "ml-model-load-failed";
/// Inference failed on the given image (undecodable image, bad output).
pub const ML_MODEL_INFERENCE_FAILED: &str = "ml-model-inference-failed";
/// The model refuses the spec or fails its test inference (D100) — args `{detail}`.
pub const ML_MODEL_INVALID: &str = "ml-model-invalid";

// ── Custom firmware (custom-firmware.md D87–D94) ─────────────────────────

/// Managing firmware projects requires the owner, admin or member role.
pub const FIRMWARE_WRITE_FORBIDDEN: &str = "firmware-write-forbidden";
/// Custom firmware compiling is off on this server (settings.firmware.custom.enabled).
pub const FIRMWARE_CUSTOM_DISABLED: &str = "firmware-custom-disabled";
/// The device SoC differs from the project chip family.
pub const FIRMWARE_CHIP_MISMATCH: &str = "firmware-chip-mismatch";
/// A custom firmware project on a model outside the generic family: a
/// predefined board keeps the firmware maintained by PneX.
pub const FIRMWARE_FAMILY_LOCKED: &str = "firmware-family-locked";
/// Library id outside the pinned catalog (`$value` = id).
pub const FIRMWARE_LIB_UNKNOWN: &str = "firmware-lib-unknown";
/// Catalog library not available for the project chip (`$value` = id).
pub const FIRMWARE_LIB_INCOMPATIBLE: &str = "firmware-lib-incompatible";
/// The sketch failed the lexical guard (`violations` = per-line codes).
pub const FIRMWARE_SOURCE_REFUSED: &str = "firmware-source-refused";
/// Devices are attached to the project (`$count`).
pub const FIRMWARE_PROJECT_IN_USE: &str = "firmware-project-in-use";
/// Sketch violation: absolute include path.
pub const FIRMWARE_INCLUDE_ABSOLUTE: &str = "firmware-include-absolute";
/// Sketch violation: `..` in an include path.
pub const FIRMWARE_INCLUDE_PARENT: &str = "firmware-include-parent";
/// Sketch violation: macro-expanded include.
pub const FIRMWARE_INCLUDE_NOT_LITERAL: &str = "firmware-include-not-literal";
/// Sketch violation: `#embed` directive.
pub const FIRMWARE_EMBED: &str = "firmware-embed";
/// Sketch violation: assembler `.incbin`.
pub const FIRMWARE_INCBIN: &str = "firmware-incbin";
/// Sketch larger than MAX_MAIN_CPP_BYTES.
pub const FIRMWARE_SOURCE_TOO_LARGE: &str = "firmware-source-too-large";
/// Stale `expected_revision_number` on a firmware project save.
pub const FIRMWARE_VERSION_CONFLICT: &str = "firmware-version-conflict";
/// Custom command not announced by the device firmware (`$value` = name).
pub const DEVICE_COMMAND_UNKNOWN: &str = "device-command-unknown";

// ── Edge agent (D95) ─────────────────────────────────────────────────────

/// The target device is not an edge agent.
pub const AGENT_NOT_AN_AGENT: &str = "agent-not-an-agent";
/// Action not applicable to an edge agent (firmware build, OTA, pins).
pub const AGENT_UNSUPPORTED_ACTION: &str = "agent-unsupported-action";
/// Distinct keys quota out of range.
pub const AGENT_QUOTA_INVALID: &str = "agent-quota-invalid";
/// Enrollment code unknown, already used or expired (never says which).
pub const AGENT_ENROLL_CODE_INVALID: &str = "agent-enroll-code-invalid";
/// Too many enrollment attempts from this address.
pub const AGENT_ENROLL_RATE_LIMITED: &str = "agent-enroll-rate-limited";
/// Agent binary for the requested platform not shipped with this server.
pub const AGENT_BINARY_NOT_FOUND: &str = "agent-binary-not-found";
/// Agent management reserved to owners/admins.
pub const AGENT_WRITE_FORBIDDEN: &str = "agent-write-forbidden";
/// A per-pod compute pool (CoolProp, vision inference, runtime checks,
/// large uploads) is saturated; the client should retry shortly.
pub const SERVER_BUSY: &str = "server-busy";

// ── Rate limiting ────────────────────────────────────────────────────────

/// Too many requests from this client address on a protected route
/// (`Retry-After` header carries the wait in seconds).
pub const RATE_LIMITED: &str = "rate-limited";

// ── Org controls (D125–D127, surfaces-controls.md) ──────────────────────

/// Viewer trying to create/edit/delete a control or to operate one.
pub const CONTROL_WRITE_FORBIDDEN: &str = "control-write-forbidden";
/// Another control of the org already uses this key.
pub const CONTROL_KEY_TAKEN: &str = "control-key-taken";
/// Value refused by the control spec; `args.reason` = machine token of
/// `ControlValueError::code` (`out_of_range`, `off_step`…).
pub const CONTROL_VALUE_INVALID: &str = "control-value-invalid";
/// Two writes of one control closer than `CONTROL_WRITE_MIN_INTERVAL_MS`.
pub const CONTROL_RATE_LIMITED: &str = "control-rate-limited";
/// Delete refused: deployed flows listen to it; `args.flow` = their names.
pub const CONTROL_IN_USE: &str = "control-in-use";
/// Deploy refused: a `control-source` lists a control absent from the org;
/// `args.control` = its id.
pub const CONTROL_UNKNOWN: &str = "control-unknown";
/// Valkey not configured or unreachable: controls cannot be written.
pub const CONTROL_STORE_UNAVAILABLE: &str = "control-store-unavailable";

// ── Assistant (ai-assistant.md D143–D145) ──────────────────────────────

/// Assistant tool refused: the change touches deployed, running flows
/// (D143 flow edit, D144 coupled dashboard widget); `args.flow` = their
/// names. The user stops them in the flow editor, then asks again.
pub const AI_FLOW_RUNNING: &str = "ai-flow-running";
/// A reply is already being written in this conversation (one turn at a
/// time, D145).
pub const AI_CONVERSATION_BUSY: &str = "ai-conversation-busy";
/// Changing the org's assistant retention needs the owner or admin role.
pub const AI_RETENTION_FORBIDDEN: &str = "ai-retention-forbidden";
/// The org's assistant retention may only be shorter than the platform
/// value; `args.max` = the platform value in days.
pub const AI_RETENTION_ABOVE_PLATFORM: &str = "ai-retention-above-platform";
/// The assistant is switched off on this server (`PNEX_AI_ENABLED=false`).
pub const AI_DISABLED: &str = "ai-disabled";
/// No default LLM provider in the org (D119: no platform fallback).
pub const AI_NOT_CONFIGURED: &str = "ai-not-configured";
/// The LLM provider refused the API key (401/403).
pub const AI_AUTH_REJECTED: &str = "ai-auth-rejected";
/// The LLM provider is rate-limiting (429).
pub const AI_RATE_LIMITED: &str = "ai-rate-limited";
/// The LLM provider answered an HTTP error; `args.status` = its status.
pub const AI_UPSTREAM: &str = "ai-upstream";
/// The LLM provider did not answer in time.
pub const AI_TIMEOUT: &str = "ai-timeout";
/// The LLM provider is unreachable (DNS, connection, TLS); `args.detail` =
/// the transport error (runtime diagnostic, verbatim).
pub const AI_NETWORK: &str = "ai-network";
/// The LLM provider's answer could not be decoded.
pub const AI_BAD_RESPONSE: &str = "ai-bad-response";
/// Internal failure while an assistant tool ran (details logged server
/// side, never shown: they may carry SQL or identifiers).
pub const AI_TOOL_INTERNAL: &str = "ai-tool-internal";
/// A viewer asked the assistant for a change (writes need owner, admin or
/// member, R2).
pub const AI_WRITE_FORBIDDEN: &str = "ai-write-forbidden";

// ── Registered codes ─────────────────────────────────────────────────────

/// All registered codes. A code not listed here falls back to the verbatim
/// `description`/`message` on the frontend (never panics).
pub const ALL: &[&str] = &[
    AGENT_NOT_AN_AGENT,
    AGENT_UNSUPPORTED_ACTION,
    AGENT_QUOTA_INVALID,
    AGENT_ENROLL_CODE_INVALID,
    AGENT_ENROLL_RATE_LIMITED,
    AGENT_BINARY_NOT_FOUND,
    AGENT_WRITE_FORBIDDEN,
    RATE_LIMITED,
    SERVER_BUSY,
    CLIENT_NETWORK,
    CLIENT_UNREADABLE_BODY,
    CLIENT_EMPTY_BODY,
    CLIENT_SESSION_EXPIRED,
    FLASH_FLASHER_MISSING,
    FLASH_NOT_A_PROMISE,
    FLASH_UNAVAILABLE,
    PIN_CHECK_UNAVAILABLE,
    DEVICE_DELETED,
    DEVICE_INACTIVE,
    DEVICE_OFFLINE,
    PIN_MISSING,
    PIN_IS_OUTPUT,
    DEVICE_WRITE_PIN_MODE,
    PIN_NOT_SUBSCRIBED,
    ORG_NAME_DUPLICATE,
    ORG_RENAME_FORBIDDEN,
    ORG_DELETE_FORBIDDEN,
    ORG_NOT_EMPTY,
    ORG_MEMBER_ADD_FORBIDDEN,
    ORG_OWNER_GRANT_FORBIDDEN,
    ORG_MEMBER_UNKNOWN_USER,
    ORG_MEMBER_DUPLICATE,
    ORG_MEMBER_ROLE_FORBIDDEN,
    ORG_OWNER_EDIT_FORBIDDEN,
    ORG_LAST_OWNER,
    ORG_MEMBER_REMOVE_FORBIDDEN,
    ORG_OWNER_REMOVE_FORBIDDEN,
    POI_WRITE_FORBIDDEN,
    POI_LINK_WRITE_FORBIDDEN,
    POI_POSITION_WRITE_FORBIDDEN,
    POI_DEVICE_ALREADY_PLACED,
    POI_LINK_DUPLICATE,
    TOUR_WRITE_FORBIDDEN,
    TOUR_PUBLISH_FORBIDDEN,
    TOUR_SHARE_FORBIDDEN,
    TOUR_VERSION_CONFLICT,
    TOUR_NOT_PUBLISHED,
    DASHBOARD_WRITE_FORBIDDEN,
    DASHBOARD_VERSION_CONFLICT,
    MEDIA_WRITE_FORBIDDEN,
    MEDIA_LAST_VERSION,
    MEDIA_FILE_TOO_LARGE,
    FUNCTIONS_WRITE_FORBIDDEN,
    FUNCTIONS_RUNTIME_DISABLED,
    ANNOT_WRITE_FORBIDDEN,
    ANNOT_PUBLISH_FORBIDDEN,
    ANNOT_VERSION_CONFLICT,
    DEVICE_WRITE_FORBIDDEN,
    PIN_COMMANDS_FORBIDDEN,
    PIN_INVALID_BODY,
    PIN_GPIO_NOT_ADMITTED,
    PIN_FLOW_CONFLICT,
    PIN_DEVICE_OFFLINE,
    PIN_WRITE_VALUE_REQUIRED,
    PIN_WRITE_VALUE_INVALID,
    PIN_WRITE_DUTY_TYPE,
    PIN_WRITE_DUTY_RANGE,
    PIN_WRITE_MODE_NOT_OUTPUT,
    PIN_SUBSCRIBE_INTERVAL_REQUIRED,
    PIN_SUBSCRIBE_INTERVAL_MIN,
    PIN_SUBSCRIBE_INTERVAL_MAX,
    PIN_OP_UNKNOWN,
    PIN_ALREADY_ASSIGNED,
    PIN_RESERVED_BY_FLOW,
    FLOW_WRITE_FORBIDDEN,
    FLOW_DEPLOY_FORBIDDEN,
    FLOW_EXEC_FORBIDDEN,
    FLOW_DEBUG_DISABLED,
    FLOW_STALE_VERSION,
    FLOW_STOP_NOT_DEPLOYED,
    FLOW_START_NOT_STOPPED,
    FLOW_NO_DEPLOYED_VERSION,
    FLOW_DEVICE_WRITE_VALUES_EMPTY,
    RESOURCE_WRITE_FORBIDDEN,
    RESOURCE_EDGE_DUPLICATE,
    EDGE_WRITE_FORBIDDEN,
    EDGE_HOST_LOCKED,
    SECRET_NOT_FOUND,
    SECRET_IN_USE,
    SECRET_NOT_REFERENCED,
    SECRET_NAME_TAKEN,
    SECRET_WRITE_FORBIDDEN,
    SECRET_DESTINATION_LOCKED,
    BUILD_CREATE_FORBIDDEN,
    BUILD_DELETE_FORBIDDEN,
    BUILD_IN_PROGRESS,
    STITCH_WRITE_FORBIDDEN,
    STITCH_JOB_TERMINAL,
    STITCH_STATE_UNKNOWN,
    STITCH_DISABLED,
    STITCH_FRAME_TOO_LARGE,
    FLUID_WRITE_FORBIDDEN,
    VIZ_WRITE_FORBIDDEN,
    LLM_PROVIDER_FORBIDDEN,
    LLM_PROVIDER_NOT_FOUND,
    LLM_PROVIDER_NAME_TAKEN,
    LLM_PROVIDER_NO_KEY,
    LLM_PROVIDER_KEY_UNREADABLE,
    NOTIFY_WRITE_FORBIDDEN,
    NOTIFY_CHANNEL_NAME_CONFLICT,
    NOTIFY_TEMPLATE_NAME_CONFLICT,
    NOTIFY_TEST_BODY_INVALID,
    NOTIFY_TEMPLATE_RENDER,
    CAPS_OUT_OF_RANGE,
    CAPS_FLASH_PINS,
    CAPS_PSRAM_PINS,
    CAPS_USB_PINS,
    CAPS_CONSOLE_PINS,
    CAPS_ADC2_WIFI,
    CAPS_ANALOG_ONLY_ON_A0,
    CAPS_ADC_ONLY_ON_A0,
    CAPS_STRAPPING_LOW,
    CAPS_STRAPPING_HIGH,
    CAPS_INPUT_ONLY_PIN,
    CAPS_NO_PULL_UP,
    CAPS_PWM_UNSUPPORTED_PIN,
    BOARD_RESERVED_SCREEN,
    PLATFORM_ADMIN_REQUIRED,
    RETENTION_OUT_OF_RANGE,
    RETENTION_LOCKED_BY_PLAN,
    O2_NOT_CONFIGURED,
    O2_DELETE_FAILED,
    O2_TIME_RANGE_UNSUPPORTED,
    O2_TIME_RANGE_INVALID,
    SYSTEM_DATA_FORBIDDEN,
    CONFIRMATION_MISMATCH,
    CAMERA_WRITE_FORBIDDEN,
    CAMERA_UNKNOWN,
    CAMERA_NO_FRAME,
    CAMERA_OFFLINE,
    CAMERA_RANGE_INVALID,
    CAMERA_EXPORT_TOO_LARGE,
    CAMERA_EXPORT_EMPTY,
    CAMERA_ALREADY_RECORDED,
    VIDEO_SEGMENT_INVALID,
    VIDEO_STORE_UNAVAILABLE,
    EVENTS_UNAVAILABLE,
    NOTIFY_JOURNAL_UNAVAILABLE,
    EVENT_STREAM_INVALID,
    ML_MODEL_WRITE_FORBIDDEN,
    ML_MODEL_MISSING_BYTES,
    ML_MODEL_LOAD_FAILED,
    ML_MODEL_INFERENCE_FAILED,
    ML_MODEL_INVALID,
    FIRMWARE_WRITE_FORBIDDEN,
    FIRMWARE_CUSTOM_DISABLED,
    FIRMWARE_CHIP_MISMATCH,
    FIRMWARE_FAMILY_LOCKED,
    FIRMWARE_LIB_UNKNOWN,
    FIRMWARE_LIB_INCOMPATIBLE,
    FIRMWARE_SOURCE_REFUSED,
    FIRMWARE_PROJECT_IN_USE,
    FIRMWARE_INCLUDE_ABSOLUTE,
    FIRMWARE_INCLUDE_PARENT,
    FIRMWARE_INCLUDE_NOT_LITERAL,
    FIRMWARE_EMBED,
    FIRMWARE_INCBIN,
    FIRMWARE_SOURCE_TOO_LARGE,
    DEVICE_COMMAND_UNKNOWN,
    FIRMWARE_VERSION_CONFLICT,
    CONTROL_WRITE_FORBIDDEN,
    CONTROL_KEY_TAKEN,
    CONTROL_VALUE_INVALID,
    CONTROL_RATE_LIMITED,
    CONTROL_IN_USE,
    AI_FLOW_RUNNING,
    AI_CONVERSATION_BUSY,
    AI_RETENTION_FORBIDDEN,
    AI_RETENTION_ABOVE_PLATFORM,
    AI_DISABLED,
    AI_NOT_CONFIGURED,
    AI_AUTH_REJECTED,
    AI_RATE_LIMITED,
    AI_UPSTREAM,
    AI_TIMEOUT,
    AI_NETWORK,
    AI_BAD_RESPONSE,
    AI_TOOL_INTERNAL,
    AI_WRITE_FORBIDDEN,
    CONTROL_UNKNOWN,
    CONTROL_STORE_UNAVAILABLE,
];

/// True when the code is registered (translatable through `err-<kebab>`).
pub fn exists(code: &str) -> bool {
    ALL.contains(&code)
}

/// Fluent display key for a code (`client-network` → `err-client-network`).
/// snake_case segments become kebab (`pin_not_subscribed` →
/// `err-pin-not-subscribed`).
pub fn fluent_key(code: &str) -> String {
    format!("err-{}", code.replace('_', "-"))
}

// ── Field-status machine tokens ({"<field>": "<token>"} bodies) ──────────

/// `{"name": "required"}` — the generic-required field error.
pub const FIELD_REQUIRED: &str = "required";
/// `{"name": "max_length:255"}` — length token, `:N` suffix becomes the
/// `$value` fluent arg of `err-max-length`.
pub const FIELD_MAX_LENGTH: &str = "max_length";
/// `{"uuid": "invalid-uuid"}` — format token for identifier fields.
pub const FIELD_INVALID_UUID: &str = "invalid-uuid";
/// `{"kind": "invalid"}` — value outside the accepted set or format.
pub const FIELD_INVALID: &str = "invalid";
