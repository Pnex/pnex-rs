# Observations de terrain — registre des constats à traiter

> Constats faits en conditions réelles (UI navigateur, e2e toolchain réelle,
> base dev). Chaque entrée : symptôme → cause racine → correction proposée →
> statut. Les entrées corrigées restent pour la traçabilité. À réviser à la
> planification de chaque phase.

## 2026-08-16 — session UI après la Phase 6 (branche `phase-6-firmware-builder`)

### O1 — Fallback d'org après re-login : atterrissage sur une org viewer

- **Symptômes** (vécus via UI, alice) : liste devices vide, page Builds
  vide, création de device impossible (403) — alors que l'API répond 200
  partout et que la base contient les devices.
- **Chaîne complète** (constatée dans les logs serveur) :
  1. session expirée → `user-info` 401 → `refresh` 400 → auto-logout ;
  2. `session::logout()` appelle `org::clear()` → la dernière org
     sélectionnée (`pnex.org`) est **purgée** du localStorage ;
  3. au re-login, `org::restore()` retombe sur `memberships.first()` — le
     code suppose que la première org est « l'org personnelle JIT » ;
  4. sauf que `user-info` liste les memberships **sans `ORDER BY`**
     (`user_info.rs:51`) → ordre arbitraire Postgres, constaté `2, 4, 5, 1`
     → la première org est « Hack Co » où alice est **viewer** (0 device,
     création interdite).
- **Correction proposée** (petit commit, non appliqué) :
  - backend : `order_by_asc(organizations::Id)` sur la requête memberships
    de `user-info` — l'org personnelle JIT (créée à la première connexion,
    plus petite id en pratique) sort en premier, ordre déterministe garanti ;
  - front : le fallback de `restore()` préfère la première membership dont
    le rôle n'est **pas** `viewer` — robuste même si l'ordre change.
- **Fichiers** : `crates/pnex-backend/src/controllers/user_info.rs:51`,
  `crates/pnex-frontend/src/state/org.rs:30`,
  `crates/pnex-frontend/src/state/session.rs:52`.
