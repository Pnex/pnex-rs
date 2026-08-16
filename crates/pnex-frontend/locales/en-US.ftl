# PNEX front — English (fallback).
# Toute clé ajoutée dans un autre locale doit exister ici (test de parité).

app-name = PNEX
app-tagline = Platform Nexus

# Navigation
nav-dashboard = Dashboard
nav-devices = Devices
nav-builds = Firmware Build
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
empty-builds-title = Firmware build
empty-builds-message = Firmware builds and their live status arrive with the in-house build worker.
empty-catalog-title = Device catalog
empty-catalog-message = The catalog of predefined devices (boards, capabilities) lands with the devices API.

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
devices-search-placeholder = Search by device_id…
devices-new-placeholder = Device identifier (device_id)
devices-model-placeholder = — Model —
devices-model-required = Pick a model from the catalog.
devices-catalog-loading = Loading catalog…
devices-register = Register
devices-created = Device registered — provisioning token generated.
devices-empty = No device matches — register one above.
devices-col-id = Device
devices-col-type = Type
devices-col-model = Model
devices-col-status = Status
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
