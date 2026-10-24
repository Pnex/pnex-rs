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
/// The caller is not a member of the `X-Org-Id` organisation (403).
pub const ORG_NOT_MEMBER: &str = "org-not-member";
/// The caller's role does not hold the permission of the operation (403,
/// iam.md §8); `args.permission` names it (`service:verb`).
pub const ACCESS_DENIED: &str = "access-denied";
/// An access key called an operation outside the org ones (profile, org list, MCP, platform) or a socket (403, iam.md §7).
pub const ACCESS_KEY_NOT_ALLOWED: &str = "access-key-not-allowed";
/// Service account not found in the org (404).
pub const IAM_SERVICE_ACCOUNT_NOT_FOUND: &str = "iam-service-account-not-found";
/// A service account of the org already has this name (409).
pub const IAM_SERVICE_ACCOUNT_NAME_TAKEN: &str = "iam-service-account-name-taken";
/// Access key not found, or already revoked (404).
pub const IAM_ACCESS_KEY_NOT_FOUND: &str = "iam-access-key-not-found";
/// Two keys of the service account are active already (409): revoke one first.
pub const IAM_ACCESS_KEY_LIMIT: &str = "iam-access-key-limit";
/// Custom role not found in the org (404).
pub const IAM_ROLE_NOT_FOUND: &str = "iam-role-not-found";
/// Team not found in the org (404).
pub const IAM_TEAM_NOT_FOUND: &str = "iam-team-not-found";
/// Assignment not found in the org (404).
pub const IAM_ASSIGNMENT_NOT_FOUND: &str = "iam-assignment-not-found";
/// A custom role of the org already has this key (409).
pub const IAM_ROLE_KEY_TAKEN: &str = "iam-role-key-taken";
/// A team of the org already has this name (409).
pub const IAM_TEAM_NAME_TAKEN: &str = "iam-team-name-taken";
/// The holder already has this role (409).
pub const IAM_ASSIGNMENT_DUPLICATE: &str = "iam-assignment-duplicate";
/// The custom role changed since it was read (409, `args.current`).
pub const IAM_VERSION_CONFLICT: &str = "iam-version-conflict";
/// The caller does not hold a permission the role would give (403, `args.permission`, D293).
pub const IAM_ESCALATION: &str = "iam-escalation";
/// An org-scoped route was called without a valid `X-Org-Id` header (400).
pub const ORG_HEADER_REQUIRED: &str = "org-header-required";
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
/// `PATCH /api/v1/profile` without any field.
pub const PROFILE_PATCH_EMPTY: &str = "profile-patch-empty";
pub const POI_DEVICE_ALREADY_PLACED: &str = "poi-device-already-placed";
pub const POI_LINK_DUPLICATE: &str = "poi-link-duplicate";
pub const TOUR_VERSION_CONFLICT: &str = "tour-version-conflict";
pub const TOUR_NOT_PUBLISHED: &str = "tour-not-published";
pub const DASHBOARD_VERSION_CONFLICT: &str = "dashboard-version-conflict";
pub const MEDIA_LAST_VERSION: &str = "media-last-version";
pub const MEDIA_FILE_TOO_LARGE: &str = "media-file-too-large";
/// Upload refused: content not an allowed format, extension not matching
/// it, or format not fitting the asset's kind (media_sniff allowlist).
pub const MEDIA_FORMAT_UNSUPPORTED: &str = "media-format-unsupported";
/// Document indexing (doc-search.md P1): format not indexable.
pub const MEDIA_INDEX_UNSUPPORTED: &str = "media-index-unsupported";
/// Document indexing: container or page count over the extraction limits.
pub const MEDIA_INDEX_TOO_LARGE: &str = "media-index-too-large";
/// Document indexing: file corrupt or unparsable.
pub const MEDIA_INDEX_MALFORMED: &str = "media-index-malformed";
/// Document indexing: bytes missing from the media store.
pub const MEDIA_INDEX_UNREADABLE: &str = "media-index-unreadable";
/// Reindex asked on an asset that is not a document or a table.
pub const MEDIA_INDEX_NOT_A_DOCUMENT: &str = "media-index-not-a-document";
/// File analysis step needing an LLM while none resolves for the org (D213).
pub const MEDIA_ANALYSIS_NO_LLM: &str = "media-analysis-no-llm";
/// File analysis step already queued or running.
pub const MEDIA_ANALYSIS_BUSY: &str = "media-analysis-busy";
/// File analysis step not offered for this kind of file.
pub const MEDIA_ANALYSIS_STEP_INVALID: &str = "media-analysis-step-invalid";
/// Summary impossible: the file yielded no text.
pub const MEDIA_ANALYSIS_NO_TEXT: &str = "media-analysis-no-text";
/// The LLM call of a file analysis failed.
pub const MEDIA_ANALYSIS_LLM_FAILED: &str = "media-analysis-llm-failed";
/// The org's daily AI call cap was reached during a file analysis.
pub const MEDIA_ANALYSIS_QUOTA: &str = "media-analysis-quota";
/// A file analysis stopped with the server (restart while running).
pub const MEDIA_ANALYSIS_INTERRUPTED: &str = "media-analysis-interrupted";
/// A large file analysis was launched without its confirmation.
pub const MEDIA_ANALYSIS_LARGE: &str = "media-analysis-large";
/// No ASR profile with valid models: a file cannot be transcribed (D211).
pub const MEDIA_ANALYSIS_NO_ASR: &str = "media-analysis-no-asr";
/// The audio of the file could not be read or decoded (D211).
pub const MEDIA_ANALYSIS_CAPTURE_FAILED: &str = "media-analysis-capture-failed";
/// Every segment of the file failed its transcription (D211).
pub const MEDIA_ANALYSIS_TRANSCRIBE_FAILED: &str = "media-analysis-transcribe-failed";
/// A speech attribution changed since the version the client read (D272).
pub const MEDIA_ATTRIBUTION_CONFLICT: &str = "media-attribution-conflict";
/// No ready voice profile to compare the speakers with (D275).
pub const MEDIA_ANALYSIS_NO_VOICE_PROFILE: &str = "media-analysis-no-voice-profile";
/// No PNeX transcription with speakers to identify (D275).
pub const MEDIA_ANALYSIS_NO_SPEAKERS: &str = "media-analysis-no-speakers";
/// The voice identification could not run (model, audio) (D275).
pub const MEDIA_ANALYSIS_IDENTIFY_FAILED: &str = "media-analysis-identify-failed";
/// No valid object detection model in the org (D215).
pub const MEDIA_ANALYSIS_NO_DETECTOR: &str = "media-analysis-no-detector";
/// The pictures of the video could not be decoded or analysed (D215).
pub const MEDIA_ANALYSIS_DETECT_FAILED: &str = "media-analysis-detect-failed";
pub const FUNCTIONS_RUNTIME_DISABLED: &str = "functions-runtime-disabled";
/// Delete refused: deployed flows use the function (`args.flows`).
pub const FUNCTION_IN_USE: &str = "function-in-use";
/// Malformed `@input`/`@output` directive (`args.line`, `args.detail`).
pub const FUNCTION_DIRECTIVE_INVALID: &str = "function-directive-invalid";
pub const ANNOT_VERSION_CONFLICT: &str = "annot-version-conflict";

// ── Controller codes (batch devices/pins/flows/resources/edge_refs/builds/stitch/fluid/viz/ai/notify) + board/caps display codes ──

