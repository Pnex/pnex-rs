# PNEX front — English (fallback).
# Toute clé ajoutée dans un autre locale doit exister ici (test de parité).

# Navigation
nav-dashboard = Dashboard
nav-devices = Devices
nav-catalog = Device Catalog
nav-orgs = Organizations
nav-profile = Profile
nav-visualisation = Visualization

# Générique
common-loading = Loading…
common-cancel = Cancel
common-confirm = Confirm
common-retry = Retry
common-save = Save Changes
common-search = Search…
common-error = Error
common-close = Close
common-refresh = Refresh

# Divers
login-tagline = Platform Nexus
login-description = Connecting thousands of IoT devices with seamless data ingestion
login-signin = Sign in to PNeX
login-register = Create account
login-reset = Forgot password?
login-footer = Secure authentication powered by PNeX Platform
callback-exchanging = Signing you in…
callback-failed = Sign-in failed
callback-back = Back to sign in
not-found = Page not found: { $path }
toast-session-expired = Your session has expired. Please sign in again.
shell-logout = Log out
shell-logout-confirm-title = Sign out?
shell-logout-confirm-message = Your session will be closed and you will need to sign in again.
shell-logout-confirm-action = Sign out
server-url-title = Server URL

# Organisations
orgs-title = Organizations
orgs-subtitle = Manage your organizations and their members
orgs-new-placeholder = New organization name…
orgs-create = Create
orgs-empty = No organization yet
orgs-col-name = Name
orgs-col-role = Your role
orgs-col-tier = Tier
common-actions = Actions
orgs-current = Active
orgs-select = Set active
orgs-manage = Manage
orgs-back = Back to organizations
orgs-rename = Rename
orgs-rename-placeholder = New name…
orgs-add-member = Add member
orgs-email-placeholder = member@example.com (already signed in once)
orgs-members = Members
orgs-remove-member = Remove member
orgs-confirm-delete-title = Delete this organization?
orgs-confirm-delete-message = This action is irreversible. The organization must have no other members, and its data will be deleted.
orgs-delete = Delete
role-owner = Owner
role-admin = Admin
role-viewer = Viewer
toast-saved = Changes saved

# Tableau de bord
dash-subtitle = Monitor your devices, organizations and subscription
dash-auto-refresh = Auto · 15 s
dash-total-devices = Total devices
dash-active-devices = Active devices
dash-orgs = Organizations
dash-tier = Current tier
dash-active-org = Active organization
dash-quotas = Tier capacities
dash-quota-sensor = Sensor devices
dash-quota-actuator = Actuator devices
dash-quota-mixed = Mixed devices
dash-by-type = Devices by type
dash-no-devices = No devices yet (Phase 4)
dash-live-sensors = Devices online
dash-build-success = Build success rate
dash-no-builds = No builds yet
dash-liveness = Device status
dash-last-measurements = Latest measurements
dash-no-measurements = No measurements yet
dash-telemetry-unavailable = Telemetry unavailable (OpenObserve not configured or unreachable)
dash-col-device = Device
dash-col-metric = Metric
dash-col-value = Value
dash-col-time = Time
dash-never = never

# Profil
profile-subtitle = Manage your account settings and preferences
profile-identity = Profile information
profile-username = Username
profile-email = Email
profile-idp-managed = Identity fields are managed by the authentication server (Rauthy).
profile-preferences = Preferences
profile-language = Language
profile-timezone = Timezone
profile-date-format = Date format
profile-theme = Theme
profile-theme-light = Light
profile-theme-dark = Dark
profile-theme-auto = Auto
profile-account = Account
profile-change-password = Change password
profile-theme-note = The theme is stored in your profile; its application to the interface comes with the dark mode (later phase).

# Pages en attente de phase
empty-phase = Comes in phase { $phase }

