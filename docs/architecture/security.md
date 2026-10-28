# Sécurité : modèle de menace, règles, revue, registre

> **Statut : socle posé le 2026-10-04 (D130).** Premier audit de sécurité
> du dépôt (audit de base, §7). À partir de cette date, **toute
> modification passe la revue de sécurité du §6** — règle reprise dans
> `CLAUDE.md`. Les findings vivent dans le registre §7 jusqu'à leur
> correction ; un finding corrigé y reste (statut + commit).

## 1. Décision

| # | Décision |
|---|---|
| D130 | La sécurité est une garde permanente, pas un chantier : (1) règles invariantes §3, opposables à toute PR ; (2) revue de sécurité de chaque diff avant commit (§6) ; (3) garde dépendances bloquante en CI (`cargo deny`, §5) ; (4) registre unique des findings §7, audit complet re-joué à chaque release majeure. |

## 2. Modèle de menace

### Attaquants considérés

| Attaquant | Ce qu'il a | Ce qu'il ne doit jamais obtenir |
|---|---|---|
| **Visiteur anonyme** | Accès réseau à nginx (80/443), liens `/share/<token>` | Toute donnée hors tour publié partagé ; toute exécution de script sur l'origine de l'app |
| **Utilisateur inscrit quelconque** | Un compte Rauthy → **une org personnelle dont il est owner** (provisioning). Il peut donc créer flows, fonctions JS/Starlark, médias, tours, canaux de notification | Données, devices, secrets, télémétrie d'**une autre org** ; secrets plateforme (env du serveur, du runtime, DB, clés du coffre) ; session d'un autre utilisateur |
| **Viewer d'une org** | Lecture de l'org | Toute écriture, tout déclenchement d'action (OTA, actionneur), tout identifiant de device |
| **Member d'une org** | Écriture (flows, médias, devices) | Valeurs des secrets du coffre (D117), administration de l'org |
| **Device / agent edge compromis** | Son jeton + sa clé ChaCha20 | Agir au nom d'un autre device ou d'une autre org |
| **Réseau local / MITM** | Trafic device ↔ serveur | Jetons en clair si wss avec CA épinglée |

Conséquence clé : **« member » n'est pas une barrière** — n'importe qui
devient owner de sa propre org. Tout ce qu'un member peut faire, un
attaquant anonyme qui s'inscrit le peut aussi. La frontière qui compte est
**l'organisation**, puis la **plateforme**.

### Frontières de confiance

1. **nginx → backend** : seule entrée publique. `/internal/*` ne doit pas
   y être routé (§3 R7).
2. **backend ↔ runtime de flows** : le runtime exécute du code utilisateur
   (JS, Starlark, nœuds) de **plusieurs orgs dans un même process**. Il
   est traité comme **non fiable** : tout ce qu'il envoie au backend est
   re-vérifié côté backend.
3. **backend ↔ build worker firmware** : code C++ utilisateur compilé sous
   bwrap (sans réseau, hôte en lecture seule).
4. **navigateur** : contenu utilisateur (labels, médias, Markdown) rendu
   sur **l'origine de l'app**, où vivent les jetons (`localStorage`).

## 3. Règles invariantes (opposables à toute modification)

### Locataire et rôles

- **R1 — L'org vient du principal, jamais de la requête.** `OrgContext`
  (JWT + `X-Org-Id` + appartenance vérifiée) ou le jeton device sont les
  seules sources de `org_id` / `device_id`. Toute requête par id filtre
  `OrgId = org.org.id` ; cross-org = **404 uniforme**.
- **R2 — Chaque opération déclare sa permission au registre IAM**
  (`pnex-api-contract/src/iam.rs`, garde `tests/iam.rs`) ; l'extracteur
  `OrgContext` l'applique avant le handler (403 `access-denied`,
  `iam.md` D291). Les gardes de rôle des handlers (`can_write`,
  `can_administer`, `can_manage_secrets`, `PlatformAdmin`) restent en
  défense en profondeur jusqu'à leur retrait. Un nouvel endpoint sans
  entrée au registre = test rouge ; un test de régression « principal
  sans la permission → 403 » accompagne toute route d'écriture nouvelle.
- **R3 — Les champs de locataire sont tamponnés par le serveur en dernier.**
  Aucune config fournie par l'utilisateur (nœud, graphe, payload) ne peut
  porter `pnex_org_id`, `pnex_o2_org`, `pnex_flow_id` ou un type `pnex-*`
  qui serait relu tel quel ; la projection les écrase après tout le reste.
- **R4 — Les identifiants d'accès (jeton device, clé ChaCha20, code
  d'enrôlement) ne sortent que vers les rôles qui provisionnent**
  (`can_write` au minimum), jamais dans un DTO de lecture générique.

### Code utilisateur (flows, fonctions, firmware)

- **R5 — Registre de nœuds = liste blanche explicite.** Le runtime
  n'enregistre que les nœuds PneX et les builtins sans effet hôte
  (change, switch, json, delay, split/join…). Jamais `exec`, `file`,
  `file in`, `watch`, tcp/udp, ni aucun builtin non revu. Le `validate`
  du graphe rejette au save **et** au deploy tout type hors liste.
- **R6 — Aucun secret plateforme visible du code utilisateur.** Le code
  JS/Starlark ne voit ni l'environnement du process (`env.get`), ni les
  fichiers, ni les jetons de service. Les identifiants des nœuds natifs
  passent par un canal Rust inaccessible au sandbox.
- **R7 — Les routes `/internal/*` sont privées et scoppées.** Jeton de
  service comparé en temps constant (`auth::service_token::matches`,
  échec fermé si vide) ; **non routées par nginx** ; l'`org_id` fourni est
  re-vérifié contre ce que le déploiement autorise (device de l'org,
  secret référencé par un flow déployé de l'org) ; fencing **fermé** si
  l'en-tête manque en mode cluster.
- **R8 — Requêtes sortantes vers des hôtes choisis par l'utilisateur**
  (http-fetch, webhooks, fournisseurs LLM) : refuser loopback,
  link-local (169.254/16, fe80::/10), métadonnées cloud et noms de
  services internes ; re-vérifier à chaque redirection.
- **R9 — Un secret du coffre est lié à sa destination.** Seuls owner et
  admin (`can_manage_secrets`) câblent un secret : le choisir, le changer
  de champ, ou changer l'origine (schéma, hôte, port ; hôte, port et mode
  TLS en SMTP) d'un nœud / canal qui en porte un. Un member (et
  l'assistant) ne peut que **garder** un triplet (champ, secret,
  destination) que le **même** consommateur porte déjà (même flow —
  version la plus récente ou déployée —, même canal) ; tout le reste est
  **refusé** avec `secret-destination-locked` (403), y compris un flow
  dupliqué ou importé qui porte des références et un test-draft qui
  diffère du canal enregistré (D117 : aucun rôle ne relit une valeur, y
  compris en la redirigeant). Garde : `services/secrets/binding.rs`.
- **R10 — Processus enfants : `env_clear()` + liste blanche**, secrets
  jamais en argv ; build firmware utilisateur sous bwrap (défaut).

### Contenu servi au navigateur

- **R11 — Pas de HTML utilisateur sur l'origine de l'app.** Texte rendu
  par rsx (échappé) ou `textContent` côté JS ; bibliothèques tierces en
  mode échappement (`escapeHTML: true` pour pannellum) ; jamais
  `innerHTML` / `dangerous_inner_html` sur une donnée non assainie ;
  Markdown : HTML brut supprimé **et** URLs limitées à
  `http(s):`/`mailto:`/relatives.
- **R12 — Octets utilisateur servis = type autorisé + `nosniff`.**
  `Content-Type` en liste blanche par `kind` (images raster,
  `application/octet-stream`), jamais la valeur client brute ; toute
  réponse de contenu utilisateur porte `X-Content-Type-Options: nosniff`
  et `Content-Security-Policy: sandbox; default-src 'none'` ;
  `Content-Disposition: attachment` hors images.
- **R13 — Données vers JS : encodage JSON, jamais de `format!` en
  `'{}'`.** wasm : `JsValue` / `Reflect` ; natif : `serde_json::to_string`
  comme littéral JS, ou un helper unique `JSON.parse('…')` échappé.

### Crypto, secrets, stockage

- **R14 — Chiffrement au repos = `Keyring::seal/open`** (XChaCha20-Poly1305,
  nonce aléatoire, AAD lié org + propriétaire + ligne). Pas de crypto
  maison.