/// Board change (O39): the board does not exist.
pub const DEVICE_BOARD_UNKNOWN: &str = "device-board-unknown";
/// Board change (O39): the board carries another chip than the device.
pub const DEVICE_BOARD_SOC_MISMATCH: &str = "device-board-soc-mismatch";
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
pub const FLOW_DEBUG_DISABLED: &str = "flow-debug-disabled";
pub const FLOW_STALE_VERSION: &str = "flow-stale-version";
pub const FLOW_STOP_NOT_DEPLOYED: &str = "flow-stop-not-deployed";
pub const FLOW_START_NOT_STOPPED: &str = "flow-start-not-stopped";
pub const FLOW_NO_DEPLOYED_VERSION: &str = "flow-no-deployed-version";
pub const FLOW_DEVICE_WRITE_VALUES_EMPTY: &str = "flow-device-write-values-empty";
/// The flow runtime is unavailable or did not acknowledge; `args.detail`
/// carries the engine diagnostic verbatim.
pub const FLOW_RUNTIME: &str = "flow-runtime";
pub const RESOURCE_EDGE_DUPLICATE: &str = "resource-edge-duplicate";
/// Kind outside the organisation layer registry (`args.kind`).
pub const RESOURCE_KIND_UNKNOWN: &str = "resource-kind-unknown";
/// A move would make a resource its own ancestor.
pub const RESOURCE_CYCLE: &str = "resource-cycle";
/// Server host referential is frozen by the deployment (`PNEX_PROD_HOST`).
pub const EDGE_HOST_LOCKED: &str = "edge-host-locked";
/// Another WiFi entry of the org already uses this SSID (409).
pub const EDGE_SSID_TAKEN: &str = "edge-ssid-taken";
/// Another host entry of the org already uses this host (409).
pub const EDGE_HOST_TAKEN: &str = "edge-host-taken";
/// OTA deployment (ota.md): an assignment is already active on the device.
pub const OTA_IN_PROGRESS: &str = "ota-in-progress";
/// Device firmware predates OTA support (no `ota` cap announced).
pub const OTA_NOT_READY: &str = "ota-not-ready";
/// No deployable (successful, OTA-stamped) build, or artifact missing.
pub const OTA_NO_ARTIFACT: &str = "ota-no-artifact";
/// Image exceeds the ESP8266 OTA staging budget.
pub const OTA_TOO_LARGE: &str = "ota-too-large";
/// Device already runs at least the target version (force to redeploy).
pub const OTA_SAME_VERSION: &str = "ota-same-version";
/// No deployable build for the requested version.
pub const OTA_VERSION_NOT_FOUND: &str = "ota-version-not-found";
/// No active OTA deployment to cancel.
pub const OTA_NO_ACTIVE: &str = "ota-no-active";
/// Device-token download refused (any authentication failure, 404).
pub const OTA_AUTH_FAILED: &str = "ota-auth-failed";
/// OTA artifact missing from the store at download time.
pub const OTA_ARTIFACT_NOT_FOUND: &str = "ota-artifact-not-found";
/// Org secrets vault (secrets.md D110–D118).
pub const SECRET_NOT_FOUND: &str = "secret-not-found";
pub const SECRET_IN_USE: &str = "secret-in-use";
pub const SECRET_NOT_REFERENCED: &str = "secret-not-referenced";
pub const SECRET_NAME_TAKEN: &str = "secret-name-taken";
pub const SECRET_WRITE_FORBIDDEN: &str = "secret-write-forbidden";
/// Strict R9: only owner/admin attach a vault secret to a field or change
/// its destination; others keep it as is. Args: `field`.
pub const SECRET_DESTINATION_LOCKED: &str = "secret-destination-locked";
/// A build of this device is already queued or running.
pub const BUILD_IN_PROGRESS: &str = "build-in-progress";
pub const STITCH_JOB_TERMINAL: &str = "stitch-job-terminal";
pub const STITCH_STATE_UNKNOWN: &str = "stitch-state-unknown";
pub const STITCH_DISABLED: &str = "stitch-disabled";
pub const STITCH_FRAME_TOO_LARGE: &str = "stitch-frame-too-large";
/// CoolProp rejected a fluid, a mixture or a computation; the engine
/// message travels verbatim in `args.detail`.
pub const COOLPROP_ERROR: &str = "coolprop-error";
/// More cycle points than one request accepts (`args.max`).
pub const THERMO_TOO_MANY_POINTS: &str = "thermo-too-many-points";
pub const LLM_PROVIDER_NOT_FOUND: &str = "llm-provider-not-found";
pub const LLM_PROVIDER_NAME_TAKEN: &str = "llm-provider-name-taken";
/// Provider test: the provider has no API key set.
pub const LLM_PROVIDER_NO_KEY: &str = "llm-provider-no-key";
/// Provider test: its API key could not be read from the vault.
pub const LLM_PROVIDER_KEY_UNREADABLE: &str = "llm-provider-key-unreadable";
// Geo providers (geo-layers.md §8, phase F).
pub const GEO_PROVIDER_NOT_FOUND: &str = "geo-provider-not-found";
pub const GEO_PROVIDER_NAME_TAKEN: &str = "geo-provider-name-taken";
/// No default provider for the capability (args: `capability`).
pub const GEO_NOT_CONFIGURED: &str = "geo-not-configured";
/// The provider's rate limit is reached (args: `retry_after_s`).
pub const GEO_RATE_LIMITED: &str = "geo-rate-limited";
/// The provider failed or answered unexpectedly (detail verbatim).
pub const GEO_PROVIDER_FAILED: &str = "geo-provider-failed";
pub const NOTIFY_CHANNEL_NAME_CONFLICT: &str = "notify-channel-name-conflict";
pub const NOTIFY_TEMPLATE_NAME_CONFLICT: &str = "notify-template-name-conflict";
pub const NOTIFY_TEST_BODY_INVALID: &str = "notify-test-body-invalid";
/// Template render failure (preview, channel test, save-time syntax check);
/// the minijinja detail travels verbatim in `args.detail`.
pub const NOTIFY_TEMPLATE_RENDER: &str = "notify-template-render";
/// The channel driver refused the configuration; its English diagnostic
/// travels verbatim in `args.detail`.
pub const NOTIFY_CHANNEL_CONFIG_INVALID: &str = "notify-channel-config-invalid";
/// A required configuration field of the channel kind is missing
/// (`args.field` = field id of the `field_spec`).
pub const NOTIFY_CHANNEL_FIELD_REQUIRED: &str = "notify-channel-field-required";
/// A test send failed upstream (`args.detail` = verbatim transport error).
pub const NOTIFY_TEST_FAILED: &str = "notify-test-failed";
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
/// The subscription tier asked for an org does not exist.
pub const TIER_UNKNOWN: &str = "tier-unknown";
/// OpenObserve is not configured on this server (no `settings.openobserve`).
pub const O2_NOT_CONFIGURED: &str = "o2-not-configured";
/// OpenObserve rejected or failed a data deletion.
pub const O2_DELETE_FAILED: &str = "o2-delete-failed";
/// OpenObserve does not support time-range deletion on this stream type.
pub const O2_TIME_RANGE_UNSUPPORTED: &str = "o2-time-range-unsupported";
/// Invalid time range (start must be before end).
pub const O2_TIME_RANGE_INVALID: &str = "o2-time-range-invalid";
/// The typed confirmation does not match the organization name.
pub const CONFIRMATION_MISMATCH: &str = "confirmation-mismatch";

