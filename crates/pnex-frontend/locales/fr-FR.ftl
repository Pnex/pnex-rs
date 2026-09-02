# PNEX front — Français.
# Parité de clés avec en-US.ftl (test de parité).

app-name = PNEX
app-tagline = Platform Nexus

# Navigation
nav-dashboard = Tableau de bord
nav-devices = Appareils
nav-catalog = Catalogue d'appareils
nav-orgs = Organisations
nav-profile = Profil
nav-visualisation = Visualisation

# Générique
common-loading = Chargement…
common-cancel = Annuler
common-confirm = Confirmer
common-retry = Réessayer
common-save = Enregistrer
common-search = Rechercher…
common-error = Erreur
common-close = Fermer
common-refresh = Rafraîchir

# Divers
login-welcome = Bienvenue sur PNeX
login-tagline = Platform Nexus
login-description = Connectez des milliers d'appareils IoT avec une ingestion de données fluide
login-signin = Se connecter à PNeX
login-register = Créer un compte
login-reset = Mot de passe oublié ?
login-footer = Authentification sécurisée par la plateforme PNeX
callback-exchanging = Connexion en cours…
callback-failed = Échec de la connexion
callback-back = Retour à la connexion
not-found = Page introuvable : { $path }
toast-session-expired = Votre session a expiré. Veuillez vous reconnecter.
shell-logout = Se déconnecter
server-url-title = URL du serveur

# Organisations
orgs-title = Organisations
orgs-subtitle = Gérez vos organisations et leurs membres
orgs-new-placeholder = Nom de la nouvelle organisation…
orgs-create = Créer
orgs-empty = Aucune organisation pour l'instant
orgs-col-name = Nom
orgs-col-role = Votre rôle
orgs-col-tier = Tier
common-actions = Actions
orgs-current = Active
orgs-select = Définir active
orgs-manage = Gérer
orgs-back = Retour aux organisations
orgs-rename = Renommer
orgs-rename-placeholder = Nouveau nom…
orgs-add-member = Ajouter un membre
orgs-email-placeholder = membre@exemple.com (connecté au moins une fois)
orgs-members = Membres
orgs-remove-member = Retirer le membre
orgs-confirm-delete-title = Supprimer cette organisation ?
orgs-confirm-delete-message = Action irréversible. L'organisation ne doit plus avoir d'autres membres, ses données seront supprimées.
orgs-delete = Supprimer
role-owner = Propriétaire
role-admin = Administrateur
role-viewer = Observateur
toast-saved = Modifications enregistrées

# Tableau de bord
dash-subtitle = Suivez vos appareils, organisations et abonnement
dash-auto-refresh = Auto · 15 s
dash-total-devices = Appareils au total
dash-active-devices = Appareils actifs
dash-orgs = Organisations
dash-tier = Tier actuel
dash-active-org = Organisation active
dash-quotas = Capacités du tier
dash-quota-sensor = Appareils capteurs
dash-quota-actuator = Appareils actionneurs
dash-quota-mixed = Appareils mixtes
dash-by-type = Appareils par type
dash-no-devices = Aucun appareil pour l'instant (Phase 4)
dash-live-sensors = Appareils en ligne
dash-build-success = Réussite des builds
dash-no-builds = Aucun build pour l'instant
dash-liveness = État des appareils
dash-last-measurements = Dernières mesures
dash-no-measurements = Aucune mesure pour l'instant
dash-telemetry-unavailable = Télémétrie indisponible (OpenObserve non configuré ou injoignable)
dash-col-device = Appareil
dash-col-metric = Mesure
dash-col-value = Valeur
dash-col-time = Heure
dash-never = jamais

# Profil
profile-subtitle = Gérez les réglages et préférences de votre compte
profile-identity = Informations de profil
profile-username = Nom d'utilisateur
profile-email = E-mail
profile-keycloak-managed = L'identité est gérée par le serveur d'authentification (Keycloak).
profile-preferences = Préférences
profile-language = Langue
profile-timezone = Fuseau horaire
profile-date-format = Format de date
profile-theme = Thème
profile-theme-light = Clair
profile-theme-dark = Sombre
profile-theme-auto = Automatique
profile-account = Compte
profile-change-password = Changer de mot de passe
profile-theme-note = Le thème est enregistré dans votre profil ; son application à l'interface arrive avec le mode sombre (phase ultérieure).

# Pages en attente de phase
empty-phase = Arrive en phase { $phase }