- **Statut** : ✅ résolu (2026-08-17, itération UI Phase 6) — backend :
  `order_by_asc(OrgId)` sur les memberships de `user-info` (org personnelle
  JIT d'abord, ordre déterministe) ; front : le fallback de `org::restore()`
  préfère la première membership non-viewer.

### O2 — `loco start` seul ne drive pas la queue : builds bloqués `queued`

- **Symptôme** : `POST /build-firmware` → 201, le record reste `queued`
  indéfiniment, rien ne se passe côté UI (polling qui tourne dans le vide).
- **Cause** : `loco start` = ServerOnly ; sans `--server-and-worker`,
  personne ne consomme `pg_loco_queue`.
- **Corrigé** (commit `81c37a2`) : `task dev` → `dev:backend` lance
  `--server-and-worker` ; l'ancien comportement devient
  `dev:backend:server-only`. Rappel déploiement : le process prod doit être
  lancé avec worker (déjà consigné dans `firmware-build.md`).
- **Statut** : ✅ résolu.

### O3 — Auto-build proposé pour les devices custom (`custom_sensor`, `custom_device`)

- **Symptôme** : build lancé depuis l'UI sur un device custom → échec
  immédiat (~8 ms, « projet introuvable ») : le workspace firmware ne
  contient que `soil_sensor`, `4_chan_relay`, `tft_dev`
  (`pipeline.rs:259` vérifie `{project}/platformio.ini`).
- **Parité avec le POC initial** : le POC ne filtre pas non plus — le
  `predefined_device_name` part tel quel au job k8s, qui échoue pareil.
  Comportement backend conforme ; échec propre, file saine.
- **Amélioration UX** (directive déjà consignée `firmware-build.md` §3) :
  ne pas proposer « Compiler maintenant » à l'enregistrement d'un
  `custom_*` ni dans le formulaire Builds ; afficher à la place le
  **snippet de configuration** du code source pour guider l'utilisateur.
- **Statut** : ✅ résolu (2026-08-17, itération UI Phase 6) — le wizard
  n'a pas d'étape WiFi pour les custom (écran token + script Python
  publisher interpolé à la place) et le bouton « Recompiler » de la liste
  est masqué quand `allow_dynamic_measurements`.

### O4 — Raison d'échec d'un build invisible dans l'UI

- **Symptôme** : badge `failed` sans explication ; la cause ne vit que
  dans les logs serveur (tracing worker). `build_records` n'a pas de
  colonne d'erreur, et le front n'a rien à afficher.
- **Pistes** : colonne `last_error` sur le record (texte borné — le
  pipeline capture déjà la queue des 30 dernières lignes) exposée dans le
  DTO ; et/ou logs builds → OpenObserve (différé existant, recoupe).
- **Statut** : à traiter.

### O5 — Bruit `/_dioxus?build_id=0` en build debug servi par Loco

- Le bundle front **debug** (`task dev:frontend`, `dx build` sans
  `--release`) embarque le client hot-reload qui interpelle `/_dioxus` en
  boucle (~2 req/s) ; le fallback statique de Loco répond `index.html` 200
  à chaque fois.
- **Impact** : bruit dans les logs serveur, requêtes inutiles. Sans
  gravité.
- **Pistes** (priorité basse) : filtrer le chemin côté serveur, ou
  désactiver le client hot-reload dans les builds debug hors `dx serve`.
- **Statut** : noté, priorité basse.

### O6 — Résidus de test dans la base dev

- Org 1 accumule 66 devices dont des `seed-*` (métadonnée
  `{"seeded": true}`) ; les orgs « E2E Org » / « E2E Org 2 » (4, 5)
  restent d'un e2e Phase 6. Sans impact fonctionnel, mais fausse les
  démos et le `device_count` de `user-info`.
- **Piste** : purge SQL ciblée ou reset db dev.
- **Statut** : noté, cosmétique.

### O7 — `task dev` sans toolchain épinglée : builds soil_sensor échouent

- **Symptôme** (vécu 2026-08-17) : builds lancés depuis l'UI sur le serveur
  `task dev` — `4_chan_relay` réussit, `soil_sensor` échoue en ~2 s
  (`silly-yak`, job 53 ms côté worker). Le même projet compile en 5,9 s
  avec le venv `pio` du dépôt firmwares.
- **Cause** : le serveur hérite du shell utilisateur ; `pio` résolu sur le
  PATH peut être une installation divergente de celle du venv
  `pnex-firmwares/.venv` (la config connue-bonne). Mes serveurs de debug
  portaient `PNEX_PIO_CMD`/`PNEX_ESPTOOL_CMD`/`PNEX_ARTIFACTS_DIR`,
  `task dev` non.
- **Corrigé** : `dev:backend` (Taskfile) évalue ces trois variables —
  venv du dépôt firmwares s'il existe (`$HOME`-based), fallback `pio` du
  PATH. Les déploiements prod documentent déjà ces variables
  (`firmware-build.md`, `.env.example`).
- **Statut** : ✅ résolu (2026-08-17).

## 2026-09-14 — upgrade OpenObserve v0.92.1 → v1.0.0 (drop-in validé)

Upgrade du compose (`openobserve/openobserve:v1.0.0`) validée par passe
complète : migrations DB 64→77 propres, orgs/streams/file_list intacts,
roundtrip remote_write → memtable → `query_range` OK en ~3 s, persist
WAL→parquet OK. Le backend pnex n'est pas affecté : il n'utilise jamais
`_search` — seulement `remote_write` (prompb v1, inchangé) + PromQL
`query_range` (protocole Prometheus, secondes, inchangé).

### O8 — `_search` v1.0.0 : timestamps en microsecondes (breaking change)

- **Symptôme** : toute query `_search` écrite comme en 0.92 (start_time/
  end_time en **ms**) renvoie 0 hits sans erreur — ni 400, ni message.
  Les logs flight montrent `get file_list … files: 0` sur des streams
  qui en contiennent des centaines.
- **Cause** : l'API `_search` v1.0.0 attend des **microsecondes** (la doc
  de l'endpoint le dit : « need to be a valid micro timestamp »). Une
  plage en ms (~1.8e12) est interprétée comme µs → fenêtre en 1970 →
  0 fichier ne matche, silencieusement.
- **À retenir** : tout script/test tapant `_search` directement doit
  passer en µs (`Date.now() * 1000`). Le backend n'est pas concerné
  (`services/openobserve/client.rs` ne passe que par PromQL).
- **Statut** : noté (aucun usage `_search` dans le repo).

### O9 — le cache de résultats O2 survit à l'upgrade et fausse la validation

- **Symptôme** : après l'upgrade, une query PromQL sur une fenêtre
  « froide » renvoyait des points — alors que le lookup file_list du
  même instant renvoyait 0 fichiers. Suspicion (fausse) de perte de
  données.
- **Cause** : le cache de résultats persisté sur le volume
  (`pnex-rust_pnex-o2data` → `/data/cache/results`) est réutilisé par la
  nouvelle version ; les logs montrent `hit full cache`,
  `cache ratio: 100.35 %`. Les points venaient de requêtes pré-upgrade.
- **À retenir** : pour valider la lecture parquet réelle après une
  upgrade O2, requêter une fenêtre **jamais demandée** (dates arbitraires
  + step impair, ex. 537 s) — pas une fenêtre déjà couverte par le
  dashboard/ETL. Le cache vit dans le volume, pas en mémoire.
- **Statut** : noté (comportement normal, pas un bug).

### Divers diagnostic O2 (constats de session)

- `POST /api/{org}/prometheus/api/v1/write` en 400 « invalid wire type »
  = protobuf client malformé (vérifier : séries enveloppées dans le
  WriteRequest field 1, labels **à plat** sur la TimeSeries,
  `Sample.value` = double wire type 1, `Sample.timestamp` = varint
  scalaire). Un payload invalide mais décodable peut répondre 200 en
  ne rien stocker — toujours valider par une lecture.
- Le conteneur O2 est distroless (pas de `sh`) : pour fouiller le volume
  passer par `docker run --rm -v pnex-rust_pnex-o2data:/d busybox …`.
- Le replay WAL est **asynchrone** au boot (une query immédiate après
  un restart peut rater des données encore en WAL).

## 2026-09-30 — E2E réels pour la doc pnex.io (prédictif, notifications ntfy, caméra)

Contexte : flow #16 « Demo – predictive maintenance » construit par l'UI sur la
stack docker (inject 1 s → fonction JS « Demo – simulated pump » → anomaly +
forecast → 2 nœuds Notification ntfy + métriques), alertes reçues sur un vrai
téléphone. Les nœuds prédictifs eux-mêmes sont justes (pics détectés score ~38,
ETA forecast 791 s prédit pour 800 s réels) ; les défauts sont autour.

### O10 — Notification : un set différé part plus tard avec des valeurs périmées

- **Symptôme** (vécu, téléphone) : alerte « Bearing temperature forecast to
  reach 80 °C in about **0 min** » envoyée à 21:08 alors que la prévision
  courante donnait ~17 min. La valeur 0 datait de 20:59 (cycle précédent).
- **Cause** : `fill_vars` remplit les variables avec `or_insert` → **première
  valeur gagnante**. En mode ancres + trigger, un set complet reste en attente
  tant que le trigger est désarmé ou que l'anti-spam bloque (« différé, jamais
  perdu ») ; les valeurs plus récentes sont ignorées ; au réarmement suivant
  (`trigger_gate_update` → `Commit`), c'est ce vieux set qui est rendu et
  envoyé.