// ── Camera & video (camera-video.md D73–D80) ─────────────────────────────

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
/// The shared memory store (Valkey) is unreachable or disabled.
pub const MEMORY_UNAVAILABLE: &str = "memory-unavailable";
/// The notification journal (OpenObserve, D86) could not be read.
pub const NOTIFY_JOURNAL_UNAVAILABLE: &str = "notify-journal-unavailable";
/// An event stream name is invalid (`ev_[a-z0-9_]+`).
pub const EVENT_STREAM_INVALID: &str = "event-stream-invalid";
/// No vision model with this id in the org.
pub const ML_MODEL_NOT_FOUND: &str = "ml-model-not-found";
/// The model's ONNX file (media version) is missing.
pub const ML_MODEL_MISSING_BYTES: &str = "ml-model-missing-bytes";
/// The ONNX model could not be loaded (unsupported ops, wrong input size…).
pub const ML_MODEL_LOAD_FAILED: &str = "ml-model-load-failed";
/// Inference failed on the given image (undecodable image, bad output).
pub const ML_MODEL_INFERENCE_FAILED: &str = "ml-model-inference-failed";
/// The model refuses the spec or fails its test inference (D100) — args `{detail}`.
pub const ML_MODEL_INVALID: &str = "ml-model-invalid";

// ── Custom firmware (custom-firmware.md D87–D94) ─────────────────────────

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
/// A per-pod compute pool (CoolProp, vision inference, runtime checks,
/// large uploads) is saturated; the client should retry shortly.
pub const SERVER_BUSY: &str = "server-busy";

// ── Rate limiting ────────────────────────────────────────────────────────

/// Too many requests from this client address on a protected route
/// (`Retry-After` header carries the wait in seconds).
pub const RATE_LIMITED: &str = "rate-limited";

// ── Org controls (D125–D127, surfaces-controls.md) ──────────────────────

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
/// Not one of the caller's assistant conversations in this org.
pub const AI_CONVERSATION_NOT_FOUND: &str = "ai-conversation-not-found";
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
/// A one-time websocket ticket could not be issued (Valkey unreachable).
pub const WS_TICKET_UNAVAILABLE: &str = "ws-ticket-unavailable";
/// No device with this identifier in the org (`args.device_id`).
pub const DEVICE_NOT_FOUND: &str = "device-not-found";
/// The subscription tier allows no more devices of this type (`args.type`).
pub const DEVICE_QUOTA_REACHED: &str = "device-quota-reached";
/// Registering an identifier that is already active in the org.
pub const DEVICE_ALREADY_ACTIVE: &str = "device-already-active";
/// A soldered screen cannot be disabled (`args.name`).
pub const DEVICE_SCREEN_BUILTIN_LOCKED: &str = "device-screen-builtin-locked";
/// Screen kind not declared by the board or model (`args.kind`, `args.declared`).
pub const DEVICE_SCREEN_NOT_DECLARED: &str = "device-screen-not-declared";
/// Neither the board profile nor the model declares a screen.
pub const DEVICE_SCREEN_NONE_DECLARED: &str = "device-screen-none-declared";
/// OpenObserve not configured, failing or timed out for a telemetry read (503).
pub const TELEMETRY_UNAVAILABLE: &str = "telemetry-unavailable";
/// Too many series specs in one telemetry query (`args.max`).
pub const TELEMETRY_BATCH_TOO_LARGE: &str = "telemetry-batch-too-large";
/// Minimum interval between two builds of the org not reached.
pub const BUILD_INTERVAL_NOT_MET: &str = "build-interval-not-met";
/// The build job could not be queued.
pub const BUILD_SUBMIT_FAILED: &str = "build-submit-failed";
/// Function saved against a stale version.
pub const FUNCTION_VERSION_CONFLICT: &str = "function-version-conflict";

// ── Media ingest (media-ingest.md P2.13) ─────────────────────────────────

/// Media stream not found (or not in the org).
pub const MEDIA_STREAM_NOT_FOUND: &str = "media-stream-not-found";
/// Org already has the maximum number of media streams.
pub const MEDIA_STREAM_LIMIT: &str = "media-stream-limit";
/// Stream URL carries a credential; use the stream secret instead.
pub const MEDIA_URL_HAS_CREDENTIALS: &str = "media-url-has-credentials";
/// Stream refused or unreachable (no detail on purpose, D160).
pub const MEDIA_STREAM_UNREACHABLE: &str = "media-stream-unreachable";
/// Profile model is not a valid audio model.
pub const MEDIA_ASR_MODEL_INVALID: &str = "media-asr-model-invalid";
/// Capture carrier not available yet (worker/device: later lots).
pub const MEDIA_CAPTURE_UNSUPPORTED: &str = "media-capture-unsupported";
/// Deploy refused: a `media-source` lists a slug that is not a stream of
/// the org; `args.stream` = the slug.
pub const MEDIA_STREAM_UNKNOWN: &str = "media-stream-unknown";
/// Deploy refused: a `camera-source` reads a stream that is not a stream
/// of the org with a video track (D175); `args.stream` = the slug.
pub const CAMERA_STREAM_UNKNOWN: &str = "camera-stream-unknown";
/// ASR profile not found (or not in the org).
pub const ASR_PROFILE_NOT_FOUND: &str = "asr-profile-not-found";
/// An ASR profile of the org already has this name.
pub const ASR_PROFILE_NAME_TAKEN: &str = "asr-profile-name-taken";

/// Audio model not found (or not in the org).
pub const ASR_MODEL_NOT_FOUND: &str = "asr-model-not-found";
/// A model of the org already has this name.
pub const ASR_MODEL_NAME_TAKEN: &str = "asr-model-name-taken";
/// The ASR model is still used by profiles (`args.profiles`, comma-joined
/// names); `?force=true` deletes the model and those profiles.
pub const ASR_MODEL_IN_USE: &str = "asr-model-in-use";
/// The file is not a supported audio model.
pub const ASR_MODEL_UNSUPPORTED: &str = "asr-model-unsupported";
/// The model's runtime (e.g. whisper.cpp) is not compiled into this
/// server's `pnex-asr` build. Args: `family`.
pub const ASR_RUNTIME_UNAVAILABLE: &str = "asr-runtime-unavailable";
/// The audio dropped on a model test cannot be read.
pub const ASR_TEST_AUDIO_UNSUPPORTED: &str = "asr-test-audio-unsupported";
/// The model test could not transcribe (runtime missing or crashed).
pub const ASR_TEST_FAILED: &str = "asr-test-failed";
/// A voice profile attaches to a Person object only.
pub const VOICE_ID_NOT_PERSON: &str = "voice-id-not-person";
/// This person already has a voice profile.
pub const VOICE_ID_PROFILE_EXISTS: &str = "voice-id-profile-exists";
/// Voice profile not found.
pub const VOICE_ID_PROFILE_NOT_FOUND: &str = "voice-id-profile-not-found";
/// Voice sample not found.
pub const VOICE_ID_SAMPLE_NOT_FOUND: &str = "voice-id-sample-not-found";
/// No valid speaker embedding model in the organization.
pub const VOICE_ID_NO_MODEL: &str = "voice-id-no-model";
/// The audio file cannot be read.
pub const VOICE_ID_AUDIO_UNSUPPORTED: &str = "voice-id-audio-unsupported";
/// Not enough speech to build or test a profile.
pub const VOICE_ID_NOT_ENOUGH_SPEECH: &str = "voice-id-not-enough-speech";
/// No generated voice profile to compare with.
pub const VOICE_ID_NOT_READY: &str = "voice-id-not-ready";
/// The speaker embedding could not be computed.
pub const VOICE_ID_FAILED: &str = "voice-id-failed";
/// Transcription search failed on OpenObserve.
pub const MEDIA_TRANSCRIPTS_UNAVAILABLE: &str = "media-transcripts-unavailable";
/// Taxonomy not found (or not in the org).
pub const TAXONOMY_NOT_FOUND: &str = "taxonomy-not-found";
/// A taxonomy of the org already has this name.
pub const TAXONOMY_NAME_TAKEN: &str = "taxonomy-name-taken";
/// New version written on a stale `expected_version`; `args.current` = the
/// current version.
pub const TAXONOMY_VERSION_CONFLICT: &str = "taxonomy-version-conflict";
/// Deploy refused: a `topic_classify` pins a taxonomy version that is not
/// one of the org; `args.version` = the pinned version.
pub const TAXONOMY_UNKNOWN: &str = "taxonomy-unknown";
/// Time range not found (or not in the org).
pub const TIME_RANGE_NOT_FOUND: &str = "time-range-not-found";
/// Import body over 1 MiB or over 5000 rows; `args.max_kib`, `args.max_rows`.
pub const TIME_RANGE_IMPORT_TOO_LARGE: &str = "time-range-import-too-large";
/// Import body is not a readable CSV (header with `label`) or iCalendar file.
pub const TIME_RANGE_IMPORT_INVALID: &str = "time-range-import-invalid";
/// Deploy refused: a `range_upsert` targets a stream that is not one of the
/// org (also the internal endpoint's answer for a foreign scope).
pub const TIME_RANGE_SCOPE_UNKNOWN: &str = "time-range-scope-unknown";

