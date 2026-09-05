# PNEX front — Français.
# Parité de clés avec en-US.ftl (test de parité).

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
shell-logout-confirm-title = Se déconnecter ?
shell-logout-confirm-message = Votre session sera fermée et vous devrez vous reconnecter.
shell-logout-confirm-action = Se déconnecter
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
profile-idp-managed = L'identité est gérée par le serveur d'authentification (Rauthy).
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
pins-flows-stopped = Flows arrêtés (pin changé de mode) : { $names }

# ─────────────── Brick 0 — flash firmware générique (secteur PNEXCFG) ───────────────

# ─────────────── Flux ETL (D18) — liste et éditeur ───────────────
nav-flows = Flux
flows-subtitle = Pipelines ETL : créez, éditez et déployez vos flows
flows-new = Nouveau flow
flows-empty = Aucun flow pour l'instant
flows-search-placeholder = Rechercher un flow…
flows-filter-status-all = Tous les statuts
flows-status-draft = Brouillon
flows-status-deployed = Déployé
flows-status-error = Erreur
flows-col-name = Nom
flows-col-status = Statut
flows-col-versions = Versions
flows-col-device = Appareil
flows-col-updated = Mise à jour
flows-open = Ouvrir
flows-delete = Supprimer
flows-confirm-delete-title = Supprimer ce flow ?
flows-confirm-delete-message = Action irréversible : le flow et toutes ses versions seront supprimés.
flows-version-deployed-tag = déployée

flows-create-title = Nouveau flow
flows-field-name = Nom du flow
flows-field-name-required = Le nom est obligatoire.
flows-field-device = Appareil (optionnel)
flows-field-device-none = Aucun appareil
flows-field-note = Note (optionnelle)
toast-flow-created = Flow créé
toast-flow-saved = Version enregistrée
toast-flow-deployed = Flow déployé
toast-flow-deleted = Flow supprimé

flows-back-list = Retour à la liste
flows-dirty-unsaved = Modifications non enregistrées
flows-deploy = Déployer
flows-deploy-need-save = Enregistrez la version courante avant de déployer
flows-run-once = Exécuter une fois
flows-run-once-running = Exécution…
flows-run-once-done = { $count } message(s) injecté(s)
flows-versions = Versions
flows-debug-panel = Debug
flows-debug-title = Flux de débogage
flows-debug-empty = Aucune sortie — déployez le flow puis déclenchez-le (ou bouton « Exécuter une fois »).
flows-debug-display-tag = sonde
flows-debug-hint = 100 dernières sorties, fenêtre de 5 min. Les rafales peuvent sauter des entrées.
flows-violations-banner-title = Graphe invalide :
flows-conflict-title = Version périmée
flows-conflict-message = Quelqu'un a enregistré une nouvelle version entre-temps.
flows-conflict-reload = Recharger depuis le serveur
flows-conflict-overwrite = Écraser avec ma version
flows-runtime-running = Moteur actif
flows-runtime-stopped = Moteur arrêté
flows-runtime-outdated = à redéployer

flows-deploy-propose-text = v{ $version } enregistrée — la version en exécution reste active. La déployer maintenant ?
flows-deploy-propose-go = Déployer v{ $version }
flows-deploy-propose-later = Plus tard

flows-palette-title = Nœuds
flows-palette-inject = Inject
flows-palette-inject-help = Déclencheur : intervalle, cron ou unique
flows-palette-pnex-sql = SQL PNEX
flows-palette-pnex-sql-help = Requête Postgres en lecture seule
flows-palette-display = Display
flows-palette-display-help = Affiche le payload (panneau + badge live)
flows-display-hint = Aucune configuration : cette sonde laisse passer les messages et affiche le dernier payload sous elle (badge live) et dans le panneau Debug.
flows-palette-debug = Debug
flows-palette-debug-help = Capture la sortie d'un pipeline
flows-palette-red = Node-RED brut
flows-palette-red-help = Type builtin non modélisé (config JSON)

flows-inspector-empty = Sélectionnez un nœud pour éditer sa configuration
flows-node-name = Nom du nœud
flows-node-delete = Supprimer le nœud
flows-inject-repeat = Intervalle (s)
flows-inject-cron = Cron (5 ou 6 champs)
flows-inject-once-delay = Délai initial (s)
flows-inject-topic = Topic
flows-inject-payload = Payload (JSON)
flows-inject-payload-invalid = JSON invalide
flows-sql-query = Requête SQL (lecture seule)
flows-sql-params = Paramètres (clés du payload, séparées par des virgules)
flows-debug-active = Capture activée
flows-debug-complete = Propriété capturée (vide = payload)
flows-debug-console = Recopie sur la console du runtime
flows-red-type = Type Node-RED (ex. change, json)
flows-red-config = Config (JSON)
flows-red-config-invalid = JSON invalide

flows-canvas-empty-hint = Ajoutez des nœuds depuis la palette pour construire votre pipeline
flows-wire-remove-title = Couper ce câble ?
flows-wire-remove-message = Le câblage entre les deux nœuds sera supprimé.
flows-wire-remove = Couper

flows-versions-title = Historique des versions
flows-versions-col-author = Auteur
flows-versions-col-note = Note
flows-versions-col-date = Date
flows-versions-empty = Aucune version
flows-versions-load = Charger
flows-versions-deploy = Déployer
flows-versions-back-to-latest = Revenir à la dernière version
flows-versions-load-dirty-title = Modifications non enregistrées
flows-versions-load-dirty-message = Vos modifications locales seront perdues. Charger la version ?
flows-versions-deploy-confirm-title = Déployer cette version ?
flows-versions-deploy-confirm-message = Le runtime rechargera cette version antérieure (aucune nouvelle version n'est créée).

# ─────────────── Flux ETL (D18) — nœuds device/calc/metric (Phase 6) ───────────────
flows-palette-device = Appareil
flows-palette-device-help = Lit les dernières valeurs des pins d'un ou plusieurs appareils
flows-palette-calc = Calcul
flows-palette-calc-help = Expression sur les lectures (variables = clés de payload)
flows-palette-metric = Métrique
flows-palette-metric-help = Écrit le résultat dans OpenObserve (série etl_*)
flows-device-multi-help = Pour combiner plusieurs appareils dans un même calcul, regroupez les lectures dans le même nœud.
flows-device-device-none = Choisir un appareil…
flows-device-pin-none = Choisir un pin…
flows-device-add-read = Ajouter une lecture
flows-device-pin-overlay = défaut carte
flows-device-window = Fenêtre de fraîcheur (s)
flows-calc-expression = Expression
flows-calc-vars = Variables détectées :
flows-calc-functions-help = Fonctions : abs round floor ceil sqrt pow min max log log10 log2 exp sin cos tan asin acos atan atan2 — constantes pi, e.
flows-metric-name = Nom de la métrique
flows-metric-preview = Série écrite :
flows-metric-labels-help = Écrite avec device_id=flow_{ $id } · source_type=etl — apparaît dans Visualisation comme un capteur.