- **Correction proposée** : dernière valeur gagnante (`insert`) pour les
  variables d'un set en attente ; garder « jamais perdu » au sens « la
  notification finit par partir », pas « avec des données figées ».
  Vérifier la cohérence avec D66–D68 (`docs/architecture/notifications.md`) et
  mettre à jour la doc du module (en-tête « première valeur gagnante »).
- **Tests** : unitaire `fill_vars` (2 remplissages successifs → la 2e valeur
  reste) ; scénario trigger faux → set complet → nouvelles valeurs → trigger
  vrai → le rendu porte les dernières valeurs ; idem set bloqué par l'anti-spam
  puis fenêtre expirée.
- **Fichiers** : `crates/pnex-node-notify/src/lib.rs` (`fill_vars` l. ~546,
  `execute` accumulation l. ~376, anti-spam l. ~437).
- **Statut** : corrigé 2026-09-30 — `fill_vars` en dernière valeur gagnante,
  tests d'état (`disarmed_set_renders_latest_values_on_commit`,
  `anti_spam_deferred_set_keeps_refreshing`) ; amendement dans
  `docs/architecture/notifications.md` (§ D66–D68).

### O11 — Notification : le trigger arrivé avant la variable envoie la valeur du tick précédent

- **Symptôme** : alerte « Unusual vibration on pump P-101: **2.06** mm/s »
  (valeur normale) alors que le pic qui a déclenché l'anomalie était ~11 ;
  d'autres fois la bonne valeur (10.71) — dépend de l'ordre d'arrivée.
- **Cause** : la même sortie de fonction alimente la variable `vibration` du
  nœud Notification **et** le nœud Anomaly dont la sortie booléenne est le
  trigger. Si le trigger (échantillon t) arrive avant la variable (t), le
  `Commit` envoie le set en attente (t−1) ; la valeur t arrive ensuite, set
  complet armé, mais l'anti-spam bloque. Course du fan-out edgelink (déjà
  notée comme « ordre des ports » dans la mémoire projet).