# Builds firmware (Phase 6)
builds-field-ssid = WiFi SSID
builds-field-wifi-password = WiFi password
builds-field-server = PNEX server (host)
devices-host-loopback-hint = The device cannot reach "localhost" — enter the server's LAN address (e.g. 192.168.1.16:5150).
builds-field-ws-ssl = WebSocket SSL (wss)
builds-field-ws-ssl-help = Checked for wss:// (TLS, industrial deployment) — unchecked for ws:// (local server / Raspberry Pi without TLS).
builds-submit = Build firmware
builds-launched = Build queued — follow it in the Firmware column of the devices list.
builds-phase-queued = Queued
builds-phase-running = Running
builds-phase-succeeded = Succeeded
builds-phase-failed = Failed
builds-download = Download
empty-catalog-title = Device catalog
empty-catalog-message = The catalog of predefined devices (boards, capabilities) lands with the devices API.
catalog-subtitle = Browse and discover devices available for your projects
catalog-search-placeholder = Search (name, description, board, capability…)
catalog-type-all = All device types
catalog-board-all = All boards
catalog-empty = No devices found
catalog-empty-hint = Try adjusting your search or filter criteria
catalog-no-image = No image available
catalog-rev = Rev.
catalog-capabilities = Capabilities
catalog-docs = Docs
catalog-buy = Buy
catalog-configure = Configure

# Devices (Phase 4)
devices-subtitle = Register your devices, inspect their provisioning tokens and metadata.
devices-type-all = All types
devices-type-sensor = Sensor
devices-type-actuator = Actuator
devices-type-mixed = Mixed
devices-capability-all = All capabilities
devices-status-all = All statuses
devices-status-active = Active
devices-status-inactive = Inactive
devices-last-seen-at = seen at
devices-last-seen-never = never seen
devices-search-placeholder = Search (id, model, type, capability…)
devices-new-placeholder = Device identifier (device_id)
devices-model-required = Pick a model from the catalog.
devices-id-required = Enter the device identifier (device_id).
devices-register-title = Register a new device
devices-register = Register
devices-created = Device registered — provisioning token generated.
devices-empty = No device matches — register one above.
devices-col-id = Device
devices-col-type = Type
devices-col-model = Model
devices-col-status = Status
devices-col-firmware = Firmware
devices-build-never = Never built
devices-flash = Flash
devices-flash-title = Flash the firmware from the browser (Web Serial — Chrome/Edge)
devices-rebuild = Rebuild
devices-rebuild-title = Rebuild firmware
devices-rebuild-incomplete = Fill in the WiFi SSID, the WiFi password and the server host.
devices-detail = Detail
devices-back = Back to devices
devices-delete = Delete
devices-confirm-delete-title = Delete this device?
devices-confirm-delete-message = The device, its token and its firmware build records will be permanently removed.
devices-capabilities = Capabilities
devices-token = Provisioning token
devices-token-active = Active
devices-token-show = Show token
devices-token-hide = Hide token
devices-token-value = Token (hand it to the firmware)
devices-encryption-key = Encryption key (ChaCha20)
devices-metadata = Metadata (JSON)
devices-metadata-save = Save metadata
devices-metadata-invalid = Invalid JSON

# Wizard d'enregistrement (Phase 6)
wizard-step-identity = Identifier
wizard-step-model = Model
wizard-step-wifi = WiFi
wizard-step-review = Review
wizard-identity-help = Give the device a unique firmware identifier (16 characters max) — optionally shuffle one and attach metadata.
wizard-shuffle = Shuffle
wizard-metadata-title = Metadata (optional)
wizard-metadata-add = Add field
wizard-metadata-key = Key
wizard-metadata-value = Value
wizard-id-too-long = The identifier must be 16 characters or less.
wizard-metadata-key-required = Metadata keys cannot be empty when a value is provided.
wizard-model-section-custom = Custom (dynamic)
wizard-model-section-traditional = Traditional (strict)
wizard-model-search = Search models (name, board, capability…)
wizard-model-none = No model matches your search.
wizard-config-help = These secrets only transit the build queue to compile the firmware — they are never stored.
wizard-config-incomplete = Fill in the WiFi SSID, the WiFi password and the server host.
wizard-custom-review-note = Custom devices don't require WiFi configuration — publish your measurements with the script below.
wizard-review-build-note = On creation, a firmware build starts automatically and its progress appears right here.
wizard-back = Back
wizard-next = Continue
wizard-create = Create device
wizard-create-build = Create & build
wizard-token-warning = Save this token and encryption key now — they will never be shown again.
wizard-copy = Copy
wizard-copied = Copied
wizard-script-title = Python publisher script
wizard-build-pending = Firmware build in progress…
wizard-build-failed = Firmware build failed. Retry with “Rebuild” on the device row.
wizard-build-launch-failed = The device was created but the build could not be launched:
wizard-reactivated = This device already existed in this organization — it has been reactivated, no new token was issued.