// ── Ontology (ontology.md D176–D191) ─────────────────────────────────────

/// Object, type or link not found in the org ontology.
pub const ONTOLOGY_NOT_FOUND: &str = "ontology-not-found";
/// A type, link type or open link with this key already exists.
pub const ONTOLOGY_ALREADY_EXISTS: &str = "ontology-already-exists";
/// Optimistic concurrency: written over a stale version; `args.current`.
pub const ONTOLOGY_VERSION_CONFLICT: &str = "ontology-version-conflict";
/// The type still has live objects, references or links.
pub const ONTOLOGY_TYPE_IN_USE: &str = "ontology-type-in-use";
/// The caller's role cannot write objects of this type (D188) or is a viewer.
pub const ONTOLOGY_WRITE_FORBIDDEN: &str = "ontology-write-forbidden";
/// System types and objects are edited from their own pages.
pub const ONTOLOGY_SYSTEM_READ_ONLY: &str = "ontology-system-read-only";
/// The link type does not connect objects of these types.
pub const ONTOLOGY_LINK_NOT_ALLOWED: &str = "ontology-link-not-allowed";
/// An open link already takes this place (one-to-one, or a series already bound).
pub const ONTOLOGY_LINK_CARDINALITY: &str = "ontology-link-cardinality";
/// A claim is corroborated only with two pieces of distinct origins (cases.md K7).
pub const ONTOLOGY_CLAIM_UNSOURCED: &str = "ontology-claim-unsourced";
/// No deployed flow holds an `action-trigger` for this action (D183).
pub const ONTOLOGY_ACTION_NO_FLOW: &str = "ontology-action-no-flow";
/// The action bus (Valkey) is unavailable: the action was not sent.
pub const ONTOLOGY_ACTION_UNAVAILABLE: &str = "ontology-action-unavailable";
/// The change journal (OpenObserve) of an object cannot be read (503).
pub const ONTOLOGY_CHANGES_UNAVAILABLE: &str = "ontology-changes-unavailable";
/// An `action-trigger` names a type or action absent from the org schema;
/// `args.object_type`, `args.action`.
pub const ONTOLOGY_ACTION_UNKNOWN: &str = "ontology-action-unknown";
/// Two media versions compared by text (D311) when one is not a document\nwhose text can be read.
pub const MEDIA_DIFF_UNSUPPORTED: &str = "media-diff-unsupported";
/// A proof waits for a browser agent of the org to be online (D308).
pub const EXT_NO_AGENT: &str = "ext-no-agent";
/// The site refused the crawler (401, 403 or 429): the crawl stopped (D307).
pub const CRAWL_BLOCKED: &str = "crawl-blocked";
/// The site's robots.txt forbids the crawl (D307): nothing is read.
pub const CRAWL_ROBOTS_DISALLOWED: &str = "crawl-robots-disallowed";
/// A server crawl could not reach the site or read its robots.txt (D307).
pub const CRAWL_UNREACHABLE: &str = "crawl-unreachable";
/// A notify node reading its recipient from the message targets a channel\nthat is not SMTP with `dynamic_recipients` (D313); refused at deploy.
pub const NOTIFY_DYNAMIC_RECIPIENTS_OFF: &str = "notify-dynamic-recipients-off";
/// Only an org owner or admin lets an SMTP channel send to recipients taken\nfrom flow messages (D313).
pub const NOTIFY_DYNAMIC_RECIPIENTS_ADMIN: &str = "notify-dynamic-recipients-admin";
/// An ontology flow node names a type, key or link type absent from the org\nschema (D312), refused at deploy.
pub const ONTOLOGY_NODE_UNKNOWN: &str = "ontology-node-unknown";
/// The preview was already applied, or the import or the data changed since.
pub const IMPORT_RUN_STALE: &str = "import-run-stale";
/// A flow import run whose server stopped working on it (no heartbeat for
/// 10 minutes): journaled as failed, nothing of it was written.
pub const IMPORT_RUN_INTERRUPTED: &str = "import-run-interrupted";
/// The server is stopping: it starts no import run (try again).
pub const IMPORT_SERVER_STOPPING: &str = "import-server-stopping";
/// An import preview with row errors cannot be applied (D301).
pub const IMPORT_HAS_ERRORS: &str = "import-has-errors";
/// A snapshot run that lost over 30 % of the records of the previous one:\nrefused without closing anything (D303); `args.previous`, `args.rows`.
pub const IMPORT_SUSPICIOUS_DROP: &str = "import-suspicious-drop";
/// A scheduled run whose records lack columns of the mapping, or hold no\nrecord while the previous run had some: refused without writing (D303);\n`args.columns`.
pub const IMPORT_SCHEMA_CHANGED: &str = "import-schema-changed";
/// An import URL that cannot be fetched (refused address, network, HTTP error).
pub const IMPORT_SOURCE_UNREACHABLE: &str = "import-source-unreachable";
/// An import source over its caps: 200 MB decompressed, 50,000 records (D301).
pub const IMPORT_SOURCE_TOO_LARGE: &str = "import-source-too-large";
/// An import source that cannot be read in its format (D301).
pub const IMPORT_SOURCE_UNREADABLE: &str = "import-source-unreadable";
/// The org's staging area is full (2 GB, D318): nothing was downloaded.
pub const INGEST_STAGING_QUOTA: &str = "ingest-staging-quota";
/// The object storage holding the staging area is not reachable (D318).
pub const INGEST_STAGING_UNAVAILABLE: &str = "ingest-staging-unavailable";
/// No data.gouv.fr resource matches the pattern (D317); `args.pattern`.
pub const INGEST_NO_RESOURCE: &str = "ingest-no-resource";
/// No file to read: no extracted entry matches, or no earlier step staged
/// one (D319); `args.entry`.
pub const INGEST_ENTRY_MISSING: &str = "ingest-entry-missing";
/// An archive format that is not read (zip, tar, tar.gz and gz are; D319).
pub const INGEST_ARCHIVE_UNSUPPORTED: &str = "ingest-archive-unsupported";
/// An archive entry whose path leaves its folder (`..`, absolute): refused.
pub const INGEST_UNSAFE_PATH: &str = "ingest-unsafe-path";
/// An import run would close more than the allowed share of what the import
/// holds (D322); `args.closed`, `args.held`, `args.max_pct`.
pub const INGEST_SUSPICIOUS_CLOSURES: &str = "ingest-suspicious-closures";