- **Correction proposée** (à arbitrer) : à l'armement, ne pas committer
  immédiatement un set déjà envoyable mais attendre le prochain set complet
  pendant une courte fenêtre (ex. 1 tick / 250 ms), ou horodater le set et ne
  committer que s'il est postérieur au front montant. O10 seul ne suffit pas.
- **Fichiers** : `crates/pnex-node-notify/src/lib.rs` (`trigger_gate_update`,
  branche trigger de `execute`).
- **Statut** : corrigé 2026-09-30 — commit différé : un trigger armé sur un
  set en attente planifie le rendu à +250 ms (`COMMIT_GRACE`), exécuté par
  une tâche dédiée du nœud ; les données arrivées entre-temps rafraîchissent
  le set. Tests `trigger_before_data_commits_the_same_tick_values`,
  `data_before_trigger_still_commits`. Reste : validation E2E ntfy réelle.

### O12 — Templates : seule la forme nue `{{ x }}` déclare/lie une variable

- **Symptôme** : template « …in {{ (eta / 60) | int }} min » ou
  « {{ eta | round | int }} » → aperçu en erreur « tried to use / operator on
  unsupported types undefined and number » / « cannot round value
  (undefined) » alors que `eta` est déclarée avec un exemple. Au runtime
  l'ancre n'existe pas non plus. Contournement utilisé : fonction JS en amont.
- **Cause** : `push_template_var` n'accepte qu'un identifiant nu ou
  `vars.<leaf>` ; toute expression/filtre est ignorée par le scan, donc la
  variable n'est ni stampée ni injectée dans le contexte.
- **Correction proposée** : extraire les variables via minijinja
  (`Template::undeclared_variables(false)`, en filtrant `msg`/`meta`/`vars`)
  plutôt que par scan textuel ; mêmes règles snake_case. Garder le scan
  partagé front (wasm) / backend / runtime cohérent.
- **Tests** : `{{ x | round }}`, `{{ (x / 60) | int }}`, `{{ x if y else z }}`,
  `{{ vars.x | upper }}` → variables `x`, `y`, `z` déclarées.
- **Fichiers** : `crates/pnex-notify/src/render.rs` (`scan_template_vars`,
  `push_template_var`), appelants côté front (preview) et runtime.