- **R15 — Aléa de sécurité = CSPRNG** (`rand::rng()`, `getrandom`),
  ≥ 128 bits pour un jeton ; codes courts = hachés au repos + consommation
  atomique unique + TTL.
- **R16 — Jamais de secret en clair** dans une réponse d'API, un log, la
  file de jobs, `flows.json` ou le fil inter-pods : références UUID
  seulement, résolues au point d'usage.
- **R17 — JWT : une seule `Validation`** (`auth/jwks.rs` : RS256, `iss`,
  `aud`, `exp`, `kid`). Aucune autre construction.
- **R18 — Clés de stockage construites côté serveur** (ids +
  `sanitize_segment`) ; un endpoint d'octets résout d'abord une ligne
  filtrée par org puis lit sa `storage_key` ; jamais de chemin/clé reçu.
- **R19 — Déploiement : secrets obligatoires** (`${VAR:?}` générés par
  `install.sh`), valeurs par défaut réservées au compose de dev ; TLS
  vérifié partout (seule exception : bootstrap de l'agent edge, épinglé
  par empreinte).
- **R20 — Dépendances : licence permissive** (règle existante) **et
  `cargo deny` vert** ; toute exception `deny.toml` porte sa
  justification d'inexploitabilité. Les images livrées sont **Chainguard
  (Wolfi) uniquement** (`glibc-dynamic`, `wolfi-base`) ; les étages de
  build Debian ne sont jamais livrés. Un binaire tiers compilé hors cargo
  et copié dans une image (ffmpeg, archives sherpa-onnx) a sa source
  épinglée en sha256 dans le Dockerfile (`ADD --checksum`).
- **R22 — Aucune permission `iam:*` ni plateforme pour un compte de
  service** (`iam.md` D293) : un rôle personnalisé n'accorde ni ne retire
  jamais `iam:*`, le rôle géré admin n'est jamais attribué à un compte de
  service, et l'extracteur refuse `iam` à une clé d'accès ; les
  extracteurs humains (`AuthUser`, `PlatformAdmin`) refusent toute clé.
- **R23 — Secret de clé d'accès** : 32 octets d'un CSPRNG, préfixe
  `pnex_sk_`, montré une seule fois, stocké en sha256 seulement (jamais
  relu), expiration obligatoire (366 jours au plus), révocation effective
  à l'appel suivant (`services/iam/service.rs`).

## 4. Bons motifs existants (à imiter)

- `OrgContext` : appartenance jamais mise en cache, 404 masqué.
- `auth::service_token::matches` : `subtle::ct_eq`, vide = refus.
- Coffre : AAD `domaine ‖ propriétaire ‖ org ‖ secret_id`, trousseau
  obligatoire au boot, rotation par `key_id`.
- Enrôlement agent : code haché, `UPDATE … WHERE used_at IS NULL AND
  expires_at > now()` atomique, CA épinglée par empreinte.
- Builds firmware : `env_clear` + liste blanche, `wifi_secret_id` seul
  dans la file (test `queued_args_never_carry_the_wifi_password`).
- Tours publics : jeton 122 bits + asset référencé par la version publiée
  + asset de l'org du tour.
- OAuth : PKCE S256 obligatoire, redirect URIs exactes côté Rauthy.
- Requêtes O2 : littéraux échappés et exécutées avec les identifiants de
  l'org elle-même.

## 5. Garde dépendances

- `deny.toml` à la racine ; `task security:deps` = job CI `deny`
  (bloquant) : avis RustSec + sources (crates.io et `path =` uniquement).
- Exceptions actuelles (sans correctif amont, justifiées dans
  `deny.toml`) : `rsa` (Marvin — vérification de signature publique
  uniquement), `bincode`, `smallstr`, `atomic-polyfill`, `derivative`,
  `fxhash`, `paste`, `proc-macro-error` (non maintenues, aucune vuln).
- Une nouvelle exception = justification d'inexploitabilité dans
  `deny.toml` + ligne ici.

## 6. Revue de sécurité d'une modification (obligatoire)

