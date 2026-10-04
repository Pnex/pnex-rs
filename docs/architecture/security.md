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
- **R2 — Chaque handler d'écriture commence par sa garde de rôle**
  (`can_write`, `can_administer`, `can_manage_secrets`, `PlatformAdmin`).
  Un nouvel endpoint mutateur sans garde = refus de revue. Un test de
  régression viewer → 403 accompagne toute route d'écriture nouvelle.
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
- **R9 — Un secret du coffre est lié à sa destination.** Changer l'URL,
  l'hôte ou le port d'un canal / nœud qui référence un secret exige
  `can_manage_secrets`, sinon la référence est retirée (D117 : aucun rôle
  ne relit une valeur, y compris en la redirigeant).
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
  justification d'inexploitabilité.

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

1. **Nouvelle route ?** Garde d'auth + rôle (R2), org depuis le principal
   (R1), test viewer → 403 / cross-org → 404.
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
| SEC-10 | MEDIUM | **APK Android distribué « debuggable » : session lisible par USB** | R16 | ouvert |
| SEC-11 | MEDIUM | **Markdown de l'assistant : liens `javascript:` et images distantes** (ex-SEC-W1, relevé à l'audit de release) | R11 | corrigé |
| SEC-12 | LOW | **Sauvegarde Android (auto-backup, transfert) emportait le jeton de rafraîchissement** | R16 | corrigé |
| SEC-13 | LOW | **Assistant : widget libre re-lié au contrôle d'un flow déployé** | D144 | corrigé |
| SEC-14 | LOW | **URL d'un fournisseur LLM vers un hôte interne (SSRF aveugle)** | R8 | partiel — redirections coupées ; filtrage d'adresses avec SEC-W3 |

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
- **SEC-3** — fork `vendor/edgelinkd` : `Engine::with_json` ne copie plus
  l'environnement du process dans le magasin d'env (le JS `env.get`,
  les propriétés `env` des nœuds n'y voient que l'env du flow). Test
  `env_isolation.rs` (rouge sans le correctif, vérifié).
- **SEC-4** — `location /internal/ { return 404; }` (edge + pnex-deploy) ;
  middleware `auth::internal_guard` : toute requête `/internal/*` portant
  `X-Forwarded-For` / `X-Real-IP` / `Forwarded` → 404 (couvre l'ingress
  Helm `/`). Test `interne_via_proxy_public_404`.
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
  de SEC-W3, activable en SaaS.

**À surveiller (relevés de l'audit de release, < 8)** : corps de réponse
météo non borné (taille) ; bannissement d'un fournisseur météo par
volume (UA/IP plateforme partagés) ; erreurs brutes (`DbErr`) dans les
sorties d'outils de l'assistant ; check « flow déployé » hors de la
transaction d'écriture (`update_flow`, `update_dashboard`) ;
`restore_version` d'un dashboard ne repasse pas la synchro des contrôles
(SEC-9 « stocké, non appliqué ») ; asset d'une visite publique servi à sa
version courante et non à celle publiée ; magasin desktop
`pnex-storage.json` en 0644 ; `\` non échappé dans deux ponts JS
natifs (non exploitable, R13 demande `serde_json`).

### Sous le seuil (confiance < 8) — à surveiller, non bloquants

| # | Conf. | Sujet | Note |
|---|---|---|---|
| SEC-W1 | 7 | Markdown de l'assistant : liens `javascript:` | → **SEC-11**, corrigé |
| SEC-W2 | 7 | Member redirige un secret du coffre vers son hôte (`notify/testing.rs:90` test-draft, http-fetch avec `Ref`) | contredit D117 ; lier secret ↔ destination (R9) ou documenter comme accepté |
| SEC-W3 | 6 | SSRF http-fetch avec lecture de la réponse (`pnex-node-http-fetch/src/lib.rs:370`) | impact selon l'hébergeur (métadonnées cloud) ; résolveur DNS filtrant (R8) |
| SEC-W4 | 4 | `version` non assainie dans `ota_artifact_key` (`pnex-firmware-builder/src/store.rs:104`) | inexploitable en backend `db` ; `sanitize_segment` (R18) |
| SEC-W5 | — | `email_verified` non exigé au rattachement de compte (`auth/provisioning.rs:117`) | sûr tant que Rauthy garantit l'email ; à exiger avant tout IdP amont |
| SEC-W6 | — | Firmware `setInsecure` sans `PNEX_CA_CERT_FILE` | refuser un build `wss` sans CA épinglée |

- **SEC-7** — `controllers/ota.rs` : `deploy` et `cancel` exigent
  `can_write` (`device-write-forbidden`).
- **SEC-8** — liste et détail des devices : `device_token` absent hors
  `can_write` ; `GET /download/firmware/{id}` exige `can_write`. Test
  `devices.rs::isolation_tenant_et_roles` (viewer : pas de jeton, 403 OTA
  et téléchargement ; owner : jeton présent).
  *Note* : un member peut toujours lancer un build et télécharger
  l'image (qui embarque le PSK WiFi) — inhérent au provisioning, rattaché
  à SEC-W2.

### Reste à faire

SEC-10 (APK release non debuggable, signature de release) ; SEC-14 /
SEC-W3 (résolveur filtrant) ; les points à surveiller selon priorité
produit.