# Browser flashing (Web Serial + esptool-js — cf. js/flasher.js)
flash-title = Flash firmware
flash-unsupported = Browser flashing requires Web Serial (Chrome, Edge or Opera). Firefox and Safari don’t support it — download the firmware and flash it with esptool instead.
flash-fetching = Downloading firmware…
flash-fetch-error = Could not download firmware:
flash-instructions = Plug the board in via USB, then click “Flash”: the browser will open a port picker. The full image (bootloader, partitions, application) will be written at address 0x0.
flash-start = Flash
flash-stage-connect = Connecting to the board…
flash-stage-write = Writing firmware…
flash-stage-reset = Rebooting the board…
flash-done = Firmware flashed — the board rebooted onto the new firmware.
flash-error = Flashing failed:
flash-retry = Retry

pagination-previous = Previous
pagination-next = Next

# Visualisation (per-sensor curves)
vis-subtitle = Curves of the measurements stored in OpenObserve, sensor by sensor
vis-series = Available series
vis-metric = Metric
vis-device = Sensor
vis-window = Window
vis-window-1h = 1h
vis-window-6h = 6h
vis-window-24h = 24h
vis-add = Add
vis-chart = Chart
vis-empty = Add a series to display its curve
vis-no-data = No telemetry data in this organization
vis-no-points = No points in the selected window
vis-unavailable = Telemetry unavailable (OpenObserve unreachable or organization not provisioned)

# ─────────────── Brick 0 — pins of generic devices ───────────────
pins-title = Pins
pins-connected = Connected
pins-offline = Offline
pins-auto-refresh = 15 s refresh
pins-not-provisioned = Device not provisioned yet — it will appear here after its first connection (/ws/device).
pins-role-sensor = sensor
pins-role-actuator = actuator
pins-last-value = Last value
pins-high = HIGH
pins-low = LOW
pins-mode = Mode
pins-mode-in = Input (digital_in)
pins-mode-out = Output (digital_out)
pins-safe-state = Safe state
pins-safe-low = LOW (safe)
pins-safe-high = HIGH (safe)
pins-apply-mode = Apply mode
pins-write-high = Write HIGH
pins-write-low = Write LOW
pins-subscribe-off = Manual read
pins-subscribe-1s = Read every 1 s
pins-subscribe-5s = Read every 5 s
pins-subscribe-15s = Read every 15 s
pins-subscribe-60s = Read every 60 s
pins-apply = Apply
pins-flows-stopped = Flows stopped (pin mode changed): { $names }

# ─────────────── Brick 0 — generic firmware flash (PNEXCFG sector) ───────────────

# ─────────────── ETL flows (D18) — list and editor ───────────────
nav-flows = Flows
flows-subtitle = ETL pipelines: create, edit and deploy your flows
flows-new = New flow
flows-empty = No flows yet
flows-search-placeholder = Search a flow…
flows-filter-status-all = All statuses
flows-status-draft = Draft
flows-status-deployed = Deployed
flows-status-error = Error
flows-col-name = Name
flows-col-status = Status
flows-col-versions = Versions
flows-col-device = Device
flows-col-updated = Updated
flows-open = Open
flows-delete = Delete
flows-confirm-delete-title = Delete this flow?
flows-confirm-delete-message = Irreversible: the flow and all its versions will be deleted.
flows-version-deployed-tag = deployed

flows-create-title = New flow
flows-field-name = Flow name
flows-field-name-required = Name is required.
flows-field-device = Device (optional)
flows-field-device-none = No device
flows-field-note = Note (optional)
toast-flow-created = Flow created
toast-flow-saved = Version saved
toast-flow-deployed = Flow deployed
toast-flow-deleted = Flow deleted