Avant chaque commit touchant du code, parcourir le diff avec cette grille
(et lancer `/security-review` avant merge d'une branche) :

1. **Nouvelle route ?** Garde d'auth, permission inscrite au registre IAM
   (R2), org depuis le principal (R1), test sans la permission → 403 /
   cross-org → 404.
2. **Nouvelle donnée renvoyée ?** Pas d'identifiant d'accès ni de secret
   (R4, R16).
3. **Nouveau nœud / type de flow / champ de config ?** Liste blanche
   runtime (R5), tampons de locataire écrasés (R3), pas d'accès env (R6).
4. **Requête sortante ou URL utilisateur ?** R8, R9.
5. **Rendu de texte ou d'octets utilisateur ?** R11, R12, R13.
6. **SQL / requête O2 / clé Valkey / chemin de stockage construit avec
   une entrée ?** Paramètres liés, littéraux échappés, préfixe d'org
   d'abord, `sanitize_segment` (R18).
7. **Processus enfant, crypto, aléa, jeton ?** R10, R14, R15, R17.
8. **Nouvelle dépendance ?** Licence + `task security:deps` (R20).
9. **Config de déploiement ?** R7 (nginx), R19.

Tout écart assumé est consigné en §7 avec sa justification ; un finding
découvert en route est ajouté au registre même s'il n'est pas corrigé
dans la même PR.

## 7. Registre des findings

Audit de base du 2026-10-04 : 6 audits de domaine (authz/locataire,
surface device/WS/OTA, sandboxes/injection/SSRF, fichiers/médias,
crypto/secrets/déploiement, front XSS/ponts JS), puis vérification
contradictoire de chaque finding ; seuls les findings à confiance ≥ 8/10
sont retenus.

Statuts : **ouvert** · **corrigé** (commit) · **accepté** (justification).
Sévérité : HIGH = compromission inter-org / plateforme ou prise de compte ;
MEDIUM = escalade à l'intérieur d'une org ; LOW = défaut de cloisonnement
sans impact exploitable démontré.

### Retenus (confiance ≥ 8/10)

| # | Sév. | Finding | Règle | Statut |
|---|---|---|---|---|
| SEC-1 | HIGH | **RCE par le nœud `red` → `exec`** | R5 | corrigé |
| SEC-2 | HIGH | **Falsification de `pnex_org_id` via un nœud `red` `pnex-*`** | R3 | corrigé |
| SEC-3 | HIGH | **`env.get` des fonctions JS lit l'environnement du runtime** | R6 | corrigé (+ fork edgelinkd) |
| SEC-4 | HIGH* | **`/internal/*` exposé et `org_id` fourni par l'appelant** | R7 | corrigé — exposition ; résiduel ci-dessous |
| SEC-5 | HIGH | **XSS stocké : médias servis avec le `Content-Type` du client** | R12 | corrigé |
| SEC-6 | HIGH | **XSS stocké : labels pannellum sans `escapeHTML`** | R11 | corrigé |
| SEC-7 | MEDIUM | **Viewer : déploiement / annulation OTA** | R2 | corrigé |
| SEC-8 | MEDIUM | **Viewer : jetons et clés des devices, firmware avec PSK WiFi** | R4 | corrigé |
| SEC-9 | LOW | **Dashboard : id de contrôle d'une autre org accepté au save** | R1 | corrigé (18484ae) |
| SEC-10 | MEDIUM | **APK Android distribué « debuggable » : session lisible par USB** | R16 | accepté pour 0.1.0-beta.1 (sideload, testeurs connus) — 2026-10-09 : APK release non debuggable signé prêt (`task build:frontend:android:release`, job CI `android`) ; reste à créer la clé et la poser en secrets GitHub |
| SEC-11 | MEDIUM | **Markdown de l'assistant : liens `javascript:` et images distantes** (ex-SEC-W1, relevé à l'audit de release) | R11 | corrigé |
| SEC-12 | LOW | **Sauvegarde Android (auto-backup, transfert) emportait le jeton de rafraîchissement** | R16 | corrigé |
| SEC-13 | LOW | **Assistant : widget libre re-lié au contrôle d'un flow déployé** | D144 | corrigé |
| SEC-14 | LOW | **URL d'un fournisseur LLM vers un hôte interne (SSRF aveugle)** | R8 | corrigé 2026-10-07 — résolveur filtrant `pnex_core::egress` (avec SEC-W3) |
| SEC-15 | LOW | **Image `pnex-builder-rs` : paquets Python vulnérables (7 HIGH, 0 CRITICAL)** | dépendances | corrigé pour 0.1.0-beta.4 (11 sur 12) ; `ecdsa` accepté (pas de correctif amont) |
| SEC-16 | LOW | **Firmware : CA épinglée décodée dans un buffer fixe avant le contrôle de taille (débordement mémoire)** | D70 | corrigé 2026-10-08 (trouvé en route le 2026-10-07) |
| SEC-17 | HIGH | **Frames device non authentifiées et sans anti-rejeu** (ChaCha20 seul, D8) | D8 | corrigé (couche interne) 2026-10-08 — lien Noise NNpsk0 (D156) serveur + agent + firmware, D8 supprimé ; reste le TLS mutuel (D153, lots L2–L4) |
| SEC-18 | MEDIUM | **OTA non authentifiée, pas d'anti-downgrade sur ESP32** | — | corrigé 2026-10-09 (non commité ; banc 8266 + C6 : OTA signée acceptée, downgrade refusé — signature invalide testée sur hôte seulement) — image signée Ed25519 liée au device + version, clé publique compilée, anti-downgrade sur toutes les puces (ota.md §4) ; signature hors builder = EX-B7 (V2) |
| SEC-19 | MEDIUM | **Firmware : clé de chiffrement vide = trafic en clair** (repli silencieux) | R16 | corrigé 2026-10-08 — sans clé valide le device ne se connecte pas (`pnex_crypto.cpp`, `pnex_transport.cpp`) |
| SEC-20 | LOW | **Secrets du device en clair dans le flash** (PSK WiFi, jeton, clé ChaCha) | R16 | accepté jusqu'à V4 (EX-C1/C3) : exige des eFuses, gelés avant la validation communautaire du firmware |
| SEC-21 | HIGH | **En-têtes de l'edge crus sans preuve d'origine** (`X-Forwarded-Proto`, puis `X-Client-Cert`) : le port du backend est joignable sans nginx (compose publie 5150), un client pouvait les forger | R7 | corrigé 2026-10-09 (trouvé en route, lot L4) — secret partagé edge ↔ backend (`PNEX_EDGE_SECRET`, en-tête `X-Pnex-Edge`, comparaison à temps constant) ; sans lui les en-têtes de l'edge sont ignorés ; vérifié en direct (close 4013) |
| SEC-22 | MEDIUM | **JWT de l'utilisateur dans l'URL des WebSockets navigateur** (`/ws/notify?token=`, `/ws/camera/live?token=`) : l'URL finit dans les journaux d'accès (proxy, CDN, contrôleur d'ingress) — même classe que O19 côté devices | R16 | corrigé 2026-10-09 (inventaire des ruptures pré-0.1.0) — ticket à usage unique `POST /api/v1/ws-ticket` (60 s, lié à l'utilisateur et à l'org du principal, consommé par `GETDEL` Valkey) ; ticket rejoué → 4001 (test) |
| SEC-23 | LOW | **Filtre d'egress : angles morts** (`pnex-core/src/egress.rs`) — `ip_allowed` ne juge pas les adresses IPv6 qui embarquent une IPv4 (NAT64 `64:ff9b::/96`, 6to4 `2002::/16`) par leur IPv4 ; en `lan`, les noms `*.svc.cluster.local` et les adresses du réseau d'infra (Docker/k8s : Postgres, Valkey, O2, RustFS) restent joignables par toute org via `http_fetch` (et demain les flux média) | R8 | **corrigé** 2026-10-15 : IPv4 embarquée jugée comme IPv4, `*.svc`/`*.cluster.local` refusés, `PNEX_EGRESS_DENY_CIDRS` pour le réseau d'infra : en compose (`pnex-deploy`) = `PNEX_NET_SUBNET` par défaut ; en Helm = `api.egressDenyCidrs` (CIDR pods/services de la distribution, avertissement NOTES tant qu'il est vide) |
| SEC-24 | LOW | **Modèles audio : disque du cache d'extraction** (`services/media_ingest/asr`) — chaque modèle audio importé est extrait dans `PNEX_ASR_MODELS_DIR` (borne de 4 Gio par modèle, traversée de chemins et liens refusés) ; aucune limite du nombre de modèles par org : une org peut remplir le disque du serveur en important des modèles distincts | — | ouvert — relevé 2026-10-10 au lot 1 de `media-ingest.md` ; piste : quota de modèles audio par org (verrou advisory, école D159), purge LRU du cache, taille d'upload `model` dédiée |
| SEC-25 | LOW | **Binaires média hors veille CVE** (`deploy/docker/Dockerfile`, étages `ffmpeg` et `asr`) — ffmpeg (compilé par nous) et onnxruntime (lié statiquement dans `pnex-asr` via les archives sherpa-onnx) ne viennent pas de paquets Wolfi : ni l'avis CVE de Chainguard ni `cargo deny` ne les suivent. Ils décodent des médias non fiables | R20 | atténué 2026-10-10 : ffmpeg réduit au protocole `pipe` + démuxeurs/décodeurs audio listés (lot 6a : + décodeurs vidéo natifs H.264/HEVC/MJPEG, encodeur MJPEG, `image2pipe`, process vidéo distinct au même confinement, 2 threads), les deux process confinés (seccomp sans socket, Landlock lecture seule, rlimits, vérifié dans un conteneur Wolfi) ; reste : relever les versions épinglées à chaque release (scan d'image Trivy/Grype en CI) |
| SEC-27 | LOW | **Secret d'accès d'un flux remis à son boîtier de capture** (`services/media_ingest/boxes.rs`, `/ws/media`, lot 6b de `media-ingest.md`) — un flux `capture_on = device:<id>` dont la source exige un secret (en-tête, identifiants RTSP/Tvheadend) ne peut être ouvert que par le boîtier : le serveur lui envoie la valeur du secret avec la liste de ses flux. Nouvelle exposition : un boîtier compromis (Pi sur le LAN de l'utilisateur) livre les secrets des flux qu'il capte ; un membre qui déplacerait un flux à secret vers un agent qu'il contrôle obtiendrait le secret en clair | R9, R16 | accepté 2026-10-10 avec garde-fous : secret envoyé seulement au boîtier désigné par le flux (org et device tirés de l'identité mTLS + Noise, R1), sur son lien chiffré, jamais dans un DTO de lecture ni dans un journal, gardé en mémoire seulement par l'agent (aucun fichier) ; c'est le secret **de la source** (déjà destiné à l'hôte de l'URL), jamais un secret de stockage (le boîtier n'a ni clé RustFS ni jeton de service) ; déplacer un flux qui porte un secret vers un boîtier exige `can_manage_secrets` (R9 : `secret-destination-locked` sinon, test `media_box.rs`) ; seul un agent qui a annoncé `media_capture` est un lieu de capture valide |
| SEC-28 | LOW | **Pages : débit non borné par connexion** (`services/pages/room.rs`) — `/ws/doc/{id}` borne la taille de chaque trame (256 Kio) et de la page (8 Mio), pas le nombre de trames par seconde ; un membre peut saturer le journal `doc_updates` et le relais de présence de son org | R2 | ✅ 2026-10-10 : seau à jetons par session (rafale 200, 50 messages/s), fermeture 4429 (`controllers/ws_doc.rs`) |
| SEC-29 | LOW | **Pages : rôle figé à l'ouverture du websocket** (`controllers/ws_doc.rs`) — rôle et appartenance lus à la connexion ; un membre rétrogradé viewer ou retiré garde l'écriture jusqu'à la déconnexion (même comportement que `/ws/notify`) | R2 | **corrigé** 2026-10-15 : rôle relu à chaque battement (30 s) ; retiré → fermeture 4006, rétrogradé viewer → lecture seule ; une erreur de lecture garde le droit courant |
| SEC-35 | MEDIUM | **Plages d'objet sans garde du type** (`controllers/time_ranges.rs`) — création, modification, suppression et import d'une plage rattachée à un objet ne vérifiaient que le rôle d'org, pas le `write_role` du type de l'objet (D188) : un membre pouvait écrire le calendrier d'un type réservé aux admins | R2 | ✅ 2026-10-13 (`feat/gmao-plans`) : même garde que l'objet lui-même, test viewer/member → 403 `ontology-write-forbidden` ; le nœud `range_upsert` (flow déployé par un humain) inchangé |
| SEC-36 | MEDIUM | **PDF de page : un mot géant bloque le rendu** (`pnex-pdf`, découpe d'un mot plus large que la ligne) — chaque essai remesurait tout le préfixe : un mot de 100 000 caractères occupait un thread bloquant des heures, répétable par un viewer (`GET …/versions/{n}/pdf`) | — (déni de service) | ✅ 2026-10-13 (merge `feat/media-vision-studio`) : essais bornés à 96 caractères, plafond de 1000 pages, test « mot de 100 000 caractères » |
| SEC-37 | MEDIUM | **Vision Lab : une org monopolise l'entraînement** (`services/lab_train.rs`) — un seul entraînement par serveur, contrôle « occupé » par projet : N projets = N runs en file devant les autres orgs | — (déni de service) | ✅ 2026-10-13 : un run en file ou en cours par org, runs périmés fermés par org. Résiduel accepté : l'import vidéo du Lab (1 à la fois, ≤ 300 s) peut faire attendre les autres orgs |
| SEC-38 | LOW | **PDF de page sans CSP sandbox** (`controllers/docs.rs` `version_pdf`) — en-têtes faits main, sans la CSP des contenus utilisateur | R12 | ✅ 2026-10-13 : `default-src 'none'; sandbox` ajouté (contenu généré par le serveur, nom de fichier assaini) |
| SEC-39 | LOW | **Pièce jointe : version courante de l'asset exporté** (`/internal/flow/media/{id}`) — un membre peut poser une nouvelle version (autre fichier) sur l'asset d'un PDF exporté, que `pnex_notify` enverra | R7 | accepté 2026-10-13 : même org, membre déjà autorisé à envoyer ce fichier ; piste : mémoriser `media_version_id` dans `doc_version_exports`. Noté au même endroit : l'approbation D210 est un **contrôle de processus**, pas une frontière (l'auteur du flow peut la décocher, `can_write` requis dans les deux cas) |
| SEC-40 | LOW | **Récurrences : ~90 plages par plan, sans plafond par org** (`services/ontology/recurrence.rs`) — beaucoup de plans gonflent la base et allongent le balayage commun | — (déni de service) | accepté 2026-10-13 (aucun plafond d'objets ni de plages par org, préexistant) ; à traiter avec les quotas par tier |
| SEC-41 | LOW | **Déconnexion sans `id_token` : cookie SSO Rauthy possiblement conservé** (`pnex-frontend` `state/session.rs::logout`) — sans indice, la déconnexion purge l'app mais n'appelle plus l'end-session Rauthy (qui répondait un 400 brut quand sa session était déjà morte) ; si le cookie SSO vit encore, le login suivant peut se faire sans formulaire (poste partagé) | — (session) | **corrigé** 2026-10-15 : une déconnexion sans `id_token` pose `pnex.force_login` ; le login suivant part avec `prompt=login` (Rauthy 0.36.2 ignore alors la session SSO, vérifié dans son `get_authorize`) ; le drapeau tombe au premier jeton reçu. Seule la valeur `login` est relayée par `/oauth2/sso` (test `tenant_isolation.rs`) |
| SEC-42 | LOW | **zip < 9 : extraction sur disque non confinée** (RUSTSEC-2026-0367/0368/0369 — bits SUID/SGID conservés, liens symboliques hors de la cible, permissions appliquées à travers un lien) — failles de `ZipArchive::extract` / `ZipStreamReader::extract` | — (élévation de privilèges, écriture hors cible) | ✅ 2026-10-17 : nos dépendances directes passent à zip 9 ; reste zip 8.6 via calamine 0.36.1 (lecture xlsx en mémoire, `extract()` jamais appelé, ni chez nous ni dans calamine) : exception `deny.toml` à retirer quand calamine passe à zip 9 |
| SEC-43 | LOW | **Scan LAN déclenché par un viewer** (`GET /api/v1/edge/lan-scan?prefix=`, `controllers/edge_refs.rs`) — tout membre de l'org, viewer compris, fait sonder par le serveur un /24 privé de son choix (ports 5150 et 443) ; la réponse n'identifie que des serveurs `pnex-server`, mais c'est du sondage réseau interne piloté par l'utilisateur | reconnaissance du réseau du serveur | ✅ 2026-10-17 (relevé le même jour, audit du contrat d'API) : garde `can_write()` en tête du handler → 403 `edge-write-forbidden` pour un viewer (R2) ; un `prefix` explicite hors RFC 1918 (`a.b.c.`, `Ipv4Addr::is_private`) → 400 `field-invalid` avant toute sonde ; tests `org_isolation_and_auth_401`, `lan_scan_prefix_is_rfc1918_only` (`tests/edge_refs.rs`) et `explicit_prefix_must_be_rfc1918` |
| SEC-44 | LOW | **Agents navigateur : contrôle d'usage unique contournable** (`services/extension.rs::first_use`) — la clé anti-rejeu était le hachage du texte de la signature : la même signature suivie d'un espace, ou sa forme `(r, n−s)` (ECDSA malléable, `ring` n'impose pas s bas), repassait dans la fenêtre de ±120 s ; effet : un upload capté rejoué crée un média de plus | R15 | ✅ 2026-10-18 : clé sur `r` décodé (identique pour toutes les formes d'une signature) ; test « rejouée telle quelle, avec espace, en `(r, n−s)` → 401 » (`tests/extension.rs`) |
| SEC-45 | LOW | **Agents navigateur : trames `seen` sans débit** (`controllers/ws_extension.rs`) — un agent forgé (n'importe quel inscrit, son org, sa clé Noise) pouvait insérer des lignes `social_videos` sans limite dans la base partagée | — (déni de service) | ✅ 2026-10-18 : 60 lots `seen` par minute et par lien, au-delà ignorés ; plafonds d'org 20 agents, 200 missions (`extension-limit-reached`). Registre plafonné à 500 000 posts par org (`ext-registry-full`, O89) |
| SEC-46 | LOW | **Agents navigateur : `collect_page` sur x.com / youtube.com sans consentement par site** (`pnex-core::browser_agent::check_page_url`, `manifest.json` `content_scripts`) — ces deux hôtes sont accordés à l'installation ; un membre peut garder en document une page privée du compte connecté sur l'agent (messages, historique) | R2 | **accepté** 2026-10-18 (décision utilisateur, O90) : même org, l'agent est le compte de l'org et doit pouvoir tout collecter |
| SEC-47 | LOW | **Tours publics : `share_token` stocké en clair** (`tours.share_token`, `controllers/public_tours.rs:33`) — jeton de 122 bits comparé par égalité SQL ; une fuite de la base (sauvegarde, réplica) donne l'accès à tous les tours publiés, alors que les codes d'enrôlement sont stockés hachés | R16 | ouvert (relevé 2026-10-18 à l'étude IAM, `iam.md` §5.4) |
| SEC-48 | LOW | **Historiques : auteur de version fourni par le client** (`flow_versions`, `dashboard_versions`, `tour_versions`, `annotation_layer_versions` : colonne `author` texte, remplie depuis le corps de la requête, ex. `controllers/flows/crud.rs:40`) — tout membre en écriture peut attribuer une version à quelqu'un d'autre ; l'email recopié échappe en outre à l'effacement du compte | R3 | ouvert (relevé 2026-10-19 à l'étude IAM, correctif `iam.md` D299) |
| SEC-49 | LOW | **Imports d'ontologie : l'URL source est lue par tout membre** (`object_imports.source_url`, `ObjectImportView`, partner-watch.md D301) — une clé d'API placée dans la query de l'URL serait visible de l'org entière (R4, R16) | R4, R16 | accepté (2026-10-19) : les sources livrées sont publiques et sans clé ; aide de l'écran Imports « jamais de secret dans l'adresse » ; une source authentifiée prendra une référence du coffre (D110) quand le besoin existera |
| SEC-50 | LOW | **`/ws/extension` sans vérification de l'edge device (4013)** (`controllers/ws_extension.rs`) — un agent navigateur passe par l'adresse de l'app (ingress public), jamais par l'edge mTLS des devices qui pose `X-Pnex-Edge` : la vérification fermait toute connexion (agent toujours « hors ligne » sur dev.pnex.io, 2026-10-19) | **accepté** 2026-10-19 (décision user) : admission par ticket à usage unique obtenu en HTTPS avec le jeton de l'agent, puis poignée de main Noise NNpsk0 avec sa clé (D262) ; le TLS de l'app reste assuré par l'ingress |
| SEC-26 | MEDIUM | **Clé de frame forgée dans un message de flow** (`pnex-node-camera/src/record.rs`, `pnex-node-vision/src/lib.rs`) — `video_record` et `vision_detect` faisaient `GET` de `payload.frame_key` tel que reçu ; un nœud `function` (code utilisateur) pouvait forger une clé d'une autre org (`pnex:cam:v1:{autre org}:…`) ou toute clé Valkey de type chaîne, puis la lire via l'enregistrement ou la détection | R3 | ✅ 2026-10-10 (lot 6a média) : `camera::frame_key_in_org` — clé lue seulement si elle est une clé de frame de l'org tamponnée (espace device `pnex:cam:v1:{org}:` ou flux `pnex:media:v1:{org}:`, segment `:f:`) ; test `frame_keys_are_bound_to_their_org` |
| SEC-28 | LOW | **Fil bloqué par un document hostile** (`services/doc_search.rs`, P1 de `doc-search.md`) — l'extraction (pdf-extract, calamine, quick-xml) est du code synchrone sur des octets utilisateur ; un PDF piégé peut boucler. Garde-fous : zip-bomb refusée sur le répertoire central, inflate et texte plafonnés, `spawn_blocking` (panique → `media-index-malformed`), délai de 300 s → `media-index-too-large`. Reste : le fil bloquant d'une extraction hors délai n'est pas annulable et tourne jusqu'à sa fin (sous-processus si constaté en pratique) | — (disponibilité) | accepté 2026-10-10 (shortcut commenté dans le code) |
| SEC-30 | MEDIUM | **Export COCO du Vision Lab en mémoire** (`services/lab.rs::export`) — archive construite en mémoire, déclenchable par un lecteur ; seul plafond initial : 3000 images | — (disponibilité) | ✅ 2026-10-13 : somme des `size_bytes` vérifiée avant toute lecture, 512 Mo d'images au plus → 413 `lab-export-too-large` |
| SEC-31 | MEDIUM | **Import de trames d'enregistrement : segments chargés en bloc** (`services/lab.rs::import_recording`) — toute l'heure de segments en mémoire pour garder quelques trames | — (disponibilité) | ✅ 2026-10-13 : trames choisies sur les métadonnées, segments lus un par un (un seul en mémoire), plafond 2 Gio lus par import |
| SEC-32 | MEDIUM | **Modèle de décision partagé sans quota par org** (`services/decide.rs`) — un seul calcul à la fois pour tout le serveur ; une org pouvait l'occuper en continu | — (disponibilité) | ✅ 2026-10-13 : compteur quotidien par org (Valkey, 20 000 décisions/jour) avant le sémaphore ; message d'échec de chargement générique côté org, détail dans les journaux seulement |
| SEC-33 | LOW | **État des nœuds vision par source non borné** (`pnex-node-vision`: track, zone, caption) — identifiants de source et tableaux de détections forgeables par le code utilisateur | — (disponibilité) | ✅ 2026-10-13 : 256 sources au plus par nœud (au-delà l'état repart), 300 détections par trame ; `add_images` ne lit que des `image/*` ≤ 50 Mo |
| SEC-34 | LOW | **Routes `/internal/flow/ai/*` : org crue après jeton + fencing** (`controllers/internal_flow.rs`) — un runtime compromis pourrait consommer le fournisseur LLM et le quota d'une autre org (en mono-runtime le fencing ne lie pas l'org) ; même niveau de confiance que les autres routes internes (`event`, `time-range`, `video-annotations`). La légende est en partie protégée par `frame_key_in_org` | R7 | **corrigé** 2026-10-15 : chaque route `/internal/flow/ai/*` exige un flow déployé de l'org qui contient un nœud du type appelant (`vision_caption`, `media_fuse`, `llm_text`, `text_decide`), sinon 403 `flow-ai-node-absent` (test `internal_flow.rs`) |
| SEC-51 | MEDIUM | **Requête d'ontologie = oracle des données masquées** (`services/ontology/query.rs`) — un membre sans `ontology.personal:read` voit le titre et les propriétés personnelles masqués, mais `text`, un filtre (`contains`, `eq`…) ou `order_by` sur ces valeurs changeait le résultat : on pouvait les deviner lettre par lettre (UI, assistant, MCP, ensembles d'application). Trouvé à l'essai CRM du 2026-10-26 | R2 | **corrigé** 2026-10-26 (non commité) : `query::run` reçoit la `Redaction` de l'appelant ; sur les types où la valeur est masquée, recherche et filtre ne trouvent rien, le tri la traite comme absente (test `personal_data.rs`). Les flows lisent sans masque, comme le reste de leur org |
| SEC-52 | MEDIUM | **Effacement annulable et transfert forgeable** — (1) une coquille effacée (D446) restait modifiable : un `PUT` lui rendait nom et propriétés personnelles, et elle prenait de nouveaux liens ; (2) la relève des boîtes mail croyait l'en-tête `Authentication-Results` du message : chez un hébergeur qui n'en ajoute pas (cPanel/o2switch), l'expéditeur pouvait écrire `dmarc=pass` et faire lier son mail (ou croire un faux transfert, D463). Trouvés à l'essai CRM du 2026-10-26 | R2 | **corrigé** 2026-10-26 (non commité) : (1) objet effacé en lecture seule, `ontology-erased` 409 (test `personal_data.rs`) ; (2) PneX vérifie lui-même DKIM aligné sur le `From:` (`services/mail/verify.rs`, crate `mail-auth`), l'en-tête du message n'est jamais lu (tests `verify.rs`, `parse.rs`) ; vérifié sur un vrai Gmail (pass) et un mail usurpé (none) |
| SEC-53 | MEDIUM | **Page générée depuis un objet = lecture sans masque** (`services/ontology/cases/pages.rs`, `from_object`) — un membre sans `ontology.personal:read` qui créait une page depuis un modèle (plan de compte, fiche opportunité) y trouvait en clair le titre et les propriétés personnelles de l'objet, de ses liens (contacts) et de ses hiérarchies : la page les recopiait sans le masque appliqué à ses autres lectures. Trouvé en reprenant le reste de D443 le 2026-10-27 | R2 | **corrigé** 2026-10-27 (non commité) : la page reçoit la `Redaction` de son auteur (objet, liens, hiérarchies), test `personal_data.rs` ; reste ouvert : un lecteur ultérieur qui lit moins que l'auteur voit ce que l'auteur voyait (visibilité par objet, iam.md I5) |
| SEC-54 | LOW | **Alertes zstd (RUSTSEC-2026-0391, -0398, -0403, -0410)**, job CI `deny` rouge le 2026-10-27 : zstd 0.14.0 / zstd-safe 8.0 / zstd-sys 2.1.0 au runtime (tower-http, compression), et zstd 0.13 / zstd-safe 7 via zip 2 dans le script de build de `sherpa-onnx-sys` 1.13 | — | **corrigé** 2026-10-27 au runtime (`cargo update` : zstd 0.14.3, zstd-safe 8.2, zstd-sys 2.1.2) ; reste accepté : la copie du script de build de sherpa, jamais liée ni nourrie de données non fiables (exceptions dans `deny.toml`), à retirer au passage à sherpa-onnx 2.x (zip 9) |

\* SEC-4 seul exige le jeton de service ; c'est l'amplificateur qui rend
SEC-1 / SEC-3 inter-org (actionneurs de n'importe quelle org).

**SEC-9 — Contrôle étranger dans un layout de dashboard (trouvé en route,
2026-10-04, D131).** Contrairement aux annotations, le save d'un dashboard
ne vérifiait pas que `options.control` désignait un contrôle de l'org :
un membre pouvait stocker l'UUID d'un contrôle d'une autre org. Sans impact
démontré — l'écriture (`/controls/{id}/value`) et la lecture des valeurs
restent filtrées par l'org du principal (404 / `null`) — mais la règle R1
n'était pas tenue au stockage. *Correctif* : `services/surface_controls.rs`
ne lie qu'un contrôle de l'org ; un id inconnu est remplacé par la source
propre du widget (`tests/controls.rs`,
`surface_declared_controls_are_provisioned_and_released`).

**SEC-15 — CVE Python de l'image builder (trouvé en route, 2026-10-05,
release 0.1.0-beta.3).** `trivy image` sur `pnex-builder-rs:0.1.0-beta.2`
(builder inchangé en beta.3) : 7 HIGH, 4 MEDIUM, 1 LOW, aucune CRITICAL,
toutes Python — base wolfi propre. HIGH : `urllib3` 2.7.0 (→ 2.8.0, ×2),
`starlette` 0.52.1 (→ 1.3.1, ×2), `msgpack` 1.1.2 (→ 1.2.1), `setuptools`
70.3.0 (→ 78.1.1), `ecdsa` 0.19.2 (Minerva, sans correctif amont). Deux
venvs en cause : `/opt/pio` (pip : platformio + esptool) et
`/opt/platformio/penv` (venv interne créé par pio au pré-chauffage).
*Exposition* : faible — le worker n'expose aucun service HTTP (starlette =
PIO Home, jamais lancé), `ecdsa` ne signe rien côté serveur, le builder
compile du code utilisateur dans un conteneur dédié. *Correctif
(beta.4, Dockerfile étape `builder`)* : msgpack, setuptools et urllib3 2.7.0
étaient les copies embarquées par **pip** (`pip/_vendor`, même pip 26.2.1)
dans les trois Python de l'image → pip retiré après le pré-chauffage (venv
`/opt/pio`, `penv`, paquet apk `py3.14-pip`) ; `penv` installe avec `uv`,
pio ne le réinstalle pas. `starlette` de `penv` forcé ≥ 1.3.1 malgré
l'épinglage `<1` de pioarduino (PIO Home seul l'importe, jamais lancé). Les
5 builds de pré-chauffage repassent ; trivy : 12 → 1. *Accepté* : `ecdsa`
0.19.2 (CVE-2024-23342, attaque temporelle Minerva sur la signature), exigé
par la plateforme pioarduino (`python_deps`), sans correctif amont ; le
builder ne signe rien avec. *Constat annexe* (corrigé dans la même beta.4) : le
pré-chauffage ne couvrait pas tout — libs téléchargées à chaque build,
cores pioarduino / officiel qui s'écrasaient ; builds hors ligne désormais
(custom-firmware.md §8).

**SEC-1 — RCE par le nœud `red` (confiance 9).** `FlowNodeKind::Red`
accepte tout `type_name` (`pnex-core/src/flow/graph.rs:271`) ;
`validate.rs:253-266` ne vérifie que non-vide + objet ; `red_flows.rs:361`
recopie tel quel ; le runtime (`pnex-flow-runtime/src/main.rs:154`,
`RegistryBuilder::default()`) enregistre tous les builtins dont `exec`
(`vendor/edgelinkd/…/function_nodes/exec.rs:169`, `sh -c`), non gaté par
feature (`file`/`file in` le sont : `nodes_storage` off). Tout inscrit
(owner de son org perso) déploie inject → red:exec → debug : shell sous
l'uid du serveur, lecture de `/proc/<ppid>/environ` (`DATABASE_URL`,
`PNEX_SECRETS_KEYS`), identifiants O2 root, Valkey, jetons de service.
*Correctif* : liste blanche de types `red` dans `validate.rs` (save +
deploy) et dans `red_flows.rs` ; gater `exec` derrière une feature off dans
le fork edgelinkd ; registre runtime explicite ; à terme uid / sandbox
dédiés pour le runtime.

**SEC-2 — Falsification du locataire (confiance 8).** Un `red` de type
`pnex-device-write` / `pnex-device-read` / `pnex-memory-*` (enregistrés
sous ces noms : `pnex-node-device/src/write.rs:47`, `read.rs:65`,
`pnex-node-memory/src/lib.rs:91,211`) garde son `pnex_org_id` /
`pnex_o2_org` (`red_flows.rs:361-386` n'écrase que id/z/name/x/y/wires).
Lecture/écriture de la mémoire Valkey d'une autre org, lecture de sa
télémétrie (O2 root), actionnement de ses devices via
`/internal/flow/device-write` (lookup device sur l'org falsifiée ; fencing
passant en mono-pod). Ids d'org séquentiels, slugs device énumérables.
Ne nécessite pas `exec`. *Correctif* : refuser `type_name` `pnex-*` en
`red` ; supprimer toute clé `pnex_*` puis re-tamponner depuis `meta` ;
à terme org passée hors bande au runtime, jamais lue de la config.

**SEC-3 — Environnement du runtime lisible par le JS (confiance 9).**
`vendor/edgelinkd/crates/core/src/runtime/engine.rs:115`
(`with_process_env()`), atteint via `Engine::with_json`
(`pnex-flow-runtime/src/engines.rs:404`) ; `env` global du sandbox
(`function/mod.rs:344`, `env_class.rs:24`) ; les fonctions JS du registre
deviennent ce nœud (`red_flows.rs:273-292`) sans masquer `env`. Le
superviseur injecte `OPENOBSERVE_ROOT_*`, `VALKEY_URL`,
`PNEX_FLOW_WRITE_TOKEN`, `PNEX_NOTIFY_DELIVER_TOKEN`
(`flow_supervisor/process.rs:8-80`). `return env.get("OPENOBSERVE_ROOT_PASSWORD")`
→ télémétrie de toutes les orgs. *Correctif* : magasin d'env vide ou
liste blanche dans le fork ; secrets lus puis `remove_var` au démarrage
du runtime ; à terme identifiants O2 par org ; test de régression
`env.get(...) === undefined`.

**SEC-4 — Routes internes (confiance 8).** Loco écoute `0.0.0.0`,
nginx (`deploy/edge/nginx/templates/default.conf.template:101`,
pnex-deploy `:110`, ingress Helm `/`) route `/internal/*` ;
`internal_flow.rs` / `internal_notify.rs` : un jeton plateforme unique
puis `org_id` pris du corps/query ; `fence_ok(.., None) => true`
(`services/flow_cluster/mod.rs:444`). *Correctif* : `location /internal/
{ return 404; }` + exclusion ingress (ou listener interne) ; fence
obligatoire en mode cluster ; jetons par org (HMAC d'`org_id`).

**SEC-5 — XSS via médias (confiance 8).** `controllers/media/crud.rs:92-99`
et `versions.rs:44-50` stockent `?content_type=` tel quel
(`media_sniff.rs` l'ignore) ; `public_tours.rs:212-231` (non authentifié)
sert les octets avec ce type, `inline`, sans `nosniff` ni CSP ;
`secure_headers` Loco désactivé ; SPA et jetons (`localStorage`) sur la
même origine. Upload `text/html` ou SVG en `kind=floorplan`, tour publié
et partagé, lien envoyé à une victime d'une autre org / admin plateforme →
vol des jetons. *Correctif* : liste blanche de types par `kind` à
l'écriture ; `nosniff` + `CSP: sandbox; default-src 'none'` +
`attachment` hors images sur `public_tours` et `media/content.rs` ;
`nosniff` global en nginx ; à terme origine dédiée au contenu
utilisateur.

**SEC-6 — XSS via labels pannellum (confiance 9).**
`crates/pnex-frontend/js/viewers.js:224` (`text: h.label`) et `:454`
(annotations) ; configs `viewers.js:75`, `:237` sans `escapeHTML: true` ;
pannellum 2.5.7 fait `span.innerHTML = hs.text` dans ce cas. Labels non
validés (`pnex-core/src/tour.rs`), page publique `/share/:token`
(`pages/share.rs:47`) sur l'origine de l'app. Label
`<img src=x onerror=…>` → vol de session inter-org. *Correctif* :
`escapeHTML: true` sur chaque `pannellumLib.viewer(` + test garde qui
l'exige ; option `createTooltipFunc` + `textContent`.

**SEC-7 — OTA sans garde de rôle (confiance 8).** `controllers/ota.rs`
`deploy` (~122) et `cancel` (~293) n'appellent pas `can_write()` : un
viewer re-flashe un build antérieur du device (`force: true`) ou annule
un déploiement. *Correctif* : garde `can_write` + test viewer → 403.

**SEC-8 — Identifiants device exposés aux viewers (confiance 8).**
`controllers/devices/dto.rs:146-151` sérialise `device_token {token,
encryption_key}` pour tout membre (`GET /devices`, `/devices/{id}`) ; le
binaire OTA (`ota.rs:329`) et `GET /download/firmware/{id}`
(`builds.rs:585`, sans garde de rôle) contiennent le PSK WiFi en base64,
réservé owner/admin par le coffre. *Correctif* : `device_token` à `None`
hors `can_write` (ou endpoint « révéler » dédié) ; `builds.rs::download`
sous `can_manage_secrets`.

**SEC-10 — APK distribué debuggable (confiance 9, trouvé 2026-10-04 en
validant l'app sur téléphone).** `task build:frontend:android` produit
`gradlew assembleDebug` signé avec la clé debug de dx : l'APK porte
`android:debuggable`. Conséquences : `adb shell run-as io.pnex.app cat
files/pnex-storage.json` lit access/refresh/id tokens et la CA épinglée, et
la WebView est inspectable (`webview_devtools_remote_<pid>`) — exécution de
JS arbitraire dans la session. Prérequis : accès physique au téléphone
déverrouillé avec le débogage USB actif (d'où MEDIUM). *Correctif* :
variante release (`assembleRelease`, `debuggable false`, clé de signature
de release hors dépôt) pour l'APK distribué ; la variante debuggable reste
réservée à l'APK e2e (`task build:frontend:android:e2e`, qui en a besoin
pour `run-as`).

### Correctifs (2026-10-04)

- **SEC-1 / SEC-2** — `pnex_core::flow::node_types` : `RED_ALLOWED_TYPES`
  (transformations pures) vérifiée par `validate_graph` (save + deploy,
  violation `red_type_forbidden`) ; la projection retire toute clé
  `pnex_*` d'une config `red` ; le runtime n'enregistre que
  `runtime_type_allowed` (allowlist + `function` + `pnex-*` + placeholders
  `unknown`) — un artefact ancien portant `exec` charge un nœud inerte.
  Tests : `red_node_rejects_host_effect_and_pnex_types`,
  `red_projection_strips_user_tenant_fields`, `tests/flows.rs`
  (`red_type_forbidden`), `env_isolation.rs` (exec jamais exécuté).
  **2026-10-14** : le kind `red` est retiré (aucun usage) — la surface
  disparaît ; `RED_ALLOWED_TYPES` supprimée, le registre du runtime se
  réduit aux builtins émis par la projection (`inject`, `debug`,
  `function`, `comment`, placeholders) + `pnex-*`. Tests :
  `raw_red_kind_is_refused`, `tests/flows.rs` (kind `red` → 4xx),
  `env_isolation.rs`.
- **SEC-3** — fork `vendor/edgelinkd` : `Engine::with_json` ne copie plus
  l'environnement du process dans le magasin d'env (le JS `env.get`,
  les propriétés `env` des nœuds n'y voient que l'env du flow). Test
  `env_isolation.rs` (rouge sans le correctif, vérifié).
- **SEC-4** — `location /internal/ { return 404; }` (edge + pnex-deploy) ;
  middleware `auth::internal_guard` : toute requête `/internal/*` portant
  `X-Forwarded-For` / `X-Real-IP` / `Forwarded` → 404 (couvre l'ingress
  Helm `/`). Test `internal_via_public_proxy_404`.
  *Résiduel accepté* : jeton de service unique et `fence_ok(None) = true`
  — inexploitables sans le jeton, désormais hors de portée du code
  utilisateur (SEC-1, SEC-3) et du réseau public ; jetons par org (HMAC
  d'`org_id`) = durcissement futur.
- **SEC-5** — `services::media::safe_content_type` (liste blanche images /
  vidéo, sinon `application/octet-stream`) à l'écriture et au service ;
  `user_content_headers` : `nosniff`, `CSP: default-src 'none'; sandbox`,
  `attachment` hors liste. Test `tours.rs` (plan déclaré `text/html`).
- **SEC-6** — `escapeHTML: true` sur les deux viewers pannellum ; garde
  `pnex-frontend/src/js_guard.rs` (tout viewer pannellum échappe, aucun
  puits HTML hors `innerHTML = ''`).

### Audit de release 0.1.0-beta.1 (2026-10-04)

Delta `c2d69ae..HEAD` (42 commits : assistant v2, contrôles et surfaces,
dashboards Maison, nœuds météo / ui-control, front mobile, e2e Linux) en
4 audits de domaine (assistant ; surfaces, contrôles, médias ; flows,
nœuds, runtime ; front et ponts JS), même seuil de confiance ≥ 8.
`task security:deps` vert (advisories, sources). Aucun HIGH ; aucun
contournement inter-org trouvé.

- **SEC-11** — `components/markdown.rs` : un lien ne garde sa cible que
  pour `http(s):`, `mailto:` ou une URL relative (espaces et caractères de
  contrôle retirés avant le test du schéma) ; une image n'est jamais
  chargée (son texte alternatif reste) — un `![](https://…)` injecté
  aurait exfiltré la conversation sans clic. Tests
  `unsafe_links_render_as_text`, `safe_links_are_kept`,
  `images_never_load`.
- **SEC-12** — `patch-android-manifest.py::patch_no_backup` :
  `allowBackup=false`, `fullBackupContent=false` et
  `data_extraction_rules.xml` excluant tous les domaines (sauvegarde
  cloud et transfert d'appareil).
- **SEC-13** — `services/ai/dashboard_tools.rs::coupling_conflicts` :
  re-lier un widget existant (libre ou lié à un autre contrôle) à un
  contrôle écouté par un flow déployé est refusé (`ai-flow-running`),
  comme la modification d'un widget déjà couplé. Test
  `free_widget_rebound_onto_a_running_flow_is_refused`.
- **SEC-14** — un owner (donc tout inscrit) peut viser un hôte interne
  avec `base_url` ; la réponse n'est jamais rendue (statut seul). Les
  clients LLM ne suivent plus aucune redirection. Le filtrage des plages
  privées n'est **pas** posé : un LLM local sur le LAN (Ollama) est un
  usage central en auto-hébergé — il viendra avec le résolveur filtrant
  de SEC-W3, activable en SaaS. **2026-10-07** : résolveur filtrant posé
  (`pnex_core::egress`, feature `egress`) sur les clients LLM, les canaux
  de notification et http-fetch ; politique `settings.egress` /
  `PNEX_EGRESS` : `lan` (défaut : loopback, link-local, métadonnées cloud,
  multicast et noms d'hôte sans point refusés, LAN joignable), `public`
  (plages privées refusées aussi, hébergement partagé), `open` (tests).
  Les littéraux IP (non résolus) sont vérifiés avant l'envoi et chaque
  redirection est re-vérifiée. `PNEX_EGRESS_ALLOW_HOSTS` autorise des noms
  internes choisis (`ollama`) ; leurs adresses restent filtrées. Le moteur
  de flows hérite de la politique du serveur. Hors périmètre : SMTP
  (lettre, hôte saisi par l'owner). **2026-10-15 (SEC-23)** : une IPv6
  qui embarque une IPv4 (NAT64, 6to4, IPv4-compatible) est jugée comme
  cette IPv4 ; noms `*.svc` et `*.cluster.local` refusés hors
  `PNEX_EGRESS_ALLOW_HOSTS` ; `PNEX_EGRESS_DENY_CIDRS` (ex.
  `10.42.0.0/16,10.43.0.0/16`) refuse le réseau d'infra (pods, services,
  pont Docker) sous `lan` et `public`, transmis au moteur de flows.

**À surveiller (relevés de l'audit de release, < 8)** : corps de réponse
météo non borné (taille) ; bannissement d'un fournisseur météo par
volume (UA/IP plateforme partagés) ; ~~erreurs brutes (`DbErr`) dans les
sorties d'outils de l'assistant~~ (corrigé 2026-10-07 : refus générique
`ai-tool-internal`, détail journalisé) ; ~~check « flow déployé » hors de
la transaction d'écriture (`update_flow`~~ corrigé 2026-10-07 : relu sous
le verrou de ligne, `append_version_if_stopped`) ; `update_dashboard` :
résiduel accepté (fenêtre de l'ordre de la ms, couplage porté par les
deploys d'autres flows sans verrou commun — `ai-assistant.md` §10) ;
`restore_version` d'un dashboard ne repasse pas la synchro des contrôles
(SEC-9 « stocké, non appliqué ») ; asset d'une visite publique servi à sa
version courante et non à celle publiée ; magasin desktop
`pnex-storage.json` en 0644 ; `\` non échappé dans deux ponts JS
natifs (non exploitable, R13 demande `serde_json`).

**SEC-16 — Débordement du buffer de CA dans le firmware (trouvé en route,
2026-10-07, mise en place de la démo dev.pnex.io).** `pnex_tls_init`
(`firmware/lib/pnex/src/pnex_tls.cpp:28`) décode le base64 de
`PNEX_CA_CERT` dans `s_ca_pem[2048]` **puis** compare la longueur : une
CA de plus de 2 047 octets PEM écrit au-delà du tableau. Constaté avec un
bundle de 3 racines (GTS R4 + R1 + ISRG X1, 4 615 octets) : l'ESP32-C6
plante au boot (panic, message « CA truncated » encore lisible dans la
pile). L'entrée vient de l'opérateur (`PNEX_CA_CERT_FILE`), pas d'un
utilisateur : pas d'exploitation démontrée, d'où LOW. *Correctif proposé* :
vérifier la longueur décodée (`ceil(len_b64 / 4) * 3`) avant de décoder,
ou décoder dans un buffer alloué à la taille ; côté serveur, refuser au
build une CA plus grande que la limite firmware (code machine dédié) ;
porter la limite à 4 Ko sur ESP32 pour autoriser plusieurs racines.
Contournement : une seule racine (GTS Root R4, 765 octets, sur dev.pnex.io).
*Corrigé (2026-10-08)* : `cryptoB64DecodeBounded` (`chacha_crypto.cpp`)
calcule la longueur décodée avant d'écrire et n'écrit rien quand elle ne
tient pas ; la CA est alors refusée **en échec fermé** (ni ancre ni
`setInsecure`, toute poignée de main échoue — l'ancien code retombait en
mode non vérifié) ; même décodage borné pour
WIFI_SSID / WIFI_PASSWORD / HOST / DEVICE_ID. `PNEX_CA_PEM_MAX`
(`pnex_tls.h`) passe à 4 096 sur ESP32 (deux racines), reste 2 048 sur
ESP8266. Le worker refuse au build une CA plus grande que la limite de la
puce : `build_ca_too_large` (`pnex_core::builds::device_ca_max_pem_bytes`,
test `device_ca_limit_matches_the_firmware_buffer` qui relit l'en-tête
firmware, `oversized_device_ca_is_refused_per_soc`).

**SEC-17 à SEC-20 — Revue externe du firmware (2026-10-08).** Constats
vérifiés dans le code ; plan complet et profils dans
`security-tiers.md` (D148–D152).

- **SEC-17** — `pnex_core::frame` : `base64(nonce ‖ ChaCha20(json))`,
  sans Poly1305. Le chiffrement est malléable : le JSON étant très
  prévisible, un attaquant réseau qui capture une frame peut retrouver
  le flux de clé et forger une commande de même longueur (actionneur,
  URL + sha d'OTA). Aucun compteur : une frame capturée se rejoue. Seule
  barrière en `ws://` LAN. *Correctif prévu* : ChaCha20-Poly1305 +
  compteur monotone par session dans les données authentifiées, des
  deux côtés, golden vectors partagés.
- **SEC-18** — L'OTA vérifie un sha256 livré par le même canal que
  l'ordre : intégrité, pas authenticité. Le refus d'une version plus
  ancienne n'existe que sur ESP8266 (`firmware/lib/pnex/src/pnex.cpp`).
  *Correctif prévu* : signature Ed25519 de l'image (clé publique
  embarquée, signature hors du builder) + version minimale en NVS sur
  toutes les puces.
- **SEC-19** — `pnex_config.h` : `ENCRYPTION_KEY` vide → frames en clair.
  Le serveur réel refuse, mais le firmware ne doit pas pouvoir être
  construit ainsi hors mock. *Correctif prévu* : `#error`.
- **SEC-20** — PSK WiFi, jeton et clé compilés dans l'image : un
  `esptool read_flash` les livre. Correctif = Secure Boot + Flash
  Encryption ou NVS chiffré (eFuses) : **gelé** jusqu'à la vague V4
  (D149), ESP8266 non corrigeable (profil `open` uniquement).

### Sous le seuil (confiance < 8) — à surveiller, non bloquants

| # | Conf. | Sujet | Note |
|---|---|---|---|
| SEC-W1 | 7 | Markdown de l'assistant : liens `javascript:` | → **SEC-11**, corrigé |
| SEC-W2 | 7 | Member redirige un secret du coffre vers son hôte (`notify/testing.rs:90` test-draft, http-fetch avec `Ref`) | ✅ 2026-10-07 : R9 appliquée strictement (décision produit : seuls owner/admin câblent un secret) — `binding::check` dans `store_graph_secrets` (flows, assistant compris), `notify::plan` (création, mise à jour, test-draft) ; refus `secret-destination-locked` ; tests `secrets.rs::members_keep_secrets_but_never_rewire_them`, `members_never_wire_secrets_nor_administer`, `binding::tests`. WiFi aussi (`store::save_single`) : un member garde le mot de passe, ne le remplace pas et ne renomme pas le SSID qui le porte (cas 7 du même test) |
| SEC-W3 | 6 | SSRF http-fetch avec lecture de la réponse (`pnex-node-http-fetch/src/lib.rs:370`) | ✅ 2026-10-07 : résolveur filtrant + redirections re-vérifiées (cf. SEC-14) ; test `http_fetch_refuses_loopback_under_the_lan_policy` |
| SEC-W4 | 4 | `version` non assainie dans `ota_artifact_key` (`pnex-firmware-builder/src/store.rs:104`) | inexploitable en backend `db` ; `sanitize_segment` (R18) — ✅ 2026-10-09 : `version` assainie aussi (`ota_key_sanitizes_every_segment`) |
| SEC-W5 | — | `email_verified` non exigé au rattachement de compte (`auth/provisioning.rs:117`) | sûr tant que Rauthy garantit l'email ; à exiger avant tout IdP amont — ✅ 2026-10-09 : re-liaison par email seulement si `email_verified` (Rauthy l'émet dans l'access token), sinon 401 (`relink_by_email_requires_a_verified_email`) |
| SEC-W6 | — | Firmware `setInsecure` sans `PNEX_CA_CERT_FILE` | ✅ 2026-10-07 : build `wss` sans CA refusé (`build_no_ca`, `settings.firmware.require_device_ca`, vrai par défaut, faux en test seulement) |
| SEC-W7 | 5 | Le firmware imprime l'URL WebSocket complète sur le port série, jeton du device inclus (base64) (`firmware/lib/pnex/src/pnex.cpp:397`) ; idem l'URL OTA (`pnex_ota.cpp:105`) | accès USB requis, mais les logs série sont collés tels quels dans les forums et tickets (tutoriels) ; masquer le paramètre `token` à l'impression (R16). Relevé le 2026-10-05 — ✅ 2026-10-07 : `pnex_conn_string()` renvoie l'URL au jeton masqué (`token=***`), l'URL OTA est imprimée sans sa requête |
| SEC-W8 | 5 | Fond de carte : un owner/admin peut choisir n'importe quel secret du coffre de l'org comme clé de fond ; la clé d'un fond est une clé navigateur, ajoutée à `style_url` et visible de tous les membres, viewers compris (`services/geo/providers.rs`, `basemaps`) | **accepté** 2026-10-10 (décision user L27 : clé de fond = clé navigateur, comme Google Maps) ; même niveau de confiance que l'URL d'un fournisseur LLM choisie par un admin. Piste : n'autoriser pour un fond que des secrets dédiés `geo/<nom>/` |

- **SEC-7** — `controllers/ota.rs` : `deploy` et `cancel` exigent
  `can_write` (`device-write-forbidden`).
- **SEC-8** — liste et détail des devices : `device_token` absent hors
  `can_write` ; `GET /download/firmware/{id}` exige `can_write`. Test
  `devices.rs::tenant_isolation_and_roles` (viewer : pas de jeton, 403 OTA
  et téléchargement ; owner : jeton présent).
  *Note* : un member peut toujours lancer un build et télécharger
  l'image (qui embarque le PSK WiFi) — inhérent au provisioning, rattaché
  à SEC-W2.

### Reste à faire

SEC-10 (APK release non debuggable, signature de release — accepté
pour la beta 1) ; un member peut toujours lancer un build et télécharger
l'image qui embarque le PSK WiFi (SEC-8, inhérent au provisioning) ; les
points à surveiller selon priorité produit. SEC-17 à SEC-19 : vague V1 de
`security-tiers.md` ; SEC-20 : vague V4 (eFuses, après validation
communautaire du firmware).