# Builds firmware (Phase 6)
builds-field-ssid = SSID WiFi
builds-field-wifi-password = Mot de passe WiFi
builds-field-server = Serveur PNEX (hôte)
devices-host-loopback-hint = Le device ne peut pas joindre « localhost » — saisis l'adresse LAN du serveur (ex. 192.168.1.16:5150).
builds-field-ws-ssl = SSL WebSocket (wss)
builds-field-ws-ssl-help = Coché pour wss:// (TLS, déploiement industriel) — décoché pour ws:// (serveur local / Raspberry Pi sans TLS).
builds-submit = Compiler le firmware
builds-launched = Build lancé — le suivi se fait dans la colonne Firmware de la liste des appareils.
builds-phase-queued = En file
builds-phase-running = En cours
builds-phase-succeeded = Réussi
builds-phase-failed = Échoué
builds-download = Télécharger
empty-catalog-title = Catalogue d'appareils
empty-catalog-message = Le catalogue des appareils prédéfinis (cartes, capacités) arrive avec l'API des appareils.
catalog-subtitle = Parcourez et découvrez les appareils disponibles pour vos projets
catalog-search-placeholder = Rechercher (nom, description, carte, capacité…)
catalog-type-all = Tous les types
catalog-board-all = Toutes les cartes
catalog-empty = Aucun appareil trouvé
catalog-empty-hint = Essayez d'ajuster votre recherche ou vos filtres
catalog-no-image = Pas d'image disponible
catalog-rev = Rév.
catalog-capabilities = Capacités
catalog-docs = Docs
catalog-buy = Acheter
catalog-configure = Configurer

# Appareils (Phase 4)
devices-subtitle = Enregistrez vos appareils, consultez leurs tokens de provisioning et leurs métadonnées.
devices-type-all = Tous les types
devices-type-sensor = Capteur
devices-type-actuator = Actionneur
devices-type-mixed = Mixte
devices-capability-all = Toutes les capacités
devices-status-all = Tous les statuts
devices-status-active = Actif
devices-status-inactive = Inactif
devices-last-seen-at = vu à
devices-last-seen-never = jamais vu
devices-search-placeholder = Rechercher (id, modèle, type, capacité…)
devices-new-placeholder = Identifiant de l'appareil (device_id)
devices-model-required = Choisissez un modèle dans le catalogue.
devices-id-required = Renseignez l'identifiant de l'appareil (device_id).
devices-register-title = Enregistrer un nouvel appareil
devices-register = Enregistrer
devices-created = Appareil enregistré — token de provisioning généré.
devices-empty = Aucun appareil ne correspond — enregistrez-en un ci-dessus.
devices-col-id = Appareil
devices-col-type = Type
devices-col-model = Modèle
devices-col-status = Statut
devices-col-firmware = Firmware
devices-build-never = Jamais compilé
devices-flash = Flasher
devices-flash-title = Flasher le firmware depuis le navigateur (Web Serial — Chrome/Edge)
devices-rebuild = Recompiler
devices-rebuild-title = Recompiler le firmware
devices-rebuild-incomplete = Renseignez le SSID WiFi, le mot de passe WiFi et l'hôte du serveur.
devices-detail = Détail
devices-back = Retour aux appareils
devices-delete = Supprimer
devices-confirm-delete-title = Supprimer cet appareil ?
devices-confirm-delete-message = L'appareil, son token et ses enregistrements de build firmware seront supprimés définitivement.
devices-capabilities = Capacités
devices-token = Token de provisioning
devices-token-active = Actif
devices-token-show = Afficher le token
devices-token-hide = Masquer le token
devices-token-value = Token (à transmettre au firmware)
devices-encryption-key = Clé de chiffrement (ChaCha20)
devices-metadata = Métadonnées (JSON)
devices-metadata-save = Enregistrer les métadonnées
devices-metadata-invalid = JSON invalide