flows-back-list = Back to list
flows-dirty-unsaved = Unsaved changes
flows-deploy = Deploy
flows-deploy-need-save = Save the current version before deploying
flows-run-once = Run once
flows-run-once-running = Running…
flows-run-once-done = { $count } message(s) injected
flows-versions = Versions
flows-debug-panel = Debug
flows-debug-title = Debug feed
flows-debug-empty = No output yet — deploy the flow then trigger it (or use “Run once”).
flows-debug-display-tag = probe
flows-debug-hint = Last 100 entries, 5 min window. Bursts may skip entries.
flows-violations-banner-title = Invalid graph:
flows-conflict-title = Stale version
flows-conflict-message = Someone saved a newer version in the meantime.
flows-conflict-reload = Reload from server
flows-conflict-overwrite = Overwrite with my version
flows-runtime-running = Engine running
flows-runtime-stopped = Engine stopped

flows-palette-title = Nodes
flows-palette-inject = Inject
flows-palette-inject-help = Trigger: interval, cron or once
flows-palette-pnex-sql = PNEX SQL
flows-palette-pnex-sql-help = Read-only Postgres query
flows-palette-display = Display
flows-palette-display-help = Shows the payload (panel + live badge)
flows-display-hint = No configuration: this probe passes messages through and shows the latest payload under it (live badge) and in the Debug panel.
flows-palette-debug = Debug
flows-palette-debug-help = Captures the pipeline output
flows-palette-red = Raw Node-RED
flows-palette-red-help = Unmodelled builtin type (JSON config)

flows-inspector-empty = Select a node to edit its configuration
flows-node-name = Node name
flows-node-delete = Delete node
flows-inject-repeat = Interval (s)
flows-inject-cron = Cron (5 or 6 fields)
flows-inject-once-delay = Initial delay (s)
flows-inject-topic = Topic
flows-inject-payload = Payload (JSON)
flows-inject-payload-invalid = Invalid JSON
flows-sql-query = SQL query (read-only)
flows-sql-params = Parameters (payload keys, comma-separated)
flows-debug-active = Capture enabled
flows-debug-complete = Captured property (empty = payload)
flows-debug-console = Also on the runtime console
flows-red-type = Node-RED type (e.g. change, json)
flows-red-config = Config (JSON)
flows-red-config-invalid = Invalid JSON

flows-canvas-empty-hint = Add nodes from the palette to build your pipeline
flows-wire-remove-title = Cut this wire?
flows-wire-remove-message = The wiring between the two nodes will be removed.
flows-wire-remove = Cut

flows-versions-title = Version history
flows-versions-col-author = Author
flows-versions-col-note = Note
flows-versions-col-date = Date
flows-versions-empty = No version
flows-versions-load = Load
flows-versions-deploy = Deploy
flows-versions-back-to-latest = Back to latest version
flows-versions-load-dirty-title = Unsaved changes
flows-versions-load-dirty-message = Your local changes will be lost. Load this version?
flows-versions-deploy-confirm-title = Deploy this version?
flows-versions-deploy-confirm-message = The runtime will reload this earlier version (no new version is created).

# ─────────────── ETL flows (D18) — device/calc/metric nodes (Phase 6) ───────────────
flows-palette-device = Device
flows-palette-device-help = Reads the latest pin values of one or more devices
flows-palette-calc = Calc
flows-palette-calc-help = Expression over the readings (variables = payload keys)
flows-palette-metric = Metric
flows-palette-metric-help = Writes the result to OpenObserve (etl_* series)
flows-device-multi-help = To combine multiple devices in one calculation, group the readings in the same node.
flows-device-device-none = Pick a device…
flows-device-pin-none = Pick a pin…
flows-device-add-read = Add a reading
flows-device-pin-overlay = board default
flows-device-window = Freshness window (s)
flows-calc-vars = Detected variables:
flows-calc-expression = Expression
flows-calc-functions-help = Functions: abs round floor ceil sqrt pow min max log log10 log2 exp sin cos tan asin acos atan atan2 — constants pi, e.
flows-metric-name = Metric name
flows-metric-preview = Written series:
flows-metric-labels-help = Written with device_id=flow_{ $id } · source_type=etl — appears in Visualization like a sensor.