- **Statut** : corrigé 2026-09-30 — `template_vars` via
  `undeclared_variables` minijinja (globals, `{% set %}`, variables de boucle
  filtrés ; ordre d'apparition conservé) ; exemples/overrides numériques
  canoniques passés en nombres au rendu. Les templates existants gardent
  leurs vars stockées jusqu'à la prochaine sauvegarde (re-sauver + redéployer).

### O13 — Erreur de rendu de template affichée en français dans l'UI anglaise

- **Symptôme** : UI en-US, aperçu d'un template invalide → « **rendu du
  template :** invalid operation… ».
- **Cause** : `#[error("rendu du template : {0}")]` + autres messages FR dans
  le crate ; viole la règle i18n (code machine + description anglaise
  canonique, résolution front via `error_i18n.rs`) et la règle « code et logs
  en anglais ».
- **Correction proposée** : messages anglais ; si l'erreur remonte à l'API,
  code machine dans `pnex_core::err_codes::ALL` + clé `err-<code>` dans les deux
  `.ftl` (garde `error_codes.rs`) ; le détail minijinja reste verbatim.
  Profiter du passage pour les `log::warn!` FR de `pnex-node-notify`
  (« envoi bloqué », « rendu impossible »…).
- **Fichiers** : `crates/pnex-notify/src/error.rs:11`,
  `crates/pnex-node-notify/src/lib.rs`.
- **Statut** : corrigé 2026-09-30 — messages anglais dans `pnex-notify` et
  `pnex-node-notify` ; code `notify-template-render` (+ `err-notify-template-render`
  fr/en) sur preview, test-send et save ; front localisé au render. Reste hors
  périmètre : `field_status("kind", "Type de canal inconnu.")` dans
  `controllers/notify/testing.rs`.

### O14 — Flow « Demo – button to LED » : notifications en boucle avec le device hors ligne

- **Symptôme** : proud-ibex (NodeMCU) hors ligne ; le téléphone reçoit 3
  notifications « Button pressed on proud-ibex » toutes les 10 min (20:55,
  21:05…), sans aucun appui.
- **Cause supposée** (à confirmer) : `pnex-device-read` (fraîcheur 5 s dans ce
  flow) renvoie encore une valeur périmée de D3 (= appuyé, bouton actif bas) au
  lieu de ne rien émettre ; l'anti-spam (3 / 10 min) rythme la rafale.
- **À vérifier** : comportement cache-first Valkey (`SET EX 3900`) vs réglage
  de fraîcheur du nœud quand le device est déconnecté ; un device hors ligne
  ne doit pas produire de lectures « fraîches ».
- **Fichiers** : `crates/pnex-node-device/src/read.rs`, mémoire
  `pnex-device-read-freshness-batch`.
- **Statut** : corrigé 2026-09-30. Cause réelle : pas de valeur périmée
  (le cache Valkey vérifie bien `ts_ms`) mais le nœud émettait `payload =
  null` pour une pin sans valeur fraîche ; la fonction Starlark (`pressed =
  not inputs["button"]`) lisait `not None` = appuyé, rythmé par l'anti-spam.
  Fix `read.rs` : une pin sans valeur fraîche n'émet **rien** ; le repli O2
  applique la même fraîcheur (`last_cache::resolve`). Changement de
  contrat : plus de `null` sur timeout. Reste : redéployer le flow et
  vérifier proud-ibex hors ligne ; durcir la fonction démo (`== False`).

### O15 — ESP32-CAM : LED flash (GPIO4) et LED rouge (GPIO33) non pilotables sans détour

- **Symptôme** (remonté par l'utilisateur) : « aucune fonction pour
  activer/désactiver la LED intégrée ».
- **Constat** : les deux pins sont bien provisionnées sur proud-robin mais en
  mode `digital_in` / rôle `sensor`, étiquetées « IO4 » et « LED33 ». Le seul
  chemin : ouvrir le device, cliquer la pin, passer en `digital_out`,
  « Apply mode », puis écriture manuelle ou nœud Device (write). Rien dans
  /cameras.
- **Correction proposée** :
  - profil board `generic_esp32cam` : GPIO4 = sortie par défaut, libellé
    « Flash LED » ; GPIO33 = sortie **inversée**, libellé « Red LED » ;
  - /cameras : interrupteur « Flash » (modale Direct et/ou Réglages) qui
    passe par le chemin d'écriture de pin existant (porte 409 / une seule
    source d'écriture, cf. mémoire `pnex-pin-exclusivite-etat`) ; UI seule
    (jamais de CLI/API manuel) ; clés i18n dans les deux `.ftl` ;
  - vérifier côté firmware que le module caméra ne réserve pas GPIO4 (bus SD
    non utilisé) et qu'allumer le flash ne perturbe pas le capteur.
- **Fichiers** : profil board ESP32-CAM (`pnex-core/src/boards.rs` / seed),
  `crates/pnex-frontend/src/pages/cameras/`, firmware
  `firmware/lib/pnex/src/pnex_camera.{h,cpp}`.
- **Statut** : livré 2026-09-30 (non testé matériel). `BoardProfilePin`
  gagne `default_mode` / `safe_state` / `active_low` (optionnels, v1/v2
  intacts) appliqués au **premier** provisioning si les caps le permettent ;
  profil `esp32cam-ai-thinker` : GPIO4 « Flash LED » digital_out safe low,
  GPIO33 « Red LED » digital_out safe high active_low. /cameras : `FlashToggle`
  (modale Live, owner/admin) via `api::pins::command` ; pin encore en entrée →
  bouton « Activer le contrôle du flash » (409 = liste des flows). Firmware :
  GPIO4/33 non réservés (XCLK = LEDC timer 0 / canal 0 — risque si GPIO4 passe
  en `pwm_out`). Reste : E2E ESP32-CAM, reseed mcu_boards (libellés
  proud-robin), pinout non provisionné ignore `default_mode`, pas d'UI pour
  `active_low` / LED rouge.

### O16 — Mineurs constatés

- **Forecast** : horizon par défaut 48 pas = 48 s sur une série à 1 Hz, inutile
  pour une usure lente ; proposer un défaut plus grand ou un conseil dans
  l'inspecteur selon la cadence observée (`step_secs`).
- **Debug / node-status** : un Forecast à horizon 1000 produit un détail > 8 Ko
  (un point par pas) → le Debug n'affiche qu'un `{"preview": …}` tronqué ;
  envisager de plafonner/échantillonner `points` dans le détail.
- **Inspecteur Function** : sur un nœud fonction déployé (« ETA in minutes »,
  function_id 7, v2 épinglée), le select « Function » affiche « Select a
  function… » alors que la version épinglée est bien listée — select fantôme
  (cf. mémoire `pnex-select-fantome-dioxus`).
- **Flow editor** : un nœud posé près du bord droit a son port de sortie sous
  l'inspecteur ; le clic sélectionne le nœud au lieu de tirer un fil (gêne
  seulement l'automatisation, mais aussi l'utilisateur sur petit écran).
- **Statut** : corrigé 2026-09-30 (non testé en navigateur) — horizon par
  défaut 300 (`FORECAST_DEFAULT_HORIZON`) + indication de durée couverte dans
  l'inspecteur (conseil selon `step_secs` observé non fait : pas de plomberie
  node-status) ; `cap_message` sous-échantillonne les longs tableaux (≤ 100
  points, `"downsampled": true`) avant la troncature ; `FunctionPicker` suivi
  par id + `current_id` ; `pan_to_reveal_node` décale le canvas si le port de
  sortie passe sous l'inspecteur.
