# PNEX front — English (fallback).
# Toute clé ajoutée dans un autre locale doit exister ici (test de parité).

app-name = PNEX
app-tagline = Platform Nexus

# Navigation
nav-dashboard = Dashboard
nav-devices = Devices
nav-catalog = Device Catalog
nav-orgs = Organizations
nav-profile = Profile

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
login-welcome = Welcome to PNeX
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

# Profil
profile-subtitle = Manage your account settings and preferences
profile-identity = Profile information
profile-username = Username
profile-email = Email
profile-keycloak-managed = Identity fields are managed by the authentication server (Keycloak).
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
pagination-previous = Previous
pagination-next = Next