// ── Site navigation rules (site-recipes.md D323–D326) ───────────────────

/// A site recipe is not valid YAML or breaks a rule; `errors.violations`
/// lists `{path, token}` (`steps.3` + `one_of`…).
pub const SITE_RECIPE_INVALID: &str = "site-recipe-invalid";
/// Site recipe not found (or not in the org).
pub const SITE_RECIPE_NOT_FOUND: &str = "site-recipe-not-found";
/// The recipe was saved by someone else since it was read.
pub const SITE_RECIPE_VERSION_CONFLICT: &str = "site-recipe-version-conflict";
/// Site recipe run not found (or not in the org).
pub const SITE_RECIPE_RUN_NOT_FOUND: &str = "site-recipe-run-not-found";
/// A run is missing a value of the recipe's `params`; `args.param`.
pub const SITE_RECIPE_PARAM_MISSING: &str = "site-recipe-param-missing";
/// A flow node could not have its rule played (server unreachable, no
/// object storage, run not journaled).
pub const SITE_RECIPE_UNAVAILABLE: &str = "site-recipe-unavailable";
/// Browser endpoint not found (or not in the org).
pub const BROWSER_ENDPOINT_NOT_FOUND: &str = "browser-endpoint-not-found";
/// A browser endpoint of the org already has this name.
pub const BROWSER_ENDPOINT_NAME_TAKEN: &str = "browser-endpoint-name-taken";
/// Web search provider not found (or not in the org).
pub const WEB_SEARCH_PROVIDER_NOT_FOUND: &str = "web-search-provider-not-found";
/// A web search provider of the org already has this name.
pub const WEB_SEARCH_PROVIDER_NAME_TAKEN: &str = "web-search-provider-name-taken";
/// A `web_search` node found no provider (named one missing, or no
/// default); `args.provider` when one was named.
pub const WEB_SEARCH_NO_PROVIDER: &str = "web-search-no-provider";
/// The provider refused or failed the query; `args.status` when it answered.
pub const WEB_SEARCH_UNAVAILABLE: &str = "web-search-unavailable";
/// The org passed its daily number of web searches; `args.cap`.
pub const WEB_SEARCH_QUOTA: &str = "web-search-quota";
/// The query is empty or longer than 400 characters.
pub const WEB_SEARCH_QUERY_INVALID: &str = "web-search-query-invalid";
/// The company register did not answer an `org_resolve` lookup.
pub const ORG_RESOLVE_UNAVAILABLE: &str = "org-resolve-unavailable";
/// The org passed its daily number of company lookups; `args.cap`.
pub const ORG_RESOLVE_QUOTA: &str = "org-resolve-quota";
/// The company name is empty or longer than 255 characters.
pub const ORG_RESOLVE_QUERY_INVALID: &str = "org-resolve-query-invalid";
/// A `web_visit` URL that is not a public http(s) address.
pub const WEB_VISIT_URL_INVALID: &str = "web-visit-url-invalid";
/// The candidate site did not answer, or not with a page.
pub const WEB_VISIT_UNREACHABLE: &str = "web-visit-unreachable";
/// The site's `robots.txt` forbids reading the page.
pub const WEB_VISIT_BLOCKED: &str = "web-visit-blocked";
/// The org passed its daily number of visits; `args.cap`.
pub const WEB_VISIT_QUOTA: &str = "web-visit-quota";
/// A `site_crawl` address that is not an http(s) site (network-check.md D407).
pub const SITE_CRAWL_URL_INVALID: &str = "site-crawl-url-invalid";
/// The crawl the message describes is refused (too many rules, a rule too
/// long, a bad regular expression); `args.field`.
pub const SITE_CRAWL_INVALID: &str = "site-crawl-invalid";
/// A proposal already decided the other way, or superseded (proposals.md D373).
pub const PROPOSAL_DECIDED: &str = "proposal-decided";
/// Making a property unique (D302) on a type whose objects already share\nvalues; `args.property`, `args.values` (first duplicates, comma-separated).
pub const ONTOLOGY_UNIQUE_DUPLICATES: &str = "ontology-unique-duplicates";
/// A person is attached to a case only once its processing basis is declared
/// (cases-workspace.md K38).
pub const CASE_LEGAL_BASIS_REQUIRED: &str = "case-legal-basis-required";
/// A proposal already reviewed the other way: a rejected one is not retained,
/// a retained one is not rejected (cases-workspace.md K30).
pub const CASE_PROPOSAL_STALE: &str = "case-proposal-stale";
/// The read log of a case (OpenObserve) cannot be read (503).
pub const CASE_READS_UNAVAILABLE: &str = "case-reads-unavailable";
/// A track file whose times carry no offset needs the recording's time zone.
pub const TRACK_IMPORT_TIMEZONE_REQUIRED: &str = "track-import-timezone-required";
/// A track file without a single usable point.
pub const TRACK_IMPORT_EMPTY: &str = "track-import-empty";
/// Not a GPX document, or a CSV without time, latitude and longitude columns.
pub const TRACK_IMPORT_UNREADABLE: &str = "track-import-unreadable";

// ── Pages (pages.md #21) ─────────────────────────────────────────────────

/// Only the author edits a comment; the author or an admin deletes it.
pub const DOC_COMMENT_NOT_AUTHOR: &str = "doc-comment-not-author";

// ── Vision Lab (media-vision-studio.md D198–D200) ─────────────────────

pub const LAB_PROJECT_NOT_FOUND: &str = "lab-project-not-found";
pub const LAB_IMAGE_NOT_FOUND: &str = "lab-image-not-found";
pub const LAB_VERSION_NOT_FOUND: &str = "lab-version-not-found";
pub const LAB_CLASS_IN_USE: &str = "lab-class-in-use";
pub const LAB_ANNOTATIONS_CONFLICT: &str = "lab-annotations-conflict";
pub const LAB_BOX_INVALID: &str = "lab-box-invalid";
pub const LAB_BOXES_TOO_MANY: &str = "lab-boxes-too-many";
pub const LAB_NEGATIVE_WITH_BOXES: &str = "lab-negative-with-boxes";
pub const LAB_ASSET_NOT_IMAGE: &str = "lab-asset-not-image";
pub const LAB_DATASET_EMPTY: &str = "lab-dataset-empty";
pub const LAB_EXPORT_TOO_LARGE: &str = "lab-export-too-large";
/// Video import: not an MP4 / MOV with an H.264, H.265 or MJPEG track.
pub const LAB_VIDEO_UNSUPPORTED: &str = "lab-video-unsupported";
pub const LAB_VIDEO_TOO_LARGE: &str = "lab-video-too-large";
/// Video import: the confined decoder failed, stalled or is missing.
pub const LAB_VIDEO_DECODE_FAILED: &str = "lab-video-decode-failed";
/// Interpolation needs an annotated frame before and after in the source.
pub const LAB_INTERPOLATE_NO_KEYFRAMES: &str = "lab-interpolate-no-keyframes";
/// Training (D204): a run of the project is already queued or running.
pub const LAB_TRAIN_BUSY: &str = "lab-train-busy";
/// Training: the base model is not a YOLOX head this trainer can tune
/// (`args.detail` = verbatim diagnostic).
pub const LAB_TRAIN_BASE_INVALID: &str = "lab-train-base-invalid";
/// Training: no box in the training pictures of the version.
pub const LAB_TRAIN_NO_DATA: &str = "lab-train-no-data";
/// Training: the runner stopped (process restarted, pod gone).
pub const LAB_TRAIN_INTERRUPTED: &str = "lab-train-interrupted";
/// Training failed (`args.detail` = verbatim diagnostic).
pub const LAB_TRAIN_FAILED: &str = "lab-train-failed";
pub const LAB_RUN_NOT_FOUND: &str = "lab-run-not-found";
/// Propagation needs an annotated frame before this one in the source.
pub const LAB_PROPAGATE_NO_PREVIOUS: &str = "lab-propagate-no-previous";
/// The review queue of a project holds its maximum of pending pictures.
pub const LAB_REVIEW_FULL: &str = "lab-review-full";