- **Redeploy sans changement** : « Deploy » sur une version déjà déployée ne
  recrée pas l'état des nœuds (anti-spam, set en attente) — attendu ou non,
  à documenter.
- **Statut** : backlog.

## 2026-10-02/03 — harnais E2E Playwright (`e2e/`) + banc matériel C3 / ESP32-CAM

> Session autonome : suite Playwright sur la stack conteneurisée (edge TLS),
> org de test dédiée « E2E », cartes réelles (`/dev/ttyACM0` ESP32-C3,
> `/dev/ttyUSB0` ESP32-CAM-MB). Mode d'emploi : `e2e/README.md`.

### O17 — Corrigés pendant la session (traçabilité)

Tous trouvés par la suite e2e, chacun couvert par un test (unitaire ou e2e) :

- `0305f4a` template Starlark « seuil » sans `@input threshold` ;
  `08895d6` test live JS : exception QuickJS générique, message FR ;
  `fb07c41` éditeur de fonctions : le code **sauvé** était remplacé par
  l'ancien après save (le save suivant écrasait la nouvelle version) ;
- `f1369de`, `003ab0c` titre d'éditeur (flow, dashboard, tour) figé après
  renommage ; `5e0893e` suppression d'un flow **arrêté** = attente d'ack 10 s ;
- `e243f7d` client API : `request::<()>` rejetait les 204 (suppression de
  canal/template, renommage d'org, membre : action faite, UI en erreur) ;
  `f4f33db` suppression d'org depuis l'UI jamais envoyée ;
- `3d7dcad` + `f3cf0c9` notification : template **sans variables**
  impossible à choisir, puis jamais envoyé (aucune ancre de données) ;
- `e811fbf` firmware caméra : trames > 16 Ko tronquées en wss → uplink mort
  après 2–3 trames (~0,3 fps) — fragmentation WS, 4,8 fps mesurés ;
- `82ad947` build Docker : wasm-opt retéléchargé à chaque build ;
- accessibilité (rôles dialog, labels, selects nommés) : `8aa7d3b`,
  `f8a41e0`, `aaab578`, `bb674cc`, `a407320`, `c9410b1`, `e6d39d9`,
  `025b2c1`, `58a2d2b`, `81deb4b` ; `2b98f37` « (Phase 4) » dans l'UI.
- **Statut** : ✅ résolu.

### O18 — Base de dev `pnex` antérieure à D120 : la stack HEAD tourne sur `pnex_e2e`

- **Symptôme** : l'image HEAD refuse de démarrer sur `pnex` (« Migration
  file … is missing ») — base créée avant le squash des migrations (D120).
- **État laissé** : `pnex` intacte (+ dump `~/.cache/pnex-e2e/db-backups/`),
  HEAD sur la base `pnex_e2e` via `PNEX_DATABASE_URL` / `PNEX_VALKEY_URL`
  (surcharge ajoutée dans `compose.app.yaml`), Valkey db 1, ids d'org ≥ 1000.
  Un `task app:up` **sans** ces variables repart sur `pnex` et plante.
