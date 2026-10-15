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
- **Statut** : ✅ résolu (2026-10-07) — colonnes `failure_code` /
  `failure_detail` sur `build_records` (migration 000006), code machine
  (`pnex_core::BUILD_FAILURE_CODES`, clés `build-fail-*`) + queue de la
  sortie du compilateur, jeton / mot de passe WiFi / clé masqués
  (`scrub_secrets`). Affiché dans le détail device, le wizard et en
  infobulle du badge « échec ».

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
- **Statut** : documenté (2026-10-15) — attendu : « Restart » fait stop +
  start (nœuds reconstruits, état vidé), un redeploy de la même version le
  garde ; écrit dans la fiche assistant `flows.md`.

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
- **Statut** : ✅ résolu — fuite des logs (2026-10-03), puis jeton hors URL
  par D154 (2026-10-09) : `/ws/device`, `/ws/camera` et `/ws/media`
  n'acceptent que `Authorization: Bearer` (`device_link::device_token`).

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
- **Bug latent révélé** : la CA désormais appliquée sur la WS 8266 échouait
  (`TCP connect failed`) — BearSSL vérifie les dates du certificat et le
  8266 n'a pas d'horloge (`BR_ERR_X509_TIME_UNKNOWN`) ; l'OTA https épinglée
  8266 avait le même défaut, jamais exercé (OTA 8266 validée en http LAN).
  Corrigé dans `pnex_tls` : SNTP démarré dès qu'une CA est épinglée,
  `setX509Time` = heure NTP sinon date de build (suffit tant que le leaf
  edge n'est pas renouvelé après le build sur un réseau sans NTP — 397 j,
  renouvelé à J−30) ; erreur BearSSL loguée en clair.
- **Validé sur carte (2026-10-03, e2e `@hardware`, edge wss + CA)** :
  ESP32-C6-Zero 4/4 (enregistrement, pins, OTA non forcée, firmware
  custom) ; NodeMCU ESP8266 2/2 (enregistrement, pins).
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
- **Statut** : ouvert (partiellement corrigé, cf. O17) — 2026-10-07 : 44
  paires label → champ liées par `for`/`id` (script sur les blocs
  `label {}` suivis d'un `input`/`select`/`textarea`), champs numériques des
  contrôles et durées de l'Écriture mémoire nommés. Reste les labels suivis
  d'un composant (sélecteurs, `SecretField`…) et le passage du projet a11y
  en strict.
  — 2026-10-09 : inventaire complet des `label {}` sans `for` (hors
  fichiers en cours d'édition par une autre session) : 129 enveloppent
  déjà leur contrôle (accessibles, inchangés) ; les 20 autres sont
  rattachés — 7 `for`/`id` vers un `input`/`select`/`textarea` natif
  (source de rôle Maison, sélecteurs WiFi/hôte de `edge_refs_picker`,
  choix de fonction, canal de test des modèles de notification, labels
  des modèles ML, emoji POI), 5 composants (`SecretField` : ids
  par instance ; `TemplateEditor` : prop `id` existante branchée sur sujet
  et corps ; `HomeIconPicker` : prop `labelledby` ; `FormRow` des
  contrôles : prop `field_id`), 7 titres de listes/groupes de boutons
  passés en `p` + `role="group"`/`aria-labelledby` sur chaque ligne
  (seuils, états, options de contrôle, variables de modèle, composants
  de mélange, fenêtre de visualisation), 1 titre sans contrôle (hôte
  imposé) passé en `p`. axe : `empty-table-header` corrigé dans le socle
  (`Column::header_sr_only`, en-tête « Actions »/« Sélection » en
  `sr-only` sur caméras, contrôles et système) ; `link-in-text-block`
  corrigé (lien souligné dans la carte de rétention de /system). Reste :
  1 label de `admin_status.rs` (rétention globale, fichier alors en
  édition ailleurs), `color-contrast` (7 routes, non traité), audit des
  modales ouvertes, et le passage en strict (`PNEX_E2E_A11Y_STRICT=1`)
  qui demande une passe e2e contre la stack ; les helpers
  `fieldAfterLabel` des tests e2e peuvent désormais migrer vers
  `getByLabel`.

  — 2026-10-15 : passe strict contre la stack (image du 2026-10-14) : 25/28
  routes vertes ; restaient `select-name` (filtres de /ontology et
  /cases) et un `color-contrast` (courriel des membres sur fond gris de
  /orgs/current) → corrigés ; le rapport axe donne désormais les
  sélecteurs fautifs ; la CI e2e nightly passe le projet a11y en strict
  (`PNEX_E2E_A11Y_STRICT=1`). Audit des modales ajouté (`axe dialog
  <route>` : bouton Ajouter du socle, `data-testid="list-add"`) : seul
  l'import de fichier de /models échouait (`label`) ; champs fichier
  sans nom rattachés (modèles ML, ASR, image de test, nouvelle version
  d'un média). Passe strict contre le front local : 44/44 (28 routes,
  16 modales). Reste : migration `fieldAfterLabel` → `getByLabel`
  (confort des tests, pas de dette utilisateur).
- **Statut** : ✅ résolu (strict en CI, routes et modales vertes).
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
- **Statut** : CI posée le 2026-10-09, **à valider au premier run** —
  `.github/workflows/e2e.yml` (nightly 02:30 UTC + `workflow_dispatch` avec
  tag d'image et filtre `--grep`, jamais sur `pull_request`, dépôt `Pnex`
  seulement) : images `main` publiées, stack complète via la surcouche
  `compose.ci.yaml` (tout en localhost HTTP, issuer
  `http://localhost:8080/auth/v1/`), suite hors `@hardware`, rapport
  Playwright + logs de la stack en artefact. Non couvert inchangé
  (annotations, vision, assistant, Android/desktop, multi-utilisateurs).

## 2026-10-04 — audit d'affichage mobile (APK sur téléphone, viewport 392 px)

### O26 — UI mobile : tables CRUD coupées, barre d'éditeur trop large

- **Symptômes** (constat user) : « la ligne de boutons en haut est trop
  large, il faut glisser », « impossible de glisser sur la droite » dans
  les CRUD.
- **Cause racine** : le wrapper de `DataTable` est en `overflow-hidden` →
  colonnes au-delà de ~345 px (dont **Actions**) inaccessibles sur les 17
  pages liste ; la barre `EditorShell` fait 613–680 px avec Enregistrer /
  Déployer hors écran.
- **Détail** : 17 anomalies M1–M17 mesurées + plan en 4 lots dans
  [`architecture/mobile-ui.md`](architecture/mobile-ui.md).
- **Statut** : ✅ résolu (lots livrés et poussés, `1620f8b`, garde
  `layout.spec.ts` Android).

## 2026-10-04 — démonstration domotique de bout en bout (captures docs, `e2e-nodemcu`)

Parcours joué dans l'UI (Playwright, org E2E, image serveur rebuildée sur
`c418999`) pour les captures du site : tableau mobile « Demo – Home » depuis
le modèle **Maison** → carte Lumière (valeurs inversées 0/1, LED active bas)
→ **Créer le flow…** → Source contrôle → Appareil (écriture) D4 → déploiement
→ vue live 392 px → **Tout éteindre** ; flow « Demo – Weather » (MET Norway →
2 × Écriture mémoire) → carte Météo + pastille Extérieur.

**Vérifié OK** : modale de création + modèles ; contrôles provisionnés au save
(références `#w-000n.role`) ; brouillon jamais déployé seul ; catalogue Source
contrôle groupé par tableau ; page Contrôles (« Sans effet » / « Écouté par ») ;
interrupteur mobile → D4 `last_value=false` (allumée) ; Tout éteindre (confirm)
→ D4 `true` avec la valeur Arrêt inversée ; résumé de pièce « 1 on » ; panneau
Détail ; Depuis un device (+ Lumière / + Interrupteur sur D4) ; nœud Météo +
carte (11°, 5 jours) ; mémoire lisible comme source de carte.

### O27 — Inspecteur de carte maison : source mémoire affichée fausse à la réouverture

- **Repro** : carte « Extérieur » (Température & humidité), source
  `demo.weather.now · temperature`, Enregistrer ; rouvrir l'éditeur et
  sélectionner la carte.
- **Constat** : le select affiche `demo.weather.daily · d0_condition_code`
  (première option mémoire) alors que le layout enregistré porte bien
  `memory: {key: demo.weather.now, field: temperature}` et que la vue live
  affiche la bonne valeur (11,4 °C). Variante du « select fantôme » dioxus.
- **Risque** : l'utilisateur croit sa source perdue ; non vérifié si un
  ré-enregistrement sans toucher au select peut écraser la source.
- **Statut** : ✅ corrigé (2026-10-07) — la source mémoire enregistrée reste
  dans la liste même absente du catalogue (clé expirée), comme les séries
  télémétrie (`with_saved_memory`).

### O28 — « On and off values must differ » affiché alors que 0 ≠ 1

- **Repro** : Lumière → Marche / arrêt → Valeurs ; saisir Valeur allumé `0`
  puis Valeur éteint `1`.
- **Constat** : l'erreur reste affichée (état intermédiaire 0/0 pendant la
  saisie non réévalué) ; l'enregistrement passe et le contrôle porte bien
  `on: 0, off: 1`. Erreur d'affichage seulement.
