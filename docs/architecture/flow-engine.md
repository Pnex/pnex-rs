# Moteur de flow ETL « Node-RED full-Rust » — EdgeLinkd vendored, mode B

> **Statut : IMPLÉMENTÉ (Phase 0 + Phase 1, 2026-09-03 — branché
> `worktree-etl-flow-engine` ; Phase 5 — éditeur Dioxus — IMPLÉMENTÉ
> 2026-09-04, branche `flow-engine-phase5-editeur` ; Phase 6 — nœuds
> device/calc/metric + dépendances pin↔flows — IMPLÉMENTÉ 2026-09-04,
> branche `flow-engine-phase6-nodes-device` ; ferme d'engines « N Engines,
> 1 process » + pré-flight deploy — IMPLÉMENTÉ 2026-09-07 ; contrôles
> runtime Stop/Start/Restart + statut DB `stopped` — IMPLÉMENTÉ 2026-09-07 ;
> cluster d'exécution shardé par org D106 + bus device D107 — §7, 2026-10-01 ;
> en attente de revue humaine).** Ce document consigne (a) la décision
> d'intégration et de rechargement (spike PRD §5.1), (b) les faits
> **vérifiés** sur EdgeLinkd au commit épinglé, (c) les écarts vs la
> conception initiale.
>
> **Rappel PRD (§3, garde-fous)** : ingestion uniquement (pas de write-side
> device, frontière D13/D17) — **dépassé** : le nœud `pnex-device-write`
> écrit sur les pins depuis un flow déployé (mode « connecté », source
> d'écriture exclusive par pin, 8d60950 ; D17 amendé dans `inventory.md`) ;
> cœur EdgeLinkd jamais patché ; pas de
> type-check global du graphe (contrats typés aux frontières des nœuds
> custom) ; les utilisateurs n'écrivent pas de Rust ; l'éditeur Node-RED
> embarqué n'est **jamais exposé** (runtime headless) ; tranches fines —
> l'éditeur Dioxus est la phase 5 de la piste.

## 0. Phase 5 — Éditeur de flows (2026-09-04)

Éditeur drag & drop complet dans `crates/pnex-frontend/src/components/
flow_editor/`, page `/flows` (liste + éditeur en sous-vue — routes
statiques, pattern `devices.rs`). L'éditeur ne parle **qu'à l'API Loco**
(garde-fou PRD respecté — zéro contact avec le runtime).

Choix structurants :

- **Canevas SVG pur Dioxus** (aucune dépendance npm/Rust ajoutée) — l'SVG
  n'a **pas** de `view_box` : 1 unité utilisateur = 1 px CSS, conversion
  `(client − origine − pan)/zoom`, origine (`getBoundingClientRect`)
  mesurée au début de chaque geste.
- **Gestes** : handlers `move`/`up` sur le root SVG (fiables quand le
  pointeur sort du nœud), `pointerleave` annule un geste orphelin, hit-test
  bbox **mathématique** (pas d'`elementFromPoint`), zoom molette borné
  0.4–2.0 vers le curseur, drag snappé sur grille 20 px.
- **Validation partagée** : `pnex_core::validate_graph` exécutée **dans le
  navigateur** (pnex-core est wasm32) AVANT chaque save — surlignage des
  nœuds en cause + bandeau ; le 400 `{"violations": [...]}` serveur est
  traité à l'identique. Pas de cycle-détection ni de type-check ajoutés
  (garde-fou PRD).
- **Versioning** : save = PATCH avec `expected_version_number` (409 →
  modal « Recharger depuis le serveur / Écraser avec ma version », les deux
  branches repartent d'un détail frais) ; drawer d'historique — « Charger »
  une ancienne version = édition (le prochain save crée v(n+1) avec ce
  graphe), « Déployer cette version » = rollback serveur (ne crée pas de
  version) ; dirty dérivé au rendu (`graph != saved_graph`, jamais de
  signal dirty à tenir à jour — leçon brick0 « zéro set en render »).
- **Inspecteur par kind** : inject/debug/red (pnex_sql retiré le 2026-10-01), JSON (payload
  inject, config red) validés localement avec drapeau d'invalidité (pattern
  `MetadataEditor` devices) ; les violations du nœud sélectionné sont
  listées dans l'inspecteur (messages pnex-core affichés tels quels).
- **Deploy/rollback** : bouton gated `can_write && !dirty` (deploy =
  publier une version enregistrée) ; chip runtime pollé 5 s
  (`GET /flows/{id}/runtime`) — moteur actif/arrêté, pid/version au
  survol ; 503 `flow_runtime` toasté tel quel.
- **Frontières** : `ApiError` porte désormais `status`/`body` (409/400
  distinguables sans parser le message) ; `Serialize` ajouté sur les DTOs
  requête `pnex-core` (le front construit les requêtes typées) ; handlers
  sans capture de `String` (l'id du nœud ciblé est relu du signal
  sélection — piège FnMut).

## 0bis. Phase 6 — nœuds device/calc/metric (2026-09-04)

La « partie lecture » des devices dans les flows : choisir n'importe quel
device **dans le nœud** (rien d'imposé à la création du flow), lire ses pins,
combiner plusieurs devices, calculer, et écrire le résultat dans OpenObserve
**comme une métrique au même titre que les capteurs**.

Pipeline cible : `[inject] → [device] → [calc] → [metric]`

- **pnex-device** (crate `pnex-node-device`) : dernières valeurs des pins de
  N devices via PromQL `last_over_time(<pin>{device_id="…"}[window])` — la
  **même série** que l'ingestion (`normalize_measurement_name` déplacé dans
  pnex-core pour garantir l'égalité des noms ingestion/lecture). Payload =
  objet `clé → valeur`, clés `sanitize(device) + "_" + sanitize(pin)`
  (`device_payload_key`, prévisualisées à l'identique par l'éditeur) ; lecture
  sans donnée dans la fenêtre = clé omise + warn — **jamais de zéro inventé**,
  le calc aval échoue « variable inconnue » (fail-loud).
- **pnex-calc** : évaluateur d'expressions **maison dans pnex-core**
  (`calc.rs`, pratt parser pur, wasm-safe, zéro dep) — la MÊME fonction
  valide l'expression dans l'éditeur (validation live) et l'exécute au
  runtime. Langage : opérateurs, comparaisons → 1/0, ternaire, `^` droite-
  associatif, 19 fonctions, constantes pi/e ; division par zéro / hors
  domaine = erreur propre, jamais de panic.
- **pnex-metric** : remote-write OpenObserve depuis le runtime — série
  `etl_<nom>` avec `device_id="flow_{id}"` (device **virtuel**),
  `pred_dev="virtual_device"`, `source_type="etl"`, `ts_source="server"`.
  Le préfixe `etl_` est la **séparation d'index** demandée (pas un stream O2
  séparé) : le catalogue Visualisation étant une découverte dynamique des
  streams metrics, la série apparaît d'elle-même « comme un capteur »,
  filtrable par source_type.
- **Creds/org** : `pnex_org_id` **et `pnex_o2_org`** estampillés dans
  l'artefact au deploy (`FlowArtifactMeta` → tab + nœuds device/metric) —
  l'identifiant O2 **réel** (`openobserve_orgs.o2_org`, généré par le
  provisioning) est résolu en lecture seule (`provisioned_credentials`) au
  moment de la reprojection, **sans schéma déduit** (`pnex_org_{id}` était
  faux : le provisioning génère un id O2 arbitraire). `pnex_o2_org` vide =
  org pas encore provisionnée → nœuds device/metric dégradent (warn,
  lecture/écriture sautée, pipeline vivant) ; le provisioning O2 (1ʳᵉ
  ingestion) déclenche une reprojection qui comble le champ (self-healing,
  sink.rs). Auth Basic racine via allowlist env. Spike exécuté contre l'O2
  réel (`examples/o2_spike.rs`) : remote-write racine accepté + relecture
  `last_over_time` cohérente — le passcode d'org reste inutile aux lectures
  (O2 v0.92.1), la racine couvre lecture **et** écriture.
- **Évaluateur partagé** (`eval_calc`/`validate_calc`) + nommage centralisé
  (`naming.rs`) + structs prompb en feature (`prompb`) dans pnex-core — le
  front wasm ne compile aucune de ces parties natives.
- **Dépendances pin↔flows** : un `set_mode` in↔out sur un pin scanne les
  flows déployés de l'org dont un nœud device lit ce (device, pin) →
  **dé-déploiement automatique** (status draft, reprojection + SIGUSR1 : le
  flow s'arrête réellement ; la version publiée reste enregistrée), réponse
  enrichie `flow_impacts` → toast UI Pins. Dans l'éditeur, violations de
  **staleness** client-only (pin en sortie, pin disparu, device supprimé,
  inactif ou hors ligne — états distincts, `connected` exposé par
  `/devices/{id}/pinout`) → nœud **et câble** en rouge ; re-scan au
  changement de configuration uniquement (pas au drag).

## 0ter. Outils de debug du flow (2026-09-05) — panneau Debug, sonde `pnex-display`

Boucle de mise au point dans l'éditeur : **voir** ce que le flow émet. Un
flow se peaufine **en marche** : son inject est en répétition continue
(défaut 30 s), et l'outil « run once » (SIGUSR2/cmd.json, retiré
2026-09-07) est remplacé par ce trafic permanent. Garde-fou transversal :
ces outils sont **mode dev/debug uniquement** (`settings.flow.debug_tools`,
défaut `false`, activé par `config/development.yaml` via
`PNEX_FLOW_DEBUG_TOOLS`) — en mode run les endpoints répondent **403** et
l'éditeur ne rend même pas les boutons (porté par le chip runtime
`FlowRuntimeStatus.debug_tools`, pas d'endpoint dédié).

- **Panneau Debug** : le nœud `debug` builtin publie sur le canal debug du
  moteur **seulement si `tosidebar: true`** — la projection le positionne
  déjà (flow.rs:632), gardé par un test de non-régression. Le runtime
  enrichit chaque événement stdout avec l'attribution (`flow`, `node_red`)
  : le `DebugMessage.id` du moteur est un hex (`ElementId`, hash du id RED
  quand il n'est pas hex) et `path` l'hex du tab — non mappables côté
  backend. L'attribution vit donc dans le runtime (`src/attrib.rs`, maps
  hex→RED reconstruites au boot et **avant** l'acquittement de redeploy).
  Le superviseur parse le stdout (INFO uniquement) dans un anneau mémoire
  par flow (cap 200/flow, 64 flows, TTL 5 min, purge **avant** SIGUSR1 au
  deploy — les entrées meurent avec l'artefact qu'elles reflètent) exposé
  par `GET /flows/{id}/debug` (200 même moteur arrêté, 404 hors-org —
  entrée sans attribution = jetée, les orgs partagent un flows.json).
- **Sonde `pnex-display`** (crate `pnex-node-display`, pattern
  `pnex-node-device`) : passthrough + publication au canal debug avec
  l'id canvas **brut** (`pnex_node_id` estampillé par la projection) et la
  valeur **non stringifiée** (le debug builtin pré-stringifie — le drawer
  tente un re-parse pour pretty-printer). Badge live sous le nœud dans
  l'éditeur, purgé si le moteur s'arrête.
- **Pas de « run once »** (retrait 2026-09-07) : l'exécution manuelle
  créait plus de problèmes qu'elle n'en résolvait (ack SIGUSR2/cmd.json,
  sémantique de tir unique face au TTL 5 min du feed). Un flow se met au
  point **déployé et en marche** — inject en `repeat_secs` (défaut 30 s) ;
  le déclencheur reste obligatoire à la validation (`no_trigger`).

## 0quater. Fonctions du registre « Automation » (2026-09-15/16)

Registre de **fonctions utilisateur versionnées** (page `/functions`, groupe
« Automation ») importables dans les flows via le nœud `PnexFunction`, avec
**test en live**. Deux langages : **JS** et **Starlark** — pas de Python
(décision utilisateur 2026-09-15 : pas de sandbox réelle en Rust, subprocess
borné jugé insuffisant).

Choix structurants :

- **Détection des variables = directives en commentaire**, source de vérité
  versionnée avec le code : `// @input temperature number "desc"`,
  `// @input mode string="auto"`, `# @output heating number` (préfixe `//`
  ou `#` toléré pour les deux langages). Parseur maison dans
  `pnex_core::functions` (wasm-safe — le front appelle `parse_directives`
  en wasm pour la preview live de la signature) ; extraction au save,
  stockée structurée sur `function_versions.inputs/outputs` (JSON).
- **Contrat d'exécution** : le code définit `handle` — JS
  `function handle(inputs, msg)`, Starlark `def handle(inputs, msg):`.
  `inputs` = objet construit depuis les **bindings** du nœud
  (`input → chemin msg`, défaut `payload.<name>`), `msg` = message entrant.
  Retour : `null` → message jeté ; objet → msg sortant = msg entrant avec
  `payload` remplacé (fusionné si l'objet porte une clé `payload`) ;
  tableau → un élément par port (`@output`), élément `null` = port muet.
- **JS = le nœud `function` QuickJS du vendor, réutilisé tel quel**
  (réutilisation maximale, doctrine « ne jamais patcher ») : la projection
  génère un **wrapper** (`build_js_wrapper` dans pnex-core) — helpers
  embarqués + code utilisateur + tail qui construit `inputs` et mappe le
  retour en ports — et l'inline dans la config `func` du nœud builtin.
- **Starlark = nœud custom `pnex-starlark`** (crate `pnex-node-starlark`,
  pattern des nœuds custom `pnex-node-*`) : Starlark n'existe pas en amont. Parse +
  **évaluation du module au build du nœud** (defs, exige `handle`) → le
  pré-flight `--check` attrape les erreurs de syntaxe au deploy (400
  `engine_load`). Limits Evaluator : ticks, heap 64 Mo, callstack, deadline
  wall-time (`check_cancelled`). Driver `json.decode`/`json.encode` comme
  pont Rust ↔ script (pas de conversion Valeur ↔ JSON). **`Globals` :
  `extended_by([LibraryExtension::Json])` — pas `standard()`** (le module
  `json` est une extension) ; les variables préfixées `_` sont **privées**
  en Starlark (jamais exposées au `module.get`) — le driver utilise donc
  des noms `pnex_*` publics.
- **Artefact self-contained** : la projection inline le CODE de la version
  **épinglée** du nœud (`pnex_function_id`/`pnex_function_version` sur
  l'entrée) — éditer une fonction n'affecte jamais un flow déployé tant
  qu'il n'est pas re-déployé (école « save ≠ déployé »). Le graphe ne
  porte **jamais** de code, seulement références + snapshot d'interface +
  bindings. Ref non résolue au deploy → 400 `function_unresolved`
  (violations) ; en reprojection background → warn + artefact conservé.
- **Versioning** (école flows/media) : `functions` + `function_versions`
  append-only (m000024), save = nouvelle version (concurrence optimiste
  409), `current_version_id` circulaire SET NULL. Delete → 409
  `function_in_use` si référencée par un flow `deployed` (scan des graphs,
  double filet avec la résolution au deploy).
- **Test en live** : sous-commande `pnex-flow-runtime --test-function
  <req.json>` → **une ligne JSON** `FunctionTestResponse` sur stdout. Le
  backend (`POST /api/v1/functions/{id}/test`) spawn le même binaire que le
  superviseur (`resolve_program` + `env_clear` + `apply_runtime_env`,
  kill à 10 s) : le backend ne lie jamais edgelink. **Parité test ≡
  runtime** : Starlark = exactement l'exécuteur du nœud ; JS = miroir
  rquickjs du harnais vendor (wrap `__el_user_func`, options promise/strict
  identiques) évaluant le **même texte de wrapper** que la projection.
  Console capturée dans `logs` (shim dédié côté miroir). Test ad-hoc
  possible sans save (code du textarea + `ad_hoc`).

Limitations documentées (acceptées, décision 2026-09-15) :

- le nœud `function` vendor n'a **ni limite CPU/mémoire ni deadline** — une
  boucle infinie pend la tâche du nœud jusqu'au stop du flow ; propriété
  pré-existante, non aggravée. Depuis D130 (SEC-1/SEC-2, 2026-10-04)
  l'échappatoire `Red` est restreinte à une liste blanche de
  transformations pures (`pnex_core::flow::RED_ALLOWED_TYPES` : ni
  `function`, ni `exec`, ni `template`, ni type `pnex-*`), ses clés
  `pnex_*` sont retirées à la projection, et le registre du runtime
  n'enregistre que `runtime_type_allowed` ; le magasin d'env des engines
  ne contient plus l'environnement du process (SEC-3, fork edgelinkd) —
  voir `security.md`. Rationale : l'isolation des fonctions « bizarres » est un sujet
  **d'infrastructure** (1 tenant = 1 process/conteneur à l'horizon SaaS),
  pas de sandbox in-process. Chemin de migration si ça mord : basculer la
  projection JS vers un nœud custom (switch de projection, l'éditeur et le
  registre sont inchangés) ;
- le pré-flight reste **aveugle aux erreurs de syntaxe JS** (le vendor
  évalue au 1er message, pas au build) — la validation passe par le test en
  live et l'extraction de directives au save ;
- Starlark : pas de capture `print` dans `logs` v1 ; `inputs`/`msg` sont des
  dicts Starlark (accès `inputs["t"]`, pas d'attribut).

## 1. Architecture cible

```
Workspace PNEX                                    vendor/edgelinkd/ (submodule, épinglé)
├─ crates/pnex-core          modèle typé flow.rs (pur, wasm32)          │
├─ crates/pnex-node-*        nœuds custom (device, value, …) ◀──────────┤ path-dep edgelink-core
├─ crates/pnex-flow-runtime  binaire headless maison ◀──────────────────┘ (features core+js)
├─ crates/pnex-backend (Loco) — NE LIE JAMAIS EdgeLinkd
│   ├─ services/flow.rs             settings.flow (FirmwarePartial)
│   ├─ services/flow_supervisor.rs  process enfant + SIGUSR1 + acquittement
│   ├─ controllers/flows.rs         API /api/v1/flows (versionné, 409, deploy)
│   └─ migration m20260903_000009   flows + flow_versions (append-only)
└─ frontend (Phase 5) : éditeur → API Loco uniquement
```

Flux de déploiement :

```
Éditeur/API Loco → flow_version (Postgres, append-only)
                     │  deploy = publie une version
                     ▼
   Loco projette le CANDIDAT (to_red_flows_json de TOUS les flows
   `deployed` + la version candidate, MOINS les exclusions (Stop) —
   DB en lecture seule)
                     │  écrit flows.json.candidate
                     ▼
   pnex-flow-runtime --check (pré-flight : chaque tab construit sans start)
                     │  candidat vert → rename → flows.json
                     ▼
   SIGUSR1 → ferme d'engines : diff par tab → swap des engines modifiés
                     │  acquittement PAR FLOW : flow_started / flow_error /
                     │  flow_stopped (retrait de tab)
                     ▼
   400 engine_load (erreur moteur réelle) ou 200 + marquage DB
   (deployed_version_id) ; versions par flow tenues en DB
```

Contrôles runtime (2026-09-07) — même chemin signal/ack que le deploy :

- **Stop** `POST /flows/{id}/stop` : reprojection **excluant** le flow
  (retrait du tab → `flow_stopped`) → DB `stopped`, `deployed_version_id`
  **conservé** (reprise sans rebuild). L'ack est keyé sur la **meta du flow
  stoppé** (jamais la meta vide, sinon timeout).
- **Start** `POST /flows/{id}/start` : re-déploiement de la version
  `deployed_version_id` via le chemin `deploy_version` (l'override de
  `reproject_candidate` inclut le tab même hors statut `deployed`) → ack
  `flow_started` → DB `deployed`.
- **Restart** `POST /flows/{id}/restart` : stop puis start chaînés — deux
  cycles (le reload est un diff par hash, un tab identique ne swappe pas).
- **Deploy sur un flow stopped = start avec la nouvelle version** (aucun
  garde spécial) ; DB écrite **après** ack sur les trois chemins — un
  arrêt/démarrage non acquitté laisse l'état antérieur intact.

## 2. Spike §5.1 — décision : mode B renforcé (binaire maison)

Le PRD recommandait de démarrer en **process supervisé (B)** et chargeait le
spike de vérifier si une crate moteur réutilisable rendrait A viable plus tôt.
Faits vérifiés au commit épinglé :

- `edgelink-core` est une vraie lib (`Engine::with_json/with_flows_file/start/
  stop/redeploy_flows/subscribe_events`) — consommée en interne par
  `edgelink-pymod` (PyO3) ;
- l'enregistrement des nœuds est **automatique par `inventory`** : tout crate
  lié statiquement s'enregistre via `#[flow_node(...)]` (modèle :
  `node-plugins/edgelink-nodes-dummy`) ;
- `edgelinkd` upstream n'a **ni** hot-reload fichier **ni** SIGHUP
  (uniquement ctrl_c et son API admin web Node-RED, **non authentifiée**) ;
- inject couvre `repeat` + `crontab` (tokio-cron-scheduler) → le besoin
  « cron/interval » de la Phase 4 est déjà couvert.

**Décision : B renforcé.** On ne lance pas le `edgelinkd` upstream : notre
propre binaire `pnex-flow-runtime` lie `edgelink-core` + nœuds PNEX et ajoute
ce qu'upstream n'a pas :

- **rechargement à chaud sans coupure** : SIGUSR1 → relecture du flows.json →
  **ferme d'engines, diff par tab** (2026-09-07 — initialement
  `Engine::redeploy_flows`, écart global vs le vendor, cf. §5) — les flows
  inchangés ne sont jamais touchés, aucune surface HTTP ;
- **stdout = événements JSON-lines machine** (`started`, `flow_started`,
  `flow_error`, `flow_stopped`, `debug`, `redeployed`, `stopped`…),
  stderr = logs JSON — le superviseur Loco rejoue en `tracing`, acquitte
  les deploys par flow (oneshot `flow_started`/`flow_error`/
  `flow_stopped`) et tient la santé par flow ;
  `runtime.json` reste la santé process (pid, flow_rev, redeploys —
  **jamais de version** : artefact multi-flows) ;
- **échec process = exit(1)** : le superviseur relance avec backoff
  exponentiel borné ; un flow invalide n'est **plus** un échec process
  (`flow_error` isolé, les autres continuent).

A reste la cible à terme (EdgeLinkd stabilisé), la porte reste ouverte : le
backend n'a aucune dépendance à la couche transport, seul le superviseur
changerait.

## 3. Contrat de déploiement constaté

| Élément | Valeur (vérifiée au commit épinglé) |
|---|---|
| Artefact | `<state_dir>/flows.json`, tableau Node-RED multi-tabs ; un tab par flow déployé (`id = pnexflow{flow_id}`) |
| Métadonnées | `pnex_flow_id` / `pnex_version` sur le tab et les nœuds custom — préservées par le désérialiseur EdgeLinkd (`#[serde(flatten)] rest`) |
| Exécution | **Ferme d'engines** (`engines.rs`) : un `Engine` **par flow** (1 tab = 1 engine, build-then-swap, last-good préservé) — `redeploy_flows` du vendor n'est **plus jamais appelé** |
| Rechargement | SIGUSR1 → relecture du fichier → **diff par tab** (SHA-256) : seuls les engines des tabs modifiés sont swappés — aucune pause d'ingestion pour les autres ; fichier illisible → `reload_failed`, engines courants conservés |
| Acquittement | Événements **par flow** sur stdout : `flow_started {flow, rev}` / `flow_error {flow, error}` / `flow_stopped {flow}` (l'erreur moteur **réelle** remonte au backend) ; `<state_dir>/runtime.json` reste la santé **process** (`pid`, `running`, `flow_rev`, `redeploys`) — **aucune version** (la version déployée d'un flow vit en DB, `flows.deployed_version_id`) |
| Pré-flight | `pnex-flow-runtime --check <fichier>` : construit chaque engine sans `start()`, rapport `check_flow` par tab, exit 0/1 ; le deploy écrit `flows.json.candidate`, le vérifie, puis le renomme **seulement si vert** |
| Version incohérente | tab invalide → `flow_error` isolé sur CE flow (les autres continuent) ; le last-good tourne encore si un swap échoue ; retry = deploy de CE flow ou redémarrage du process |
| Contrôles runtime | `POST /flows/{id}/stop` (retrait de tab → `flow_stopped` → DB `stopped`, version conservée) · `/start` (re-déploiement de la version déployée → `flow_started` → DB `deployed`) · `/restart` (stop+start chaînés) — même chemin signal/ack que le deploy, DB marquée **après** acquittement |
| Secrets | env enfant/pré-flight = `PATH`, `HOME`, `PNEX_FLOW_LOG` + allowlist + creds OpenObserve injectées du yaml (`apply_runtime_env` — identiques pour les deux) — **jamais** dans flows.json |

## 4. Vérification d'acceptance (Phase 0 + 1)

- (a) `inject → debug` headless lancé/arrêté par Loco —
  `tests/flows.rs::cycle_deploy_edit_rollback_avec_runtime` +
  `pnex-flow-runtime/tests/inject_debug.rs` ;
- (b) flow créé via API (v1 persistée) → déployé → exécuté par le runtime —
  à l'origine une vraie requête SQL (`tests/sql_query.rs`), retirée avec le
  nœud `pnex-sql` (2026-10-01, cf. § 5) ; couvert aujourd'hui par les tests
  runtime des autres nœuds custom ;
- (c) édition → v2 **sans** rechargement (aucun artifact écrit au save),
  deploy v2 rechargé, artefact porte la version 2 ;
- (d) rollback v1 → ancien graphe reprojété ;
- (e) save périmé → **409**, aucune v3 ;
- (f) `msg` malformé rejeté à la frontière du nœud (contrats typés de
  `pnex_core::flow::payload`, jamais de panic) + validation de graphe en 400 `{"violations": [...]}`.

Empreinte mémoire : la mesure sur Pi (PRD Phase 0) reste un TODO manuel — le
job CI `arm-check` couvre la compilation croisée aarch64/armv7 (`cargo check`
avec cross-compilateurs C : les build scripts de `ring`/`rquickjs-sys`
compilent pour la cible même sans lien). EdgeLinkd revendique ~10× moins de
RAM que Node-RED (non vérifié).

## 5. Écarts vs la conception initiale

- **`features = ["core", "js"]` et non `["core"]` seul** (2026-09-03) : le
  commit épinglé contient un `use rquickjs::...` non conditionné dans
  `variant/mod.rs` — la feature `core` seule ne compile pas. On active la
  feature amont `js` (QuickJS embarqué, ARM OK dans leur CI) — pas de patch
  vendor. À remonter amont ; si corrigé, réduire à `core`.
- **`rust-version` workspace 1.85 → 1.88** : `edgelink-core` (edition 2024)
  utilise les let-chains.
- **Tables physiques plurielées** : le DSL Loco crée `flows`/`flow_versions`
  (`normalize_table` = pluriel cruet) — le PRD parlait de `flow`/`flow_version`
  au niveau conceptuel.
- **FK circulaire PG-only** : `ALTER TABLE ADD CONSTRAINT` n'existe pas en
  sqlite — sur le tier hobbyiste, `flows.deployed_version_id` reste une
  colonne sans contrainte (l'intégrité est portée par le contrôleur) ;
  `schema_invariants.rs` vérifie la contrainte sur PG.
- **409 au lieu de 400** pour les saves périmés : exigence explicite du PRD
  (concurrence optimiste), écart assumé avec la convention 400 historique.
- **Un seul flows.json par instance** : le deploy reprojette l'ensemble des
  flows `deployed` (tous tenants confondus — le runtime exécute multi-tabs).
  L'isolation runtime par org/device est une décision de Phase 3 (attachement
  produit aux devices) — à concevoir avec le modèle de déploiement multi-tenant.
- **Superviseur dans `after_routes`, pas `connect_workers`** : doit vivre aussi
  en ServerOnly (même logique que `spawn_reaper`) ; gate = `settings.flow.enabled`
  uniquement (les tests d'intégration l'activent par env avant boot).
- **Ferme d'engines « N Engines, 1 process » (2026-09-07)** : remplace le
  « 1 Engine, N tabs » initial. Un flow invalide au chargement ne fait plus
  `exit(1)` global (l'ancien crash-loop downait **tous** les flows, toutes
  orgs, et la version fautive restait marquée `deployed`) : `flow_error`
  isolé, les autres continuent, le last-good tourne si un swap échoue.
  Context « global », link call et registre http-response deviennent
  **intra-flow** (chaque engine est autonome — zéro état global mutable dans
  le vendor, seul `inventory` immuable) ; catch déjà intra-tab. Plus de
  pause d'ingestion globale au deploy d'un flow. Le déploiement est
  **pré-flighté** (`--check`) : un tab invalide répond 400 `engine_load`
  avec l'erreur moteur réelle et la DB n'est marquée `deployed` **qu'après**
  acquittement. Santé par flow (`engine_status`/`last_error` sur
  `GET /flows/{id}/runtime`), chip rouge dans l'éditeur. Un tab inchangé en
  erreur n'est pas retenté (retry = deploy de CE flow ou redémarrage).
- **Contrôles runtime + statut `stopped` (2026-09-07)** : Stop = retrait de
  tab (même mécanique que le delete, sans détruire le flow) → DB `stopped`
  avec `deployed_version_id` **conservé** — la reprise (Start ou Deploy)
  re-déploie cette version. Distinction avec le dé-déploiement **automatique**
  (cascade suppression de pin → `draft` + `deployed_version_id` NULL, car la
  version n'a plus de sens) : le stop est **volontaire** et réversible sans
  rebuild. Le chip distingue `engine_status "stopped"` (ardoise) de
  `"error"` (rouge) et de la santé process. L'ack du stop est toujours keyé
  sur la **meta du flow stoppé** — jamais la meta vide (le retrait du
  dernier flow déployé sinon attend un ack impossible).
- **Subflows/templates ignorés par la ferme** : `parse_tabs` regroupe par
  `z` et jette les entrées sans `z` (subflow templates, global config nodes
  — jamais produits par la projection PNEX) ; un subflow instance orphelin
  fera échouer son tab (`flow_error`), ce qui est l'isolation voulue.
- **Nœuds exclus Phase 1** : `join`, `csv`, `file` partiellement cassés en
  amont (tests de spéc diff) — non utilisés ; à re-évaluer avant la Phase 3.
- **Crédentials Node-RED non implémentés côté EdgeLinkd** : confirmé — notre
  règle « secrets par env seulement » est la seule voie.

### Retrait du nœud `pnex-sql` (2026-10-01)

Le premier nœud custom `pnex-sql` (SELECT-only, sqlx Postgres, connexion
par `DATABASE_URL` dans l'env du runtime) n'a jamais été utilisé ; il est
**supprimé** (crate `pnex-node-sql`, kind `pnex_sql`, inspecteur, palette,
clés i18n). Effet sécurité : `DATABASE_URL` ne franchit plus la frontière
process vers le runtime (retiré de `env_allowlist` par défaut et des yaml).
Un graphe enregistré qui contient encore `pnex_sql` échoue proprement au
parse (« node kind `pnex_sql` has been removed ») : tab d'un autre flow
isolé (`skipped_tabs`), deploy du flow lui-même en 400 `graph_unreadable`.

## 6. Provenance et règles vendor (`vendor/edgelinkd`)

- Submodule épinglé à `d0a5e114468ee1b26147de55cdca10484ade6b05`
  (master, 2026-01-19 — pas de release versionnée amont : on épingle un SHA).
- **Ne jamais patcher** : extension par nœuds custom + binaire maison
  uniquement ; retours amont = issues/PR ; mise à jour = bump de submodule
  (SHA re-épinglé + note d'écart ici).
- Sous-module amont `3rd-party/node-red` **volontairement non initialisé**
  (~100 Mo, inutile à la compilation) — jamais `git submodule update --recursive`.
- Licence Apache-2.0 (code vendored séparé, non modifié) — compatible avec la licence MIT du workspace
  du workspace.

## 7. Cluster d'exécution des flows (D106) + bus de commandes device (D107) — 2026-10-01

> **Statut : IMPLÉMENTÉ (branche `feat/flow-sharding`, en attente de revue
> humaine).** Remplace « un runtime par process `pnex-server` » : avec N
> pods, les N runtimes exécutaient **les mêmes flows N fois** (N commandes
> sur le même relais, N notifications), et chaque deploy reprojetait
> **tous** les flows de l'instance (O(total)).

### 7.1 Décisions

| # | Décision |
|---|---|
| **D106** | **Plan de contrôle / plan d'exécution séparés, sharding par org.** Un contrôleur de placement **élu** (bail en ligne `flow_leases`, portable PG/sqlite, pas d'advisory lock de session — compatible pgbouncer) décide quelle org tourne sur quel worker ; il n'exécute aucun flow. N workers (un worker = un superviseur = un runtime enfant) exécutent les flows des orgs placées chez eux. Unité de placement = **l'org** (nœuds mémoire partagés par org, isolation tenant à l'horizon SaaS). Le deploy ne projette **que l'org** concernée (O(flows de l'org)). Mode embarqué (Pi/VM/compose) = **cluster d'un seul worker**, même chemin de code. |
| **D107** | **Bus de commandes device inter-pods via Valkey**, adressé par **pod** (1 souscription par pod, quel que soit le nombre de devices) : route `pnex:devroute:v1:{device}` = pod (EX 90, rafraîchie toutes les 30 s par la session, supprimée par son seul propriétaire), canal `pnex:devcmd:v1:{pod}`, `PUBLISH` = 0 récepteur ⇒ route périmée supprimée, device hors ligne. Sans Valkey : comportement local inchangé. |

### 7.2 Données

- `flow_workers(id, boot, advertise_url, capacity, draining, started_at, heartbeat_at)` — `boot` change à chaque démarrage : une ancienne incarnation du même id est **supplantée** (son heartbeat échoue → elle se coupe).
- `flow_placements(org_id PK, worker_id, epoch, revision, updated_at)` — `epoch` +1 à chaque déplacement (jeton de fencing), `revision` +1 à chaque changement des flows projetés de l'org (le propriétaire ne reprojette **que** les orgs dont la révision a bougé : O(orgs possédées) par tick).
- `flow_leases(name PK, holder, expires_at)` — `flow-placement` = contrôleur.
- Index `flows(status, org_id)` pour le poids des orgs (flows déployés par org).

### 7.3 Mécanique

- **Worker** (`services/flow_cluster/worker.rs`) : heartbeat dans **sa propre tâche** (un deploy lent qui tient le verrou d'état n'affame jamais le heartbeat — trouvé par les tests), réconciliation séparée ; `apply_org` (push API, acquitté) et réconciliation sérialisés par le verrou d'état ; un fragment poussé enregistre son `(epoch, revision)` comme « vu » (une org neuve n'est pas reprojetée depuis une base pas encore marquée) ; resync complet toutes les 5 min (répare un bump de révision manqué / un crash entre ack et marquage DB). Aucun `flows.json` résiduel n'est rejoué au boot : tout part de la base.
- **Auto-fencing** : heartbeat en échec pendant `self_fence_ms` (< `worker_ttl_ms`) ⇒ runtime tué ; supplanté ⇒ arrêt définitif. Le contrôleur ne réaffecte qu'après `worker_ttl_ms` : l'ancien runtime est **déjà** coupé.
- **Fencing des écritures** : le runtime reçoit `PNEX_FLOW_WORKER_FENCE=<id>:<boot>` et le pose en `x-pnex-flow-worker` sur device-write, event, video-segment, video-annotations, notify deliver/journal ; le backend refuse (`409 {"code":"fenced"}`) si le worker ne possède plus l'org (cache 1 s). Sans en-tête = appel legacy autorisé (le jeton de service authentifie ; le fencing est une garantie de cohérence). Les canaux notify externes (ntfy, SMTP…) partent en direct : couverts par l'auto-fencing uniquement.
- **Contrôleur** (`controller.rs` + planificateur **pur** `placement.rs`) : bascule des workers morts/en drain (jamais bornée), rééquilibrage **borné** (`max_moves`/tour, ne déplace qu'une org qui réduit strictement l'écart), libération des orgs sans flow déployé seulement après `release_grace_ms` **et** recomptage frais à zéro (un deploy en vol garde sa placement — trouvé par les tests). Poids rafraîchis toutes les `weights_refresh_ms` (un agrégat indexé) ; la bascule n'en a pas besoin.
- **Drain** (`App::on_shutdown`) : `draining=true` → le contrôleur déplace les orgs → le worker les voit partir → runtime coupé → désinscription. Rolling update sans double exécution.
- **Transport** : in-process si le propriétaire vit dans le process, sinon HTTP interne `/internal/flow-cluster/{apply, flows/{id}/runtime, flows/{id}/debug}` (jeton `x-pnex-cluster-token`, fail-closed ; worker cible nommé par `x-pnex-cluster-worker`). Runtime/debug/node-status de l'API sont demandés au propriétaire.
- **Caméras (D102)** : la demande est tenue par worker source ; la poussée `CameraConfig` passe par le bus D107.
- **Runtime lié à son serveur** : l'enfant reçoit `PR_SET_PDEATHSIG=SIGKILL` (Linux) — un `pnex-server` tué hors conteneur ne laisse plus de runtime orphelin exécutant des flows déjà réaffectés (trouvé en préparant l'E2E). Les boucles locales runtime → backend (device-write, notify) suivent le **port réel** du serveur (étaient figées sur 5150).
- **Sweeps singletons** : chaque tâche périodique prend le bail `task:<nom>` (3 périodes) à chaque tour ; un seul pod l'exécute, un autre reprend à l'expiration.

### 7.4 Configuration (`settings.flow.cluster`)

Nœud unique : **rien à régler**. Plusieurs pods : `PNEX_FLOW_ADVERTISE_URL=http://$(POD_IP):5150`, même `PNEX_FLOW_CLUSTER_TOKEN` partout, Valkey configurée (D107), `PNEX_FLOW_RUN_WORKER=false` pour un pod API seul. `worker_id` par défaut = `HOSTNAME` (nom du pod). Délais par défaut : heartbeat 2 s, TTL 10 s, auto-fencing 6 s, tick contrôleur 2 s, bail 10 s.

### 7.5 Vérification

- Unitaires : planificateur (9 tests, dont 10 000 orgs × 1 000 workers < 5 s, convergence du rééquilibrage, capacité, drain), bus D107 contre une vraie Valkey (route, relais verbatim, route périmée, libération propriétaire seul).
- Intégration `tests/flow_cluster.rs` (vrais workers + contrôleur + fake runtime) : sharding (chaque flow sur **exactement un** worker, artefact disque de chaque runtime = ses seuls tabs), bascule d'un worker crashé (≥ TTL, epoch +1) + écritures fencées, rééquilibrage vers un nouveau worker + drain, supplantation, **deploy/stop/runtime/debug vers un worker distant en vrai HTTP**, sémantique des baux. Stable 4/4.
- **E2E multi-process** `tests/flow_cluster_e2e.rs` (`--ignored`) : **deux vrais process `pnex-server`** (5251/5252) + **vrai `pnex-flow-runtime` release** + base fraîche + Valkey isolée (db 9). Flows de 4 orgs déployés via les DEUX serveurs → 2 orgs par worker, artefacts disjoints ; statut runtime répondu cross-pod ; **SIGKILL du pod B** → son runtime meurt avec lui (`PR_SET_PDEATHSIG`), bascule de ses orgs, le pod A exécute tous les flows ; **SIGTERM du pod A** → drain, runtime arrêté, worker désinscrit. ~4 s, stable 3/3.

### 7.6 Coût mesuré d'un flow (runtime release, 2026-10-01)

| Flows | Variante | RSS | Par flow | Boot | CPU (1 msg / 5 s / flow) |
|---|---|---|---|---|---|
| 200 | inject→debug | 120 Mo | 615 Ko | 0,16 s | ~0 % |
| 2 000 | inject→debug | 1 002 Mo | 513 Ko | 0,58 s | ~0 % |
| 1 000 | inject→function JS→debug | 750 Mo | 768 Ko | 0,39 s | ~0 % |
| 2 000 | inject→function JS→debug | 1 472 Mo | 753 Ko | 0,74 s | 2 % |

⇒ ~0,5–0,75 Mo **par flow résident** (≈ 250 Ko par nœud : un `Engine` edgelink complet par flow). Un worker 16 Go ≈ 20 000 flows. 2 M de flows **tous résidents** ≈ 100 workers / ~1,5 To : c'est le plafond économique du modèle « tout résident ».

### 7.7 Ce qui reste (honnêtement) pour « 10 000 clients / 2 M flows »

1. **Mise en sommeil des flows inactifs** (acteurs virtuels) : seuls les flows actifs résident ; réveil sur événement/cron, état des nœuds (anomaly, forecast, merge…) checkpointé en Valkey. Levier principal au-delà de ~20 k flows/worker.
2. **Réduire le coût par engine** (~250 Ko/nœud) : piste vendor (canaux pré-alloués, registres par engine) — à profiler.
3. **Journal d'événements partitionné** (NATS JetStream ou Valkey Streams, par org) si des événements doivent survivre à une bascule : aujourd'hui un message en vol pendant la bascule est perdu (fenêtre ≈ TTL).
4. **État encore local à un pod** : `LAST_VALUES` des pins (valeurs live de la page Pins : vides si l'UI interroge un autre pod que celui du device — à servir depuis le cache live Valkey). Les sweeps périodiques (reaper de liveness, watchdog OTA, pruner vidéo, reconcile rétention O2) tournent désormais sur **un seul pod** (bail `task:<nom>`, `services/singleton.rs`).
5. **E2E avec devices physiques** sur 2 pods (un device connecté au pod A, son flow sur le pod B → écriture via le bus D107) : non joué sur matériel — le bus est testé contre une vraie Valkey, le multi-process sans device.
6. **Manifests k8s** (Deployment, `POD_IP` → `PNEX_FLOW_ADVERTISE_URL`, secret du jeton cluster, `terminationGracePeriodSeconds` ≥ `drain_timeout`) : à écrire dans `pnex-deploy`. Exigences opérationnelles (rôles API / worker, arrêt ≥ 40 s, migrations sous verrou, pools, cohérence base) : `horizontal-scaling.md`.

## 8. Palette de nœuds par catégories (D109) — 2026-10-01

| # | Décision |
|---|---|
| **D109** | **Chaque kind appartient à une catégorie unique**, déclarée par un `match` exhaustif (`kind_category`, `flow_editor/canvas/palette.rs`) : ajouter un kind sans choisir sa section ne compile pas. La palette `+` affiche des **sections titrées** dans l'ordre fixe `PALETTE_CATEGORIES` (parcours d'un flow : Déclencheurs → Devices → Régulation → Données & calcul → Code → Stockage & séries → IA & prédictif → Intégrations → Debug). La recherche reste transverse (libellé, description **et** nom de catégorie) ; une section vidée par le filtre disparaît avec son titre. `PaletteItem.group` est optionnel : dashboard et studio restent en liste plate. |

Répartition et suite (catalogue piloté par descripteurs, parité n8n) : `roadmap.md` P1.7 / P2.11. La couleur des nœuds au canevas reste par kind.

## 9. Nœud `pnex-control-source` (D127) — 2026-10-03

Source événementielle des **contrôles d'org** (`surfaces-controls.md`) :
config `{controls: [uuid], emit_on_start}`, un port de sortie par contrôle
(dans l'ordre de la liste). `SUBSCRIBE pnex:ctl:v1:{org}` ; chaque écriture
d'une surface sort `payload` = valeur, `topic` = clé du contrôle,
`msg.control = {id, key, by, via, ts_ms}`. `emit_on_start` relit les
dernières valeurs (MGET) **après** l'abonnement : aucune écriture perdue
entre les deux. Reconnexion avec backoff (≤ 30 s), statut `control-listening`
/ `control-bus-unavailable`, compteur `commands`. Le deploy refuse un
contrôle absent de l'org (`control-unknown`) ; un contrôle écouté par un flow
déployé ne peut pas être supprimé (`control-in-use`). Un interrupteur (1/0)
ou un curseur (0..100) se branche tel quel sur `device-write` ; la règle
« une source d'écriture par pin » reste la seule voie vers un pin (D128).