- **À faire** : trancher (décision roadmap #12) — `db:reset` de `pnex`
  (doctrine D120) ou adopter `pnex_e2e` comme base de dev. Les devices de
  l'ancienne org (quiet-puffin, proud-robin) n'existent que dans `pnex` ;
  leurs flashs d'origine sont dans `~/.cache/pnex-e2e/flash-backups/`.
- **Décision (2026-10-03)** : rien en prod avant la 1re release — `pnex`
  et `pnex_e2e` détruites, `pnex` recréée (doctrine D120), Valkey vidée.
- **Statut** : ✅ résolu.

### O19 — Jetons device en clair dans l'URL des WebSockets (logs nginx)

- **Constat** : `/ws/device?token=…` et `/ws/camera?token=…` (base64 du
  jeton de provisioning) apparaissent tels quels dans les access logs de
  l'edge nginx — toute personne qui lit les logs peut rejouer un device.
- **Pistes** : masquer la query string dans le `log_format` de l'edge
  (rapide) ; à terme, authentifier par en-tête ou premier message chiffré
  (impact firmware + contrat, à concevoir).
- **Fait (2026-10-03)** : edge nginx (`2a80844`) — access log sans query
  string (`pnex_noqs`, posé par bloc `server` : au niveau http il
  s'ajouterait au `main` de l'image), error log de `/ws/` en `crit` (les
  lignes d'erreur nginx citent la requête complète). pnex-deploy : même
  traitement en compose ; chart Helm = Ingress `/ws/` dédié sans access log
  (ingress-nginx, Traefik ≥ 3.1), HAProxy documenté (`%HPO`). Vérifié sur
  nginx 1.29 : `?token=` absent des deux logs.
- **Reste (à concevoir)** : auth par en-tête / premier message, le jeton ne
  passerait plus du tout par l'URL.
- **Statut** : ✅ résolu (fuite des logs) ; refonte d'auth = backlog.

### O20 — Firmware : handshake WebSocket limité à 1 s (ArduinoWebsockets)

- **Constat** : `_CONNECTION_TIMEOUT` = 1000 ms en dur (`ws_config_defs.hpp`,
  non surchargeable par `-D`) pour lire la réponse 101. Sur un WiFi à RTT
  élevé (100–500 ms mesurés à −70 dBm), l'ESP abandonne avant la réponse →
  `499` côté nginx, reconnexions en boucle.
- **Pistes** : patch de la lib (extra_script PlatformIO ou fork vendu dans
  `firmware/common_libs`), ou lib WS alternative.
- **À auditer avec** : toute écriture > 16 Ko sur `/ws/device` (même cause
  que la caméra, O17 `e811fbf`) — aujourd'hui les messages de contrôle sont
  petits, mais rien ne le garantit (gros announce, futures commandes).
- **Fait (2026-10-03, `6a0a929`)** : tous les clients WS PneX (transport,
  caméra, TLS « lean » 8266) passent par notre propre client TCP
  (`firmware/lib/pnex/src/pnex_ws_tcp.h`) dont `readLine()` attend
  `PNEX_WS_HANDSHAKE_TIMEOUT_MS` (5000, surchargeable `-D`) en rendant la
  main (`delay(1)` : une boucle active de 5 s déclencherait le WDT 8266).
  La lib n'est ni vendue ni patchée : elle est **GPL-3.0** (et non MIT).
- **Licence (tranché 2026-10-03)** : ArduinoWebsockets GPL-3 ⇒ chaque binaire
  firmware compilé était GPL. **Remplacée** par un client RFC 6455 maison,
  `firmware/lib/pnex/src/pnex_ws.{h,cpp}` (Apache-2.0 comme la lib) ;
  `pnex_ws_tcp.*` supprimé, dépendance retirée de tous les `platformio.ini`
  et de `library.json`. Alternatives écartées après lecture des LICENSE :
  Links2004/arduinoWebSockets (LGPL-2.1), PicoWebsocket (LGPL-3.0),
  Arduino-Websocket-Fast (MIT mais texte seul, non maintenue),
  ArduinoHttpClient (Apache-2.0 mais messages tronqués à 128 o et écriture
  en un bloc), esp_websocket_client / courier (ESP32 seulement).
  Le client : handshake patient 5 s + contrôle `Sec-WebSocket-Accept`,
  pong automatique, fragments serveur réassemblés, envoi par blocs masqués
  avec reprise des écritures partielles (fin de la troncature 16 Ko pour
  **toute** écriture, `/ws/device` compris), CA appliquée aussi sur la WS
  ESP8266. Firmwares plus légers (−17 Ko 8266, −22 Ko ESP32).
- **Règle** : toute brique tierce ajoutée doit avoir une licence compatible,
  la plus permissive possible (MIT / BSD / Apache-2.0) — LICENSE lu, pas
  un résumé.
- **Statut** : ✅ résolu.

### O21 — Dette d'accessibilité restante

- **Constat** : ~149 `<label>` non rattachés à leur contrôle sur 44 fichiers
  (formulaires de pages et de modales) ; les tests contournent avec
  `fieldAfterLabel`. Rapport axe (`task e2e -- --project=a11y`, état initial
  des routes seulement) : `color-contrast` sur 7 routes, `empty-table-header`
  (cameras, system), `link-in-text-block` (system).
- **Piste** : un composant `FormField` du socle CRUD (label + contrôle liés
  par id) puis migration page par page ; passer le projet a11y en strict
  (`PNEX_E2E_A11Y_STRICT=1`) une fois la dette résorbée ; auditer aussi les
  modales ouvertes.
- **Statut** : ouvert (partiellement corrigé, cf. O17).

### O22 — Rebuild = même build record = même version firmware

- **Constat** : « Rebuild » réutilise l'enregistrement de build (même id) ;
  la version firmware étant cet id, deux builds différents ont la même
  version → l'OTA d'après rebuild est toujours un « redeploy forcé », et
  l'UI ne peut pas dire si le device tourne le dernier binaire.
- **Fait (2026-10-03, `a239c87`)** : chaque build = un nouvel enregistrement
  (nouvel id = nouvelle version numérique croissante, compatible garde
  anti-downgrade 8266) ; 409 `build-in-progress` si un build du device est
  en cours ; rétention 5 builds/device (jamais la version en service ni une
  cible OTA active). Page Devices : version en service vs dernier build +
  badge (à jour / mise à jour dispo / build en cours / échec / hors ligne),
  sélection multiple, « Builder la sélection » / « OTA de la sélection »
  avec plan par device confirmé par l'opérateur — rien d'automatique, un
  build ne déclenche jamais d'OTA. Le device annonçait déjà sa version
  (`Announce.fw` → `device_registries.fw_version`).
- **Statut** : ✅ résolu.

### O23 — Notification sans variables : envoi à chaque `trigger = true`

- **Constat** : depuis `f3cf0c9`, un template sans variables part à chaque
  trigger armé — un Inject à 5 s qui maintient l'alarme vraie envoie un mail
  toutes les 5 s (cohérent avec le mode à variables, mais bruyant).
- **Décision + fait (2026-10-03)** : envoi sur **front montant** uniquement
  (false/non reconnu → true), un `false` réarme ; l'anti-spam reste
  optionnel (alarme qui oscille). Cf. notifications.md.
- **Statut** : ✅ résolu.

### O24 — Mineurs constatés

- **Diagnostic du runtime de flows en conteneur** : aucun log exploitable
  (logger `info`, rien sur la livraison des notifications) ; le diagnostic a
  dû passer par `flows.json` et des nœuds Debug ajoutés via l'API. Prévoir
  un niveau/flux de logs runtime activable.
- **Catalogue télémétrie** : `last_seen` d'une série reste « frais » sans
  nouveau point (présence du device ≠ dernière mesure) — libellé trompeur
  dans Quick charts.
- **Cameras** : colonne « Last frame » reste « No frame received yet »
  pendant le streaming (pas de rafraîchissement).
- **Studio** : plan importé sans dimensions (`width`/`height` null,
  l'inspecteur affiche 0) ; les scènes ajoutées s'empilent au centre du plan
  (1000,500 puis 1024,524) — à placer à la main ou mieux répartir.
- **Accueil** : « Current tier — » en self-hosted (aucun tier seedé) et
  « Active devices 1 » à côté de « Devices online 2/2 ».
- **Dashboard Value** : l'unité apparaît deux fois (en-tête et sous la valeur).
- **Build Docker** : l'étage `agent-dist` (apt-get mingw) est invalidé à
  chaque changement de source (≈ +10 Go de cache par série de rebuilds,
  50 Go consommés sur la session) — sortir l'apt-get avant `COPY . .`.
- **Fait (2026-10-03, `f0d3bf3`, `e814f1d`)** : tous les points ci-dessus —
  `PNEX_FLOW_LOG` relayé (compose) et niveaux du runtime respectés ;
  `last_seen` = vrai dernier point (SQL O2, le PromQL instantané renvoyait
  l'heure d'évaluation — même bug corrigé sur les dernières mesures de
  l'accueil) ; caméras « connecté, en attente d'images » ; plan Studio
  mesuré depuis l'image, scènes réparties sur l'étage actif ; accueil
  « Self-hosted » + compteurs depuis le résumé d'org ; unité unique ;
  Dockerfile `toolchain-base`.
- **Statut** : ✅ résolu.

### O25 — Suite e2e : ce qui reste à faire

- **CI** : la suite n'est pas branchée (il faut la stack complète : Rauthy,
  Postgres, Valkey, O2, Mailcrab, builder PlatformIO). À concevoir : job
  manuel/nightly sur une machine qui a Docker, ou runner auto-hébergé
  (attention : dépôt public, jamais de fork PR sur self-hosted).
- **Non couvert** : annotations, registre de modèles + détection vision,
  enregistrements caméra, assistant IA (fournisseur LLM requis), agents
  edge, OTA de l'ESP32-CAM, apps Android/desktop, membres d'org multi-
  utilisateurs (un seul compte Rauthy de test).
- **Matériel** : séquence manuelle (`task e2e:hardware`, ports + WiFi en
  variables) ; chaque run réenregistre les cartes (nouveau jeton, ~4 min).
- **Statut** : ouvert.