- **Statut** : ✅ corrigé (2026-10-07) — les champs numériques du domaine
  s'appliquent à chaque frappe (brouillon local pour « - » ou « 0. »),
  l'erreur suit la saisie.

### O29 — Vue live mobile : en-tête écrasé, cartes trop hautes

- **En-tête** à 392 px : titre coupé mot par mot (« Demo / – / Home »),
  « Back to list » sur deux lignes, « v2 · live » et « Refresh every » tassés
  (capture `home-mobile-live`). À rapprocher de mobile-ui.md (pas vu dans
  M1–M17 pour la vue live d'un tableau).
- **Cartes** : hauteur fixe (`h-44`, carte Lumière ~350 px de haut sur
  téléphone pour une ligne de contenu ; carte Météo idem dans l'éditeur) → beaucoup
  de vide, peu de cartes visibles par écran.
- **Statut** : ✅ corrigé (vérifié navigateur 2026-10-07, 392 px) — en-tête
  OK (titre sur une ligne, actions en icônes, aucun défilement horizontal) ;
  les cartes Maison prennent la hauteur de leur contenu avec un plancher
  (`mobile_card_classes` : `min-h-*`, Lumière 80 px au lieu de 176) ; météo,
  flux d'énergie et graphes gardent une hauteur fixe (contenu qui la remplit).

### O30 — Libellés générés : clé de rôle brute, homonymes indiscernables

- Le contrôle et le brouillon de flow prennent la **clé** du rôle :
  « Ceiling light · power », « Control — Ceiling light · power » (au lieu de
  « Marche / arrêt », non traduit).
- Le modèle Maison crée deux « Ceiling light » (Salon, Cuisine) et deux
  « Shutter » : rien ne les distingue dans le catalogue Source contrôle ni
  sur la page Contrôles (la pièce n'apparaît pas).
- Après déploiement, le sous-titre du nœud Source contrôle affiche l'id
  brut (`f53a092b`) au lieu de la clé affichée en brouillon.
- **Statut** : ✅ corrigé (2026-10-07) — contrôle d'une carte maison nommé
  « Pièce · Carte · rôle » (titre de section = pièce) ; le rôle s'affiche
  traduit dans l'UI (`control_label`) ; l'éditeur de flows charge les clés
  des contrôles au montage (sous-titre Source de contrôle).

### O31 — Mineurs constatés

- Sous-titre de la page Tableaux de bord resté « Composable SCADA screens… »
  (ignore le format mobile).
- « Créer et ouvrir le flow » ouvre l'éditeur sur `/flows` sans id dans
  l'URL : un rechargement perd le flow.
- A11y : confirmation « Tout éteindre » et panneau Détail sans
  `role=dialog` ; champs Valeur allumé/éteint et Clé/Durée de vie de
  l'Écriture mémoire sans label associé ; cliquer une carte du composeur
  mobile au clic Playwright standard échoue (élément jamais « stable »,
  re-rendus continus ?) — clic souris à coordonnées nécessaire.
- FR : « Source contrôle » (palette) vs « Source de contrôle » (aide) ;
  « Device cible… » vs « Appareil » partout ailleurs ; groupe de palette
  « Commandes » homonyme du type de contrôle « Commandes » ; menu « Flux »
  vs « flow » dans tout le reste de l'UI.
- Section « Pastilles » du modèle Maison sans titre → l'éditeur affiche le
  placeholder « SECTION TITLE ».
- **Statut** : ✅ corrigé (2026-10-07) — sous-titre Tableaux de bord ;
  `/flows?id=` (rechargement, liens profonds : recherche, assistant,
  événements, « Créer et ouvrir le flow ») ; panneau Détail en
  `role=dialog` (la confirmation l'était déjà) ; champs Valeur
  allumé/éteint et durées nommés ; « Source de contrôle », « Appareil
  cible », groupe « Contrôles », menu « Flows » ; pastilles sans titre =
  indication « titre facultatif ». Non traité : clic Playwright instable
  sur le composeur mobile.

### O32 — État laissé par la démo

- Org E2E : tableau « Demo – Home », flows #18 « Control — Ceiling light ·
  power » et #20 « Demo – Weather » **déployés** ; D4 de `e2e-nodemcu` passée
  en `digital_out` et réservée par le flow #18. La suite e2e vide l'org E2E à
  chaque run (les captures sont déjà dans pnex-website) ; elle n'utilise pas
  D4.
- Image `pnex-builder:dev` non reconstruite (le second target a échoué sur
  un travail non commité d'une autre session, `AiRetention`) : le builder
  tourne encore l'image du 2026-10-03.
- **Statut** : informatif.

### O33 — Image Docker : migration absente du binaire (cache cargo périmé) → 500 partout

- **Symptôme** (2026-10-04 14:00) : après `task docker:build` + `task app:up`,
  toutes les routes authentifiées répondent 500 en 1 ms (`/api/v1/user-info`,
  `/api/v1/orgs`) ; le boot logue « No pending migrations » alors que la base
  est à `000004` et que le code enregistre `m20261004_000005_org_ai_retention`.
- **Cause** : le binaire servi contient l'entité `organizations.ai_retention_days`
  mais pas la migration 000005 (aucune chaîne `m20261004_000005` dans le
  binaire). `target/` vit dans un cache mount BuildKit et cargo juge la
  fraîcheur à la **mtime** ; `COPY . .` conserve les mtimes de l'arbre. Un
  build précédent (lancé à 12:27) avait compilé le migrateur avant le commit
  `5cccb2d` (12:35) : `lib.rs` modifié pendant ce build est resté « plus
  ancien » que son artefact → jamais recompilé. `touch` seul ne suffit pas :
  la couche `RUN cargo build` est reprise du cache Docker (clé = contenu).
- **Contournement appliqué** : `touch` des fichiers de migration +
  `docker buildx build --no-cache-filter toolchain,web,build` → l'image
  `pnex-builder` a bien reçu un binaire à jour, mais **pas** `pnex-server`
  (même binaire 13:49, malgré la recompilation de `pnex-migration` dans le
  log) ; le serveur a alors crash-loopé (« migration applied but its file is
  missing », 000005 appliquée entre-temps par le builder). Dépannage local :
  image `pnex-server:dev` dérivée qui recopie `/usr/local/bin/pnex-server`
  depuis `pnex-builder:dev`. La cause exacte du binaire figé côté target
  `server` reste à comprendre.
- **Correctif proposé** (non fait) : dans le `RUN` du stage `build`, forcer la
  recompilation des crates du workspace (`find crates -name '*.rs' -newer …`
  ne suffit pas ; plus simple : `touch` de tous les `crates/**/*.rs` avant
  `cargo build`, ou `cargo clean -p` des crates du workspace — les dépendances
  restent en cache). Concerne aussi les images de release.
- **Statut** : ✅ corrigé (2026-10-07) — fraîcheur par contenu :
  `deploy/docker/touch-changed.sh` touche les fichiers dont le hash diffère
  du manifeste du dernier build réussi (gardé dans le cache `target/`),
  dans les étapes web, build et agent-dist.

### O34 — NodeMCU V3 + OLED : le bouton FLASH n'est jamais lu sur GPIO0

- **Constat** (2026-10-04, carte ex-`e2e-nodemcu` réenregistrée `proud-ibex`
  en org Demo) : D3/GPIO0 en `digital_in` souscrit à 1 s ; journal série
  (`firmware/hil/serial_tail.py`, sans reset) : 45 lectures `GPIO0 value=1`
  pendant que l'opérateur appuie plusieurs fois sur FLASH. La chaîne
  flow → carte marche (`write GPIO2 -> HIGH (readback=HIGH)` toutes les 5 s).
- **Pistes** : sérigraphie « G01 » près du bouton (GPIO1/TX ?) sur cette
  variante OLED ; ou lecture des entrées numériques cassée dans le firmware
  générique 8266 (la suite e2e ne teste sur cette carte que D7 en sortie et
  A0, jamais une entrée numérique).
- **Impact** : étape 07 des captures (bouton → LED → notification) non
  rejouable sur cette carte.
- **Statut** : ✅ non reproduit (2026-10-07, même carte flashée
  `e2e-nodemcu`, org E2E) — D3/GPIO0 `digital_in` souscrit à 300 ms :
  4 appuis FLASH relevés (`false` pendant l'appui, `true` relâché) via
  `GET /devices/{id}/pins`. Le câblage et la lecture sont bons. Cause
  probable du 2026-10-04 : souscription à 1 s = échantillonnage, un appui
  bref tombe entre deux lectures (il n'y a pas de détection de front).
  Pour un bouton : intervalle ≤ 200 ms, ou une détection de front côté
  firmware (à évaluer si un tutoriel en a besoin).


## 2026-10-07 — mise en place de la démo « Usine pilote » sur dev.pnex.io (0.1.0-beta.6, Helm)

Contexte : réinstallation à neuf du cluster perso (chart pnex 0.2.0,
`values-dev.yaml`), org « Usine pilote » montée par API (scripts du pitch
`pnex-pitch/capture`), cartes réelles : NodeMCU V3 OLED, ESP32-C6-Zero
(firmware custom LED RGB), ESP32-CAM.

### O35 — Splat : on tourne autour, on ne s'y promène pas

- **Constat** : dans le viewer splat (médias, annotations), impossible de
  « se balader » dans la scène (camion Tanks and Temples).
- **Cause** : `crates/pnex-frontend/js/viewers.js:210` crée les
  `OrbitControls` de gsplat.js avec `enableKeyboardControls = false` ; seuls
  restent l'orbite (clic gauche), le zoom molette borné à `0.05 R`
  (`minZoom`) et le pan (clic droit). Pas de flèches/ZQSD, pas de vue à la
  première personne.
- **Correction proposée** : activer le clavier (vérifier qu'il ne capte pas
  les touches quand un champ de l'éditeur d'annotations a le focus), baisser
  `minZoom` pour entrer dans la scène, indiquer les commandes dans le viewer
  (aide « glisser / molette / clic droit / flèches »).
- **Statut** : ✅ corrigé (2026-10-08) — clavier gsplat activé derrière une
  garde (`withGuardedKeys` : pointeur sur le viewer, aucun champ de saisie
  actif ; flèches sans défilement de la page ; écouteurs retirés avec le
  viewer), `minZoom` à `0.005 R`. Vérifié sous Chrome (SwiftShader) sur le
  camion réduit à 30 000 points. Pas d'aide affichée dans le viewer.

### O36 — Quota de build : l'appareil construit se compte lui-même

- **Symptôme** : premier build d'une org neuve (tier Free, 1 appareil
  « mixed ») refusé : « Device limit reached for mixed devices ».
- **Cause** : `controllers/builds.rs:311` compare `count_devices_of_type`
  (qui inclut l'appareil déjà enregistré qu'on construit) avec `>=` à la
  limite : avec une limite de 1, aucun build n'est jamais possible.
- **Correction proposée** : au build, exclure l'appareil cible du compte (ou
  `>`), le quota s'appliquant déjà à l'enregistrement (`devices/crud.rs`).
- **Statut** : ✅ corrigé (2026-10-08) — refus au build seulement au-delà
  du quota (`>`), test `build_quota_403` réécrit (au quota : 201 ; tier
  abaissé : 403).

### O37 — `deploymentMode: self_hosted` n'enlève pas les quotas de tier

- **Symptôme** : après passage en `self_hosted`, même refus que O36 ; la
  page Abonnement affiche toujours « Free ».
- **Cause** : `tier_limit_for` (`controllers/devices/crud.rs:4`) et le
  contrôle d'intervalle de build lisent le tier de l'org sans regarder
  `PNEX_DEPLOYMENT_MODE` ; les orgs créées prennent `PNEX_DEFAULT_ORG_TIER`
  (Free par défaut), que le chart ne pose pas ; aucune route ne change le
  tier d'une org existante (contournement : `UPDATE organizations SET
  subscription_tier_id = 6`).
- **Correction proposée** : en `self_hosted`, aucune limite de tier
  (`tier_limit_for` → `None`, intervalle de build non appliqué) ; exposer
  `api.defaultOrgTier` dans le chart ; un changement de tier par org dans
  `/admin/status` (admin plateforme).
- **Statut** : ✅ corrigé (2026-10-08) — `tiers_enforced()` : quotas et
  intervalle de build seulement en `saas` (test
  `self_hosted_ignores_subscription_tiers`). Changement de tier par org
  livré le 2026-10-09 : `GET /system/tiers` + `PUT /system/orgs/{id}/tier`
  (admin plateforme, `tier-unknown` → 422), liste déroulante dans
  `/admin/status` (test `org_tier_is_changed_by_platform_admins_only`).
  `api.defaultOrgTier` ajouté au chart (pnex-deploy, non commité).

### O38 — Chart Helm : aucune CA d'appareil → tout build wss refusé

- **Symptôme** : `build_no_ca` sur tous les builds (« no device CA to pin
  (PNEX_CA_CERT_FILE) »).
- **Cause** : seul le compose pose `PNEX_CA_CERT_FILE` (pki-init) ; le chart
  ne le posait pas, alors que SEC-W6 refuse désormais un build wss sans CA.
- **Correction** : chart pnex-deploy, valeur `deviceCa.pem` → ConfigMap
  `<release>-device-ca` montée en `/pki/device-ca.pem` (API + worker) +
  `PNEX_CA_CERT_FILE` (non commité au 2026-10-07). Derrière Cloudflare :
  GTS Root R4 seule (limite firmware 2 Ko, cf. SEC-16) ; si Cloudflare change
  d'autorité, rebuild + reflash. À documenter dans le README du chart.
- **Statut** : ✅ corrigé — chart 0.2.1 (pnex-deploy) : `deviceCa.pem` par
  défaut ISRG Root X1, surcharge documentée pour Cloudflare (GTS Root R4) ;
  limite par puce et `build_ca_too_large` côté serveur (SEC-16).

### O39 — NodeMCU V3 OLED enregistrée sur la board NodeMCU par défaut

- **Symptôme** : écran activé dans le détail + rebuild, OLED noir.
- **Cause** : enregistrement par API sans `board_id` → board `esp8266`
  (OLED externe D2/D1) au lieu de `nodemcu_v3_oled` (OLED soudé D6/D5) ; la
  board est figée à l'enregistrement → suppression, réenregistrement,
  rebuild, reflash.
- **Statut** : ✅ corrigé (2026-10-09) — « Changer de carte » dans le
  détail du device : `PUT /api/v1/devices/{id}/board` (rôles d'écriture,
  même SoC sinon `device-board-soc-mismatch`, écran soudé qui suit la
  carte), puis rebuild + mise à jour ; plus de suppression/réenregistrement
  (test `device_board.rs`). Reste la piste « détection au premier
  announce » (firmware).

### O40 — État laissé par la démo (dev.pnex.io)

- Chart déployé depuis l'arbre de travail pnex-deploy (non commité) avec
  `--set-file deviceCa.pem=…` ; `values-dev.yaml` : `self_hosted`,
  `customFirmware.enabled: true`, **`sandbox: none`** (code C++ utilisateur
  dans le worker, qui détient les identifiants de la base) — acceptable pour
  un environnement jetable, jamais pour une instance ouverte aux
  inscriptions.
- Orgs 1 et 2 passées au tier Admin par SQL.
- Clé Anthropic de démo dans le coffre de l'org « Usine pilote » (à révoquer
  après la démo).
- Firmwares compilés pour le WiFi « Chez Shan » : changement de réseau =
  rebuild + reflash (le WiFi est compilé dans l'image).
- **Statut** : informatif.

## 2026-10-13 — refonte du site pnex-website (stack locale 192.168.1.185, image du 2026-10-12)

### O41 — Org personnelle nommée en français quelle que soit la langue

- **Symptôme** : l'UI en anglais affiche « Organisation de admin@example.com »
  (accueil, sélecteur d'org, cartes d'org).
- **Cause** : le nom est figé en base à la création JIT,
  `format!("Organisation de {display}")`
  (`crates/pnex-backend/src/auth/provisioning.rs:216`) : texte français en dur
  côté serveur, contraire à la règle i18n (l'UI résout au rendu).
- **Correction proposée** : nom neutre stocké (`{display}`, ou « {display}'s
  org » en anglais canonique), ou drapeau `personal` rendu par une clé fluent
  au rendu ; renommage possible par l'utilisateur inchangé.
- **Statut** : corrigé (7ae8b706) ; restes éventuels dans le suivi « 2026-10-13 (soir) ».
### O42 — Stack de démo vidée : données « Demo – … » et appareils perdus

- **Constat** : org 1 sans aucune ressource « Demo – … » (recherche « demo » :
  aucun résultat), appareils jamais revus depuis le 2026-10-10, accueil
  « Telemetry unavailable » (org O2 non configurée). Probablement le
  `task db:reset` imposé par la baseline d'ingestion média du 2026-10-10.
- **Effet** : les cartes branchées (ESP32-C6 en boucle de connexion Wi-Fi sur
  le port série, ESP32-CAM muette) ne correspondent plus à la base ; le
  scénario de captures du site (`pnex-website/scripts/capture`) ne peut pas
  être rejoué sans réenregistrer + reflasher les cartes et reconfigurer O2.
- **Statut** : informatif (session de captures à planifier avec l'opérateur).

### O43 — Libellés UI incohérents relevés en documentant

- `nav-group-devices = Devices` en **français** (`locales/fr-FR.ftl:16`) :
  attendu « Appareils » (sauf choix délibéré).
- Contenu des packs non traduit : types, propriétés, actions et liens de
  `crates/pnex-backend/packs/maintenance.yaml` et `investigation.yaml`
  apparaissent en anglais dans l'UI française (« Augmented maintenance »,
  « Start maintenance », « 5 whys »…) ; l'aide
  `flows-palette-action-trigger-help` (fr-FR.ftl:3208) cite
  « Mettre en maintenance », jamais affiché.
- `flows-palette-notify-help` (en-US.ftl:1453 / fr-FR.ftl:1452) : « (websocket/webhook) »
  alors que le nœud envoie aussi ntfy, Telegram, Slack, Discord, SMTP et
  des pièces jointes PDF.
- Vocabulaire de diffusion dans un produit industriel :
  `flows-palette-range-upsert-help` (« a show, a shift… »,
  en-US.ftl:2215) et `err-range-upsert-origin-invalid` (« EPG »,
  en-US.ftl:3747).
- Pages › Nouvelle page ne propose aucun modèle : les modèles de page ne
  sont accessibles que depuis un dossier d'enquête.
- **Statut** : corrigé (7ae8b706) ; restes fermés le 2026-10-15 : contenu des
  packs traduisible (`names` / `inverse_names` / `descriptions` par langue
  dans les définitions, packs livrés traduits en français — maintenance
  1.3.0, investigation 1.5.0 — ; un nom retapé dans l'éditeur de type
  remplace ses traductions) ; Pages › Nouvelle page propose les modèles
  des packs installés ; l'onglet « Events » des notifications devient
  « Journal des envois » (« Delivery log »).
### O44 — Fiches de l'assistant (assistant-kb) en retard sur la navigation

- `media.md:11` et `documents.md:11` : « Data › Library » → aujourd'hui
  Media › Library ; `documents.md` ne mentionne pas la liste blanche
  d'upload ni `media-format-unsupported`.
- `pages.md:11` : « right under Visualisation » → Knowledge › Pages.
- `calendar.md:43` : nœud « Upsert range » → libellé UI « Time range writer ».
- `cases.md` : cadrage éditorial (« story », modèle « Script / rundown »,
  « broadcasts ») et identifiant interne (« K38 ») dans une fiche lue par
  l'assistant (cf. ton sobre du repo public).
- **Statut** : corrigé (afd7a8e9) ; restes éventuels dans le suivi « 2026-10-13 (soir) ».
### O45 — roadmap.md : incohérences internes (relevées en refaisant la roadmap du site)

1. Le journal du 2026-10-13 cite une section « Branches en attente » ; la
   section s'appelle « Branches mergées le 2026-10-13 ». L'entrée « M2b → M5 »
   dit encore « Tout sur feat/media-vision-studio ».
2. P2.15 : « cible après la 0.2.0 sauf exception » et « M5 (studio de
   rapports) », dépassés par D207–D210.
3. P2.13, P2.16–P2.19 : « zéro code avant validation » alors que le code est
   sur main ; même contradiction dans la ligne de statut d'`ontology.md` ;
   `media-ingest.md` et `geo-layers.md` encore « Proposé ».
4. `security-tiers.md` : « rien n'est implémenté » alors que V1 est livré
   (2026-10-09) ; décision #15 « V1 : prochaine passe sécurité » encore ouverte.
5. Tableau d'état : OTA « OTA sur ESP32 réel » encore à faire ; Organisations
   « UI arbre » alors que P1.1 a livré Sites + labels ; Android « dérivé du
   nom du crate » vs P1.2 « PNeX » ; titre P2.10 « L4 partiel » vs tableau
   L4 complet.
6. Décision #24 demande encore si C0–C1 peuvent déroger au gel (C0–C7 livrés).
7. Ordre : décisions 15 avant 14, 19 avant 18, 26 avant 25 ; journal du
   2026-10-11 après celui du 2026-10-12.
8. « Aucune fonctionnalité promise pour la 0.2.0 » vs ligne P2.14
   « 0.2.0 = L0–L5 + pack Maintenance augmentée ».
- **Statut** : corrigé (7ae8b706) ; restes éventuels dans le suivi « 2026-10-13 (soir) ».
### O46 — assistant-kb : chemins de menu périmés (complément d'O44)

| Chemin périmé | Fiches | Attendu |
|---|---|---|
| Data › Audio streams | streams.md:11, cameras.md:20, cameras.md:46 | Media › Audio streams |
| Data › Cameras | cameras.md:11 | Media › Cameras |
| Data › Models | cameras.md:20, lab.md:19/31/37, models.md:11 | Automation › Models |
| Data › Studio Tour / Annotations | studio.md:11/18, map.md:21 | Media › … |
| Data › Controls | controls.md:11 | Automation › Controls |
| Visualization › Dashboards / Map / Sites / Quick charts | dashboards.md:11, visualisation.md:11/23, map.md:11, sites.md:11, home.md:25, troubleshooting-dashboard-no-data.md:32 | Monitor › … |
| (Edges menu) | devices.md:11, catalog.md:11, edge-refs.md:11, firmware.md:11 | groupe Devices |

Autres écarts fiche ↔ app :
- troubleshooting-stream-silent.md:16-17 : états « Backoff / Running /
  skipped (backlog) » ≠ UI « Retrying soon / Capturing / Too late (skipped) »
  (en-US.ftl:3772, 3774, 3809).
- secrets.md:18 : « Used by » sans flux média ni fournisseur géo (en-US.ftl:3760-3761).
- assistant.md:11 : bouton « en bas à droite » → pied de la barre latérale
  (shell.rs:604-633) ; assistant.md:7 : outils ontologie, enquêtes,
  calendrier absents (services/ai/tools.rs:393-460).
- profile.md:22 : mode sombre « à venir » → livré.
- flows.md:17 : « Save Changes » → « Save ».
- devices.md:15 : « Filter by label (site:serre) » → « Filter by label » (en-US.ftl:1283).
- cameras.md:42 : un flow déployé garderait la caméra éveillée en « On demand » ;
  l'UI dit qu'enregistrer exige « Continuous » (en-US.ftl:2232, 2282).
- cameras.md:35 + notes text_decide / vision_zone de node_docs.rs : routage
  par un nœud « Switch » qui n'existe pas dans la palette (seulement en
  builtin Node-RED brut, node_docs.rs:514-520).
- node_docs.rs:283 : media_source « (Media page) » → Media › Audio streams ;
  node_docs.rs:364 : text_decide « worst first » ≠ UI « lowest first » (en-US.ftl:2523).
- **Statut** : corrigé (afd7a8e9) ; restes éventuels dans le suivi « 2026-10-13 (soir) ».
### O47 — Libellés UI : suite

- `flows-category-devices = Devices` en français (fr-FR.ftl), comme `nav-group-devices`.
- en-US.ftl:3787 : liste d'enregistrements vide cite la source « stream »,
  l'option s'appelle « Media stream (IP camera) » (en-US.ftl:2224).
- en-US.ftl:3763/3766 : sous-titre et vide des flux ne citent que radio et
  HLS (pas caméras IP ni fichiers de la bibliothèque).
- en-US.ftl:4790 (+FR) : Vision Lab « export it for training » alors que
  l'entraînement se fait désormais dans PNeX.
- en-US.ftl:1464 : anti-spam « delayed, never dropped » alors que le code
  bloque et journalise (`pnex-node-notify/src/lib.rs:634-660`).
- en-US.ftl:1460 : guillemets français « Page PDF » dans une aide anglaise.
- en-US.ftl:2375 : « Video description » pour media_fuse, qui fusionne aussi
  les transcriptions audio.
- fr-FR.ftl:228 : « Enregistrer » = à la fois *register* et *save* sur la
  page appareils.
- **Statut** : corrigé (7ae8b706) ; restes éventuels dans le suivi « 2026-10-13 (soir) ».
### O48 — Écarts code ↔ doc d'architecture

- **Itinéraire géo** : `geo-layers.md:367` promet le tracé sur la carte ;
  le backend a `POST /api/v1/geo/route` (`controllers/geo.rs:309`) mais le
  front n'a ni appel (`pnex-frontend/src/api/geo.rs`) ni UI.
- **Boucle de l'assistant** : `services/ai/agent.rs:80` itère
  `0..=MAX_ITERATIONS` (6) → jusqu'à 7 appels LLM par tour, la doc dit six.
- **Stockage local encore par défaut malgré la décision #25 (S3 seul)** :
  `crates/pnex-backend/config/production.yaml:137` (firmware `db`), `:154`
  (média `fs`), `compose.app.yaml:34` ; côté pnex-deploy, `install.sh`
  (l. 63, 378) et `compose.yaml` (l. 65-66). Le site décrit `fs` comme « en
  cours de retrait » en attendant.
- **Statut** : ouvert.

### O49 — Relecture des tutos sur la nouvelle UI

Libellés :
- « Devices » en anglais dans l'UI française aussi en bouton
  (« Depuis un device », fr-FR.ftl:3300) et catégorie de palette (fr-FR.ftl:529).
- En anglais, le groupe « Devices » contient « Devices / Agents »
  (en-US.ftl:6, 16) → chemin « Devices → Devices / Agents ».
- Marque mal orthographiée : « Generic PneX » / « Generic PneX firmware »
  (en-US.ftl:280, 2697), « PNEX v… » (en-US.ftl:64) → PNeX.
- Deux « Events » distincts : Monitor › Events (journal des flows,
  en-US.ftl:2331) et l'onglet Events des notifications (journal d'envoi,
  en-US.ftl:1377).
- Plomberie exposée : canal « WebSocket (internal bus) » avec `/ws/notify`
  (en-US.ftl:1342, 1350-1351) ; « OpenObserve » dans le sous-titre et
  l'erreur des graphiques rapides (en-US.ftl:351, 364).
- Accueil : « subscription » sur un produit auto-hébergé (en-US.ftl:137).
- FR : format « PC » (fr-FR.ftl:994), « Rafraîchissement » sans verbe
  (:2702), « Lire chaque 15 s » → « toutes les 15 s » (:397-400).
- Guillemets « » dans des chaînes anglaises (en-US.ftl:289, 599, 601, 1460,
  1795-1799) ; « WiFi » vs « Wi-Fi » (en-US.ftl:274 vs 3428).

Comportement :
- Modales de création validées par « New … » au lieu du verbe (règle du
  design system) : `pages/firmware/mod.rs:264`,
  `pages/functions/create_modal.rs:52`, `pages/cases/mod.rs:158`.
- Liste de cadence de lecture du panneau de pins sans libellé visible
  (aria-label seul, `components/pins_panel.rs:304-308`).
- Sonde Display rangée dans la catégorie Devices au lieu de Debug
  (`components/flow_editor/canvas/palette.rs:237`).
- « New flow » crée « Flow <date> » sans champ de nom ; renommage par un
  petit crayon seulement (`pages/flows.rs:220-241`,
  `components/editor_shell/mod.rs:268-281`).
- Le bouton principal du formulaire de template passe de Create à Save après
  un Preview/Test (brouillon auto-enregistré,
  `components/notify_template_form.rs:399-405`) : déroutant.

Manque :
- Aucun nœud délai/minuterie dans la palette (`palette.rs:228-270`) :
  « arroser N secondes » n'est pas constructible (tuto arrosage réécrit en
  seuil hystérésis lecture → calc → écriture).
- **Statut** : corrigé (7ae8b706) ; restes éventuels dans le suivi « 2026-10-13 (soir) ».
### O50 — Stack conteneurisée : firmwares compilés sur le port 443 au lieu de 4443

- **Symptôme** : carte flashée depuis le serveur en conteneur (`task app:up`
  avec l'edge local) : Wi-Fi et TLS OK (`101` sur `/ws/device`), puis
  `[NOISE] closed by the server before its answer (key refused?)` en boucle ;
  le journal série montre `wss://192.168.1.185/ws/device` (port 443).
- **Cause** : `apply-edge.sh` écrit `PNEX_DEVICE_HOST=:4443` dans `edge.env`
  (lu par le backend lancé sur l'hôte) mais n'exporte que
  `PNEX_DEVICE_PORT` vers compose, et `compose.app.yaml` ne transmet que
  `PNEX_DEVICE_PORT` au conteneur — variable que le serveur ne lit pas
  (`services/firmware.rs::device_endpoint_env` lit `PNEX_DEVICE_HOST`). Le
  point d'accès compilé retombe sur l'hôte nu (443, sans le certificat client
  vu par l'edge) → refus.
- **Correction (appliquée 2026-10-13)** : `compose.app.yaml` passe
  `PNEX_DEVICE_HOST` ; `apply-edge.sh` l'exporte (`:$DEVICE_PORT` par
  défaut). Vérifié : rebuild + flash ESP32-C6 → `wss://…:4443`, `[NOISE] link
  established`. Reste : `PNEX_DEVICE_PORT` est mort côté serveur (à retirer
  ou documenter) ; une garde e2e « firmware conteneur → 4443 » éviterait la
  récidive.
- **Statut** : corrigé (7ae8b706) ; restes éventuels dans le suivi « 2026-10-13 (soir) ».
### O51 — Éditeur de flows : pas de « tout afficher », nœud hors cadre à l'ouverture

- **Constat** : à l'ouverture d'un flow déployé (« Demo – button to LED »),
  le nœud Inject est coupé au bord gauche du canevas ; aucun bouton « ajuster
  à la vue » (le tableau de bord en a un, `board-zoom-reset`), seul le
  glisser du fond recadre.
- **Correction proposée** : cadrer sur l'emprise des nœuds à l'ouverture +
  bouton « Tout afficher » dans la barre du canevas.
- **Statut** : corrigé (7ae8b706) ; restes éventuels dans le suivi « 2026-10-13 (soir) ».
### O52 — Dashboard mobile : deux boutons « modifier » à 390 px

- **Constat** : vue live d'un dashboard mobile (« Demo – Home ») à largeur
  téléphone : l'en-tête affiche une clé seule (icône) **et** le bouton
  « Edit » juste dessous — deux entrées pour la même action.
- **Correction proposée** : un seul bouton (icône seule sous `sm`, libellé
  au-dessus).
- **Statut** : corrigé (7ae8b706) ; restes éventuels dans le suivi « 2026-10-13 (soir) ».
### O53 — Calendrier : grille claire et numéros de jour illisibles en mode sombre

- **Constat** : `/calendar` en thème sombre : la grille du mois reste sur fond
  clair (gris très pâle), les numéros des jours hors aujourd'hui sont
  quasiment invisibles ; seules les occurrences restent lisibles. La revue
  sombre du 2026-10-11 a précédé l'arrivée du calendrier (2026-10-13).
- **Correction proposée** : jetons `surface`/`line`/`slate` (ou classes
  `gray-*` miroitées) au lieu de couleurs claires fixes ; ajouter la page à
  `design_guard`/revue sombre.
- **Statut** : corrigé (7ae8b706) ; restes éventuels dans le suivi « 2026-10-13 (soir) ».
### O54 — Recherche documentaire : Markdown brut dans les extraits

- **Constat** : Media › Library › « Search in documents » : l'extrait d'un
  `.md` affiche `**4.5 mm/s**` (astérisques) au lieu du texte.
- **Correction proposée** : retirer la syntaxe Markdown à l'extraction
  (texte indexé) ou au rendu de l'extrait.
- **Statut** : corrigé — extraits sans Markdown (7ae8b706) ; la recherche globale
  a un groupe « Documents » (contenu indexé, un résultat par document,
  ouvre le média ; 2026-10-15, non commité, testé dans `doc_search.rs`).
### O55 — Carte : POI choisi dans la liste non centré, adresses en double

- **Constats** : (1) cliquer un POI dans la liste ouvre son tiroir mais la
  carte reste à l'échelle Europe ; il faut « Recenter ». (2) la recherche
  d'adresse (Photon) renvoie deux fois « Avenue Tony Garnier, 69007 Lyon »
  (deux segments OSM de la même voie) sans déduplication.
- **Correction proposée** : `flyTo` à la sélection depuis la liste ;
  dédupliquer les résultats de géocodage par libellé + distance < ~200 m.
- **Statut** : corrigé (7ae8b706) ; restes éventuels dans le suivi « 2026-10-13 (soir) ».
### O56 — Transcription : runtime whisper.cpp absent des images, boucles de répétition

- **Constats (premier essai réel des flux audio, 2026-10-13)** :
  1. Un modèle GGML (`ggml-tiny.bin`) est accepté à l'upload (`kind=model`)
     puis refusé au contrôle : « runtime whisper.cpp not compiled in this
     pnex-asr build ». L'image ne compile `pnex-asr` qu'avec `sherpa`
     (`deploy/docker/Dockerfile`, étape `asr`) ; la fiche KB et la liste des
     formats acceptés (`services/media_sniff.rs`) annoncent pourtant GGML.
  2. Avec l'export sherpa-onnx de Whisper tiny (contrôle OK, RTF 0,05), le
     Test sur une radio musicale (FIP) produit une boucle « de plus de plus de
     plus… » : aucune suppression de répétition / hallucination.
  3. API : la mise à jour d'un flux est `PATCH` (un `PUT` répond 405) —
     cohérent avec le reste ? (les fournisseurs géo sont en `PUT`).
- **Corrections proposées** : (1) refuser GGML à l'upload quand le runtime
  n'est pas compilé, ou le dire dans l'UI du registre ; (2) filtre de
  répétition (n-grammes répétés > N → segment marqué douteux) et/ou VAD
  musique ; (3) harmoniser PUT/PATCH sur les ressources.
- **Statut** : runtime et boucles corrigés (7ae8b706). 2026-10-15 (non commité) :
  `word_timestamps` du profil appliqué au stockage (faux → `words` vide
  enregistré) ; `words` vide avec Whisper = limite de l'export sherpa-onnx
  (pas d'horodatage par jeton), documentée dans la fiche `streams.md` ;
  slug figé au renommage = voulu (il nomme les transcriptions stockées,
  déjà dans la fiche). Reste : rien côté code.
- **Compléments O56** : après renommage d'un flux (« FIP » → « franceinfo »),
  les transcriptions restent étiquetées `fip_radio` (slug figé à la
  création) ; `words` reste `[]` alors que le profil a `word_timestamps: true`.

### O57 — Page introuvable : texte brut hors coquille

- **Constat** : une route inconnue (ex. `/streams/<uuid>`) affiche « Page not
  found: streams/… » en texte brut en haut à gauche, sans barre latérale,
  sans lien de retour ni mise en forme (`pages/not_found.rs`).
- **Correction proposée** : rendre la page 404 dans la coquille (titre,
  phrase, bouton « Accueil »), selon les pages vides du design system.
- **Statut** : corrigé (7ae8b706) ; restes éventuels dans le suivi « 2026-10-13 (soir) ».
### O58 — Flows : second fil d'une sortie de fonction refusé sans retour

- **Constat** : dans l'éditeur, tirer un deuxième fil depuis une sortie
  nommée d'un nœud Function (règle « 1 fil par ancre ») ne fait rien : ni
  fil, ni message, ni curseur interdit. Un utilisateur qui veut envoyer la
  même valeur vers une détection, une alerte et une métrique croit à un bug.
  Les autres nœuds (ex. Forecast) acceptent plusieurs fils par sortie.
- **Correction proposée** : retour visible au relâcher (toast « une sortie de
  fonction ne prend qu'un fil : ajoutez une sortie dans le code ») et/ou
  curseur `not-allowed` au survol ; documenter la règle dans la NodeDoc
  `pnex_function` (pièges).
- **Statut** : corrigé (7ae8b706) ; restes éventuels dans le suivi « 2026-10-13 (soir) ».
### O59 — Accueil : quotas de tier affichés en auto-hébergé, séries de flows supprimés

- **Constats** : (1) en `self_hosted` (aucun quota appliqué depuis O37),
  l'accueil affiche « Tier capacities — Mixed devices 2 / 1 » : un
  dépassement apparent qui n'en est pas un. (2) « Latest measurements » liste
  encore les séries de flows supprimés (`flow_58`, `flow_61`).
- **Corrections proposées** : masquer le bloc tier hors `saas` ; filtrer les
  séries `flow_<id>` dont le flow n'existe plus (ou les marquer « supprimé »).
- **Statut** : corrigé (7ae8b706) ; restes éventuels dans le suivi « 2026-10-13 (soir) ».
## 2026-10-13 (soir) — suivi des constats O41–O59

Corrigé (non commité ; `task check` natif + wasm vert, `cargo test --workspace
--no-fail-fast` : 164 binaires verts, `task fmt` passé) :

- **O41** : l'org personnelle prend le nom neutre de l'utilisateur
  (`auth/provisioning.rs`), plus de texte français côté serveur.
- **O43 / O47 / O49 (libellés)** : « Appareils » en français (groupe, palette,
  bouton), entrée « Devices & agents », marque « PNeX », guillemets droits en
  anglais, « Wi-Fi », aide du nœud Notification (tous les canaux + PDF),
  anti-spam « bloqués et journalisés », plages horaires sans vocabulaire de
  diffusion, sous-titres Vision Lab / flux / enregistrements, plomberie
  (OpenObserve, `/ws/notify`, « subscription ») retirée de l'UI,
  « Ordinateur », « Rafraîchir toutes les », « Lire toutes les », « Ajouter un
  appareil » (plus de collision avec « Enregistrer »).
- **O44 / O46** : fiches assistant-kb alignées sur la navigation et l'UI
  réelles (chemins, états, outils, nœuds, liste blanche d'upload) ; cases.md
  neutre ; modèle « Script / rundown » renommé « Presentation outline ».
- **O45** : `roadmap.md` — les 8 incohérences reprises.
- **O48 (assistant)** : exactement 6 appels LLM par tour (5 avec outils + la
  réponse forcée) ; jetons de l'appel forcé comptés ; test `ai.rs` aligné.
- **O50** : `PNEX_DEVICE_PORT` retiré de l'env serveur (mort côté code).
- **O51** : éditeur de flows cadré sur les nœuds à l'ouverture + bouton
  « Tout afficher » (`geometry::fit_view`, testé).
- **O52** : un seul bouton « modifier » sur la vue live mobile.
- **O53** : calendrier sur jetons (lisible en sombre) ; `design_guard`
  refuse désormais `dark:` sur une nuance numérotée (cause : échelles déjà
  miroitées par dark.css).
- **O54** : Markdown retiré à l'extraction (`doc_extract::md_plain`, testé) ;
  les documents déjà indexés gardent l'ancien texte jusqu'à réindexation.
- **O55** : choix d'un POI dans la liste → la carte s'y rend ; résultats de
  géocodage dédupliqués (< 200 m, même libellé ; testé).
- **O56** : `pnex-asr runtimes` + refus `asr-runtime-unavailable` (422) à
  l'enregistrement d'un modèle dont le runtime n'est pas compilé ; garde
  anti-boucle `collapse_repeats` (≤ 3 répétitions d'un n-gramme 1–4 mots,
  testée). Nécessite de reconstruire l'image (binaire `pnex-asr`).
- **O57** : page introuvable rendue dans la coquille (titre, chemin, Accueil).
- **O58** : cause réelle ≠ règle « 1 fil » (inexistante) : l'inspecteur ouvert
  couvre la droite du canevas et un relâcher hors entrée annulait en
  silence. Désormais toast `flows-wire-missed` quand un fil est lâché hors
  d'une entrée ; la note de la NodeDoc pnex_function qui évoquait « un fil par
  sortie » est retirée (plusieurs fils par sortie sont permis).
- **O59** : bloc « Tier capacities » masqué hors `saas` ; séries `flow_<id>`
  de flows supprimés filtrées de « Latest measurements ».
- Modales de création validées par « Create » ; cadence de lecture du
  panneau de pins avec libellé visible ; sonde Display rangée en Debug ;
  bouton du formulaire de template stable (« Save »).

Restent ouverts :

- **O48** : tracé d'itinéraire sur la carte (fonctionnalité) ; **stockage S3
  par défaut** (décision #25 vs `production.yaml` et `install.sh`) — décision
  produit.
- **O56** : `words` vide — l'export sherpa-onnx de Whisper ne fournit pas
  d'horodatage par jeton ; et `word_timestamps` du profil n'est lu nulle
  part ; slug de flux figé au renommage (par conception ?).
- **O43** : contenu des packs non traduisible (pas de champ i18n dans le
  format de pack) ; Pages › Nouvelle page sans choix de modèle ; deux
  « Events » distincts (Monitor › Events et onglet des notifications).
- **O54** : la recherche globale ne cherche pas dans le contenu des documents.

### O60 — Flow arrêté affiché « Deployed »

- **Constat** : un flow dont la version est déployée mais le runtime arrêté
  (stoppé, ou refusé parce qu'une autre source écrit déjà la pin) montre la
  pastille verte « Deployed » à côté d'un bouton « Start » : l'état réel
  (arrêté) n'est pas lisible, ni la raison du refus de démarrer.
- **Correction proposée** : pastille « Stopped » / « Deployed · stopped » et,
  si `Start` est refusé (pin-already-assigned), le message avec le flow
  concurrent.
- **Cause** : `GET /flows/{id}/runtime` renvoyait `running` = processus
  runtime vivant ; ce processus sert tous les flows de l'org, et la santé
  par flow est inconnue après un redémarrage du backend → pastille verte.
- **Correction (2026-10-14)** : `running` faux dès que le flow n'est pas
  `deployed` (`controllers/flows/lifecycle.rs`, assertion dans
  `tests/flows.rs`). Le refus de Start passait déjà par le deploy
  (`pin-already-assigned` + flow concurrent, traduit en toast).
- **Statut** : corrigé (700cb617) ; restes éventuels dans le suivi « 2026-10-13 (soir) ».
### O61 — POI : aperçu épinglé sur une ressource supprimée

- **Constat** : à l'ouverture d'un POI, le visualiseur affichait
  « not_found : Resource was not found » avec l'UUID brut en titre. L'aperçu
  épinglé (`map_pins.preview_kind/preview_id`) pointait une visite 3D
  supprimée ; son arête `placed_on` était bien fermée, mais pas l'épingle.
- **Correction** : `resources::purge_for` retire l'épingle des POI qui
  visaient la ressource supprimée (test `pois::pin_apercu_poi`).
- **Statut** : corrigé.

## 2026-10-14 — déconnexion

### O62 — Déconnexion : page JSON brute de Rauthy

- **Constat** : `/auth/v1/oidc/logout` affichait `{"error":"BadRequest",
  "message":"need at least one of id_token_hint, logout_token, or a valid
  session"}`. Un refresh sans `id_token` effaçait celui stocké, puis la
  déconnexion POSTait l'end-session sans indice, session Rauthy déjà morte.
- **Correction** : `id_token` conservé si le refresh n'en renvoie pas ; sans
  indice, pas d'appel Rauthy (retour à l'écran de login). Résiduel : SEC-41.
- **Statut** : corrigé (déploiement à faire avec la prochaine image).
## 2026-10-14 — harmonisation des listes CRUD

Règle posée : `docs/architecture/design-system.md` §6 point 8 (tableau +
pagination, jamais de tuiles, bouton « Ajouter » toujours visible).

### O63 — Ontologie, dossiers d'enquête, empreintes vocales : listes à part

- **Constat** : en ontologie, le bouton « Nouvel objet » n'apparaissait
  qu'une fois un type créable choisi dans le filtre ; par défaut (« Tous les
  types »), on croyait ne rien pouvoir ajouter (ex. une Personne). Les
  dossiers d'enquête s'affichaient en tuiles, sans pagination. Les empreintes
  vocales : liste faite main, pas de bouton d'ajout, et un sélecteur de
  modèle vide quand l'org n'a aucun modèle d'empreinte.
- **Correction** : ontologie — bouton toujours visible, sélecteur « Que
  voulez-vous créer ? » (renvoie aux Packs si aucun type créable). Dossiers —
  `DataTable` + `ListPager`. Empreintes — tableau paginé (côté client),
  « Nouvelle empreinte » = choix de la personne puis onglet Voix, messages
  « aucun modèle » / « aucune empreinte prête » à la place du testeur.
- **Statut** : corrigé (7ae8b706) ; restes éventuels dans le suivi « 2026-10-13 (soir) ».
### O64 — Listes CRUD : lignes manquantes et écarts au socle

Revue de toutes les pages de liste (2026-10-14). Le serveur borne les listes
(`controllers/pagination.rs` : 10 lignes sans `limit`, 100 au plus) : une
page qui demande « tout » ou plus de 100 tronque sans le dire.

- **Lignes manquantes (bugs)** :
  - notifications : canaux et modèles, le pager ne pilote pas la requête
    (toujours les 10 premiers) ;
  - mélanges de fluides : aucune `limit` → 10 lignes, pager client sur ces
    10 ; `can_write: true` en dur (bouton Ajouter visible des lecteurs) ;
  - organisations, sites (carte) : aucune `limit` → 10 lignes ;
  - fonctions, documents, contrôles, références Edge (wifi, hôtes),
    annotations (et leurs sélecteurs média/visites), calendrier : `limit`
    200/500 ramenée à 100 ;
  - flux : les onglets Transcriptions et Plages reçoivent la page courante
    des flux comme liste de choix.
- **Pas de pager, API déjà paginée** : contrôles, documents, références Edge,
  annotations, organisations, sites.
- **Pas de pager, API non paginée (backend à faire)** : firmware, labo,
  caméras, taxonomies, modèles et profils ASR, types d'objets et types de
  liens, packs, empreintes vocales (pager client provisoire).
- **Écarts de forme** : packs d'ontologie en tuiles ; sites en lignes faites
  main (sans `ListStates`) ; organisations créées par un formulaire inline au
  lieu du bouton Ajouter ; types de liens avec un bouton sous le tableau ;
  tableaux écrits à la main au lieu de `DataTable` (modèles ML, onglets de
  flux, journal d'événements, statut plateforme) ; barres d'onglets hors du
  slot `tabs` (flux, notifications, références Edge) ; catalogue en pages
  de 12.
- **Ordre proposé** : lignes manquantes → pagers sur API existante →
  harmonisation de forme → pagination backend.
- **Lignes manquantes : corrigé (2026-10-14, non commité)**. Helper
  `api::client::request_all` (pages de 100 jusqu'à la fin) pour les listes
  « tout » : canaux et modèles de notification (pager client), mélanges
  (+ `can_write` réel, actions masquées aux lecteurs), organisations (+ pager),
  fonctions, Pages, contrôles, Wi-Fi, hôtes, médias et visites des
  annotations, calendrier, choix de flux des onglets Transcriptions/Plages,
  médias du formulaire de flux, sélecteurs d'appareils (éditeurs de flows et
  de dashboards : 200 demandés, 100 reçus). Sites et liste latérale de la
  carte : pager serveur (10 sur N auparavant). Requêtes ontologie à 500 :
  plafond serveur 500, pas de troncature.
- **Pagers sur API existante : corrigé (2026-10-15, non commité)** —
  contrôles, documents, Wi-Fi, hôtes et ensembles d'annotations paginés côté
  client (`crud::pager::page_slice` sur la liste complète de `request_all`,
  testé) ; les annotations chargent tous les ensembles pour que la barre
  « autres ensembles » de l'éditeur reste complète.
- **Suite (2026-10-15, non commité)** : pagers aussi sur firmware, labo,
  caméras, types d'objets, taxonomies, modèles et profils ASR (découpage
  client, `page_slice`) ; taxonomies, packs (plus de tuiles) et types de
  liens en `DataTable` ; sites sous `ListStates` (une erreur d'API
  affichait la liste vide) ; organisations créées par le bouton Ajouter
  (modale, e2e `settings.spec.ts` adapté) ; onglets des références Edge,
  notifications et flux dans le slot `tabs`.
- **Statut** : reste cosmétique, volontairement laissé : tableaux faits
  main avec pager déjà en place (modèles ML, onglets de flux, journal
  d'événements, statut plateforme) ; catalogue en pages de 12 (grille).
  Pagination backend non nécessaire tant que ces listes restent courtes
  (raccourci noté dans `page_slice`).

### O65 — TFT : progression OTA figée à 0 %

- **Constat** (banc P0.1, 2026-10-14, ESP32 38p TXD + ST7735) : pendant une
  OTA réussie, la page OTA du TFT affiche « Downloading » et **0 %** jusqu'à
  la fin, puis la carte redémarre normalement. Purement visuel.
- **Vérifié** : la taille est annoncée (`[OTA] 1018064 octets annoncés`,
  pas de chunked), le téléchargement dure ~17 s (nginx 18:17:50 → 18:18:05),
  `pnex_ota.cpp` publie `set_ota("downloading", pct)` à chaque palier et
  appelle `hooks.tick` (= `pnex_screen::tick`) à chaque tour de boucle ;
  `tickOtaPage` / `drawOtaNumber` redessinent sur changement à la lecture.
- **Piste** : tracer en série les appels de `tickOtaPage` (pct lu, page
  courante) pendant le téléchargement ; vérifier aussi l'OLED (même chemin).
- **Statut** : ouvert, bloqué sur le banc. Relu le 2026-10-15 : la chaîne
  `set_ota` → `pnex_status` → `tickOtaPage`/`drawOtaNumber` est correcte
  (même tampon, hook `screen_wait_tick` posé, redessin sur changement) ;
  il faut la trace série sur la carte.
### O66 — Calc : message rejeté en silence (payload non numérique)

- **Constat** (banc P0.1, 2026-10-14, chaîne caméra) : `vision_detect → calc
  "count > 0" → pnex_notify` ne notifie jamais. Le calc n'accepte qu'un
  payload plat `clé → nombre` (`pnex-node-device/src/calc.rs`) ; la sortie de
  `vision_detect` porte des tableaux/objets (`detections`, `labels`,
  `best_below`) → message rejeté par un `log::warn` côté runtime, **rien**
  dans le statut de nœud ni dans le panneau Debug : l'utilisateur voit un
  flow « vert » qui ne fait rien.
- **Contournement validé** : `json_split` (keys `["count"]`) branché sur la
  ligne `trigger` du notify (nombre non nul = armé).
- **Correction proposée** : remonter le rejet en statut de nœud (`warn`,
  code machine + clé fluent) ; ajouter le contrat d'entrée dans la `NodeDoc`
  du calc (`notes` vide aujourd'hui) pour que l'assistant ne génère pas ce
  câblage ; éventuellement accepter les clés numériques et ignorer les autres.
- **Statut** : corrigé (78378c26) — le calc ignore les clés non numériques
  (`vision_detect → calc "count > 0"` fonctionne) et un message rejeté
  affiche un badge ambre sur le nœud ; `NodeDoc` du calc complétée.

## 2026-10-14 — P0.3 : cluster sur dev.pnex.io (3 pods, ESP32-CAM réelle)

### O67 — Rolling update : timeouts pendant l'arrêt des anciens pods

- **Constat** : `rollout restart` de `pnex-server-api` (3 pods) sous sonde
  HTTP toutes les 0,2 s : 2 requêtes sur 1 365 en timeout de 5 s, chacune
  4 à 7 s après le SIGTERM d'un ancien pod. Kills isolés (arrêt propre ou
  `--force`) : zéro erreur.
- **Cause** : pas de `preStop` ; au SIGTERM le serveur cesse d'accepter
  pendant qu'ingress-nginx route encore vers le pod.
- **Correction** (`pnex-deploy`, chart) : `lifecycle.preStop.sleep` 10 s
  (natif k8s ≥ 1.30, l'image n'a pas de shell), budget 45 s inchangé. Même
  passe : `flowWorker.replicas` (pods flow-worker dédiés, API en
  `PNEX_FLOW_RUN_WORKER=false`), ce qui fait taire l'avertissement « API
  and flow runtime in the same cgroup » des pods.
- **Statut** : corrigé et déployé sur dev.pnex.io (rejoué : 1 timeout sur
  743, 50 s après le roulement, dû à un contrôleur ingress microk8s en
  CrashLoopBackOff depuis 15 jours sur un nœud — infra, hors PneX). Contrôleur
  réparé puis rejoué le même soir : 572 requêtes sur 572 en 200, aucun timeout.

### O68 — Device (écriture) : appareil hors ligne absent du sélecteur

- **Constat** : le sélecteur ne liste que les appareils actifs
  (`inspector/device.rs`, `active: Some(true)`) et charge la liste une
  seule fois à l'ouverture de l'éditeur. Une carte en cours de reconnexion
  ou débranchée disparaît sans explication ; il faut recharger la page.
- **Correction proposée** : lister tous les appareils de l'org, les hors
  ligne grisés avec la mention « hors ligne » (on doit pouvoir préparer un
  flow carte débranchée) ; rafraîchir la liste à l'ouverture du sélecteur.
- **Statut** : corrigé (4f452096) — sélecteurs read/write : tous les
  appareils de l'org, hors ligne marqués, liste rechargée à l'ouverture.

### O69 — Device (écriture) : valeur rejetée invisible dans l'éditeur

- **Constat** : `inject` sans payload (timestamp par défaut) → `Device
  (écriture)` sur une sortie digitale : `invalid digital value (true/false
  or 0/1 required)` uniquement en WARN dans les logs serveur ; le flow est
  « Déployé » au vert et rien ne bouge. Même famille qu'O66.
- **Correction proposée** : statut de nœud `warn` avec code machine + clé
  fluent, visible sur le canevas et dans le panneau Debug.
- **Statut** : corrigé (78378c26) — badge ambre « Nothing written » avec
  le compte et la raison (cliquable), visible sur le canevas et dans le
  panneau Debug ; log serveur dédupliqué (2026-10-15, voir O72).

### O70 — Flows : le modèle « payload » n'est pas intuitif pour un non-dev

- **Constat** (retour utilisateur) : piloter une LED demande de comprendre
  que l'`inject` porte la valeur (payload) et que la case cochée du
  `Device (écriture)` ne dit que *quoi* piloter ; l'aide « payload attendu :
  une map pin → valeur » suppose le modèle connu.
- **Direction retenue** : la partie flows doit devenir intuitive pour un
  non-développeur, même avec beaucoup de nœuds — un nœud de plus ne gêne
  pas si l'éditeur offre groupes et zoom (lisibilité façon classes).
  Pistes : valeur déclarée par sortie cochée (interrupteur / curseur /
  champ, utilisée quand le message n'en porte pas), payload typé dans
  l'`inject` (aucun / nombre / booléen / texte / JSON), rejets visibles
  (O66, O69), sélecteur complet (O68). Toute évolution passe par la
  `NodeDoc` et l'assistant (D142–D145).
- **Statut** : en partie livré — payload typé de l'`inject` et type des
  ports/fils (4f452096), rejets visibles (O66, O69), sélecteur complet
  (O68), **valeur déclarée par sortie cochée** du `Device (écriture)`
  (2026-10-15, non commité : `values` {pin: valeur}, « Toujours allumé /
  éteint », « Fixe » 0–100 en PWM ; le message ne fait que déclencher ;
  `NodeDoc` + fiche assistant `flows.md`). Le reste (groupes de nœuds,
  valeur déclarée affichée sur le canevas) passe en roadmap P2.21.

## 2026-10-15 — nuit : logs de dev.pnex.io après `helm upgrade` (révision 3)

### O71 — ESP32-CAM : écritures non acquittées puis session muette

- **Constat** : `mellow-crane` (ESP32-CAM, branchée en continu) passe hors
  ligne de temps en temps. Flow de test : `inject` 1 s (valeur `0`) →
  `Device (écriture)` sur la LED flash (GPIO 4), plus `Camera source` →
  `Video recording` (0 trame reçue). Chronologie (00:31 → 00:41) : connexion
  00:32:14, la plupart des écritures acquittées pendant ~2 min, puis plus
  aucune (le firmware acquitte pourtant toute écriture, même refusée) alors
  que la session reste vivante ; reconnexion 00:35:14 sur un autre pod, même
  dérive, silence total dès 00:37:20, fermeture anti-zombie 00:38:06 ; pas de
  retour avant au moins 00:41. Le flow prend alors 60 `409 offline` par
  minute.
- **Piste** : boucle unique du firmware saturée (une écriture par seconde =
  écriture + `StateReport` + `Ack`, plus le flux caméra sur le même `loop()`,
  plus le Wi-Fi brouillé par le capteur, cf. XCLK). Pas une chute de
  tension : la valeur écrite est `0`, la LED reste éteinte.
- **À faire** : rejouer avec l'`inject` à 30 s (ou le flow arrêté) ; si les
  décrochages cessent, ne pas pousser une écriture identique à l'état connu
  de la pin, ou limiter le débit par pin côté serveur. Sinon, trace série
  (ouvrir le port réinitialise la carte MB : l'ouvrir une fois, le garder).
- **Suite (06:50)** : les pods api datent de 00:31, donc les décrochages de
  00:32–00:38 tombent pendant le `helm upgrade` (deux coupures en 3 min).
  Après le dernier, la carte reste muette jusqu'à 02:12 (1 h 34, alors que
  le backoff du firmware plafonne à 60 s). Depuis, c'est stable :
  enregistrement continu (~3 i/s, un segment par minute), reconnexion
  éclair à 06:30, environ 2 % d'écritures non acquittées (renvoyées une
  fois). Le trou de 1 h 34 reste inexpliqué sans trace série : carte
  bloquée après les coupures de session, ou blocage dans `pnex_ws_connect`.
  Côté serveur, aucune tentative de connexion n'arrive pendant le trou, et
  avant lui la carte se tait (plus de PING) sans fermer : sa boucle est
  bloquée. Or l'ESP32 n'a pas de watchdog sur la boucle Arduino.
- **Correctif (2026-10-15)** : firmware ESP32 : watchdog de 60 s sur la
  tâche de la boucle (nourri dans les attentes longues : backoff, Wi-Fi,
  OTA) ; redémarrage après 10 min sans session (ESP32 et ESP8266) ;
  handshake TLS borné à 20 s (120 s par défaut). Serveur : le refus 4003
  (bail tenu par une session vivante) est loggé, il était silencieux.
  Compilé (CAM, C6, 8266), non flashé.
- **Statut** : correctif à valider sur la carte (flash OTA, puis un
  `helm upgrade`).

### O72 — Logs du runtime de flows : bruit et avertissement trompeur

- **Constat** : en 15 min, un pod flow-worker écrit 264 fois la même ligne
  `pnex-device-write … 409 Conflict {"code":"offline"}` (une par message),
  76 `Failed to parse ID 'pnexflow1…' as hex` et 14 `No output wires`
  (à chaque chargement d'un flow valide) ; les pods flow-worker dédiés
  annoncent « this pod serves the API AND runs a flow runtime » alors
  qu'aucun Service ne les expose.
- **Correction (2026-10-15, non commité)** : refus du calc et du
  `Device (écriture)` logués une fois par minute pour un même détail (le
  badge garde le compte exact) ; les deux avertissements de chargement
  d'edgelink ne sont plus relayés ; le chart pose `PNEX_SERVES_API=false`
  sur les pods flow-worker (`pnex-deploy`, non commité) et le serveur tait
  alors l'avertissement.
- **Statut** : corrigé (non commité, non déployé).

### O73 — Éditeur de flows : défauts visuels

- **Constat** (capture utilisateur) : point d'entrée sur `Inject` et
  `Camera source` (sources, rien n'y entre) ; port de sortie vidéo plus gros
  (anneau de la couleur du bord, même rose que le type) ; sous-titre
  « mellow-crane… » collé au libellé de port « name » ; badge « Nothing
  written · 48 ok / 451 rejected » qui déborde de son cadre ; libellés de fil
  (« number », « video ») à moitié sur le trait ; « No frames · 0 frames ».
- **Correction (2026-10-15, non commité)** : `accepts_input` exclut `Inject`
  et `Camera source` ; anneau des ports à la couleur de fond du nœud ;
  troncature du sous-titre à ~6 px par caractère ; badge dimensionné sur son
  texte, centré sous le nœud (coupé au-delà de deux largeurs, texte complet
  en infobulle) ; libellé de fil centré sur le trait avec halo ; « No
  frames » seul quand rien n'a été reçu. Vérifié dans le navigateur (build
  debug, stack locale).
- **Statut** : corrigé (non commité).