// ── Body-level codes of the shared error shapes (D239) ───────────────────

/// One or more fields are invalid: `errors.fields = {"<field>": "<token>"}`.
pub const FIELD_INVALID_BODY: &str = "field-invalid";
/// The request could not be read (malformed JSON, unknown enum value, bad
/// query or path); `description` carries the parser message.
pub const REQUEST_INVALID: &str = "request-invalid";
/// Flow graph violations: `errors.violations = [...]`.
pub const FLOW_INVALID: &str = "flow-invalid";
/// Dashboard layout violations.
pub const DASHBOARD_INVALID: &str = "dashboard-invalid";
/// Dashboard widget violations.
pub const WIDGET_INVALID: &str = "widget-invalid";
/// Tour document violations.
pub const TOUR_INVALID: &str = "tour-invalid";
/// Annotation layer document violations.
pub const ANNOTATION_LAYER_INVALID: &str = "annotation-layer-invalid";
/// Fluid mixture violations.
pub const MIXTURE_INVALID: &str = "mixture-invalid";

// ── Browser agents (extension-collector.md D254–D269) ───────────────────

/// Browser agent not found (or not in the org).
pub const EXTENSION_AGENT_NOT_FOUND: &str = "extension-agent-not-found";
/// Browser agent mission not found (or not in the org).
pub const EXTENSION_MISSION_NOT_FOUND: &str = "extension-mission-not-found";
/// Mission changed meanwhile (stale `expected_version`); `args.current` = the
/// current version.
pub const EXTENSION_MISSION_VERSION_CONFLICT: &str = "extension-mission-version-conflict";
/// Pairing refused: malformed install key or bad signature (D266).
pub const EXTENSION_PAIR_REFUSED: &str = "extension-pair-refused";
/// This browser install is paired with another organisation: remove the
/// agent there first.
pub const EXTENSION_INSTALL_PAIRED_ELSEWHERE: &str = "extension-install-paired-elsewhere";
/// A browser agent is created by pairing from the extension, never from
/// the device wizard.
pub const EXTENSION_AGENT_PAIR_ONLY: &str = "extension-agent-pair-only";
/// Websocket ticket refused: unknown agent token, bad signature or clock
/// skew.
pub const EXTENSION_TICKET_REFUSED: &str = "extension-ticket-refused";
/// Websocket tickets unavailable (Valkey unreachable).
pub const EXTENSION_TICKET_UNAVAILABLE: &str = "extension-ticket-unavailable";
/// Video not in the registry of this mission, or already captured.
pub const EXTENSION_VIDEO_NOT_FOUND: &str = "extension-video-not-found";
/// Capture upload refused: unknown agent token, bad or reused signature,
/// body not matching its signature.
pub const EXTENSION_UPLOAD_REFUSED: &str = "extension-upload-refused";
/// The org holds as many browser agents (pairing) or missions (creation)
/// as allowed; args: `max`.
pub const EXTENSION_LIMIT_REACHED: &str = "extension-limit-reached";
/// This server ships no browser extension build (`PNEX_EXTENSION_DIST_DIR`).
pub const EXTENSION_DIST_NOT_FOUND: &str = "extension-dist-not-found";
/// Agent state: the selector matched nothing on the page.
pub const EXT_SELECTOR_NOT_FOUND: &str = "ext-selector-not-found";
/// Agent state: the value read is not a number.
pub const EXT_VALUE_NOT_NUMERIC: &str = "ext-value-not-numeric";
/// Agent state: the site asks to sign in (D258).
pub const EXT_LOGIN_REQUIRED: &str = "ext-login-required";
/// Agent state: the site shows a challenge (captcha); never bypassed (D258).
pub const EXT_CHALLENGE: &str = "ext-challenge";
/// Agent state: the site says the pace is too high (D258).
pub const EXT_RATE_LIMITED: &str = "ext-rate-limited";
/// Agent state: the agent's tab is hidden, the mission is paused (D269).
pub const EXT_TAB_HIDDEN: &str = "ext-tab-hidden";
/// Agent state: the video is protected (EME/DRM); never recorded (D260).
pub const EXT_DRM_PROTECTED: &str = "ext-drm-protected";
/// Agent state: the org's video registry is full, new posts are not kept.
pub const EXT_REGISTRY_FULL: &str = "ext-registry-full";
/// Agent state: the site is not allowed yet in the extension (optional host
/// permission, granted by a click in its options page).
pub const EXT_PERMISSION_MISSING: &str = "ext-permission-missing";
/// The address is not one person's profile page on a known social network
/// (search, relations, "people also viewed"…): never captured (D333).
pub const EXT_TARGET_NOT_PROFILE: &str = "ext-target-not-profile";
/// The org captured as many profiles of this network today as allowed
/// (D333); args: `max`.
pub const EXT_PROFILE_DAILY_CAP: &str = "ext-profile-daily-cap";
/// The person objected to prospecting: no capture, no re-creation (D341).
pub const PROSPECTION_OPPOSED: &str = "prospection-opposed";

// ── Registered codes ─────────────────────────────────────────────────────