# Wizard d'enregistrement (Phase 6)
wizard-step-identity = Identifiant
wizard-step-model = Modèle
wizard-step-wifi = WiFi
wizard-step-review = Revue
wizard-identity-help = Donnez au device un identifiant firmware unique (16 caractères max) — générez-en un au hasard et ajoutez des métadonnées si besoin.
wizard-shuffle = Générer
wizard-metadata-title = Métadonnées (optionnel)
wizard-metadata-add = Ajouter un champ
wizard-metadata-key = Clé
wizard-metadata-value = Valeur
wizard-id-too-long = L'identifiant doit faire 16 caractères maximum.
wizard-metadata-key-required = Les clés de métadonnées ne peuvent pas être vides quand une valeur est renseignée.
wizard-model-section-custom = Custom (dynamique)
wizard-model-section-traditional = Traditionnel (strict)
wizard-model-search = Chercher un modèle (nom, board, capacité…)
wizard-model-none = Aucun modèle ne correspond à votre recherche.
wizard-config-help = Ces secrets ne transitent que la queue de build pour compiler le firmware — ils ne sont jamais stockés.
wizard-config-incomplete = Renseignez le SSID WiFi, le mot de passe WiFi et l'hôte du serveur.
wizard-custom-review-note = Les devices custom ne nécessitent pas de configuration WiFi — publiez vos mesures avec le script ci-dessous.
wizard-review-build-note = À la création, un build firmware démarre automatiquement et sa progression s'affiche ici même.
wizard-back = Retour
wizard-next = Continuer
wizard-create = Créer le device
wizard-create-build = Créer & compiler
wizard-token-warning = Sauvegardez ce token et cette clé de chiffrement maintenant — ils ne seront plus jamais affichés.
wizard-copy = Copier
wizard-copied = Copié
wizard-script-title = Script Python publisher
wizard-build-pending = Build firmware en cours…
wizard-build-failed = Le build firmware a échoué. Relancez avec « Recompiler » sur la ligne du device.
wizard-build-launch-failed = Le device a été créé mais le build n'a pas pu être lancé :
wizard-reactivated = Ce device existait déjà dans cette organisation — il a été réactivé, aucun nouveau token n'a été émis.

# Flash navigateur (Web Serial + esptool-js — cf. js/flasher.js)
flash-title = Flasher le firmware
flash-unsupported = Le flash navigateur nécessite Web Serial (Chrome, Edge ou Opera). Firefox et Safari ne le supportent pas — téléchargez le firmware et flashez-le avec esptool.
flash-fetching = Téléchargement du firmware…
flash-fetch-error = Téléchargement du firmware impossible :
flash-instructions = Branchez la carte en USB, puis cliquez sur « Flasher » : le navigateur ouvrira un sélecteur de port. L'image complète (bootloader, partitions, application) sera écrite à l'adresse 0x0.
flash-start = Flasher
flash-stage-connect = Connexion à la carte…
flash-stage-write = Écriture du firmware…
flash-stage-reset = Redémarrage de la carte…
flash-done = Firmware flashé — la carte a redémarré sur le nouveau firmware.
flash-error = Le flash a échoué :
flash-retry = Réessayer

pagination-previous = Précédent
pagination-next = Suivant

# Visualisation (courbes capteur par capteur)
vis-subtitle = Courbes des mesures stockées dans OpenObserve, capteur par capteur
vis-series = Séries disponibles
vis-metric = Métrique
vis-device = Capteur
vis-window = Fenêtre
vis-window-1h = 1 h
vis-window-6h = 6 h
vis-window-24h = 24 h
vis-add = Ajouter
vis-chart = Courbe
vis-empty = Ajoutez une série pour afficher sa courbe
vis-no-data = Aucune donnée de télémétrie dans cette organisation
vis-no-points = Aucun point sur la fenêtre sélectionnée
vis-unavailable = Télémétrie indisponible (OpenObserve injoignable ou organisation non provisionnée)

# ─────────────── Brick 0 — pins devices génériques ───────────────
pins-title = Pins
pins-connected = Connecté
pins-offline = Hors ligne
pins-auto-refresh = actualisation 15 s
pins-not-provisioned = Device non provisionné — il apparaîtra ici après sa première connexion (/ws/device).
pins-role-sensor = capteur
pins-role-actuator = actionneur
pins-last-value = Dernière valeur
pins-high = HIGH
pins-low = LOW
pins-mode = Mode
pins-mode-in = Entrée (digital_in)
pins-mode-out = Sortie (digital_out)
pins-safe-state = État de repos
pins-safe-low = LOW (sûr)
pins-safe-high = HIGH (sûr)
pins-apply-mode = Appliquer le mode
pins-write-high = Écrire HIGH
pins-write-low = Écrire LOW
pins-subscribe-off = Lecture manuelle
pins-subscribe-1s = Lire chaque 1 s
pins-subscribe-5s = Lire chaque 5 s
pins-subscribe-15s = Lire chaque 15 s
pins-subscribe-60s = Lire chaque 60 s
pins-apply = Appliquer

# ─────────────── Brick 0 — flash firmware générique (secteur PNEXCFG) ───────────────