/// All registered codes. A code not listed here falls back to the verbatim
/// `description`/`message` on the frontend (never panics).
pub const ALL: &[&str] = &[
    FIELD_INVALID_BODY,
    REQUEST_INVALID,
    FLOW_INVALID,
    DASHBOARD_INVALID,
    WIDGET_INVALID,
    TOUR_INVALID,
    ANNOTATION_LAYER_INVALID,
    MIXTURE_INVALID,
    ONTOLOGY_NOT_FOUND,
    ONTOLOGY_ALREADY_EXISTS,
    ONTOLOGY_VERSION_CONFLICT,
    ONTOLOGY_TYPE_IN_USE,
    ONTOLOGY_WRITE_FORBIDDEN,
    ONTOLOGY_SYSTEM_READ_ONLY,
    ONTOLOGY_LINK_NOT_ALLOWED,
    ONTOLOGY_LINK_CARDINALITY,
    ONTOLOGY_CLAIM_UNSOURCED,
    ONTOLOGY_ACTION_NO_FLOW,
    ONTOLOGY_ACTION_UNAVAILABLE,
    ONTOLOGY_CHANGES_UNAVAILABLE,
    ONTOLOGY_ACTION_UNKNOWN,
    MEDIA_DIFF_UNSUPPORTED,
    EXT_NO_AGENT,
    EXT_TARGET_NOT_PROFILE,
    EXT_PROFILE_DAILY_CAP,
    PROSPECTION_OPPOSED,
    CRAWL_BLOCKED,
    CRAWL_ROBOTS_DISALLOWED,
    CRAWL_UNREACHABLE,
    NOTIFY_DYNAMIC_RECIPIENTS_OFF,
    NOTIFY_DYNAMIC_RECIPIENTS_ADMIN,
    ONTOLOGY_NODE_UNKNOWN,
    IMPORT_RUN_STALE,
    IMPORT_RUN_INTERRUPTED,
    IMPORT_SERVER_STOPPING,
    IMPORT_HAS_ERRORS,
    IMPORT_SUSPICIOUS_DROP,
    IMPORT_SCHEMA_CHANGED,
    IMPORT_SOURCE_UNREACHABLE,
    IMPORT_SOURCE_TOO_LARGE,
    IMPORT_SOURCE_UNREADABLE,
    INGEST_STAGING_QUOTA,
    INGEST_STAGING_UNAVAILABLE,
    INGEST_NO_RESOURCE,
    INGEST_ENTRY_MISSING,
    INGEST_ARCHIVE_UNSUPPORTED,
    INGEST_UNSAFE_PATH,
    INGEST_SUSPICIOUS_CLOSURES,
    SITE_RECIPE_INVALID,
    SITE_RECIPE_NOT_FOUND,
    SITE_RECIPE_VERSION_CONFLICT,
    SITE_RECIPE_RUN_NOT_FOUND,
    SITE_RECIPE_PARAM_MISSING,
    SITE_RECIPE_UNAVAILABLE,
    BROWSER_ENDPOINT_NOT_FOUND,
    BROWSER_ENDPOINT_NAME_TAKEN,
    WEB_SEARCH_PROVIDER_NOT_FOUND,
    WEB_SEARCH_PROVIDER_NAME_TAKEN,
    WEB_SEARCH_NO_PROVIDER,
    WEB_SEARCH_UNAVAILABLE,
    WEB_SEARCH_QUOTA,
    WEB_SEARCH_QUERY_INVALID,
    ORG_RESOLVE_UNAVAILABLE,
    ORG_RESOLVE_QUOTA,
    ORG_RESOLVE_QUERY_INVALID,
    WEB_VISIT_URL_INVALID,
    WEB_VISIT_UNREACHABLE,
    WEB_VISIT_BLOCKED,
    WEB_VISIT_QUOTA,
    SITE_CRAWL_URL_INVALID,
    SITE_CRAWL_INVALID,
    PROPOSAL_DECIDED,
    ONTOLOGY_UNIQUE_DUPLICATES,
    CASE_LEGAL_BASIS_REQUIRED,
    CASE_PROPOSAL_STALE,
    CASE_READS_UNAVAILABLE,
    TRACK_IMPORT_TIMEZONE_REQUIRED,
    TRACK_IMPORT_EMPTY,
    TRACK_IMPORT_UNREADABLE,
    WS_TICKET_UNAVAILABLE,
    DEVICE_NOT_FOUND,
    DEVICE_QUOTA_REACHED,
    DEVICE_ALREADY_ACTIVE,
    DEVICE_SCREEN_BUILTIN_LOCKED,
    DEVICE_SCREEN_NOT_DECLARED,
    DEVICE_SCREEN_NONE_DECLARED,
    TELEMETRY_UNAVAILABLE,
    TELEMETRY_BATCH_TOO_LARGE,
    BUILD_INTERVAL_NOT_MET,
    BUILD_SUBMIT_FAILED,
    FUNCTION_VERSION_CONFLICT,
    AGENT_NOT_AN_AGENT,
    AGENT_UNSUPPORTED_ACTION,
    AGENT_QUOTA_INVALID,
    AGENT_ENROLL_CODE_INVALID,
    AGENT_ENROLL_RATE_LIMITED,
    AGENT_BINARY_NOT_FOUND,
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
    ORG_NOT_MEMBER,
    ACCESS_DENIED,
    ACCESS_KEY_NOT_ALLOWED,
    IAM_SERVICE_ACCOUNT_NOT_FOUND,
    IAM_SERVICE_ACCOUNT_NAME_TAKEN,
    IAM_ACCESS_KEY_NOT_FOUND,
    IAM_ACCESS_KEY_LIMIT,
    IAM_ROLE_NOT_FOUND,
    IAM_TEAM_NOT_FOUND,
    IAM_ASSIGNMENT_NOT_FOUND,
    IAM_ROLE_KEY_TAKEN,
    IAM_TEAM_NAME_TAKEN,
    IAM_ASSIGNMENT_DUPLICATE,
    IAM_VERSION_CONFLICT,
    IAM_ESCALATION,
    ORG_HEADER_REQUIRED,
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
    PROFILE_PATCH_EMPTY,
    POI_DEVICE_ALREADY_PLACED,
    POI_LINK_DUPLICATE,
    TOUR_VERSION_CONFLICT,
    TOUR_NOT_PUBLISHED,
    DASHBOARD_VERSION_CONFLICT,
    MEDIA_LAST_VERSION,
    MEDIA_FILE_TOO_LARGE,
    MEDIA_FORMAT_UNSUPPORTED,
    MEDIA_INDEX_UNSUPPORTED,
    MEDIA_INDEX_TOO_LARGE,
    MEDIA_INDEX_MALFORMED,
    MEDIA_INDEX_UNREADABLE,
    MEDIA_INDEX_NOT_A_DOCUMENT,
    MEDIA_ANALYSIS_NO_LLM,
    MEDIA_ANALYSIS_BUSY,
    MEDIA_ANALYSIS_STEP_INVALID,
    MEDIA_ANALYSIS_NO_TEXT,
    MEDIA_ANALYSIS_LLM_FAILED,
    MEDIA_ANALYSIS_QUOTA,
    MEDIA_ANALYSIS_INTERRUPTED,
    MEDIA_ANALYSIS_LARGE,
    MEDIA_ANALYSIS_NO_ASR,
    MEDIA_ANALYSIS_CAPTURE_FAILED,
    MEDIA_ANALYSIS_TRANSCRIBE_FAILED,
    MEDIA_ATTRIBUTION_CONFLICT,
    MEDIA_ANALYSIS_NO_VOICE_PROFILE,
    MEDIA_ANALYSIS_NO_SPEAKERS,
    MEDIA_ANALYSIS_IDENTIFY_FAILED,
    MEDIA_ANALYSIS_NO_DETECTOR,
    MEDIA_ANALYSIS_DETECT_FAILED,
    FUNCTIONS_RUNTIME_DISABLED,
    FUNCTION_IN_USE,
    FUNCTION_DIRECTIVE_INVALID,
    ANNOT_VERSION_CONFLICT,
    DEVICE_BOARD_UNKNOWN,
    DEVICE_BOARD_SOC_MISMATCH,
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
    FLOW_DEBUG_DISABLED,
    FLOW_STALE_VERSION,
    FLOW_STOP_NOT_DEPLOYED,
    FLOW_START_NOT_STOPPED,
    FLOW_NO_DEPLOYED_VERSION,
    FLOW_DEVICE_WRITE_VALUES_EMPTY,
    FLOW_RUNTIME,
    RESOURCE_EDGE_DUPLICATE,
    RESOURCE_KIND_UNKNOWN,
    RESOURCE_CYCLE,
    EDGE_HOST_LOCKED,
    EDGE_SSID_TAKEN,
    EDGE_HOST_TAKEN,
    OTA_IN_PROGRESS,
    OTA_NOT_READY,
    OTA_NO_ARTIFACT,
    OTA_TOO_LARGE,
    OTA_SAME_VERSION,
    OTA_VERSION_NOT_FOUND,
    OTA_NO_ACTIVE,
    OTA_AUTH_FAILED,
    OTA_ARTIFACT_NOT_FOUND,
    SECRET_NOT_FOUND,
    SECRET_IN_USE,
    SECRET_NOT_REFERENCED,
    SECRET_NAME_TAKEN,
    SECRET_WRITE_FORBIDDEN,
    SECRET_DESTINATION_LOCKED,
    BUILD_IN_PROGRESS,
    STITCH_JOB_TERMINAL,
    STITCH_STATE_UNKNOWN,
    STITCH_DISABLED,
    STITCH_FRAME_TOO_LARGE,
    COOLPROP_ERROR,
    THERMO_TOO_MANY_POINTS,
    LLM_PROVIDER_NOT_FOUND,
    LLM_PROVIDER_NAME_TAKEN,
    LLM_PROVIDER_NO_KEY,
    LLM_PROVIDER_KEY_UNREADABLE,
    GEO_PROVIDER_NOT_FOUND,
    GEO_PROVIDER_NAME_TAKEN,
    GEO_NOT_CONFIGURED,
    GEO_RATE_LIMITED,
    GEO_PROVIDER_FAILED,
    NOTIFY_CHANNEL_NAME_CONFLICT,
    NOTIFY_TEMPLATE_NAME_CONFLICT,
    NOTIFY_TEST_BODY_INVALID,
    NOTIFY_TEMPLATE_RENDER,
    NOTIFY_CHANNEL_CONFIG_INVALID,
    NOTIFY_CHANNEL_FIELD_REQUIRED,
    NOTIFY_TEST_FAILED,
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
    TIER_UNKNOWN,
    O2_NOT_CONFIGURED,
    O2_DELETE_FAILED,
    O2_TIME_RANGE_UNSUPPORTED,
    O2_TIME_RANGE_INVALID,
    CONFIRMATION_MISMATCH,
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
    MEMORY_UNAVAILABLE,
    NOTIFY_JOURNAL_UNAVAILABLE,
    EVENT_STREAM_INVALID,
    ML_MODEL_NOT_FOUND,
    ML_MODEL_MISSING_BYTES,
    ML_MODEL_LOAD_FAILED,
    ML_MODEL_INFERENCE_FAILED,
    ML_MODEL_INVALID,
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
    CONTROL_KEY_TAKEN,
    CONTROL_VALUE_INVALID,
    CONTROL_RATE_LIMITED,
    CONTROL_IN_USE,
    AI_FLOW_RUNNING,
    AI_CONVERSATION_BUSY,
    AI_CONVERSATION_NOT_FOUND,
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
    CONTROL_UNKNOWN,
    CONTROL_STORE_UNAVAILABLE,
    MEDIA_STREAM_NOT_FOUND,
    MEDIA_STREAM_LIMIT,
    MEDIA_URL_HAS_CREDENTIALS,
    MEDIA_STREAM_UNREACHABLE,
    MEDIA_ASR_MODEL_INVALID,
    MEDIA_CAPTURE_UNSUPPORTED,
    MEDIA_STREAM_UNKNOWN,
    CAMERA_STREAM_UNKNOWN,
    ASR_PROFILE_NOT_FOUND,
    ASR_PROFILE_NAME_TAKEN,
    ASR_MODEL_NOT_FOUND,
    ASR_MODEL_NAME_TAKEN,
    ASR_MODEL_IN_USE,
    ASR_MODEL_UNSUPPORTED,
    ASR_RUNTIME_UNAVAILABLE,
    ASR_TEST_AUDIO_UNSUPPORTED,
    ASR_TEST_FAILED,
    VOICE_ID_NOT_PERSON,
    VOICE_ID_PROFILE_EXISTS,
    VOICE_ID_PROFILE_NOT_FOUND,
    VOICE_ID_SAMPLE_NOT_FOUND,
    VOICE_ID_NO_MODEL,
    VOICE_ID_AUDIO_UNSUPPORTED,
    VOICE_ID_NOT_ENOUGH_SPEECH,
    VOICE_ID_NOT_READY,
    VOICE_ID_FAILED,
    MEDIA_TRANSCRIPTS_UNAVAILABLE,
    TAXONOMY_NOT_FOUND,
    TAXONOMY_NAME_TAKEN,
    TAXONOMY_VERSION_CONFLICT,
    EXTENSION_AGENT_NOT_FOUND,
    EXTENSION_MISSION_NOT_FOUND,
    EXTENSION_MISSION_VERSION_CONFLICT,
    EXTENSION_PAIR_REFUSED,
    EXTENSION_INSTALL_PAIRED_ELSEWHERE,
    EXTENSION_AGENT_PAIR_ONLY,
    EXTENSION_TICKET_REFUSED,
    EXTENSION_TICKET_UNAVAILABLE,
    EXTENSION_DIST_NOT_FOUND,
    EXTENSION_LIMIT_REACHED,
    EXTENSION_UPLOAD_REFUSED,
    EXTENSION_VIDEO_NOT_FOUND,
    EXT_SELECTOR_NOT_FOUND,
    EXT_VALUE_NOT_NUMERIC,
    EXT_LOGIN_REQUIRED,
    EXT_CHALLENGE,
    EXT_RATE_LIMITED,
    EXT_TAB_HIDDEN,
    EXT_PERMISSION_MISSING,
    EXT_DRM_PROTECTED,
    EXT_REGISTRY_FULL,
    TAXONOMY_UNKNOWN,
    TIME_RANGE_NOT_FOUND,
    LAB_PROJECT_NOT_FOUND,
    LAB_IMAGE_NOT_FOUND,
    LAB_VERSION_NOT_FOUND,
    LAB_CLASS_IN_USE,
    LAB_ANNOTATIONS_CONFLICT,
    LAB_BOX_INVALID,
    LAB_BOXES_TOO_MANY,
    LAB_NEGATIVE_WITH_BOXES,
    LAB_ASSET_NOT_IMAGE,
    LAB_DATASET_EMPTY,
    LAB_EXPORT_TOO_LARGE,
    LAB_VIDEO_UNSUPPORTED,
    LAB_VIDEO_TOO_LARGE,
    LAB_VIDEO_DECODE_FAILED,
    LAB_INTERPOLATE_NO_KEYFRAMES,
    LAB_TRAIN_BUSY,
    LAB_TRAIN_BASE_INVALID,
    LAB_TRAIN_NO_DATA,
    LAB_TRAIN_INTERRUPTED,
    LAB_TRAIN_FAILED,
    LAB_RUN_NOT_FOUND,
    LAB_PROPAGATE_NO_PREVIOUS,
    LAB_REVIEW_FULL,
    TIME_RANGE_IMPORT_TOO_LARGE,
    TIME_RANGE_IMPORT_INVALID,
    TIME_RANGE_SCOPE_UNKNOWN,
    // Pages
    DOC_COMMENT_NOT_AUTHOR,
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
/// `{"name": "unique"}`: the value is already used in the organisation.
pub const FIELD_UNIQUE: &str = "unique";
/// `{"wifi_credential_id": "not-found"}` — the referenced object does not
/// exist in the org.
pub const FIELD_NOT_FOUND: &str = "not-found";
